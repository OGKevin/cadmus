use super::transport::{CadmusHttpService, TransportError};
use super::types::{
    AccessTokenResponse, DeviceCodeResponse, ScopeError, TokenPollResult, VerifyScopesError,
};
use crate::github::GithubError;
use crate::http::{ChunkedDownloadError, Client, USER_AGENT};
use bytes::Bytes;
use http::header::{ACCEPT, HeaderName, HeaderValue, USER_AGENT as USER_AGENT_HEADER};
use http::{HeaderMap, StatusCode, Uri};
use http_body_util::BodyExt;
use octocrab::service::middleware::auth_header::AuthHeaderLayer;
use octocrab::service::middleware::base_uri::BaseUriLayer;
use octocrab::service::middleware::extra_headers::ExtraHeadersLayer;
use octocrab::{AuthState, Octocrab, OctocrabBuilder};
use reqwest_middleware::RequestBuilder;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// GitHub OAuth App client ID, baked in at build time via `GH_OAUTH_CLIENT_ID` env var.
///
/// Kept private so callers never need to know or pass it; [`GithubClient`] uses it internally.
const GITHUB_OAUTH_CLIENT_ID: &str = env!("GH_OAUTH_CLIENT_ID");

const API_URI: &str = "https://api.github.com";
const UPLOAD_URI: &str = "https://uploads.github.com";
const DEVICE_URI: &str = "https://github.com";

/// OAuth scopes that the saved token must have for OTA operations to succeed.
///
/// This is the single source of truth for required scopes. Both
/// [`GithubClient::initiate_device_flow`] and
/// [`GithubClient::verify_token_scopes`] derive from this list, so adding or
/// removing a scope here is the only change needed.
///
/// Current requirements:
/// - `public_repo` — required to download Actions artifacts from public repositories
///   (`repo` also satisfies this requirement)
pub const REQUIRED_SCOPES: &[&str] = &["public_repo"];

/// Returns whether `granted` satisfies a scope listed in [`REQUIRED_SCOPES`].
///
/// GitHub reports broader classic PAT scopes under different names — for
/// example `repo` covers everything `public_repo` allows — so this helper
/// accepts those supersets during verification.
fn grants_required_scope(required: &str, granted: &[&str]) -> bool {
    if granted.contains(&required) {
        return true;
    }

    match required {
        "public_repo" => granted.contains(&"repo"),
        _ => false,
    }
}

/// Failure from an `octocrab` call on the shared HTTP client.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GithubRequestError {
    #[error(transparent)]
    Middleware(#[from] reqwest_middleware::Error),
    #[error("{0}")]
    Message(String),
}

/// Non-success HTTP status from a GitHub JSON call.
#[derive(Debug)]
pub(crate) struct ApiStatusError {
    status: StatusCode,
}

impl ApiStatusError {
    pub(crate) fn from_status(status: StatusCode) -> Self {
        Self { status }
    }

    pub(crate) fn status(&self) -> Option<StatusCode> {
        Some(self.status)
    }

    pub(crate) fn status_code(&self) -> StatusCode {
        self.status
    }
}

impl std::fmt::Display for ApiStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP status error ({})", self.status)
    }
}

impl std::error::Error for ApiStatusError {}

/// JSON response from an `octocrab` GitHub call.
pub(crate) struct ApiResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl ApiResponse {
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    pub(crate) fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub(crate) fn error_for_status(self) -> Result<Self, ApiStatusError> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(ApiStatusError::from_status(self.status))
        }
    }

    pub(crate) async fn json<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }
}

enum ApiAuth {
    Authenticated,
    Public,
}

/// Builder for one GitHub JSON request sent through `octocrab`.
pub(crate) struct ApiRequest<'a> {
    client: &'a GithubClient,
    auth: ApiAuth,
    url: String,
    headers: HeaderMap,
    error: Option<GithubRequestError>,
}

