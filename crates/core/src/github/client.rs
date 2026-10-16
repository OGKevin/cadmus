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
use octocrab::params::pulls::Sort;
use octocrab::params::{Direction, State};
use octocrab::service::middleware::auth_header::AuthHeaderLayer;
use octocrab::service::middleware::base_uri::BaseUriLayer;
use octocrab::service::middleware::extra_headers::ExtraHeadersLayer;
use octocrab::{AuthState, Octocrab, OctocrabBuilder};
use reqwest_middleware::RequestBuilder;
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashSet;
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

/// GitHub page size while listing every open pull request.
const OPEN_PULL_PAGE: u8 = 100;
/// Stop after this many pages so a stuck link header cannot loop.
/// Two thousand open pull requests is past what this repository lists.
const OPEN_PULL_PAGE_CAP: u32 = 20;

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

/// Public because [`GithubClient::poll_device_token`], [`GithubClient::initiate_device_flow`]
/// and [`crate::github::GithubError`] all name it in their signatures.
#[derive(Debug, thiserror::Error)]
pub enum GithubRequestError {
    #[error(transparent)]
    Middleware(#[from] reqwest_middleware::Error),
    #[error("{0}")]
    Message(String),
}

/// Non-success HTTP status from a GitHub JSON call.
#[derive(Debug)]
pub(crate) struct ApiStatusError {
    status: StatusCode,
    /// GitHub's own `message` field, or a truncated body snippet.
    ///
    /// Without it every failure reads as "HTTP status error (404)", which
    /// discards the only thing that says *why* — a rate limit, a missing scope,
    /// or a bad ref all look identical in the log.
    detail: Option<String>,
}

/// Longest body snippet kept in an [`ApiStatusError`].
const MAX_ERROR_DETAIL: usize = 200;

impl ApiStatusError {
    fn from_response(status: StatusCode, body: &Bytes) -> Self {
        let detail = github_error_message(body)
            .or_else(|| Some(String::from_utf8_lossy(body).trim().to_string()));
        Self {
            status,
            detail: detail
                .filter(|d| !d.is_empty())
                .map(|d| d.chars().take(MAX_ERROR_DETAIL).collect()),
        }
    }

    /// A status with no body to explain it, for callers that only saw a code.
    pub(crate) fn from_status(status: StatusCode) -> Self {
        Self {
            status,
            detail: None,
        }
    }

    pub(crate) fn status_code(&self) -> StatusCode {
        self.status
    }
}

impl std::fmt::Display for ApiStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "HTTP status error ({}): {detail}", self.status),
            None => write!(f, "HTTP status error ({})", self.status),
        }
    }
}

