//! `Logs.Download`: a Log bundle streamed to the browser.
//!
//! One blocking task builds the zip and sends it in chunks over a small
//! channel, so a slow phone slows the compression instead of filling memory.
//! The task ends when the bundle is complete or when the channel closes, which
//! happens when the browser disconnects or Companion stops and drops the
//! response stream. Nothing is staged on disk.

use super::bundle::{BundleFile, write_bundle};
use cadmus_api_proto::cadmus::api::public::v1::DownloadEvent;
use connectrpc::{ConnectError, ServiceStream};
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

/// Bytes of bundle per event.
const CHUNK_BYTES: usize = 64 * 1024;

/// Events buffered between the encoder and the response stream.
const BACKLOG: usize = 4;

type Events = mpsc::Sender<Result<DownloadEvent, ConnectError>>;

/// Takes the permit for one bundle job, or refuses because one is running.
///
/// # Errors
///
/// `resource_exhausted` while another Download holds the permit.
pub(super) fn try_start_job(jobs: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit, ConnectError> {
    Arc::clone(jobs).try_acquire_owned().map_err(|_| {
        ConnectError::resource_exhausted("another log bundle is being prepared, try again shortly")
    })
}

/// File name of a bundle made at `now`.
pub(super) fn bundle_name(now: chrono::DateTime<chrono::Utc>) -> String {
    format!("cadmus-logs-{}.zip", now.format("%Y%m%dT%H%M%SZ"))
}

/// Streams `files` as a zip named `name`, holding `permit` until the job ends.
pub(super) fn stream_bundle(
    files: Vec<BundleFile>,
    name: String,
    permit: OwnedSemaphorePermit,
) -> ServiceStream<DownloadEvent> {
    let (tx, rx) = mpsc::channel(BACKLOG);
    let total = files.iter().map(|file| file.size).sum();
    crate::runtime::spawn_blocking(move || {
        let _permit = permit;
        run(&files, name, total, &tx);
    });
    Box::pin(futures_lite::stream::unfold(rx, |mut rx| async {
        let item = rx.recv().await?;
        Some((item, rx))
    }))
}

fn run(files: &[BundleFile], name: String, total: u64, tx: &Events) {
    let read = Arc::new(AtomicU64::new(0));
    let sink = ChunkSink {
        tx,
        read: Arc::clone(&read),
        total,
        name: Some(name),
        buffer: Vec::with_capacity(CHUNK_BYTES),
    };
    let outcome = write_bundle(files, sink, &read).and_then(ChunkSink::finish);
    if let Err(error) = outcome
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        tracing::warn!(error = %error, "log bundle failed");
        let _ = tx.blocking_send(Err(ConnectError::internal("log bundle failed")));
    }
}

/// Collects the encoder's output into [`CHUNK_BYTES`] events.
struct ChunkSink<'a> {
    tx: &'a Events,
    read: Arc<AtomicU64>,
    total: u64,
    name: Option<String>,
    buffer: Vec<u8>,
}

impl ChunkSink<'_> {
    fn send_buffer(&mut self) -> io::Result<()> {
        let event = DownloadEvent {
            read: self.read.load(Ordering::Relaxed),
            total: self.total,
            data: std::mem::replace(&mut self.buffer, Vec::with_capacity(CHUNK_BYTES)),
            file_name: self.name.take().unwrap_or_default(),
            ..Default::default()
        };
        self.tx
            .blocking_send(Ok(event))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the client went away"))
    }

    /// Sends what is left, ending the stream.
    fn finish(mut self) -> io::Result<()> {
        self.send_buffer()
    }
}

