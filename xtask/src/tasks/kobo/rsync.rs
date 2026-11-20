//! Delta deployment of the assembled distribution over the existing SSH session.
//!
//! The rsync engine runs in process and drives a `rsync --server` child on the
//! device through a channel opened by [`super::ssh`]. Host keys, authentication,
//! and `ssh_config` are therefore handled by the same `russh` connection as
//! `kobo restart`; no host `ssh` executable or password helper is involved.
//! Files are written to a temporary name and renamed into place, so the running
//! binary and mapped libraries are never truncated.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use bytesize::ByteSize;
use oc_rsync_core::client::ClientConfig;
use oc_rsync_core::client::remote::{RemoteInvocationBuilder, RemoteRole};
use oc_rsync_core::server::{
    ServerConfig, ServerRole, ServerStats, perform_handshake_with_max, run_server_with_handshake,
};
use russh::ChannelMsg;
use russh::client::{Handle, Msg};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc as tokio_mpsc;
use tokio::task::JoinHandle;

use super::INSTALL_DIR;
use super::ssh::Handler;

/// Runs one delta transfer of `dist/` into [`INSTALL_DIR`].
pub(crate) async fn run(handle: &mut Handle<Handler>, dist: &Path) -> Result<()> {
    run_plan(handle, build_plan(dist, INSTALL_DIR)?).await
}

async fn run_plan<H>(handle: &mut Handle<H>, plan: TransferPlan) -> Result<()>
where
    H: russh::client::Handler<Error = anyhow::Error> + Send + Sync + 'static,
{
    let destination = plan.destination;
    let channel = handle
        .channel_open_session()
        .await
        .context("failed to open an SSH session for rsync")?;
    channel
        .exec(true, plan.remote_command.clone())
        .await
        .context("failed to start rsync on the device")?;

    let (reader, mut writer, outcome, task) = spawn_bridge(channel);
    let protocol = plan.local.protocol;
    let transfer = crate::blocking(move || {
        let mut reader = reader;
        let handshake = perform_handshake_with_max(&mut reader, &mut writer, protocol)?;
        run_server_with_handshake(plan.local, handshake, &mut reader, writer, None, None, None)
            .map_err(anyhow::Error::from)
    })
    .await;

    task.await
        .context("the rsync SSH bridge task stopped unexpectedly")?;
    let outcome = outcome
        .lock()
        .map_err(|_| anyhow!("the rsync SSH bridge state is poisoned"))?
        .clone();
    let stderr = outcome.stderr_context();

    if let Some(failure) = outcome.failure {
        bail!("the device rejected the rsync command: {failure}{stderr}");
    }
    if let Some(status) = outcome.exit_status
        && status != 0
    {
        bail!("rsync on the device exited with status {status}{stderr}");
    }

    let stats = transfer.map_err(|error| anyhow!("rsync deployment failed: {error}{stderr}"))?;
    let ServerStats::Generator(stats) = stats else {
        bail!("rsync negotiated the wrong transfer role");
    };

    println!(
        "Synced {} changed files to {destination} ({} literal, {} matched, {} sent, {} received)",
        stats.files_transferred,
        ByteSize(stats.literal_data),
        ByteSize(stats.matched_data),
        ByteSize(stats.bytes_sent),
        ByteSize(stats.bytes_read),
    );
    Ok(())
}

struct TransferPlan {
    remote_command: String,
    destination: String,
    local: ServerConfig,
}

fn build_plan(dist: &Path, destination: &str) -> Result<TransferPlan> {
    let config = ClientConfig::builder()
        .recursive(true)
        .times(true)
        .links(false)
        .whole_file(false)
        .inplace(false)
        .checksum(true)
        .build();
    let remote_args = RemoteInvocationBuilder::new(&config, RemoteRole::Sender).build(destination);
    let remote_command = remote_command(&remote_args)?;
    let flags = compact_flags(&remote_args)?;
    let mut local = ServerConfig::from_flag_string_and_args(
        ServerRole::Generator,
        flags,
        vec![source_path(dist)],
    )
    .map_err(|error| anyhow!("failed to configure the local rsync sender: {error}"))?;
    local.connection.client_mode = true;
    Ok(TransferPlan {
        remote_command,
        destination: destination.to_owned(),
        local,
    })
}

