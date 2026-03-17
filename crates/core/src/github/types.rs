use secrecy::SecretString;
use serde::Deserialize;
use std::path::PathBuf;

/// Error returned when a GitHub token lacks required OAuth scopes.
///
/// OAuth scopes are required for certain GitHub API operations. When a token
/// doesn't have the necessary scopes, this error indicates which ones are missing.
///
/// # Example
///
/// ```
/// use cadmus_core::github::ScopeError;
///
/// let missing = vec!["public_repo".to_string(), "repo_deployment".to_string()];
/// let error = ScopeError::new(missing);
/// eprintln!("Missing scopes: {}", error);
/// ```
#[derive(Debug, Clone)]
pub struct ScopeError {
    missing: Vec<String>,
}

impl ScopeError {
    /// Creates a new scope error with the given list of missing scopes.
    pub fn new(missing: Vec<String>) -> Self {
        ScopeError { missing }
    }

    /// Returns a reference to the list of missing scopes.
    pub fn missing_scopes(&self) -> &[String] {
        &self.missing
    }
}

impl std::fmt::Display for ScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GitHub token missing required scopes: {}",
            self.missing.join(", ")
        )
    }
}

impl std::error::Error for ScopeError {}

/// Error returned by [`crate::github::GithubClient::verify_token_scopes`].
///
/// Distinguishes between a transport/HTTP failure during the scope check and a
/// token that was accepted by GitHub but is missing required OAuth scopes.
#[derive(Debug, thiserror::Error)]
pub enum VerifyScopesError {
    /// The HTTP request to GitHub failed (network error or non-2xx status).
    #[error("scope check request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// The token was accepted but lacks one or more required OAuth scopes.
    #[error(transparent)]
    InsufficientScopes(#[from] ScopeError),
}

// ── GitHub REST API response types ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct PullRequest {
    pub head: PrHead,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PrHead {
    pub sha: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRunsResponse {
    pub workflow_runs: Vec<WorkflowRun>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowRun {
    pub name: String,
    pub id: u64,
    #[serde(default)]
    pub head_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Repository {
    pub default_branch: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ArtifactsResponse {
    pub artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Artifact {
    pub name: String,
    pub id: u64,
    pub size_in_bytes: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    pub tag_name: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

// ── Device flow types ─────────────────────────────────────────────────────────

/// Response from the GitHub device code endpoint.
///
/// Contains the codes to display to the user and the polling parameters.
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    /// Opaque code passed back to the poll endpoint — do not display to the user.
    pub device_code: String,
    /// Short code the user types at `verification_uri` (e.g. `WDJB-MJHT`).
    pub user_code: String,
    /// URL the user must visit to authorize (always `https://github.com/login/device`).
    pub verification_uri: String,
    /// Seconds until both codes expire (default 900 = 15 minutes).
    pub expires_in: u64,
    /// Minimum seconds to wait between poll attempts.
    pub interval: u64,
}

/// Result of a single poll attempt during device flow.
#[derive(Debug)]
pub enum TokenPollResult {
    /// User has not yet authorized — keep polling after `interval` seconds.
    Pending,
    /// GitHub is being polled too fast — caller must add 5 s to the poll interval.
    SlowDown,
    /// User authorized successfully; token is ready to use.
    Complete(SecretString),
    /// The device code expired before the user authorized.
    Expired,
    /// User explicitly cancelled the authorization on GitHub.
    Cancelled,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AccessTokenResponse {
    pub access_token: Option<String>,
    pub error: Option<String>,
}

// ── OTA progress ──────────────────────────────────────────────────────────────

/// Progress states during an OTA download operation.
#[derive(Debug, Clone)]
pub enum OtaProgress {
    /// Verifying the pull request exists and fetching its metadata.
    CheckingPr,
    /// Searching for the latest successful build on the default branch.
    FindingLatestBuild,
    /// Searching for the associated GitHub Actions workflow run.
    FindingWorkflow,
    /// Actively downloading the artifact with optional progress tracking.
    DownloadingArtifact { downloaded: u64, total: u64 },
    /// Download completed successfully, artifact saved to disk.
    Complete { path: PathBuf },
}