/// GitHub's `{"message": "..."}` envelope, if the body carries one.
fn github_error_message(body: &Bytes) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        message: String,
    }
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|envelope| envelope.message)
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
            Err(ApiStatusError::from_response(self.status, &self.body))
        }
    }

    pub(crate) fn json<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
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
    /// Adds one header for this GitHub JSON call.
    ///
    /// These requests go through `octocrab`, not [`crate::http::Client`]'s
    /// `RequestBuilder`, so per-call headers live on this builder. Invalid
    /// names or values are recorded and returned from [`Self::send`] instead
    /// of panicking. Canonical names such as `Accept` are stored lowercase.
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
        let url = self.url;
        let response = crab
            ._get_with_headers(url.as_str(), headers)
            .await
            .map_err(|error| map_octocrab(&url, error))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|error| map_octocrab(&url, error))?
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
        total_size: Option<u64>,
        dest: &PathBuf,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<&crate::http::CancelFlag>,
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

    /// Downloads a file into memory using HTTP Range requests.
    ///
    /// Delegates to [`Client::download_to_vec`]. `request_builder` is called once
    /// per chunk to produce a `RequestBuilder` for the given URL.
    ///
    /// # Errors
    ///
    /// Returns `ChunkedDownloadError` if all retry attempts for any chunk fail.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, request_builder, progress_callback))
    )]
    pub async fn download_to_vec<B, F>(
        &self,
        url: &str,
        total_size: Option<u64>,
        request_builder: B,
        progress_callback: &mut F,
        should_cancel: Option<&crate::http::CancelFlag>,
    ) -> Result<Vec<u8>, ChunkedDownloadError>
    where
        B: Fn(&str) -> RequestBuilder,
        F: FnMut(u64, u64),
    {
        self.http
            .download_to_vec(
                url,
                total_size,
                request_builder,
                progress_callback,
                should_cancel,
            )
            .await
    }

    /// Attaches the token as a sensitive header.
    ///
    /// Chunked downloads need a raw builder per range request, so they cannot go
    /// through the `octocrab` service. The header is built by [`bearer`] here
    /// too, so both routes mark the token sensitive and cannot drift apart.
    fn with_auth(&self, builder: RequestBuilder) -> RequestBuilder {
        match self.auth_header() {
            Ok(Some(header)) => builder.header(AUTHORIZATION, header),
            Ok(None) => {
                tracing::warn!("Authentication requested but no token configured");
                builder
            }
            Err(error) => {
                tracing::error!(%error, "Cannot build the authorization header");
                builder
            }
        }
    }

    /// The `Authorization` header for this client, or `None` without a token.
    ///
    /// Returns `Err` when the stored token cannot be encoded as a header value.
    fn auth_header(&self) -> Result<Option<HeaderValue>, GithubRequestError> {
        self.token.as_ref().map(bearer).transpose()
    }

    fn authenticated(&self) -> Result<Arc<Octocrab>, GithubRequestError> {
        let header = self.auth_header()?;
        Ok(Arc::new(
            self.api
                .get_or_init(|| {
                    build_octocrab(
                        self.http.middleware(),
                        Uri::from_static(API_URI),
                        header,
                        standard_headers(),
                    )
                })
                .clone(),
        ))
    }

    fn unauthenticated(&self) -> Arc<Octocrab> {
        Arc::new(
            self.public_api
                .get_or_init(|| {
                    build_octocrab(
                        self.http.middleware(),
                        Uri::from_static(API_URI),
                        None,
                        standard_headers(),
                    )
                })
                .clone(),
        )
    }

    fn device_flow(&self) -> Arc<Octocrab> {
        Arc::new(
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
                .clone(),
        )
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
    pub async fn initiate_device_flow(&self) -> Result<DeviceCodeResponse, GithubRequestError> {
        tracing::info!(
            client_id = GITHUB_OAUTH_CLIENT_ID,
            "Initiating GitHub device auth flow"
        );

        tracing::debug!(scopes = ?REQUIRED_SCOPES, "Requesting device code with scopes");

        let client_id = SecretString::from(GITHUB_OAUTH_CLIENT_ID.to_owned());
        let codes = self
            .device_flow()
            .authenticate_as_device(&client_id, REQUIRED_SCOPES.iter().copied())
            .await
            .map_err(|error| {
                GithubRequestError::Message(format!("Device code request failed: {error}"))
            })?;

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
    // `skip_all`: `device_code` is a bearer credential and must not become a
    // span field.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn poll_device_token(
        &self,
        device_code: &str,
    ) -> Result<TokenPollResult, GithubRequestError> {
        tracing::debug!("Polling GitHub for device token");

        let crab = self.device_flow();
        let response = crab
            ._post(
                "https://github.com/login/oauth/access_token",
                Some(&DeviceTokenPoll {
                    client_id: GITHUB_OAUTH_CLIENT_ID,
                    device_code,
                    grant_type: "urn:ietf:params:oauth:grant-type:device_code",
                }),
            )
            .await
            .map_err(|error| {
                GithubRequestError::Message(format!("Token poll request failed: {error}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(GithubRequestError::Message(format!(
                "Token poll error response: HTTP {status}"
            )));
        }

        let body = crab.body_to_string(response).await.map_err(|error| {
            GithubRequestError::Message(format!("Failed to parse token response: {error}"))
        })?;
        let body: AccessTokenResponse = serde_json::from_str(&body).map_err(|error| {
            GithubRequestError::Message(format!("Failed to parse token response: {error}"))
        })?;

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
                Err(GithubRequestError::Message(format!(
                    "Unexpected device flow error: {other}"
                )))
            }
            None => {
                tracing::error!("Empty body from token poll endpoint");
                Err(GithubRequestError::Message(
                    "Empty response from token endpoint".to_owned(),
                ))
            }
        }
    }

    /// One page of open pull requests, newest update first.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(per_page, page), err)
    )]
    pub(crate) async fn list_open_pulls(
        &self,
        per_page: u8,
        page: u32,
    ) -> Result<PullList, GithubRequestError> {
        let crab = self.authenticated()?;
        let listed = crab
            .pulls(REPO_OWNER, REPO_NAME)
            .list()
            .state(State::Open)
            .sort(Sort::Updated)
            .direction(Direction::Descending)
            .per_page(per_page)
            .page(page)
            .send()
            .await
            .map_err(|error| map_octocrab("list open pull requests", error))?;
        let has_next = github_page_has_next(listed.next.is_some(), listed.items.len(), per_page);
        let pulls: Vec<ListedPull> = listed
            .items
            .into_iter()
            .map(|pull| ListedPull {
                number: pull.number,
                title: pull.title.unwrap_or_default(),
                draft: pull.draft.unwrap_or(false),
                head_sha: pull.head.sha,
            })
            .collect();
        tracing::debug!(
            pulls = pulls.len(),
            has_next,
            "open pull requests page listed"
        );
        Ok(PullList { pulls, has_next })
    }

    /// Every open pull request, newest activity first.
    ///
    /// Stops after [`OPEN_PULL_PAGE_CAP`] pages.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), err))]
    pub(crate) async fn list_all_open_pulls(&self) -> Result<Vec<ListedPull>, GithubRequestError> {
        let mut all = Vec::new();
        for page in 1..=OPEN_PULL_PAGE_CAP {
            let listed = self
                .list_open_pulls(OPEN_PULL_PAGE, page)
                .await
                .inspect_err(|error| {
                    tracing::warn!(
                        error = %error,
                        page,
                        "open pull list failed"
                    );
                })?;
            let has_next = listed.has_next;
            all.extend(listed.pulls);
            if !has_next {
                break;
            }
        }
        tracing::debug!(pulls = all.len(), "open pull requests listed");
        Ok(all)
    }

    /// Recent Cargo workflow runs triggered by a pull request.
    ///
    /// The hundred newest pull-request runs of `cargo.yml`. Each run keeps its
    /// id so the install check can read that run's packages. A pull request
    /// whose run is older than this list is not checked.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), err))]
    pub(crate) async fn list_cargo_pull_request_runs(
        &self,
    ) -> Result<Vec<ListedRun>, GithubRequestError> {
        let crab = self.authenticated()?;
        let listed = crab
            .workflows(REPO_OWNER, REPO_NAME)
            .list_runs("cargo.yml")
            .event("pull_request")
            .per_page(100u8)
            .send()
            .await
            .map_err(|error| map_octocrab("list cargo pull-request runs", error))?;
        let runs: Vec<ListedRun> = listed
            .items
            .into_iter()
            .map(|run| ListedRun {
                id: run.id.into_inner(),
                name: run.name,
                head_sha: run.head_sha,
                status: run.status,
                conclusion: run.conclusion,
            })
            .collect();
        tracing::debug!(runs = runs.len(), "cargo pull-request runs listed");
        Ok(runs)
    }

    /// Packages uploaded by one workflow run.
    ///
    /// One page of 100. A Cargo run uploads a handful of packages.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(run_id), err)
    )]
    pub(crate) async fn list_run_artifacts(
        &self,
        run_id: u64,
    ) -> Result<Vec<ListedArtifact>, GithubRequestError> {
        let crab = self.authenticated()?;
        let listed = crab
            .actions()
            .list_workflow_run_artifacts(REPO_OWNER, REPO_NAME, run_id.into())
            .per_page(100u8)
            .send()
            .await
            .map_err(|error| map_octocrab("list workflow run artifacts", error))?;
        let Some(page) = listed.value else {
            return Ok(Vec::new());
        };
        let artifacts: Vec<ListedArtifact> = page
            .items
            .into_iter()
            .map(|artifact| ListedArtifact {
                id: artifact.id.into_inner(),
                name: artifact.name,
                size_in_bytes: u64::try_from(artifact.size_in_bytes).unwrap_or(u64::MAX),
                expired: artifact.expired,
            })
            .collect();
        tracing::debug!(artifacts = artifacts.len(), "workflow run artifacts listed");
        Ok(artifacts)
    }
}

