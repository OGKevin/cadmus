use super::types::{
    AccessTokenResponse, DeviceCodeResponse, ScopeError, TokenPollResult, VerifyScopesError,
};
use reqwest::blocking::{Client, RequestBuilder};
use rustls::RootCertStore;
use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;

pub const CLIENT_TIMEOUT_SECS: u64 = 30;

/// GitHub OAuth App client ID, baked in at build time via `GH_OAUTH_CLIENT_ID` env var.
///
/// Kept private so callers never need to know or pass it; [`GithubClient`] uses it internally.
const GITHUB_OAUTH_CLIENT_ID: &str = env!("GH_OAUTH_CLIENT_ID");

/// OAuth scopes that the saved token must have for OTA operations to succeed.
///
/// This is the single source of truth for required scopes. Both
/// [`GithubClient::initiate_device_flow`] and
/// [`GithubClient::verify_token_scopes`] derive from this list, so adding or
/// removing a scope here is the only change needed.
///
/// Current requirements:
/// - `public_repo` — required to download Actions artifacts from public repositories
pub const REQUIRED_SCOPES: &[&str] = &["public_repo"];

/// Thin HTTP wrapper around the GitHub REST API.
///
/// Handles TLS setup, authentication headers, and base URL construction.
/// Consumers (e.g. [`OtaClient`](crate::ota::OtaClient)) call the specific
/// API methods they need.
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
    client: Client,
    token: Option<SecretString>,
}

impl GithubClient {
    /// Creates a new client with optional GitHub token authentication.
    ///
    /// Uses `webpki-roots` certificates for TLS — no system cert store
    /// required, which matters on Kobo devices that ship without a CA bundle.
    ///
    /// # Errors
    ///
    /// Returns an error string if the underlying HTTP client fails to build.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cadmus_core::github::GithubClient;
    ///
    /// let client = GithubClient::new(None).expect("failed to build client");
    /// ```
    #[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
    pub fn new(token: Option<SecretString>) -> Result<Self, String> {
        tracing::debug!(token_provided = token.is_some(), "Building GitHub client");

        let root_store = build_root_store();

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let client = Client::builder()
            .use_preconfigured_tls(tls_config)
            .user_agent("github.com/OGKevin/cadmus")
            .timeout(Duration::from_secs(CLIENT_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        tracing::debug!("GitHub client built successfully");
        Ok(Self { client, token })
    }

    /// Returns a GET request builder with the `Authorization` header set if a
    /// token is present.
    pub fn get(&self, url: &str) -> RequestBuilder {
        self.with_auth(self.client.get(url))
    }

    /// Returns a POST request builder with the `Authorization` header set if a
    /// token is present.
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.with_auth(self.client.post(url))
    }

    /// Returns a GET request builder **without** any `Authorization` header.
    ///
    /// Used for public URLs (e.g. release asset downloads) where sending a
    /// token would cause GitHub to reject the request with a 401.
    pub fn get_unauthenticated(&self, url: &str) -> RequestBuilder {
        self.client.get(url)
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
    /// let client = GithubClient::new(None).expect("failed to build client");
    /// let response = client.initiate_device_flow().expect("device flow failed");
    /// println!("Go to {} and enter {}", response.verification_uri, response.user_code);
    /// ```
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub fn initiate_device_flow(&self) -> Result<DeviceCodeResponse, String> {
        tracing::info!(
            client_id = GITHUB_OAUTH_CLIENT_ID,
            "Initiating GitHub device auth flow"
        );

        let scope = REQUIRED_SCOPES.join(" ");
        tracing::debug!(scope = %scope, "Requesting device code with scopes");

        let response = self
            .client
            .post("https://github.com/login/device/code")
            .header("Accept", "application/json")
            .form(&[("client_id", GITHUB_OAUTH_CLIENT_ID), ("scope", &scope)])
            .send()
            .map_err(|e| format!("Device code request failed: {}", e))?
            .error_for_status()
            .map_err(|e| format!("Device code request error: {}", e))?;

        let device_code_response = response
            .json::<DeviceCodeResponse>()
            .map_err(|e| format!("Failed to parse device code response: {}", e))?;

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
    /// let token = SecretString::from("ghp_…".to_owned());
    /// let client = GithubClient::new(Some(token)).expect("failed to build client");
    ///
    /// match client.verify_token_scopes() {
    ///     Ok(()) => println!("Token has all required scopes"),
    ///     Err(e) => println!("Error: {}", e),
    /// }
    /// ```
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub fn verify_token_scopes(&self) -> Result<(), VerifyScopesError> {
        tracing::debug!("Verifying token scopes");

        let response = self
            .get("https://api.github.com/user")
            .header("Accept", "application/json")
            .send()?
            .error_for_status()?;

        let granted: Vec<&str> = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').map(str::trim).collect())
            .unwrap_or_default();

        tracing::debug!(granted = ?granted, required = ?REQUIRED_SCOPES, "Comparing token scopes");

        let missing: Vec<String> = REQUIRED_SCOPES
            .iter()
            .filter(|&&required| !granted.contains(&required))
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
    /// let client = GithubClient::new(None).expect("failed to build client");
    /// let flow = client.initiate_device_flow().expect("device flow failed");
    ///
    /// loop {
    ///     std::thread::sleep(Duration::from_secs(flow.interval));
    ///     match client.poll_device_token(&flow.device_code).expect("poll failed") {
    ///         TokenPollResult::Complete(token) => break,
    ///         TokenPollResult::Pending => continue,
    ///         _ => break,
    ///     }
    /// }
    /// ```
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub fn poll_device_token(&self, device_code: &str) -> Result<TokenPollResult, String> {
        tracing::debug!("Polling GitHub for device token");

        let response = self
            .client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", GITHUB_OAUTH_CLIENT_ID),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .map_err(|e| format!("Token poll request failed: {}", e))?
            .error_for_status()
            .map_err(|e| format!("Token poll error response: {}", e))?;

        let body: AccessTokenResponse = response
            .json()
            .map_err(|e| format!("Failed to parse token response: {}", e))?;

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
                Err(format!("Unexpected device flow error: {}", other))
            }
            None => {
                tracing::error!("Empty body from token poll endpoint");
                Err("Empty response from token endpoint".to_owned())
            }
        }
    }
}

fn build_root_store() -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}
