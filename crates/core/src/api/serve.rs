//! HTTP listener: Connect RPCs, `GET /openapi.json`, optional static site,
//! optional bearer auth.

use super::Command;
use super::handler::PublicRpc;
use crate::device::inhibitor::{Inhibitor, InhibitorGuard, Kind, SoftSuspendName};
use crate::view::{Event, Hub};
use bytes::Bytes;
use cadmus_api_proto::cadmus::api::shared::v1::{FileOpsExt, Locale, SettingsExt, StatusExt};
use cadmus_api_proto::locale_from_dir;

pub(super) use cadmus_api_proto::locale_dir;
use connectrpc::{ConnectError, ConnectRpcService, Router};
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body::{Body, Frame, SizeHint};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
#[cfg(any(not(debug_assertions), test))]
use secrecy::ExposeSecret;
use secrecy::SecretString;
use std::convert::Infallible;
use std::future::Future;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tower::Service;

const RPC_PREFIX: &str = "/cadmus.api.";
const OPENAPI_PATH: &str = "/openapi.json";
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);
const MAX_CONNECTIONS: usize = 16;
const MAX_FILE_READS: usize = 4;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(60);
const HTTP2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// HTTP/2 streams one connection may run at once.
const HTTP2_MAX_CONCURRENT_STREAMS: u32 = 16;
/// Longest a connection may sit without a pending request, or with pending response bodies that yield no frames.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(90);
/// How often connection deadlines are checked.
const CONNECTION_SWEEP_INTERVAL: Duration = Duration::from_secs(1);
/// Bytes per streamed file chunk.
///
/// Chunk size trades read syscalls and framing overhead against worst-case
/// buffering, not throughput. 64 KiB keeps at most [`MAX_FILE_READS`] streams
/// at 256 KiB of chunk buffers on a 512 MB device; larger chunks would raise
/// that peak without changing wire throughput on Wi-Fi.
pub(super) const FILE_CHUNK_BYTES: usize = 64 * 1024;

/// Bind a reuseaddr TCP listener on `addr`.
///
/// An unspecified IPv6 address binds a dual-stack socket, so IPv4 clients reach
/// the same port. Hosts without IPv6 fall back to an IPv4 wildcard, which the
/// Kobo needs because its kernel build may not have IPv6.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(addr = %addr)))]
pub(super) async fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    match addr {
        SocketAddr::V6(v6) if v6.ip().is_unspecified() => match bind_dual_stack(v6.port()).await {
            Ok(listener) => Ok(listener),
            Err(error) => {
                tracing::warn!(error = %error, "api dual-stack bind failed, using ipv4");
                bind_one(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    v6.port(),
                ))
                .await
            }
        },
        _ => bind_one(addr).await,
    }
}

/// Bind one reuseaddr socket for the family of `addr`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn bind_one(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = match addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(128)
}

/// Bind an IPv6 wildcard socket that accepts IPv4 clients as mapped addresses.
#[cfg(unix)]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn bind_dual_stack(port: u16) -> std::io::Result<TcpListener> {
    use std::os::fd::AsRawFd;
    let socket = tokio::net::TcpSocket::new_v6()?;
    socket.set_reuseaddr(true)?;
    unsafe { set_ipv6_only_off(socket.as_raw_fd()) }?;
    socket.bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))?;
    socket.listen(128)
}

/// Hosts without `IPV6_V6ONLY` listen on IPv4 only.
#[cfg(not(unix))]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn bind_dual_stack(port: u16) -> std::io::Result<TcpListener> {
    bind_one(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)).await
}