impl ApiRequest<'_> {
    /// Adds one header.
    ///
    /// Canonical names such as `Accept` are stored in lowercase. An invalid
    /// name or value is recorded and returned from [`Self::send`] instead of
    /// panicking.
    pub(crate) fn header(mut self, name: &'static str, value: &str) -> Self {
        if self.error.is_some() {
            return self;
        }
        let name = match HeaderName::from_bytes(name.as_bytes()) {
            Ok(name) => name,
            Err(error) => {
                self.error = Some(GithubRequestError::Message(error.to_string()));
                return self;
            }
        };
        match HeaderValue::from_str(value) {
            Ok(value) => {
                self.headers.insert(name, value);
            }
            Err(error) => {
                self.error = Some(GithubRequestError::Message(error.to_string()));
            }
        }
        self
    }

    pub(crate) async fn send(self) -> Result<ApiResponse, GithubRequestError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let crab = match self.auth {
            ApiAuth::Authenticated => self.client.authenticated()?,
            ApiAuth::Public => self.client.unauthenticated(),
        };
        let headers = if self.headers.is_empty() {
            None
        } else {
            Some(self.headers)
        };
        let response = crab
            ._get_with_headers(self.url, headers)
            .await
            .map_err(map_octocrab)?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(map_octocrab)?
            .to_bytes();
        Ok(ApiResponse {
            status,
            headers,
            body,
        })
    }
}

/// GitHub API client.
///
/// REST calls and device flow go through `octocrab`. The leaf service is the
/// shared [`crate::http::Client`], so retries and request spans match
/// every other HTTP call. Chunked downloads stay on [`Self::get`] and
/// [`Self::get_unauthenticated`] so each range request can be built separately.
///
/// # Examples
///
/// ```no_run
/// use cadmus_core::github::GithubClient;
///
/// // Unauthenticated client for public endpoints
/// let client = GithubClient::new(None).expect("failed to build client");
/// ```
///
/// ```no_run
/// use cadmus_core::github::GithubClient;
/// use secrecy::SecretString;
///
/// // Authenticated client for private/token-gated endpoints
/// let token = SecretString::from("ghp_…".to_owned());
/// let client = GithubClient::new(Some(token)).expect("failed to build client");
/// ```
pub struct GithubClient {
    http: Client,
    token: Option<SecretString>,
    api: OnceLock<Octocrab>,
    public_api: OnceLock<Octocrab>,
    device: OnceLock<Octocrab>,
}

impl GithubClient {
    /// Creates a new client with optional GitHub token authentication.
    ///
    /// Uses `webpki-roots` certificates for TLS — no system cert store
    /// required, which matters on Kobo devices that ship without a CA bundle.
    /// The `octocrab` services are built on first use, on the runtime that
    /// issues the request.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying HTTP client fails to build.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cadmus_core::github::GithubClient;
    ///
    /// let client = GithubClient::new(None).expect("failed to build client");
    /// ```
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn new(token: Option<SecretString>) -> Result<Self, GithubError> {
        tracing::debug!(token_provided = token.is_some(), "Building GitHub client");

        let http = Client::new()?;

        tracing::debug!("GitHub client built successfully");
        Ok(Self {
            http,
            token,
            api: OnceLock::new(),
            public_api: OnceLock::new(),
            device: OnceLock::new(),
        })
    }

    /// Returns a GET request builder with the `Authorization` header set if a
    /// token is present.
    ///
    /// Chunked downloads use this builder once per range. JSON GitHub calls
    /// go through [`Self::api_get`] so they use `octocrab`.
    pub fn get(&self, url: &str) -> RequestBuilder {
        self.with_auth(self.http.get(url))
    }

