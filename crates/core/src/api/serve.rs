//! HTTP listener: Connect RPCs, `GET /openapi.json`, optional static site,
//! optional bearer auth.

use super::Command;
use super::handler::PublicRpc;
use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
use crate::view::Hub;
use bytes::Bytes;
use cadmus_api_proto::cadmus::api::shared::v1::{FileOpsExt, Locale, SettingsExt, StatusExt};
use cadmus_api_proto::locale_from_dir;

pub(super) use cadmus_api_proto::locale_dir;
use connectrpc::{ConnectError, ConnectRpcService, Router};
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::{Either, Full};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use secrecy::{ExposeSecret, SecretString};
use std::convert::Infallible;
use std::future::Future;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::Service;

const RPC_PREFIX: &str = "/cadmus.api.";
const OPENAPI_PATH: &str = "/openapi.json";
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

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

type RespBody = Either<connectrpc::ConnectRpcBody, Full<Bytes>>;

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
        tx,
        hub,
        inhibitor: Arc::clone(&inhibitor),
        file_root: config.file_root.map(Arc::new),
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
    };
    let app = AppService {
        rpc,
        root: config.site_root.map(Arc::new),
    };
    let http = Builder::new(TokioExecutor::new());
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
                        connections.spawn(serve_connection(
                            app.clone(),
                            http.clone(),
                            cancel.clone(),
                            Arc::clone(&inhibitor),
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

/// Serves one accepted connection until it closes or the listener cancels.
///
/// Each connection runs inside its own span, so the requests it carries nest
/// under it while the accept loop stays span-free.
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
    inhibitor: Arc<Inhibitor>,
    stream: TcpStream,
) {
    let _soft_suspend = match inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Api) {
        Ok(guard) => Some(guard),
        Err(error) => {
            tracing::error!(
                error = %error,
                soft_suspend_lease = %SoftSuspendName::Api,
                "failed to acquire soft-suspend lease for api connection"
            );
            None
        }
    };
    let io = TokioIo::new(stream);
    let service = TowerToHyperService::new(app);
    let conn = http.serve_connection(io, service);
    tokio::pin!(conn);
    tokio::select! {
        _ = cancel.cancelled() => {}
        _ = conn.as_mut() => {}
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
    #[cfg(feature = "kobo")]
    fn from_listener(listener: TcpListener) -> Result<Self, ()> {
        let listener = match listener.into_std() {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(error = %error, "api listener unavailable");
                return Err(());
            }
        };
        let inner = match tokio::io::unix::AsyncFd::new(listener) {
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
    use std::os::fd::{FromRawFd, RawFd};
    let accepted = unsafe { libc::accept(fd, std::ptr::null_mut(), std::ptr::null_mut()) };
    if accepted < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { std::net::TcpStream::from_raw_fd(accepted as RawFd) };
    stream.set_nonblocking(true)?;
    Ok(stream)
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
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn site_locale(
    root: &Path,
    locale: Option<&unic_langid::LanguageIdentifier>,
) -> Locale {
    let Some(locale) = locale else {
        return Locale::LOCALE_EN;
    };
    let kind = locale_from_dir(locale.language.as_str());
    let dir = locale_dir(kind);
    if tokio::fs::metadata(root.join(dir))
        .await
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
    {
        kind
    } else {
        Locale::LOCALE_EN
    }
}

#[derive(Clone)]
struct Auth {
    inner: ConnectRpcService,
    token: Option<SecretString>,
    scope: Scope,
}

impl Service<Request<Incoming>> for Auth {
    type Response = Response<connectrpc::ConnectRpcBody>;
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
            && !bearer_matches(token.expose_secret(), req.headers())
        {
            let response =
                ConnectError::unauthenticated("unauthorized").into_http_response(req.headers());
            log_access(scope, &method, &path, response.status(), Duration::ZERO);
            return Box::pin(async { Ok(response) });
        }
        let mut inner = self.inner.clone();
        Box::pin(async move {
            let started = Instant::now();
            let result = Service::call(&mut inner, req).await;
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
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(path)))]
fn open_status(path: &str) -> bool {
    path.trim_end_matches('/')
        .ends_with("/cadmus.api.shared.v1.Status/GetStatus")
}

/// Debug emulator skips bearer checks so curl works without the overlay token.
#[cfg(debug_assertions)]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn bearer_matches(_expected: &str, _headers: &HeaderMap) -> bool {
    true
}

/// Constant-time Bearer compare. Release only.
#[cfg(not(debug_assertions))]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn bearer_matches(expected: &str, headers: &HeaderMap) -> bool {
    let Some(presented) = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
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
}

impl Service<Request<Incoming>> for AppService {
    type Response = Response<RespBody>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::poll_ready(&mut self.rpc, cx)
    }

    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let rpc = self.rpc.clone();
        let root = self.root.clone();
        Box::pin(async move { respond(rpc, root.as_deref().map(PathBuf::as_path), req).await })
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
) -> Result<Response<RespBody>, Infallible> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let response = if path == OPENAPI_PATH {
        openapi(&method)
    } else if path.starts_with(RPC_PREFIX) {
        let response = Service::call(&mut rpc, req).await?;
        let (parts, body) = response.into_parts();
        Response::from_parts(parts, Either::Left(body))
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
fn openapi(method: &http::Method) -> Response<RespBody> {
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
        .body(Either::Right(Full::new(body)))
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// GET/HEAD a file under the site root. 404 when the path escapes.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn static_file(root: &Path, req: Request<Incoming>) -> Response<RespBody> {
    let method = req.method().clone();
    if method != http::Method::GET && method != http::Method::HEAD {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let Some(path) = resolve(root, req.uri().path()).await else {
        return empty(StatusCode::NOT_FOUND);
    };
    let meta = match tokio::fs::metadata(&path).await {
        Ok(meta) => meta,
        Err(_) => return empty(StatusCode::NOT_FOUND),
    };
    if meta.len() > MAX_FILE_BYTES {
        return empty(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let bytes = if method == http::Method::HEAD {
        Bytes::new()
    } else {
        match tokio::fs::read(&path).await {
            Ok(bytes) => Bytes::from(bytes),
            Err(_) => return empty(StatusCode::NOT_FOUND),
        }
    };
    let len = if method == http::Method::HEAD {
        meta.len()
    } else {
        bytes.len() as u64
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, content_type(&path))
        .header(http::header::CONTENT_LENGTH, len)
        .body(Either::Right(Full::new(bytes)))
        .unwrap_or_else(|_| empty(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Empty HTTP response with `status`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn empty(status: StatusCode) -> Response<RespBody> {
    Response::builder()
        .status(status)
        .body(Either::Right(Full::new(Bytes::new())))
        .expect("empty response")
}

/// Resolve a URL path under `root`. Rejects `..` and storybook.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn resolve(root: &Path, url_path: &str) -> Option<PathBuf> {
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
    let path = tokio::fs::canonicalize(&path).await.ok()?;
    path.starts_with(&root).then_some(path)
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
    use super::{accept_failure_is_benign, safe_segments};
    use std::io::{Error, ErrorKind};

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

    #[test]
    fn safe_segments_reject_traversal_and_storybook() {
        assert!(safe_segments("/en/guide/index.html").is_some());
        assert!(safe_segments("/../etc/passwd").is_none());
        assert!(safe_segments("/en/%2e%2e/secret").is_none());
        assert!(safe_segments("/storybook/index.html").is_none());
        assert!(safe_segments("/en/storybook").is_none());
    }
}