/// Clear `IPV6_V6ONLY` on an unbound socket so it accepts IPv4 as well.
///
/// # Safety
/// `fd` must be an open socket owned by the caller, and `value` must outlive
/// the call.
#[cfg(unix)]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
unsafe fn set_ipv6_only_off(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    let value: libc::c_int = 0;
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_V6ONLY,
            std::ptr::from_ref(&value).cast(),
            std::mem::size_of_val(&value) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

type RespBody = ActiveBody;

/// Error type shared by the response body halves.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Any response body, boxed with the shared error type.
type BoxedBody = Pin<Box<dyn Body<Data = Bytes, Error = BoxError> + Send>>;

/// RPC or static-file response body before it carries connection activity.
type InnerBody = BoxedBody;

/// In-flight request marker; decrements its connection's counter on drop.
struct InFlight {
    activity: Arc<ConnectionActivity>,
}

impl InFlight {
    fn new(activity: Arc<ConnectionActivity>) -> Self {
        activity.request_started();
        Self { activity }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.activity.request_finished();
    }
}

/// Response body that keeps its connection marked active until dropped.
struct ActiveBody {
    inner: BoxedBody,
    in_flight: InFlight,
}

impl ActiveBody {
    /// Wraps `body`, counting it as in flight until the body is dropped.
    fn new(body: InnerBody, in_flight: InFlight) -> Self {
        Self {
            inner: body,
            in_flight,
        }
    }
}

impl Body for ActiveBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        let poll = this.inner.as_mut().poll_frame(cx);
        if let Poll::Ready(Some(Ok(_))) = poll {
            this.in_flight.activity.body_progressed();
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// RPC response body that holds the soft-suspend lease until it ends.
///
/// Streaming handlers return their `ServiceStream` before any chunk is read,
/// so the lease must live with the body, not with the handler future. Auth
/// rejections answer without a lease.
struct LeasedBody {
    inner: connectrpc::ConnectRpcBody,
    _lease: Option<InhibitorGuard>,
}

/// Wraps an RPC response body with a body that owns `lease`.
fn leased_response(
    response: Response<connectrpc::ConnectRpcBody>,
    lease: Option<InhibitorGuard>,
) -> Response<LeasedBody> {
    let (parts, body) = response.into_parts();
    Response::from_parts(
        parts,
        LeasedBody {
            inner: body,
            _lease: lease,
        },
    )
}

impl Body for LeasedBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        Pin::new(&mut self.get_mut().inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Boxes a body with the shared error type.
fn boxed_body<B>(body: B) -> BoxedBody
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    Box::pin(body.map_err(Into::into))
}

/// Per-connection request activity for the connection sweep.
struct ConnectionActivity {
    /// Whether any request reached this connection.
    started: AtomicBool,
    /// Requests whose response body is still pending.
    active: AtomicUsize,
    /// Monotonic milliseconds since process start when the connection was accepted.
    accepted_ms: AtomicU64,
    /// Monotonic milliseconds since process start when the last response body
    /// finished. Zero before any.
    finished_ms: AtomicU64,
    /// Monotonic milliseconds since process start when a request started or a
    /// pending response body last yielded a frame.
    progress_ms: AtomicU64,
}

impl ConnectionActivity {
    fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            accepted_ms: AtomicU64::new(now_ms()),
            finished_ms: AtomicU64::new(0),
            progress_ms: AtomicU64::new(0),
        }
    }

    fn request_started(&self) {
        self.started.store(true, Ordering::Relaxed);
        self.active.fetch_add(1, Ordering::Relaxed);
        self.progress_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn body_progressed(&self) {
        self.progress_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn request_finished(&self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
        self.finished_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// Whether the connection outlived its deadline with nothing in flight, or
    /// its in-flight bodies yielded no frame for [`IDLE_CONNECTION_TIMEOUT`].
    fn expired(&self) -> bool {
        self.expired_at(now_ms())
    }

    fn expired_at(&self, now: u64) -> bool {
        if !self.started.load(Ordering::Relaxed) {
            return now.saturating_sub(self.accepted_ms.load(Ordering::Relaxed))
                >= HEADER_READ_TIMEOUT.as_millis() as u64;
        }
        let idle = IDLE_CONNECTION_TIMEOUT.as_millis() as u64;
        if self.active.load(Ordering::Relaxed) > 0 {
            return now.saturating_sub(self.progress_ms.load(Ordering::Relaxed)) >= idle;
        }
        let finished = self.finished_ms.load(Ordering::Relaxed);
        finished > 0 && now.saturating_sub(finished) >= idle
    }
}

/// Monotonic milliseconds since process start for connection deadlines.
///
/// Wall-clock jumps, such as the NTP `settimeofday` on Wi-Fi bring-up, must
/// neither expire nor extend a connection.
fn now_ms() -> u64 {
    static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    START.elapsed().as_millis() as u64
}

/// Which RPC surface a listener exposes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    /// Companion listener: shared RPCs only.
    Public,
    /// Loopback listener: shared RPCs plus `cadmus.api.internal.v1`.
    Internal,
}

impl Scope {
    /// Label used in access logs.
    fn name(self) -> &'static str {
        match self {
            Scope::Public => "public",
            Scope::Internal => "internal",
        }
    }
}

/// How this listener authenticates, what it exposes, and whether it serves
/// site files.
pub(super) struct ListenConfig {
    /// `Some` requires bearer auth except GetStatus. `None` is open (loopback).
    pub token: Option<SecretString>,
    /// Install-directory site. `None` serves RPCs only.
    pub site_root: Option<PathBuf>,
    /// RPC surface. Private-only services are never registered when public.
    pub scope: Scope,
    /// Uppermost directory `FileOps` may read. `None` is unrestricted.
    pub file_root: Option<PathBuf>,
    /// Rejected bearer credentials, shared with the main loop's session.
    pub auth_failures: Arc<AtomicU32>,
}

/// RPCs both listeners serve, all from `cadmus.api.shared.v1`.
fn shared_router(handler: &Arc<PublicRpc>) -> Router {
    let router = StatusExt::register(Arc::clone(handler), Router::new());
    let router = SettingsExt::register(Arc::clone(handler), router);
    FileOpsExt::register(Arc::clone(handler), router)
}

/// Accept loop until cancel. Optional bearer and optional static site.
///
/// The loop outlives every request, so it carries no span of its own; the
/// accept branch traces each connection instead.
pub(super) async fn listen(
    cancel: CancellationToken,
    listener: TcpListener,
    config: ListenConfig,
    tx: mpsc::UnboundedSender<Command>,
    hub: Hub,
    inhibitor: Arc<Inhibitor>,
) {
    let handler = Arc::new(PublicRpc {
        tx: tx.clone(),
        hub: hub.clone(),
        inhibitor: Arc::clone(&inhibitor),
        file_root: config.file_root.map(Arc::new),
        file_limit: Arc::new(Semaphore::new(MAX_FILE_READS)),
    });
    let router = shared_router(&handler);
    let router = match config.scope {
        Scope::Public => router,
        Scope::Internal => super::internal::register(&handler, router),
    };
    let rpc = Auth {
        inner: ConnectRpcService::new(router),
        token: config.token,
        scope: config.scope,
        tx,
        hub,
        inhibitor,
        failures: config.auth_failures,
    };
    let app = AppService {
        rpc,
        root: config.site_root.map(Arc::new),
        activity: Arc::new(ConnectionActivity::new()),
    };
    let mut http = Builder::new(TokioExecutor::new());
    http.http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .keep_alive(true);
    http.http2()
        .timer(TokioTimer::new())
        .keep_alive_interval(HTTP2_KEEP_ALIVE_INTERVAL)
        .keep_alive_timeout(HTTP2_KEEP_ALIVE_TIMEOUT)
        .max_concurrent_streams(HTTP2_MAX_CONCURRENT_STREAMS);
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut connections = tokio::task::JoinSet::new();
    let accept = match AcceptIo::from_listener(listener) {
        Ok(accept) => accept,
        Err(()) => return,
    };
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = connections.join_next(), if !connections.is_empty() => {}
            accepted = accept.accept() => {
                match accepted {
                    Ok(stream) => {
                        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                            tracing::debug!("api connection dropped: at capacity");
                            continue;
                        };
                        connections.spawn(serve_connection(
                            app.clone(),
                            http.clone(),
                            cancel.clone(),
                            permit,
                            stream,
                        ));
                    }
                    Err(error) if accept_failure_is_benign(&error) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, "api accept failed");
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                    }
                }
            }
        }
    }
    connections.abort_all();
}