    /// Returns a POST request builder with the `Authorization` header set if a
    /// token is present.
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.with_auth(self.http.post(url))
    }

    /// Returns a GET request builder **without** any `Authorization` header.
    ///
    /// Used for public URLs (e.g. release asset downloads) where sending a
    /// token would cause GitHub to reject the request with a 401.
    pub fn get_unauthenticated(&self, url: &str) -> RequestBuilder {
        self.http.get(url)
    }

    /// Returns an authenticated JSON GET sent through `octocrab`.
    pub(crate) fn api_get(&self, url: &str) -> ApiRequest<'_> {
        if self.token.is_none() {
            tracing::warn!("Authentication requested but no token configured");
        }
        self.api_request(ApiAuth::Authenticated, url)
    }

    /// Returns an unauthenticated JSON GET sent through `octocrab`.
    ///
    /// Release metadata uses this. A token on the request makes GitHub reject
    /// some public asset URLs with 401.
    pub(crate) fn api_get_unauthenticated(&self, url: &str) -> ApiRequest<'_> {
        self.api_request(ApiAuth::Public, url)
    }

    fn api_request<'a>(&'a self, auth: ApiAuth, url: &str) -> ApiRequest<'a> {
        ApiRequest {
            client: self,
            auth,
            url: url.to_owned(),
            headers: HeaderMap::new(),
            error: None,
        }
    }

    /// Downloads a file to `dest` using HTTP Range requests.
    ///
    /// Delegates to [`Client::download`]. `request_builder` is called once per
    /// chunk to produce a `RequestBuilder` for the given URL.
    ///
    /// # Errors
    ///
    /// Returns `ChunkedDownloadError` if the file cannot be written or if all
    /// retry attempts for any chunk fail.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, request_builder, progress_callback))
    )]
    pub async fn download<B, F>(
        &self,
        url: &str,
        total_size: u64,
        dest: &PathBuf,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<crate::http::CancelFunc<'_>>,
    ) -> Result<(), ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        self.http
            .download(
                url,
                total_size,
                dest,
                request_builder,
                progress_callback,
                should_cancel,
            )
            .await
    }

    fn with_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.token {
            Some(token) => {
                builder.header("Authorization", format!("Bearer {}", token.expose_secret()))
            }
            None => {
                tracing::warn!("Authentication requested but no token configured");
                builder
            }
        }
    }

    fn authenticated(&self) -> Result<Octocrab, GithubRequestError> {
        if let Some(crab) = self.api.get() {
            return Ok(crab.clone());
        }
        let header = self.token.as_ref().map(bearer).transpose()?;
        let crab = build_octocrab(
            self.http.middleware(),
            Uri::from_static(API_URI),
            header,
            standard_headers(),
        );
        let _ = self.api.set(crab.clone());
        Ok(self.api.get().cloned().unwrap_or(crab))
    }

    fn unauthenticated(&self) -> Octocrab {
        self.public_api
            .get_or_init(|| {
                build_octocrab(
                    self.http.middleware(),
                    Uri::from_static(API_URI),
                    None,
                    standard_headers(),
                )
            })
            .clone()
    }

    fn device_flow(&self) -> Octocrab {
        self.device
            .get_or_init(|| {
                let mut headers = standard_headers();
                headers.push((ACCEPT, HeaderValue::from_static("application/json")));
                build_octocrab(
                    self.http.middleware(),
                    Uri::from_static(DEVICE_URI),
                    None,
                    headers,
                )
            })
            .clone()
    }

    /// Initiates GitHub device flow authentication.
    ///
    /// POSTs to `/login/device/code` to obtain a short user code and the
    /// verification URL. The caller must display these to the user and then
    /// call [`poll_device_token`](Self::poll_device_token) repeatedly until
    /// authorization completes or the code expires.
    ///
    /// The required OAuth scopes are derived from [`REQUIRED_SCOPES`] so this
    /// method and [`verify_token_scopes`](Self::verify_token_scopes) always
    /// stay in sync.
    ///
    /// # Errors
    ///
    /// Returns an error if the network request fails or GitHub returns a
    /// non-2xx status.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cadmus_core::github::GithubClient;
    ///
    /// # async fn example() {
    /// let client = GithubClient::new(None).expect("failed to build client");
    /// let response = client.initiate_device_flow().await.expect("device flow failed");
    /// println!("Go to {} and enter {}", response.verification_uri, response.user_code);
    /// # }
    /// ```
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn initiate_device_flow(&self) -> Result<DeviceCodeResponse, String> {
        tracing::info!(
            client_id = GITHUB_OAUTH_CLIENT_ID,
            "Initiating GitHub device auth flow"
        );

        let scope = REQUIRED_SCOPES.join(",");
        tracing::debug!(scope = %scope, "Requesting device code with scopes");

        let client_id = SecretString::from(GITHUB_OAUTH_CLIENT_ID.to_owned());
        let codes = self
            .device_flow()
            .authenticate_as_device(&client_id, REQUIRED_SCOPES.iter().copied())
            .await
            .map_err(|error| format!("Device code request failed: {error}"))?;

        let device_code_response = DeviceCodeResponse {
            device_code: codes.device_code,
            user_code: codes.user_code,
            verification_uri: codes.verification_uri,
            expires_in: codes.expires_in,
            interval: codes.interval,
        };

        tracing::debug!(
            verification_uri = %device_code_response.verification_uri,
            expires_in = device_code_response.expires_in,
            interval = device_code_response.interval,
            "Device code obtained"
        );

        Ok(device_code_response)
    }

    /// Verifies that the current token has all scopes listed in
    /// [`REQUIRED_SCOPES`].
    ///
    /// Makes a lightweight `GET /user` request and reads the
    /// `X-OAuth-Scopes` response header, which GitHub includes on every
    /// authenticated API call. Returns `Ok(())` if all required scopes are
    /// present, or `Err(missing)` listing the absent scope names.
    ///
    /// Call this once before starting a download to catch stale tokens
    /// early, rather than failing mid-download with confusing 403.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the network request fails, GitHub returns a non-2xx
    /// status, or one or more required scopes are absent.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cadmus_core::github::GithubClient;
    /// use secrecy::SecretString;
    ///
    /// # async fn example() {
    /// let token = SecretString::from("ghp_…".to_owned());
    /// let client = GithubClient::new(Some(token)).expect("failed to build client");
    ///
    /// match client.verify_token_scopes().await {
    ///     Ok(()) => println!("Token has all required scopes"),
    ///     Err(e) => println!("Error: {}", e),
    /// }
    /// # }
    /// ```
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn verify_token_scopes(&self) -> Result<(), VerifyScopesError> {
        tracing::debug!("Verifying token scopes");

        let response = self
            .api_get("https://api.github.com/user")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(verify_transport)?
            .error_for_status()
            .map_err(|error| VerifyScopesError::HttpStatus {
                status: error.status_code(),
            })?;

        let granted: Vec<&str> = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').map(str::trim).collect())
            .unwrap_or_default();

        tracing::debug!(granted = ?granted, required = ?REQUIRED_SCOPES, "Comparing token scopes");

        let missing: Vec<String> = REQUIRED_SCOPES
            .iter()
            .filter(|&&required| !grants_required_scope(required, &granted))
            .map(|&s| s.to_owned())
            .collect();

        if missing.is_empty() {
            tracing::debug!("Token scopes verified — all required scopes present");
            Ok(())
        } else {
            tracing::warn!(missing = ?missing, "Token is missing required scopes");
            Err(VerifyScopesError::from(ScopeError::new(missing)))
        }
    }

    /// Polls GitHub once to check if the user has authorized the device.
    ///
    /// Must be called at least `interval` seconds apart (from the
    /// [`DeviceCodeResponse`]). GitHub returns `slow_down` if polled too
    /// frequently; the caller must add 5 seconds to the interval before the
    /// next attempt.
    ///
    /// # Arguments
    ///
    /// * `device_code` - The `device_code` from [`initiate_device_flow`](Self::initiate_device_flow)
    ///
    /// # Errors
    ///
    /// Returns an error if the network request fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cadmus_core::github::GithubClient;
    /// use cadmus_core::github::TokenPollResult;
    /// use std::time::Duration;
    ///
    /// # async fn example() {
    /// let client = GithubClient::new(None).expect("failed to build client");
    /// let flow = client.initiate_device_flow().await.expect("device flow failed");
    ///
    /// loop {
    ///     tokio::time::sleep(Duration::from_secs(flow.interval)).await;
    ///     match client.poll_device_token(&flow.device_code).await.expect("poll failed") {
    ///         TokenPollResult::Complete(token) => break,
    ///         TokenPollResult::Pending => continue,
    ///         _ => break,
    ///     }
    /// }
    /// # }
    /// ```
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn poll_device_token(&self, device_code: &str) -> Result<TokenPollResult, String> {
        tracing::debug!("Polling GitHub for device token");

        let response = self
            .device_flow()
            ._post(
                "https://github.com/login/oauth/access_token",
                Some(&DeviceTokenPoll {
                    client_id: GITHUB_OAUTH_CLIENT_ID,
                    device_code,
                    grant_type: "urn:ietf:params:oauth:grant-type:device_code",
                }),
            )
            .await
            .map_err(|error| format!("Token poll request failed: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("Token poll error response: HTTP {status}"));
        }

        let body = self
            .device_flow()
            .body_to_string(response)
            .await
            .map_err(|error| format!("Failed to parse token response: {error}"))?;
        let body: AccessTokenResponse = serde_json::from_str(&body)
            .map_err(|error| format!("Failed to parse token response: {error}"))?;

        if let Some(token) = body.access_token {
            tracing::info!("Device flow authorization complete");
            return Ok(TokenPollResult::Complete(SecretString::from(token)));
        }

        match body.error.as_deref() {
            Some("authorization_pending") => {
                tracing::debug!("Device flow authorization pending");
                Ok(TokenPollResult::Pending)
            }
            Some("slow_down") => {
                tracing::warn!("Device flow polling too fast — caller must increase interval");
                Ok(TokenPollResult::SlowDown)
            }
            Some("expired_token") => {
                tracing::warn!("Device flow code expired");
                Ok(TokenPollResult::Expired)
            }
            Some("access_denied") => {
                tracing::info!("Device flow cancelled by user");
                Ok(TokenPollResult::Cancelled)
            }
            Some(other) => {
                tracing::error!(error = other, "Unexpected device flow error");
                Err(format!("Unexpected device flow error: {other}"))
            }
            None => {
                tracing::error!("Empty body from token poll endpoint");
                Err("Empty response from token endpoint".to_owned())
            }
        }
    }
}