fn source_path(dist: &Path) -> OsString {
    let mut source = dist.as_os_str().to_owned();
    source.push("/");
    source
}

fn remote_command(args: &[OsString]) -> Result<String> {
    args.iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("the remote rsync command contains a non-UTF-8 argument"))
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join(" "))
}

fn compact_flags(args: &[OsString]) -> Result<String> {
    args.iter()
        .find_map(|arg| {
            let text = arg.to_str()?;
            (text.starts_with('-') && !text.starts_with("--") && text.len() > 1)
                .then(|| text.to_owned())
        })
        .ok_or_else(|| anyhow!("the remote rsync command has no compact flag string"))
}

#[derive(Debug, Clone, Default)]
struct Outcome {
    stderr: Vec<u8>,
    exit_status: Option<u32>,
    failure: Option<String>,
}

impl Outcome {
    fn stderr_context(&self) -> String {
        let stderr = String::from_utf8_lossy(&self.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            String::new()
        } else {
            format!("\nrsync stderr:\n{stderr}")
        }
    }
}

struct BridgeReader {
    rx: tokio_mpsc::Receiver<Option<Vec<u8>>>,
    buffered: Vec<u8>,
    offset: usize,
}

impl Read for BridgeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.offset >= self.buffered.len() {
            match self.rx.blocking_recv() {
                Some(Some(chunk)) => {
                    self.buffered = chunk;
                    self.offset = 0;
                }
                Some(None) | None => return Ok(0),
            }
        }
        let available = &self.buffered[self.offset..];
        let len = available.len().min(buf.len());
        buf[..len].copy_from_slice(&available[..len]);
        self.offset += len;
        Ok(len)
    }
}

struct BridgeWriter {
    tx: Option<tokio_mpsc::Sender<Vec<u8>>>,
    buffer: Vec<u8>,
}

const WRITE_BUFFER_SIZE: usize = 32 * 1024;
const MAX_INBOUND_ITEMS: usize = 64;

impl BridgeWriter {
    fn send(&mut self, bytes: Vec<u8>) -> io::Result<()> {
        self.tx
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "rsync bridge closed"))?
            .blocking_send(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "rsync bridge closed"))
    }
}

impl Write for BridgeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.buffer.len() + buf.len() > WRITE_BUFFER_SIZE {
            self.flush()?;
        }
        if buf.len() >= WRITE_BUFFER_SIZE {
            self.send(buf.to_vec())?;
        } else {
            self.buffer.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.buffer);
        self.send(bytes)
    }
}

impl Drop for BridgeWriter {
    fn drop(&mut self) {
        let _ = self.flush();
        self.tx = None;
    }
}

type ChannelWriter = Pin<Box<dyn AsyncWrite + Send>>;
type PendingWrite = Pin<Box<dyn Future<Output = (ChannelWriter, io::Result<()>)> + Send>>;