/// Serves one accepted connection until it closes, idles out, or the listener cancels.
///
/// The permit bounds how many connections run at once. Silent or partial-preface
/// peers are dropped after the header timeout, a connection that finishes its
/// last request is dropped after [`IDLE_CONNECTION_TIMEOUT`], and one whose
/// pending response bodies yield no frame for that long is dropped too, so a
/// peer that never opens its flow-control window cannot hold a permit forever.
/// The soft-suspend lease is taken per request inside [`Auth`]
/// so an idle keep-alive socket holds no wake lease.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip_all,
        level = tracing::Level::DEBUG,
        name = "api-connection",
        fields(peer = ?stream.peer_addr().ok())
    )
)]
async fn serve_connection(
    app: AppService,
    http: Builder<TokioExecutor>,
    cancel: CancellationToken,
    _permit: tokio::sync::OwnedSemaphorePermit,
    stream: TcpStream,
) {
    let app = app.for_connection();
    let activity = Arc::clone(&app.activity);
    let io = TokioIo::new(stream);
    let service = TowerToHyperService::new(app);
    let conn = http.serve_connection(io, service);
    tokio::pin!(conn);
    let mut sweep = tokio::time::interval(CONNECTION_SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sweep.tick().await;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = conn.as_mut() => break,
            _ = sweep.tick() => {
                if activity.expired() {
                    tracing::debug!("api connection dropped: silent or idle");
                    break;
                }
            }
        }
    }
}

/// Listening socket the accept loop polls.
///
/// Kobo holds a plain `std` listener behind [`AsyncFd`] because its kernel
/// rejects `accept4`; every other target holds Tokio's `TcpListener`.
struct AcceptIo {
    #[cfg(feature = "kobo")]
    inner: tokio::io::unix::AsyncFd<std::net::TcpListener>,
    #[cfg(not(feature = "kobo"))]
    inner: TcpListener,
}

impl AcceptIo {
    /// Registers the listener with Tokio's reactor.
    ///
    /// [`TcpListener::into_std`] hands over the only owner of the descriptor,
    /// and that ownership moves into [`tokio::io::unix::AsyncFd`] here, so the
    /// descriptor stays valid, unchanged, and un-duplicated for the `AsyncFd`'s
    /// lifetime — the safety contract of
    /// [`tokio::io::unix::AsyncFd::register`].
    #[cfg(feature = "kobo")]
    fn from_listener(listener: TcpListener) -> Result<Self, ()> {
        let listener = match listener.into_std() {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(error = %error, "api listener unavailable");
                return Err(());
            }
        };
        let inner = match unsafe { tokio::io::unix::AsyncFd::register(listener) } {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(error = %error, "api listener registration failed");
                return Err(());
            }
        };
        Ok(Self { inner })
    }

    #[cfg(not(feature = "kobo"))]
    fn from_listener(listener: TcpListener) -> Result<Self, ()> {
        Ok(Self { inner: listener })
    }

    async fn accept(&self) -> std::io::Result<tokio::net::TcpStream> {
        accept_stream(&self.inner).await
    }
}