#[derive(Serialize)]
struct DeviceTokenPoll<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'a str,
}

fn bearer(token: &SecretString) -> Result<HeaderValue, GithubRequestError> {
    let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
        .map_err(|error| GithubRequestError::Message(error.to_string()))?;
    value.set_sensitive(true);
    Ok(value)
}

fn standard_headers() -> Vec<(HeaderName, HeaderValue)> {
    vec![(USER_AGENT_HEADER, HeaderValue::from_static(USER_AGENT))]
}

fn build_octocrab(
    client: reqwest_middleware::ClientWithMiddleware,
    base_uri: Uri,
    auth_header: Option<HeaderValue>,
    extra_headers: Vec<(HeaderName, HeaderValue)>,
) -> Octocrab {
    let upload_uri = Uri::from_static(UPLOAD_URI);
    let base = BaseUriLayer::new(base_uri.clone());
    let extra = ExtraHeadersLayer::new(Arc::new(extra_headers));
    let auth = AuthHeaderLayer::new(auth_header, base_uri, upload_uri);
    match OctocrabBuilder::new_empty()
        .with_service(CadmusHttpService::new(client))
        .with_layer(&base)
        .with_layer(&extra)
        .with_layer(&auth)
        .with_auth(AuthState::None)
        .build()
    {
        Ok(crab) => crab,
        Err(error) => match error {},
    }
}