/// Cadmus repository the OTA picker reads.
const REPO_OWNER: &str = "ogkevin";
const REPO_NAME: &str = "cadmus";

/// One page of open pull requests.
#[derive(Debug)]
pub(crate) struct PullList {
    /// Pull requests on this page.
    pub pulls: Vec<ListedPull>,
    /// GitHub has another page.
    pub has_next: bool,
}

/// Fields of a pull request the picker keeps.
#[derive(Debug)]
pub(crate) struct ListedPull {
    /// Pull request number.
    pub number: u64,
    /// Title, or empty when GitHub omitted it.
    pub title: String,
    /// Draft pull request.
    pub draft: bool,
    /// Head commit SHA.
    pub head_sha: String,
}

/// Fields of a workflow run the picker keeps.
#[derive(Debug)]
pub(crate) struct ListedRun {
    /// GitHub workflow run id.
    pub id: u64,
    /// Workflow name.
    pub name: String,
    /// Commit the run built.
    pub head_sha: String,
    /// GitHub status.
    pub status: String,
    /// GitHub conclusion, when the run has one.
    pub conclusion: Option<String>,
}

/// One run per head, in `heads` order.
///
/// `runs` is newest first, as GitHub returns them. A completed success wins.
/// An in-progress run is used when that head has no success in the list.
/// A head that only failed is skipped.
pub(crate) fn install_artifact_runs<'a>(
    runs: &'a [ListedRun],
    heads: &[&str],
) -> Vec<&'a ListedRun> {
    let mut seen = HashSet::new();
    let mut chosen = Vec::new();
    for head in heads {
        if !seen.insert(*head) {
            continue;
        }
        let mut matching = runs.iter().filter(|run| run.head_sha == *head);
        let run = matching
            .clone()
            .find(|run| run.status == "completed" && run.conclusion.as_deref() == Some("success"))
            .or_else(|| matching.find(|run| run.status == "in_progress"));
        if let Some(run) = run {
            chosen.push(run);
        }
    }
    chosen
}