/// Accepts one connection.
///
/// On Kobo the kernel rejects `accept4` (`ENOSYS`), so that build uses plain
/// `accept` through [`AsyncFd`]. Other targets use Tokio's `TcpListener::accept`.
#[cfg(feature = "kobo")]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn accept_stream(
    listener: &tokio::io::unix::AsyncFd<std::net::TcpListener>,
) -> std::io::Result<tokio::net::TcpStream> {
    use std::os::fd::AsRawFd;
    loop {
        let mut guard = listener.readable().await?;
        match guard.try_io(|inner| unsafe { accept_fd(inner.get_ref().as_raw_fd()) }) {
            Ok(Ok(stream)) => return tokio::net::TcpStream::from_std(stream),
            Ok(Err(error)) => return Err(error),
            Err(_would_block) => continue,
        }
    }
}

/// Tokio accept on hosts where accept4 works.
#[cfg(not(feature = "kobo"))]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn accept_stream(listener: &TcpListener) -> std::io::Result<tokio::net::TcpStream> {
    listener.accept().await.map(|(stream, _)| stream)
}

/// Accepts one connection with plain `accept`.
///
/// # Safety
/// `fd` must be a live listening socket owned by the [`AsyncFd`]. The null peer
/// address asks the kernel to skip the address out-parameter. On success the
/// new descriptor is uniquely owned by the returned stream.
#[cfg(feature = "kobo")]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
unsafe fn accept_fd(fd: std::os::fd::RawFd) -> std::io::Result<std::net::TcpStream> {
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    let accepted = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
    if accepted < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { std::net::TcpStream::from_raw_fd(accepted as RawFd) };
    stream.set_nonblocking(true)?;
    unsafe { set_cloexec(stream.as_raw_fd()) }?;
    Ok(stream)
}

/// Marks `fd` close-on-exec so spawned children never inherit the socket.
///
/// # Safety
/// `fd` must be an open descriptor.
#[cfg(feature = "kobo")]
unsafe fn set_cloexec(fd: std::os::fd::RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Connection reset/abort while accepting is not a listener failure.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn accept_failure_is_benign(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset
    )
}

/// Device language if that site directory exists, otherwise English.
///
/// The full BCP-47 tag is tried first so a regional site such as `pt-BR` wins
/// over its bare language, then the language subtag, then English.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn site_locale(
    root: &Path,
    locale: Option<&unic_langid::LanguageIdentifier>,
) -> Locale {
    let Some(locale) = locale else {
        return Locale::LOCALE_EN;
    };
    let tags = [locale.to_string(), locale.language.to_string()];
    for tag in &tags {
        let Some(candidate) = locale_from_dir(tag) else {
            continue;
        };
        if tokio::fs::metadata(root.join(locale_dir(candidate)))
            .await
            .map(|meta| meta.is_dir())
            .unwrap_or(false)
        {
            return candidate;
        }
    }
    Locale::LOCALE_EN
}

#[derive(Clone)]
struct Auth {
    inner: ConnectRpcService,
    token: Option<SecretString>,
    scope: Scope,
    tx: mpsc::UnboundedSender<Command>,
    hub: Hub,
    inhibitor: Arc<Inhibitor>,
    failures: Arc<AtomicU32>,
}

impl Service<Request<Incoming>> for Auth {
    type Response = Response<LeasedBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::<Request<Incoming>>::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let scope = self.scope;
        let method = req.method().clone();
        let path = req.uri().path().to_owned();
        if let Some(token) = self.token.as_ref()
            && !open_status(&path)
        {
            if self.failures.load(Ordering::Relaxed) >= super::MAX_AUTH_FAILURES {
                report_auth_failure(&self.tx, &self.hub);
                let response =
                    ConnectError::unavailable("api stopped").into_http_response(req.headers());
                log_access(scope, &method, &path, response.status(), Duration::ZERO);
                return Box::pin(async { Ok(leased_response(response, None)) });
            }
            let presented = presented_bearer(req.headers());
            if bearer_ok(token, presented) {
                self.failures.store(0, Ordering::Relaxed);
            } else {
                if presented.is_some() {
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    report_auth_failure(&self.tx, &self.hub);
                }
                let response =
                    ConnectError::unauthenticated("unauthorized").into_http_response(req.headers());
                log_access(scope, &method, &path, response.status(), Duration::ZERO);
                return Box::pin(async { Ok(leased_response(response, None)) });
            }
        }
        let lease = match self
            .inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Api)
        {
            Ok(guard) => guard,
            Err(error) => {
                tracing::error!(
                    error = %error,
                    soft_suspend_lease = %SoftSuspendName::Api,
                    "failed to acquire soft-suspend lease for api request"
                );
                let response =
                    ConnectError::unavailable("api stopped").into_http_response(req.headers());
                log_access(scope, &method, &path, response.status(), Duration::ZERO);
                return Box::pin(async { Ok(leased_response(response, None)) });
            }
        };
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let started = Instant::now();
            let result = Service::call(&mut inner, req)
                .await
                .map(|response| leased_response(response, Some(lease)));
            if let Ok(response) = &result {
                log_access(scope, &method, &path, response.status(), started.elapsed());
            }
            result
        })
    }
}