impl Write for ChunkSink<'_> {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let written = data.len();
        while !data.is_empty() {
            let room = CHUNK_BYTES - self.buffer.len();
            let take = room.min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == CHUNK_BYTES {
                self.send_buffer()?;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::StreamExt;
    use std::io::{Cursor, Read};
    use std::time::Duration;

    fn log(dir: &std::path::Path, name: &str, body: &[u8]) -> BundleFile {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        BundleFile {
            name: name.to_owned(),
            path,
            size: body.len() as u64,
            modified: 1_700_000_000,
        }
    }

    async fn collect(mut stream: ServiceStream<DownloadEvent>) -> Vec<DownloadEvent> {
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
        events
    }

    fn unpack(bytes: Vec<u8>) -> Vec<(String, Vec<u8>)> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index(index).unwrap();
                let name = entry.name().unwrap().into_owned();
                let mut body = Vec::new();
                entry.read_to_end(&mut body).unwrap();
                (name, body)
            })
            .collect()
    }

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_stream_carries_one_bundle_with_its_name_and_progress() {
        let dir = tempfile::tempdir().unwrap();
        let body = "a line worth keeping\n".repeat(20_000);
        let files = vec![
            log(dir.path(), "cadmus-a.json", body.as_bytes()),
            log(dir.path(), "info.log", b"launcher\n"),
        ];
        let jobs = Arc::new(Semaphore::new(1));
        let stream = stream_bundle(
            files,
            "cadmus-logs-test.zip".to_owned(),
            try_start_job(&jobs).unwrap(),
        );
        let events = collect(stream).await;

        assert_eq!(events[0].file_name, "cadmus-logs-test.zip");
        assert!(events[1..].iter().all(|event| event.file_name.is_empty()));
        let total = body.len() as u64 + 9;
        assert!(events.iter().all(|event| event.total == total));
        let last = events.last().unwrap();
        assert_eq!(last.read, total);
        assert!(events.windows(2).all(|pair| pair[0].read <= pair[1].read));

        let bytes: Vec<u8> = events.into_iter().flat_map(|event| event.data).collect();
        assert_eq!(
            unpack(bytes),
            [
                ("cadmus-a.json".to_owned(), body.into_bytes()),
                ("info.log".to_owned(), b"launcher\n".to_vec()),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_incompressible_log_spans_events_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let body = noise(300 * 1024);
        let files = vec![
            log(dir.path(), "cadmus-a.json", &body),
            log(dir.path(), "info.log", b"launcher\n"),
        ];
        let jobs = Arc::new(Semaphore::new(1));
        let stream = stream_bundle(
            files,
            "cadmus-logs-test.zip".to_owned(),
            try_start_job(&jobs).unwrap(),
        );
        let events = collect(stream).await;

        assert!(events.len() > 1, "the bundle spans several events");
        assert_eq!(events[0].file_name, "cadmus-logs-test.zip");
        assert!(events[1..].iter().all(|event| event.file_name.is_empty()));
        assert!(events.iter().all(|event| event.data.len() <= CHUNK_BYTES));
        let bytes: Vec<u8> = events.into_iter().flat_map(|event| event.data).collect();
        assert_eq!(
            unpack(bytes),
            [
                ("cadmus-a.json".to_owned(), body),
                ("info.log".to_owned(), b"launcher\n".to_vec()),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_job_is_refused_until_the_first_ends() {
        let jobs = Arc::new(Semaphore::new(1));
        let first = try_start_job(&jobs).unwrap();
        let refused = try_start_job(&jobs).unwrap_err();
        assert_eq!(refused.code, connectrpc::ErrorCode::ResourceExhausted);
        drop(first);
        let _again = try_start_job(&jobs).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_stream_stops_the_job_and_frees_the_permit() {
        let dir = tempfile::tempdir().unwrap();
        let files = vec![log(dir.path(), "cadmus-a.json", &noise(8_000_000))];
        let jobs = Arc::new(Semaphore::new(1));
        let mut stream = stream_bundle(
            files,
            "cadmus-logs-test.zip".to_owned(),
            try_start_job(&jobs).unwrap(),
        );
        assert!(stream.next().await.is_some());
        assert!(try_start_job(&jobs).is_err(), "the job is still running");
        drop(stream);

        let mut freed = false;
        for _ in 0..100 {
            if try_start_job(&jobs).is_ok() {
                freed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(freed, "the job must end when the client goes away");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_log_that_vanished_ends_the_stream_with_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = log(dir.path(), "cadmus-a.json", b"x\n");
        std::fs::remove_file(&file.path).unwrap();
        file.size = 2;
        let jobs = Arc::new(Semaphore::new(1));
        let mut stream = stream_bundle(
            vec![file],
            "cadmus-logs-test.zip".to_owned(),
            try_start_job(&jobs).unwrap(),
        );
        let mut failed = false;
        while let Some(item) = stream.next().await {
            failed |= item.is_err();
        }
        assert!(failed);
    }
}