/// Fields of an artifact the picker keeps.
#[derive(Debug)]
pub(crate) struct ListedArtifact {
    /// Artifact id.
    pub id: u64,
    /// Artifact name.
    pub name: String,
    /// Size in bytes.
    pub size_in_bytes: u64,
    /// GitHub has expired the artifact.
    pub expired: bool,
}

#[derive(Serialize)]
struct DeviceTokenPoll<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'a str,
}

/// Name of the header both auth routes set, so they cannot drift apart.
const AUTHORIZATION: &str = "Authorization";

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

fn github_page_has_next(link: bool, count: usize, per_page: u8) -> bool {
    link || count == usize::from(per_page)
}

/// Turns an octocrab failure into something a log can act on.
///
/// `octocrab::Error::GitHub`'s own `Display` is only the variant name, so
/// every API failure would otherwise read as "GitHub" with no status or body.
/// The status code and GitHub's own message come from its source instead.
fn map_octocrab(request: &str, error: octocrab::Error) -> GithubRequestError {
    let mapped = match error {
        octocrab::Error::Service { source, .. } => match source.downcast::<TransportError>() {
            Ok(error) => match *error {
                TransportError::Middleware(error) => GithubRequestError::Middleware(error),
                other => GithubRequestError::Message(other.to_string()),
            },
            Err(error) => GithubRequestError::Message(error.to_string()),
        },
        octocrab::Error::GitHub { source, .. } => GithubRequestError::Message(format!(
            "HTTP {} from GitHub: {source}",
            source.status_code
        )),
        other => GithubRequestError::Message(other.to_string()),
    };
    tracing::debug!(request, error = %mapped, "github request failed");
    mapped
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

    fn listed_run(id: u64, head_sha: &str, status: &str, conclusion: Option<&str>) -> ListedRun {
        ListedRun {
            id,
            name: "Cargo".into(),
            head_sha: head_sha.into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
        }
    }

    #[test]
    fn install_artifact_runs_picks_one_run_per_head() {
        let runs = vec![
            listed_run(10, "aaa", "in_progress", None),
            listed_run(9, "aaa", "completed", Some("success")),
            listed_run(8, "bbb", "completed", Some("failure")),
            listed_run(7, "bbb", "in_progress", None),
            listed_run(6, "ccc", "completed", Some("success")),
            listed_run(5, "ddd", "completed", Some("failure")),
        ];
        let heads = ["aaa", "bbb", "aaa", "ddd"];
        let picked: Vec<u64> = install_artifact_runs(&runs, &heads)
            .iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(picked, vec![9, 7]);
    }

    #[test]
    fn a_full_page_counts_as_having_a_next_page() {
        assert!(github_page_has_next(false, 10, 10));
        assert!(!github_page_has_next(false, 9, 10));
        assert!(github_page_has_next(true, 3, 10));
        assert!(!github_page_has_next(false, 0, 10));
    }

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
    fn an_api_error_keeps_githubs_message() {
        let body = Bytes::from_static(br#"{"message":"Bad credentials"}"#);
        let error = ApiStatusError::from_response(StatusCode::UNAUTHORIZED, &body);
        assert_eq!(error.status_code(), StatusCode::UNAUTHORIZED);
        assert!(
            error.to_string().contains("Bad credentials"),
            "the message is the only thing that says why: {error}"
        );
    }

    #[test]
    fn an_api_error_falls_back_to_a_body_snippet() {
        let body = Bytes::from_static(b"<html>gateway exploded</html>");
        let error = ApiStatusError::from_response(StatusCode::BAD_GATEWAY, &body);
        assert!(error.to_string().contains("gateway exploded"));
    }

    #[test]
    fn an_api_error_truncates_a_long_body() {
        let body = Bytes::from(vec![b'x'; MAX_ERROR_DETAIL * 2]);
        let detail = ApiStatusError::from_response(StatusCode::INTERNAL_SERVER_ERROR, &body)
            .detail
            .expect("detail");
        assert_eq!(detail.chars().count(), MAX_ERROR_DETAIL);
    }

    #[test]
    fn the_token_header_is_sensitive_on_the_raw_route() {
        crate::crypto::init_crypto_provider();
        let client = GithubClient::new(Some(secret("ghp_example"))).expect("client build");
        let header = client.auth_header().expect("header").expect("token");
        assert!(header.is_sensitive());
        assert_eq!(header.to_str().expect("utf8"), "Bearer ghp_example");
    }

    #[test]
    fn a_client_without_a_token_has_no_auth_header() {
        crate::crypto::init_crypto_provider();
        let client = GithubClient::new(None).expect("client build");
        assert!(client.auth_header().expect("header").is_none());
    }

    fn secret(value: &str) -> SecretString {
        SecretString::from(value.to_owned())
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