/// Trace-level access log for one RPC, tagged with the listener it reached.
pub(super) fn log_access(
    scope: Scope,
    method: &Method,
    path: &str,
    status: StatusCode,
    elapsed: Duration,
) {
    tracing::trace!(
        router = scope.name(),
        method = %method,
        path,
        status = status.as_u16(),
        elapsed_us = elapsed.as_micros() as u64,
        "api request"
    );
}

/// GetStatus is public even when a bearer token is set.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path = path)))]
fn open_status(path: &str) -> bool {
    path.trim_end_matches('/')
        .ends_with("/cadmus.api.shared.v1.Status/GetStatus")
}

/// Tells the main loop that a request presented a bad credential.
///
/// The loop shuts the listener down once the shared count of consecutive
/// bad credentials reaches [`super::MAX_AUTH_FAILURES`], while [`Auth`]
/// already rejects requests. A matching credential clears the count, so a
/// client that proves it knows the token starts over.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn report_auth_failure(tx: &mpsc::UnboundedSender<Command>, hub: &Hub) {
    if tx.send(Command::AuthFailed).is_ok() {
        hub.send(Event::Api.into()).ok();
    }
}

/// Bearer credential a request presents, when it presents one at all.
///
/// A missing header, a non-`Bearer` scheme, or an empty `Bearer ` credential
/// all return `None`, so only real guesses count as strikes.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn presented_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty())
}

/// Whether the presented credential authorizes the request.
///
/// Debug builds accept anything, including a missing header, so a browser
/// replaying a stale token keeps working across emulator relaunches. Tests
/// and release builds require a matching Bearer.
#[cfg(all(debug_assertions, not(test)))]
fn bearer_ok(_token: &SecretString, _presented: Option<&str>) -> bool {
    true
}

#[cfg(any(not(debug_assertions), test))]
fn bearer_ok(token: &SecretString, presented: Option<&str>) -> bool {
    matches!(presented, Some(presented) if bearer_matches(token.expose_secret(), presented))
}

/// Constant-time Bearer compare.
#[cfg(any(not(debug_assertions), test))]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn bearer_matches(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();
    if expected.len() != presented.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in expected.iter().zip(presented) {
        diff |= left ^ right;
    }
    diff == 0
}

#[derive(Clone)]
struct AppService {
    rpc: Auth,
    root: Option<Arc<PathBuf>>,
    activity: Arc<ConnectionActivity>,
}

impl AppService {
    /// Copy for one accepted connection with a fresh activity gate.
    fn for_connection(&self) -> AppService {
        AppService {
            rpc: self.rpc.clone(),
            root: self.root.clone(),
            activity: Arc::new(ConnectionActivity::new()),
        }
    }
}

impl Service<Request<Incoming>> for AppService {
    type Response = Response<RespBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::poll_ready(&mut self.rpc, cx)
    }

    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let in_flight = InFlight::new(Arc::clone(&self.activity));
        let rpc = self.rpc.clone();
        let root = self.root.clone();
        Box::pin(async move {
            let response = respond(rpc, root.as_deref().map(PathBuf::as_path), req).await?;
            let (parts, body) = response.into_parts();
            Ok(Response::from_parts(
                parts,
                ActiveBody::new(body, in_flight),
            ))
        })
    }
}

/// RPC under /cadmus.api., document spec, else a static file when site_root is set.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip_all,
        level = tracing::Level::DEBUG,
        name = "api-request",
        fields(method = %req.method(), path = %req.uri().path())
    )
)]
async fn respond(
    mut rpc: Auth,
    root: Option<&Path>,
    req: Request<Incoming>,
) -> Result<Response<InnerBody>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let response = if path == OPENAPI_PATH {
        openapi(&method)
    } else if path.starts_with(RPC_PREFIX) {
        let response = Service::call(&mut rpc, req).await?;
        let (parts, body) = response.into_parts();
        Response::from_parts(parts, boxed_body(body))
    } else if let Some(root) = root {
        static_file(root, req).await
    } else {
        empty(StatusCode::NOT_FOUND)
    };
    tracing::debug!(
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        "api request"
    );
    Ok(response)
}