fn map_octocrab(error: octocrab::Error) -> GithubRequestError {
    match error {
        octocrab::Error::Service { source, .. } => match source.downcast::<TransportError>() {
            Ok(error) => match *error {
                TransportError::Middleware(error) => GithubRequestError::Middleware(error),
                other => GithubRequestError::Message(other.to_string()),
            },
            Err(error) => GithubRequestError::Message(error.to_string()),
        },
        other => GithubRequestError::Message(other.to_string()),
    }
}

fn verify_transport(error: GithubRequestError) -> VerifyScopesError {
    match error {
        GithubRequestError::Middleware(error) => VerifyScopesError::from(error),
        GithubRequestError::Message(message) => VerifyScopesError::Transport(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_required_scope_accepts_public_repo() {
        assert!(grants_required_scope("public_repo", &["public_repo"]));
    }

    #[test]
    fn grants_required_scope_accepts_repo_for_public_repo() {
        assert!(grants_required_scope("public_repo", &["repo"]));
    }

    #[test]
    fn grants_required_scope_rejects_missing_public_repo() {
        assert!(!grants_required_scope("public_repo", &["gist"]));
    }

    #[test]
    fn new_does_not_need_a_runtime() {
        crate::crypto::init_crypto_provider();
        GithubClient::new(None).expect("client build");
    }

    #[test]
    fn header_accepts_canonical_accept_name() {
        crate::crypto::init_crypto_provider();
        let client = GithubClient::new(None).expect("client build");
        let request = client
            .api_get_unauthenticated("https://api.github.com/user")
            .header("Accept", "application/json");
        assert!(request.error.is_none());
        let value = request
            .headers
            .get(http::header::ACCEPT)
            .expect("accept header");
        assert_eq!(value.as_bytes(), b"application/json");
    }

    #[test]
    fn header_records_invalid_name() {
        crate::crypto::init_crypto_provider();
        let client = GithubClient::new(None).expect("client build");
        let request = client
            .api_get_unauthenticated("https://api.github.com/user")
            .header("Not A Header", "application/json");
        assert!(request.error.is_some());
    }
}