fn spawn_bridge(
    channel: russh::Channel<Msg>,
) -> (
    BridgeReader,
    BridgeWriter,
    Arc<Mutex<Outcome>>,
    JoinHandle<()>,
) {
    let (data_tx, data_rx) = tokio_mpsc::channel::<Option<Vec<u8>>>(64);
    let (write_tx, mut write_rx) = tokio_mpsc::channel::<Vec<u8>>(64);
    let outcome = Arc::new(Mutex::new(Outcome::default()));
    let state = Arc::clone(&outcome);
    let mut writer: Option<ChannelWriter> = Some(Box::pin(channel.make_writer()));

    let task = tokio::spawn(async move {
        let mut channel = channel;
        let mut channel_open = true;
        let mut write_open = true;
        let mut data_open = true;
        let mut inbound = VecDeque::<Option<Vec<u8>>>::new();
        let mut remote_eof = false;
        let mut remote_exit_status = None;
        let mut write_fut: Option<PendingWrite> = None;
        loop {
            tokio::select! {
                message = channel.wait(), if channel_open && (inbound.len() < MAX_INBOUND_ITEMS || write_fut.is_some()) => {
                    match message {
                        Some(ChannelMsg::Data { data }) => {
                            if data_open && !remote_eof {
                                inbound.push_back(Some(data.to_vec()));
                            }
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            if let Ok(mut outcome) = state.lock() {
                                outcome.stderr.extend_from_slice(&data);
                            }
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => {
                            remote_exit_status = Some(exit_status);
                            if let Ok(mut outcome) = state.lock() {
                                outcome.exit_status = Some(exit_status);
                            }
                            if !data_open
                                || (remote_eof && !write_open && inbound.is_empty())
                            {
                                break;
                            }
                        }
                        Some(ChannelMsg::Failure) => {
                            if let Ok(mut outcome) = state.lock() {
                                outcome.failure = Some("the server rejected the command request".to_owned());
                            }
                            let _ = channel.close().await;
                            break;
                        }
                        Some(ChannelMsg::Eof) => {
                            remote_eof = true;
                            if data_open {
                                inbound.push_back(None);
                            }
                        }
                        Some(_) => {}
                        None => {
                            channel_open = false;
                            write_open = false;
                            write_fut = None;
                            writer = None;
                            write_rx.close();
                            if data_open && !remote_eof {
                                remote_eof = true;
                                inbound.push_back(None);
                            }
                        }
                    }
                }
                permit = data_tx.reserve(), if data_open && !inbound.is_empty() => {
                    match permit {
                        Ok(permit) => {
                            if let Some(data) = inbound.pop_front() {
                                permit.send(data);
                            }
                            if remote_eof
                                && !write_open
                                && inbound.is_empty()
                                && remote_exit_status.is_some()
                            {
                                break;
                            }
                        }
                        Err(_) => {
                            inbound.clear();
                            data_open = false;
                            if remote_exit_status.is_some() {
                                break;
                            }
                        }
                    }
                }
                outgoing = write_rx.recv(), if write_open && write_fut.is_none() => {
                    match outgoing {
                        Some(bytes) => {
                            let mut writer = writer.take().expect("writer is present while writes are open");
                            write_fut = Some(Box::pin(async move {
                                let result = writer.write_all(&bytes).await;
                                (writer, result)
                            }));
                        }
                        None => {
                            write_open = false;
                            writer = None;
                            let _ = channel.eof().await;
                            if !data_open && remote_exit_status.is_some() {
                                break;
                            }
                            if remote_eof
                                && inbound.is_empty()
                                && remote_exit_status.is_some()
                            {
                                break;
                            }
                        }
                    }
                }
                result = async {
                    match write_fut.as_mut() {
                        Some(fut) => fut.await,
                        None => std::future::pending().await,
                    }
                }, if write_fut.is_some() => {
                    let (w, result) = result;
                    writer = Some(w);
                    write_fut = None;
                    if result.is_err() {
                        break;
                    }
                }
                else => break,
            }
        }
    });

    (
        BridgeReader {
            rx: data_rx,
            buffered: Vec::new(),
            offset: 0,
        },
        BridgeWriter {
            tx: Some(write_tx),
            buffer: Vec::with_capacity(WRITE_BUFFER_SIZE),
        },
        outcome,
        task,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::kobo::SshOptions;
    use crate::tasks::kobo::ssh;

    struct AcceptAnyKey;

    impl russh::client::Handler for AcceptAnyKey {
        type Error = anyhow::Error;

        async fn check_server_key(
            &mut self,
            _: &russh::keys::PublicKeyOrCertificate,
        ) -> Result<bool> {
            Ok(true)
        }
    }

    async fn connect_test_server<S>(server: S) -> (Handle<AcceptAnyKey>, JoinHandle<io::Result<()>>)
    where
        S: russh::server::Server<Handler = S>
            + russh::server::Handler<Error = russh::Error>
            + Send
            + 'static,
    {
        use std::time::Duration;

        use russh::keys::{Algorithm, PrivateKey, key::safe_rng};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let host_key = PrivateKey::random(&mut safe_rng(), Algorithm::Ed25519).unwrap();
        let running = tokio::spawn(async move {
            let mut server = server;
            server
                .run_on_socket(
                    Arc::new(russh::server::Config {
                        auth_rejection_time: Duration::ZERO,
                        inactivity_timeout: None,
                        keys: vec![host_key],
                        ..Default::default()
                    }),
                    &listener,
                )
                .await
        });

        let mut handle = russh::client::connect(
            Arc::new(russh::client::Config::default()),
            address,
            AcceptAnyKey,
        )
        .await
        .unwrap();
        assert!(handle.authenticate_none("test").await.unwrap().success());
        (handle, running)
    }

    #[tokio::test]
    async fn rejected_exec_closes_the_bridge_and_reader() {
        use std::time::Duration;

        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        struct RejectExec;

        impl russh::server::Server for RejectExec {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self
            }
        }

        impl russh::server::Handler for RejectExec {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_failure(channel)?;
                Ok(())
            }
        }

        let (handle, running) = connect_test_server(RejectExec).await;
        let channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "rsync --server").await.unwrap();

        let (mut reader, writer, outcome, bridge) = spawn_bridge(channel);
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .expect("the bridge must stop after the command is rejected")
            .unwrap();
        assert_eq!(
            outcome.lock().unwrap().failure.as_deref(),
            Some("the server rejected the command request")
        );
        let read = std::thread::spawn(move || {
            let mut byte = [0u8; 1];
            reader.read(&mut byte)
        })
        .join()
        .unwrap();
        assert_eq!(read.unwrap(), 0);
        drop(writer);
        running.abort();
    }

    #[tokio::test]
    async fn half_closed_channel_does_not_stall_the_bridge() {
        use std::time::Duration;

        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        struct HalfClose;

        impl russh::server::Server for HalfClose {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self
            }
        }

        impl russh::server::Handler for HalfClose {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                session.data(channel, &b"payload"[..])?;
                session.eof(channel)?;
                session.exit_status_request(channel, 0)?;
                Ok(())
            }

            async fn channel_eof(
                &mut self,
                channel: russh::ChannelId,
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.close(channel)?;
                Ok(())
            }
        }

        let (handle, running) = connect_test_server(HalfClose).await;
        let channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "rsync --server").await.unwrap();

        let (mut reader, writer, outcome, bridge) = spawn_bridge(channel);
        let read = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).map(|_| bytes)
        });
        drop(writer);
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .expect("the bridge must stop after both sides reach EOF")
            .unwrap();
        assert_eq!(outcome.lock().unwrap().exit_status, Some(0));
        assert_eq!(read.join().unwrap().unwrap(), b"payload");
        running.abort();
    }

    #[tokio::test]
    async fn reader_loss_still_records_exit_status() {
        use std::time::Duration;

        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        struct ReaderLoss;

        impl russh::server::Server for ReaderLoss {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self
            }
        }

        impl russh::server::Handler for ReaderLoss {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                session.data(channel, &b"payload"[..])?;
                let handle = session.handle();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let _ = handle.exit_status_request(channel, 42).await;
                    let _ = handle.close(channel).await;
                });
                Ok(())
            }
        }

        let (handle, running) = connect_test_server(ReaderLoss).await;
        let channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "rsync --server").await.unwrap();

        let (reader, writer, outcome, bridge) = spawn_bridge(channel);
        drop(reader);
        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .expect("the bridge must stop after the remote closes")
            .unwrap();
        assert_eq!(outcome.lock().unwrap().exit_status, Some(42));
        drop(writer);
        running.abort();
    }

    #[tokio::test]
    async fn nonzero_exit_status_fails_run_plan() {
        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        struct NonzeroExit;

        impl russh::server::Server for NonzeroExit {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self
            }
        }

        impl russh::server::Handler for NonzeroExit {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                session.data(channel, &b"not an rsync stream"[..])?;
                session.eof(channel)?;
                session.exit_status_request(channel, 42)?;
                session.close(channel)?;
                Ok(())
            }
        }

        let (mut handle, running) = connect_test_server(NonzeroExit).await;
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("example.txt"), b"payload").unwrap();
        let plan = build_plan(source.path(), "/tmp/does-not-matter").unwrap();

        let error = run_plan(&mut handle, plan).await.unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("rsync on the device exited with status 42"),
            "{message}"
        );
        running.abort();
    }

    #[tokio::test]
    #[ignore = "requires a local OpenSSH server and rsync"]
    async fn localhost_transfer_round_trip() {
        let user = std::env::var("USER").unwrap();
        let settings = ssh::resolve(&SshOptions {
            ip: "127.0.0.1".to_owned(),
            port: None,
            user: Some(user),
            identity: Vec::new(),
            identities_only: false,
            known_hosts: None,
            accept_new: true,
            ssh_config: None,
        })
        .await
        .unwrap();
        let mut handle = ssh::connect(&settings).await.unwrap();
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let payload = (0..4 * 1024 * 1024)
            .map(|offset| (offset % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(source.path().join("example.txt"), &payload).unwrap();

        let plan = build_plan(source.path(), destination.path().to_str().unwrap()).unwrap();
        run_plan(&mut handle, plan).await.unwrap();

        assert_eq!(
            std::fs::read(destination.path().join("example.txt")).unwrap(),
            payload
        );
    }

    #[test]
    fn plan_uses_the_native_rsync_server_command() {
        let dist = tempfile::tempdir().unwrap();
        let plan = build_plan(dist.path(), INSTALL_DIR).unwrap();

        assert!(plan.remote_command.starts_with("rsync --server "));
        assert!(!plan.remote_command.contains("ssh"));
        assert_eq!(plan.local.role, ServerRole::Generator);
        assert!(plan.local.connection.client_mode);
        assert!(plan.local.flag_string.contains('r'));
        assert!(plan.local.flag_string.contains('c'));
    }

    #[test]
    fn reader_drains_oversized_chunks_across_reads() {
        let (tx, rx) = tokio_mpsc::channel(1);
        tx.try_send(Some(vec![1, 2, 3])).unwrap();
        let mut reader = BridgeReader {
            rx,
            buffered: Vec::new(),
            offset: 0,
        };

        let mut first = [0u8; 2];
        assert_eq!(reader.read(&mut first).unwrap(), 2);
        assert_eq!(first, [1, 2]);
        let mut second = [0u8; 2];
        assert_eq!(reader.read(&mut second).unwrap(), 1);
        assert_eq!(second[0], 3);
    }

    #[test]
    fn writer_batches_small_writes_until_flush() {
        let (tx, mut rx) = tokio_mpsc::channel(2);
        let mut writer = BridgeWriter {
            tx: Some(tx),
            buffer: Vec::new(),
        };

        writer.write_all(&[1, 2, 3]).unwrap();
        assert!(rx.try_recv().is_err());
        writer.flush().unwrap();
        assert_eq!(rx.try_recv().unwrap(), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn slow_reader_does_not_stall_the_bridge() {
        use std::time::Duration;

        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        struct SlowReader {
            first_data: bool,
        }

        impl SlowReader {
            fn new() -> Self {
                Self { first_data: true }
            }
        }

        impl russh::server::Server for SlowReader {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self::new()
            }
        }

        impl russh::server::Handler for SlowReader {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                session.data(channel, &b"payload"[..])?;
                Ok(())
            }

            async fn data(
                &mut self,
                channel: russh::ChannelId,
                _data: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                if self.first_data {
                    self.first_data = false;
                    session.data(channel, &b"payload2"[..])?;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    session.eof(channel)?;
                    session.exit_status_request(channel, 0)?;
                }
                Ok(())
            }
        }

        let (handle, running) = connect_test_server(SlowReader::new()).await;
        let channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "rsync --server").await.unwrap();

        let (mut reader, mut writer, _outcome, bridge) = spawn_bridge(channel);

        let write_task = tokio::task::spawn_blocking(move || {
            let data = vec![0u8; 4 * 1024 * 1024];
            writer.write_all(&data).unwrap();
            writer.flush().unwrap();
        });

        let read_task = tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 7];
            reader.read_exact(&mut buf).unwrap();
            let mut buf2 = [0u8; 8];
            reader.read_exact(&mut buf2).unwrap();
            (buf, buf2)
        });

        let (buf, buf2) = tokio::time::timeout(Duration::from_secs(5), read_task)
            .await
            .expect("the bridge must keep reading while a write is pending")
            .unwrap();
        assert_eq!(&buf, b"payload");
        assert_eq!(&buf2, b"payload2");

        tokio::time::timeout(Duration::from_secs(10), write_task)
            .await
            .expect("the write must not deadlock")
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), bridge)
            .await
            .expect("the bridge must stop")
            .unwrap();

        running.abort();
    }

    #[tokio::test]
    async fn bidirectional_backpressure_does_not_deadlock() {
        use std::time::Duration;

        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        const CHUNKS: usize = 1500;
        const CHUNK_SIZE: usize = 16 * 1024;
        const PEER_STALL: Duration = Duration::from_secs(6);

        struct FloodThenDelay {
            first_read: bool,
        }

        impl FloodThenDelay {
            fn new() -> Self {
                Self { first_read: true }
            }
        }

        impl russh::server::Server for FloodThenDelay {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self::new()
            }
        }

        impl russh::server::Handler for FloodThenDelay {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                let handle = session.handle();
                tokio::spawn(async move {
                    let chunk = vec![0x5au8; CHUNK_SIZE];
                    for _ in 0..CHUNKS {
                        if handle.data(channel, chunk.clone()).await.is_err() {
                            return;
                        }
                    }
                    let _ = handle.eof(channel).await;
                    let _ = handle.exit_status_request(channel, 0).await;
                });
                Ok(())
            }

            async fn data(
                &mut self,
                _: russh::ChannelId,
                _: &[u8],
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                if self.first_read {
                    self.first_read = false;
                    tokio::time::sleep(PEER_STALL).await;
                }
                Ok(())
            }
        }

        let (handle, running) = connect_test_server(FloodThenDelay::new()).await;
        let channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "rsync --server").await.unwrap();

        let (mut reader, mut writer, _outcome, bridge) = spawn_bridge(channel);

        let engine = tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; CHUNK_SIZE];
            for _ in 0..CHUNKS {
                reader.read_exact(&mut buf).unwrap();
                writer.write_all(&buf).unwrap();
            }
            drop(writer);
        });

        let (engine_result, bridge_result) = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(engine, bridge)
        })
        .await
        .expect("bidirectional backpressure must not deadlock the bridge");
        engine_result.unwrap();
        bridge_result.unwrap();

        running.abort();
    }

    #[tokio::test]
    async fn terminal_close_cancels_pending_write_and_releases_engine() {
        use std::time::Duration;

        use russh::server::{Auth, ChannelOpenHandle, Msg as ServerMsg, Session};

        struct CloseWithoutReading {
            first_read: bool,
        }

        impl CloseWithoutReading {
            fn new() -> Self {
                Self { first_read: true }
            }
        }

        impl russh::server::Server for CloseWithoutReading {
            type Handler = Self;

            fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
                Self::new()
            }
        }

        impl russh::server::Handler for CloseWithoutReading {
            type Error = russh::Error;

            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }

            async fn channel_open_session(
                &mut self,
                _: russh::Channel<ServerMsg>,
                reply: ChannelOpenHandle,
                _: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }

            async fn exec_request(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                Ok(())
            }

            async fn data(
                &mut self,
                channel: russh::ChannelId,
                _: &[u8],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                if self.first_read {
                    self.first_read = false;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    let handle = session.handle();
                    tokio::spawn(async move {
                        let _ = handle.close(channel).await;
                    });
                }
                Ok(())
            }
        }

        let (handle, running) = connect_test_server(CloseWithoutReading::new()).await;
        let channel = handle.channel_open_session().await.unwrap();
        channel.exec(true, "rsync --server").await.unwrap();

        let (_reader, mut writer, _outcome, bridge) = spawn_bridge(channel);
        let engine = tokio::task::spawn_blocking(move || {
            let payload = vec![0x5au8; 32 * 1024 * 1024];
            writer.write_all(&payload)
        });

        let (engine_result, bridge_result) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(engine, bridge)
        })
        .await
        .expect("terminal channel closure must release the pending write");
        engine_result
            .unwrap()
            .expect("the engine must finish queuing its write");
        bridge_result.unwrap();
        running.abort();
    }
}