/// Public OpenAPI document. No bearer token: the protos are public docs.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn openapi(method: &http::Method) -> Response<InnerBody> {
    if method != http::Method::GET && method != http::Method::HEAD {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let spec = cadmus_api_proto::OPENAPI_JSON;
    let body = if method == http::Method::HEAD {
        Bytes::new()
    } else {
        Bytes::from_static(spec.as_bytes())
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::CONTENT_LENGTH, spec.len())
        .body(boxed_body(Full::new(body)))
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// GET/HEAD a file under the site root. 404 when the path escapes.
///
/// A bare locale path redirects to that locale's companion page. A `<file>.gz`
/// sibling is sent as stored with `Content-Encoding: gzip` when the client
/// accepts gzip, and inflated in memory when it does not. Stored files stream
/// in [`FILE_CHUNK_BYTES`] frames.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn static_file(root: &Path, req: Request<Incoming>) -> Response<InnerBody> {
    let method = req.method().clone();
    if method != http::Method::GET && method != http::Method::HEAD {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let head = method == http::Method::HEAD;
    if let Some(location) = companion_redirect(root, req.uri().path()).await {
        return Response::builder()
            .status(StatusCode::FOUND)
            .header(http::header::LOCATION, location)
            .body(boxed_body(Full::new(Bytes::new())))
            .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR));
    }
    let Some(file) = resolve(root, req.uri().path(), accepts_gzip(req.headers())).await else {
        return empty(StatusCode::NOT_FOUND);
    };
    let sent = match file.delivery {
        Delivery::Inflate => inflated_body(file.path, head).await,
        Delivery::Stored | Delivery::Gzip => stored_body(&file.path, head).await,
    };
    let (length, body) = match sent {
        Ok(sent) => sent,
        Err(status) => return empty(status),
    };
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, file.content_type)
        .header(http::header::CONTENT_LENGTH, length)
        .header(http::header::VARY, "accept-encoding");
    let builder = match file.delivery {
        Delivery::Gzip => builder.header(http::header::CONTENT_ENCODING, "gzip"),
        Delivery::Stored | Delivery::Inflate => builder,
    };
    builder
        .body(body)
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Length and body of a file sent as stored. HEAD gets an empty body.
async fn stored_body(path: &Path, head: bool) -> Result<(u64, InnerBody), StatusCode> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if head {
        return Ok((meta.len(), boxed_body(Full::new(Bytes::new()))));
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok((
        meta.len(),
        boxed_body(StreamBody::new(file_frames(file.take(meta.len())))),
    ))
}

/// Length and body of a `.gz` file inflated for a client without gzip.
///
/// Rare: every browser sends `Accept-Encoding: gzip`. The whole file is held
/// in memory, capped at [`MAX_FILE_BYTES`].
async fn inflated_body(path: PathBuf, head: bool) -> Result<(u64, InnerBody), StatusCode> {
    let bytes = match tokio::task::spawn_blocking(move || inflate(&path)).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "inflating a gzip site file failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(error) => {
            tracing::warn!(error = %error, "gzip inflate task failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let length = bytes.len() as u64;
    if length > MAX_FILE_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let body = if head { Bytes::new() } else { bytes };
    Ok((length, boxed_body(Full::new(body))))
}

/// Decompressed bytes of the gzip file at `path`, read to one byte past [`MAX_FILE_BYTES`].
fn inflate(path: &Path) -> std::io::Result<Bytes> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path)?)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut out)?;
    Ok(Bytes::from(out))
}

/// Read position of [`file_frames`].
enum FrameState<R> {
    /// Reading continues.
    Reading(R),
    /// The previous read failed; the stream ends.
    Done,
}

/// [`FILE_CHUNK_BYTES`] frames of `reader` for a streamed response body.
fn file_frames<R>(
    reader: R,
) -> impl futures_lite::stream::Stream<Item = Result<Frame<Bytes>, std::io::Error>> + Send
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    futures_lite::stream::unfold(FrameState::Reading(reader), |state| async move {
        let FrameState::Reading(mut reader) = state else {
            return None;
        };
        let mut buf = vec![0u8; FILE_CHUNK_BYTES];
        match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
            Ok(0) => None,
            Ok(read) => {
                buf.truncate(read);
                Some((
                    Ok(Frame::data(Bytes::from(buf))),
                    FrameState::Reading(reader),
                ))
            }
            Err(error) => Some((Err(error), FrameState::Done)),
        }
    })
}

/// Empty HTTP response with `status`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn empty(status: StatusCode) -> Response<InnerBody> {
    let mut response = Response::new(boxed_body(Full::new(Bytes::new())));
    *response.status_mut() = status;
    response
}

/// File the static site serves for one request.
struct SiteFile {
    /// Canonical file on disk.
    path: PathBuf,
    /// Content-Type of the requested file, never of its `.gz` sibling.
    content_type: &'static str,
    /// How the stored bytes reach the client.
    delivery: Delivery,
}

/// How a [`SiteFile`]'s stored bytes reach the client.
enum Delivery {
    /// The requested file itself, sent as stored.
    Stored,
    /// The `.gz` sibling, sent as stored with `Content-Encoding: gzip`.
    Gzip,
    /// The `.gz` sibling, inflated because the client does not accept gzip.
    Inflate,
}

/// Where a bare locale path such as `/en/` redirects: that locale's companion.
///
/// A redirect, not a rewrite, so the page's client router sees the companion
/// path and drops the site header the companion does not want.
async fn companion_redirect(root: &Path, url_path: &str) -> Option<String> {
    let segments = safe_segments(url_path)?;
    let [locale] = segments.as_slice() else {
        return None;
    };
    is_dir(&root.join(locale).join("companion"))
        .await
        .then(|| format!("/{locale}/companion/"))
}

/// Resolve a URL path under `root`. Rejects `..`, storybook, and escapes.
///
/// Prefers the `.gz` sibling when `gzip` is accepted, then the file itself,
/// then the `.gz` sibling to inflate. The device ships the Rust API docs as
/// `.gz` only.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn resolve(root: &Path, url_path: &str, gzip: bool) -> Option<SiteFile> {
    let mut segments = safe_segments(url_path)?;
    if segments.len() >= 2 && segments[1] == "api" {
        let local_api = root.join(&segments[0]).join("api");
        let use_shared = match tokio::fs::symlink_metadata(&local_api).await {
            Ok(meta) => meta.file_type().is_symlink(),
            Err(_) => true,
        };
        if use_shared {
            let rest = segments.split_off(2);
            segments = vec!["_shared".to_string(), "api".to_string()];
            segments.extend(rest);
        }
    }
    let mut path = root.to_path_buf();
    for segment in &segments {
        path.push(segment);
    }
    if tokio::fs::metadata(&path)
        .await
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
    {
        path.push("index.html");
    }
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let (file, delivery) = match contained(&root, &with_gz(&path)).await {
        Some(compressed) if gzip => (compressed, Delivery::Gzip),
        compressed => match contained(&root, &path).await {
            Some(file) => (file, Delivery::Stored),
            None => (compressed?, Delivery::Inflate),
        },
    };
    Some(SiteFile {
        path: file,
        content_type: content_type(&path),
        delivery,
    })
}

/// Whether `path` is a directory.
async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

/// Canonical `path` when it exists inside the canonical `root`.
async fn contained(root: &Path, path: &Path) -> Option<PathBuf> {
    let path = tokio::fs::canonicalize(path).await.ok()?;
    path.starts_with(root).then_some(path)
}

/// `path` with `.gz` appended to its file name.
fn with_gz(path: &Path) -> PathBuf {
    let mut gz = path.as_os_str().to_owned();
    gz.push(".gz");
    PathBuf::from(gz)
}

/// Whether `Accept-Encoding` allows gzip. An explicit `gzip` entry wins over `*`.
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut wildcard = false;
    for coding in headers
        .get_all(http::header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
    {
        let (name, params) = coding.split_once(';').unwrap_or((coding, ""));
        let refused = params.split(';').any(|param| {
            param
                .trim()
                .strip_prefix("q=")
                .and_then(|q| q.trim().parse::<f32>().ok())
                == Some(0.0)
        });
        match name.trim() {
            "*" => wildcard = !refused,
            name if name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip") => {
                return !refused;
            }
            _ => {}
        }
    }
    wildcard
}

/// Path segments that stay inside the site root.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn safe_segments(path: &str) -> Option<Vec<String>> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    if decoded.contains('\\') || decoded.contains('\0') {
        return None;
    }
    let mut out = Vec::new();
    for segment in decoded.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." || segment == "storybook" {
            return None;
        }
        out.push(segment.to_string());
    }
    Some(out)
}

/// Content-Type from the suffix, shared with `GetFile`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("avif") => "image/avif",
        Some("ico") => "image/vnd.microsoft.icon",
        Some("pdf") => "application/pdf",
        Some("epub") => "application/epub+zip",
        Some("mobi") => "application/x-mobipocket-ebook",
        Some("cbz") => "application/vnd.comicbook+zip",
        Some("cbr") => "application/vnd.comicbook-rar",
        Some("djvu" | "djv") => "image/vnd.djvu",
        Some("fb2") => "application/x-fictionbook+xml",
        Some("xps" | "oxps") => "application/oxps",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("txt" | "md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionActivity, Delivery, IDLE_CONNECTION_TIMEOUT, Locale, accept_failure_is_benign,
        accepts_gzip, bearer_matches, companion_redirect, presented_bearer, resolve, safe_segments,
        site_locale,
    };
    use http::{HeaderMap, HeaderValue, header};
    use std::io::{Error, ErrorKind};
    use std::sync::atomic::Ordering;

    fn authorization(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn stalled_body_expires_after_idle_timeout_but_progressing_one_does_not() {
        let activity = ConnectionActivity::new();
        let idle = IDLE_CONNECTION_TIMEOUT.as_millis() as u64;
        activity.request_started();
        let start = activity.progress_ms.load(Ordering::Relaxed);
        assert!(!activity.expired_at(start + idle - 1));
        assert!(activity.expired_at(start + idle));
        activity.body_progressed();
        let progressed = activity.progress_ms.load(Ordering::Relaxed);
        assert!(!activity.expired_at(progressed + idle - 1));
        assert!(activity.expired_at(progressed + idle));
    }

    #[test]
    fn presented_bearer_only_counts_a_non_empty_bearer_credential() {
        assert_eq!(
            presented_bearer(&authorization("Bearer 123456")),
            Some("123456")
        );
        assert_eq!(presented_bearer(&HeaderMap::new()), None);
        assert_eq!(presented_bearer(&authorization("Bearer123456")), None);
        assert_eq!(presented_bearer(&authorization("Bearer ")), None);
        assert_eq!(presented_bearer(&authorization("Basic 123456")), None);
    }

    #[test]
    fn bearer_matches_only_accepts_the_exact_token() {
        let token = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
        assert!(bearer_matches(token, token));
        assert!(!bearer_matches(token, "a1b2c3d4e5f60718293a4b5c6d7e8f91"));
        assert!(!bearer_matches(token, "a1b2c3d4e5f60718293a4b5c6d7e8f90ff"));
        assert!(!bearer_matches(token, ""));
        assert!(!bearer_matches(
            token,
            "Bearer a1b2c3d4e5f60718293a4b5c6d7e8f90"
        ));
    }

    #[test]
    fn accept_failure_is_benign_only_for_aborted_connections() {
        assert!(accept_failure_is_benign(&Error::from(
            ErrorKind::ConnectionAborted
        )));
        assert!(accept_failure_is_benign(&Error::from(
            ErrorKind::ConnectionReset
        )));
        assert!(!accept_failure_is_benign(&Error::from(
            ErrorKind::OutOfMemory
        )));
    }

    fn accept_encoding(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn accepts_gzip_follows_accept_encoding_weights() {
        assert!(accepts_gzip(&accept_encoding("gzip")));
        assert!(accepts_gzip(&accept_encoding("br, GZIP;q=0.5")));
        assert!(accepts_gzip(&accept_encoding("gzip, deflate, br, zstd")));
        assert!(accepts_gzip(&accept_encoding("*")));
        assert!(accepts_gzip(&accept_encoding("*, gzip;q=1")));
        assert!(!accepts_gzip(&accept_encoding("gzip;q=0")));
        assert!(!accepts_gzip(&accept_encoding("*, gzip; q=0")));
        assert!(!accepts_gzip(&accept_encoding("*;q=0")));
        assert!(!accepts_gzip(&accept_encoding("br, deflate")));
        assert!(!accepts_gzip(&accept_encoding("identity")));
        assert!(!accepts_gzip(&HeaderMap::new()));
    }

    #[tokio::test]
    async fn resolve_picks_the_delivery_by_what_is_stored_and_accepted() {
        let root = tempfile::tempdir().unwrap();
        let api = root.path().join("_shared/api/cadmus_core");
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(api.join("index.html.gz"), b"gz").unwrap();
        std::fs::write(root.path().join("both.css"), b"css").unwrap();
        std::fs::write(root.path().join("both.css.gz"), b"gz").unwrap();

        let gzipped = resolve(root.path(), "/en/api/cadmus_core/", true)
            .await
            .unwrap();
        assert!(matches!(gzipped.delivery, Delivery::Gzip));
        assert_eq!(gzipped.content_type, "text/html; charset=utf-8");
        assert!(gzipped.path.ends_with("cadmus_core/index.html.gz"));

        let inflated = resolve(root.path(), "/en/api/cadmus_core/", false)
            .await
            .unwrap();
        assert!(matches!(inflated.delivery, Delivery::Inflate));
        assert_eq!(inflated.content_type, "text/html; charset=utf-8");

        let stored = resolve(root.path(), "/both.css", false).await.unwrap();
        assert!(matches!(stored.delivery, Delivery::Stored));
        assert!(stored.path.ends_with("both.css"));

        assert!(resolve(root.path(), "/absent.css", true).await.is_none());
    }

    #[tokio::test]
    async fn a_bare_locale_path_redirects_to_its_companion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("en/companion")).unwrap();
        std::fs::create_dir_all(root.path().join("en/guide")).unwrap();
        std::fs::create_dir_all(root.path().join("_next")).unwrap();

        for url in ["/en/", "/en"] {
            assert_eq!(
                companion_redirect(root.path(), url).await.as_deref(),
                Some("/en/companion/"),
                "{url}"
            );
        }
        for url in ["/", "/en/guide/", "/en/companion/", "/_next/", "/fr/"] {
            assert_eq!(companion_redirect(root.path(), url).await, None, "{url}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_rejects_a_gz_symlink_that_leaves_the_root() {
        let outside = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.gz"), b"secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.gz"),
            root.path().join("page.html.gz"),
        )
        .unwrap();

        assert!(resolve(root.path(), "/page.html", true).await.is_none());
        assert!(resolve(root.path(), "/page.html", false).await.is_none());
    }

    #[test]
    fn safe_segments_reject_traversal_and_storybook() {
        assert!(safe_segments("/en/guide/index.html").is_some());
        assert!(safe_segments("/../etc/passwd").is_none());
        assert!(safe_segments("/en/%2e%2e/secret").is_none());
        assert!(safe_segments("/storybook/index.html").is_none());
        assert!(safe_segments("/en/storybook").is_none());
    }

    #[tokio::test]
    async fn site_locale_tries_the_full_tag_then_the_language() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("fr")).unwrap();
        let regional: unic_langid::LanguageIdentifier = "fr-CA".parse().unwrap();
        assert_eq!(
            site_locale(root.path(), Some(&regional)).await,
            Locale::LOCALE_FR
        );
        let absent: unic_langid::LanguageIdentifier = "pt-BR".parse().unwrap();
        assert_eq!(
            site_locale(root.path(), Some(&absent)).await,
            Locale::LOCALE_EN
        );
        assert_eq!(site_locale(root.path(), None).await, Locale::LOCALE_EN);
    }
}
