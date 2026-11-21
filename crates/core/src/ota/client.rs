use super::xz::{self, XzSource};
use crate::github::types::{
    Artifact, ArtifactsResponse, Release, ReleaseAsset, Repository, WorkflowRun,
    WorkflowRunsResponse,
};
use crate::github::{GithubClient, OtaProgress};
use crate::http::{CancelFlag, CancelFunc, ChunkedDownloadError};
use crate::version::GitVersion;
use async_zip::base::read1::ZipFileReader;
use async_zip::base::read1::seek::ZipArchiveReader;
use async_zip::error::ZipError;
use async_zip::spec::constructs::CDR;
use futures_lite::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncSeek, Cursor};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::io::Error;
use std::path::{Path, PathBuf};
use tokio_util::compat::TokioAsyncReadCompatExt;

#[cfg(test)]
use std::cell::Cell;

#[cfg(all(not(test), not(feature = "emulator")))]
use crate::settings::INTERNAL_CARD_ROOT;

/// Downloads and deploys OTA updates from GitHub.
///
/// Delegates all HTTP communication to [`GithubClient`] and focuses solely on
/// the OTA-specific workflow: finding artifacts, chunked downloading, ZIP
/// extraction, and deploying `KoboRoot.tgz` to the Kobo device.
pub struct OtaClient {
    github: GithubClient,
    tmp_dir: PathBuf,
}

/// Indicates where artifacts were expected but not found.
#[derive(Debug, Clone)]
pub enum ArtifactSource {
    /// No artifacts found for a specific pull request
    PullRequest(u32),
    /// No artifacts found for the default branch
    DefaultBranch,
    /// No artifact matching the expected name pattern in a workflow run
    WorkflowRun(String),
    /// No release asset found with the expected name
    ReleaseAsset(String),
}

impl std::fmt::Display for ArtifactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactSource::PullRequest(pr) => write!(f, "No build artifacts found for PR #{}", pr),
            ArtifactSource::DefaultBranch => {
                write!(f, "No build artifacts found for default branch")
            }
            ArtifactSource::WorkflowRun(pattern) => {
                write!(
                    f,
                    "No artifact matching '{}' found in workflow run",
                    pattern
                )
            }
            ArtifactSource::ReleaseAsset(name) => write!(f, "No release asset '{}' found", name),
        }
    }
}

/// Error types that can occur during OTA operations.
#[derive(thiserror::Error, Debug)]
pub enum OtaError {
    /// GitHub API returned an error response
    #[error("GitHub API error: {0}")]
    Api(String),

    /// HTTP request failed during communication with GitHub
    #[error("HTTP request error: {0}")]
    Request(#[from] reqwest::Error),

    /// The specified pull request number was not found in the repository
    #[error("PR #{0} not found")]
    PrNotFound(u32),

    /// No build artifacts found for the specified source
    #[error("{0}")]
    ArtifactsNotFound(ArtifactSource),

    /// GitHub token was not provided
    #[error("GitHub token not configured")]
    NoToken,

    /// GitHub token is invalid or has been revoked — re-authentication required
    #[error("GitHub token is invalid or revoked")]
    Unauthorized,

    /// GitHub token is missing one or more required OAuth scopes
    ///
    /// The token was accepted by GitHub but lacks the permissions needed for
    /// OTA operations. Re-authentication with the correct scopes is required.
    #[error(transparent)]
    InsufficientScopes(#[from] crate::github::ScopeError),

    /// Insufficient disk space available for download (requires 100MB minimum)
    #[error("Insufficient disk space: need 100MB, have {0}MB")]
    InsufficientSpace(u64),

    /// File system I/O operation failed
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// System-level error from nix library
    #[error("System error: {0}")]
    Nix(#[from] nix::errno::Errno),

    /// TLS/SSL configuration failed when setting up HTTPS client
    #[error("TLS configuration error: {0}")]
    TlsConfig(String),

    /// Failed to extract files from ZIP archive
    #[error("ZIP extraction error: {0}")]
    ZipError(String),

    /// Deployment process failed after successful download
    #[error("Deployment error: {0}")]
    DeploymentError(String),

    /// Failed to parse version string
    #[error(transparent)]
    VersionParse(#[from] crate::version::VersionError),

    /// OTA download or deploy was cancelled by the user
    #[error("OTA download cancelled")]
    Cancelled,

    /// The xz payload is corrupt, fails its integrity check or exceeds the
    /// decoder memory limit
    #[error("Decompression error: {0}")]
    Decompress(String),

    /// The deploy filesystem cannot hold the decompressed bundle
    #[error("Insufficient space to stage update: need {needed_mb}MB, have {available_mb}MB")]
    InsufficientDeploySpace {
        /// Estimated size of the staged `KoboRoot.tgz` in megabytes.
        needed_mb: u64,
        /// Free space on the deploy filesystem in megabytes.
        available_mb: u64,
    },
}

/// Result of committing `KoboRoot.tgz` to the deploy path.
///
/// Rename publishes the bundle to Nickel. A later parent-directory sync
/// failure does not undo that publication, so callers must treat
/// [`DeployOutcome::CommittedNotDurable`] as a successful install and still
/// schedule reboot.
#[derive(Debug)]
pub enum DeployOutcome {
    /// The bundle is at this path and the parent directory was synced.
    Durable(PathBuf),
    /// The bundle was renamed into place, but the parent directory could not
    /// be synced. The new file is visible to Nickel; durability is not
    /// guaranteed across a crash before the next successful directory sync.
    CommittedNotDurable {
        /// Path of the published bundle.
        path: PathBuf,
        /// Error returned by the parent-directory sync.
        error: OtaError,
    },
}

impl DeployOutcome {
    /// Returns the path where the bundle was published.
    pub fn path(&self) -> &Path {
        match self {
            Self::Durable(path) | Self::CommittedNotDurable { path, .. } => path,
        }
    }

    /// Consumes the outcome and returns the published path.
    pub fn into_path(self) -> PathBuf {
        match self {
            Self::Durable(path) | Self::CommittedNotDurable { path, .. } => path,
        }
    }
}

/// Where a downloaded OTA payload lives before it is deployed.
///
/// Build artifacts ship as a ZIP wrapping `KoboRoot.tgz`. When the archive fits
/// in RAM it is buffered whole ([`Self::InMemory`]) so the ZIP never touches
/// flash; otherwise it is downloaded to `tmp_dir` and streamed from disk
/// ([`Self::Zipped`]). Stable releases ship a bare `KoboRoot.tgz` and are always
/// on disk ([`Self::Bundle`]). Newer builds and releases also ship an xz copy
/// ([`Self::Xz`]) that is decoded into `KoboRoot.tgz` on the device.
#[derive(Debug)]
pub enum DownloadedArtifact {
    /// The artifact ZIP, held entirely in memory.
    InMemory(Vec<u8>),
    /// Path to the artifact ZIP on disk, used when it does not fit in RAM.
    Zipped(PathBuf),
    /// Path to a bare `KoboRoot.tgz` release asset.
    Bundle(PathBuf),
    /// An xz copy of the bundle, decoded and re-compressed during deploy.
    Xz(XzSource),
}

impl From<crate::runtime::Cancelled> for OtaError {
    fn from(_: crate::runtime::Cancelled) -> Self {
        OtaError::Cancelled
    }
}

impl From<ChunkedDownloadError> for OtaError {
    fn from(e: ChunkedDownloadError) -> Self {
        match e {
            ChunkedDownloadError::Cancelled => OtaError::Cancelled,
            ChunkedDownloadError::Request(r) if r.status().is_some() => api_error(r),
            ChunkedDownloadError::Request(r) => OtaError::Request(r),
            ChunkedDownloadError::Failed(message) => OtaError::Api(message),
            ChunkedDownloadError::Io(e) => OtaError::Io(e),
        }
    }
}

impl From<reqwest_middleware::Error> for OtaError {
    fn from(error: reqwest_middleware::Error) -> Self {
        match crate::http::reqwest_error(error) {
            Ok(error) => Self::Request(error),
            Err(message) => Self::Api(message),
        }
    }
}

impl From<crate::github::GithubRequestError> for OtaError {
    fn from(error: crate::github::GithubRequestError) -> Self {
        match error {
            crate::github::GithubRequestError::Middleware(error) => Self::from(error),
            crate::github::GithubRequestError::Message(message) => Self::Api(message),
        }
    }
}

impl From<serde_json::Error> for OtaError {
    fn from(error: serde_json::Error) -> Self {
        Self::Api(error.to_string())
    }
}

impl OtaClient {
    /// Creates a new OTA client wrapping the provided GitHub client.
    ///
    /// # Errors
    ///
    /// Returns `OtaError::TlsConfig` if the underlying HTTP client fails to build.
    pub fn new(github: GithubClient, tmp_dir: PathBuf) -> Self {
        Self { github, tmp_dir }
    }

    /// Downloads the build artifact from a GitHub pull request.
    ///
    /// This performs the complete download workflow:
    /// 1. Verifies sufficient disk space (100MB required)
    /// 2. Fetches PR metadata to get the commit SHA
    /// 3. Finds the associated "Cargo" workflow run
    /// 4. Locates the `cadmus-kobo-xz-pr*` artifact, falling back to `cadmus-kobo-pr*`
    /// 5. Downloads the artifact ZIP file to `tmp_dir/cadmus-ota-{pr_number}.zip`
    ///
    /// GitHub authentication is required for this operation.
    ///
    /// # Arguments
    ///
    /// * `pr_number` - The pull request number from ogkevin/cadmus repository
    /// * `progress_callback` - Function called with progress updates during download
    ///
    /// # Returns
    ///
    /// The downloaded artifact, buffered in memory when it fits.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in the configured temp directory
    /// * `OtaError::NoToken` - GitHub token not configured
    /// * `OtaError::PrNotFound` - PR number doesn't exist in repository
    /// * `OtaError::ArtifactsNotFound` - No matching build artifacts found for the PR
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::Io` - Failed to write the streaming-fallback file to disk
    pub async fn download_pr_artifact<F>(
        &self,
        pr_number: u32,
        mut progress_callback: F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<DownloadedArtifact, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        if should_cancel.is_some_and(CancelFlag::is_cancelled) {
            return Err(OtaError::Cancelled);
        }

        check_disk_space(&self.tmp_dir)?;
        verify_scopes(&self.github).await?;

        progress_callback(OtaProgress::CheckingPr);
        tracing::info!(pr_number, "Starting PR build download");
        tracing::debug!(pr_number, "Checking PR");

        let pr_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/pulls/{}",
            pr_number
        );
        tracing::debug!(url = %pr_url, "Fetching PR");

        let response = self
            .github
            .api_get(&pr_url)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(pr_number, status = ?e.status_code(), error = %e, "PR fetch failed");
                if e.status_code() == reqwest::StatusCode::UNAUTHORIZED {
                    OtaError::Unauthorized
                } else {
                    OtaError::PrNotFound(pr_number)
                }
            })?;

        let pr: crate::github::types::PullRequest = response.json()?;
        tracing::debug!("Successfully parsed PR response");
        let head_sha = pr.head.sha;
        tracing::debug!(pr_number, head_sha = %head_sha, "Retrieved PR head SHA");

        progress_callback(OtaProgress::FindingWorkflow);
        tracing::debug!(head_sha = %head_sha, "Finding workflow runs");

        let runs_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/runs?head_sha={}&event=pull_request",
            head_sha
        );
        tracing::debug!(url = %runs_url, "Fetching workflow runs");

        let runs: WorkflowRunsResponse = self
            .github
            .api_get(&runs_url)
            .send().await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(head_sha = %head_sha, status = ?e.status_code(), error = %e, "Workflow runs fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(count = runs.workflow_runs.len(), "Found workflow runs");

        #[cfg(feature = "tracing")]
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (idx, run) in runs.workflow_runs.iter().enumerate() {
                tracing::debug!(
                    index = idx,
                    name = %run.name,
                    id = run.id,
                    status = %run.status,
                    conclusion = ?run.conclusion,
                    "Workflow run"
                );
            }
        }

        let prefixes = ArtifactPrefixes::for_pr(pr_number);

        let mut artifact = None;
        for run in runs
            .workflow_runs
            .iter()
            .filter(|r| is_ota_candidate_run(r))
        {
            tracing::debug!(
                run_id = run.id,
                status = %run.status,
                conclusion = ?run.conclusion,
                "Checking Cargo workflow run for artifacts"
            );
            match self.find_ota_artifact_in_run(run.id, &prefixes).await {
                Ok(found) => {
                    tracing::debug!(run_id = run.id, "Selected Cargo workflow run");
                    artifact = Some(found);
                    break;
                }
                Err(OtaError::ArtifactsNotFound(_)) => {
                    tracing::debug!(
                        run_id = run.id,
                        conclusion = ?run.conclusion,
                        "No matching artifact in run; trying next"
                    );
                }
                Err(e) => return Err(e),
            }
        }

        let (artifact, format) = artifact.ok_or_else(|| {
            tracing::error!(
                pr_number,
                "No Cargo workflow run with matching artifacts found"
            );
            OtaError::ArtifactsNotFound(ArtifactSource::PullRequest(pr_number))
        })?;

        tracing::debug!(
            name = %artifact.name,
            id = artifact.id,
            size_bytes = artifact.size_in_bytes,
            "Found artifact"
        );

        let download_path = self.tmp_dir.join(format!("cadmus-ota-{}.zip", pr_number));

        let artifact = self
            .download_artifact(
                &artifact,
                format,
                &download_path,
                &mut progress_callback,
                should_cancel,
            )
            .await?;

        progress_callback(OtaProgress::Complete);

        tracing::info!(pr_number, "PR build download completed");
        Ok(artifact)
    }

    /// Downloads the latest build artifact from the default branch.
    ///
    /// This performs the complete download workflow for default branch builds:
    /// 1. Verifies sufficient disk space (100MB required)
    /// 2. Queries GitHub API for the latest successful `cargo.yml` workflow run on the default branch
    /// 3. Locates artifacts matching "cadmus-kobo-{sha}" pattern (or "cadmus-kobo-test-{sha}" with `test` feature)
    /// 4. Downloads the artifact ZIP file to `tmp_dir/cadmus-ota-{sha}.zip`
    ///
    /// GitHub authentication is required for this operation.
    ///
    /// # Arguments
    ///
    /// * `progress_callback` - Function called with progress updates during download
    ///
    /// # Returns
    ///
    /// The downloaded artifact, buffered in memory when it fits.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in the configured temp directory
    /// * `OtaError::NoToken` - GitHub token not configured
    /// * `OtaError::ArtifactsNotFound` - No matching build artifacts found for default branch
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::Io` - Failed to write the streaming-fallback file to disk
    pub async fn download_default_branch_artifact<F>(
        &self,
        mut progress_callback: F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<DownloadedArtifact, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        if should_cancel.is_some_and(CancelFlag::is_cancelled) {
            return Err(OtaError::Cancelled);
        }

        check_disk_space(&self.tmp_dir)?;
        verify_scopes(&self.github).await?;

        progress_callback(OtaProgress::FindingLatestBuild);
        tracing::info!("Starting main branch build download");
        tracing::debug!("Finding latest default branch build");

        let default_branch = self.fetch_default_branch().await?;

        let encoded_branch = utf8_percent_encode(&default_branch, NON_ALPHANUMERIC);
        let runs_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/workflows/cargo.yml/runs?branch={}&event=push&status=success&per_page=1",
            encoded_branch
        );
        tracing::debug!(url = %runs_url, "Fetching Cargo workflow runs on default branch");

        let runs: WorkflowRunsResponse = self
            .github
            .api_get(&runs_url)
            .send().await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status_code(), error = %e, "Cargo workflow runs fetch failed");
                api_error(e)
            })?
            .json()?;

        let cargo_run = runs.workflow_runs.first().ok_or_else(|| {
            tracing::error!("No successful Cargo workflow run found on default branch");
            OtaError::ArtifactsNotFound(ArtifactSource::DefaultBranch)
        })?;

        tracing::debug!(run_id = cargo_run.id, "Found Cargo workflow run");

        let head_sha = cargo_run.head_sha.as_deref().ok_or_else(|| {
            tracing::error!(run_id = cargo_run.id, "Workflow run missing head_sha");
            OtaError::Api(format!("Workflow run {} missing head_sha", cargo_run.id))
        })?;
        let short_sha = &head_sha[..7.min(head_sha.len())];

        let prefixes = ArtifactPrefixes::for_commit(short_sha);

        tracing::debug!(xz = %prefixes.xz, legacy = %prefixes.legacy, "Looking for artifact");

        progress_callback(OtaProgress::FindingWorkflow);

        let (artifact, format) = self
            .find_ota_artifact_in_run(cargo_run.id, &prefixes)
            .await
            .map_err(|e| match e {
                OtaError::ArtifactsNotFound(ArtifactSource::WorkflowRun(pattern)) => {
                    tracing::error!(pattern = %pattern, "No matching artifact found on default branch");
                    OtaError::ArtifactsNotFound(ArtifactSource::DefaultBranch)
                }
                other => other,
            })?;

        tracing::debug!(
            name = %artifact.name,
            id = artifact.id,
            size_bytes = artifact.size_in_bytes,
            "Found default branch artifact"
        );

        let download_path = self.tmp_dir.join(format!("cadmus-ota-{}.zip", short_sha));

        let artifact = self
            .download_artifact(
                &artifact,
                format,
                &download_path,
                &mut progress_callback,
                should_cancel,
            )
            .await?;

        progress_callback(OtaProgress::Complete);

        tracing::info!(sha = %short_sha, "Main branch build download completed");
        Ok(artifact)
    }

    /// Downloads the latest stable release artifact from GitHub releases.
    ///
    /// This performs the complete download workflow for stable releases:
    /// 1. Verifies sufficient disk space (100MB required)
    /// 2. Fetches the latest release from GitHub API
    /// 3. Locates the `KoboRoot.tar.xz` asset in the release, falling back to `KoboRoot.tgz`
    /// 4. Downloads the asset into memory when it fits, otherwise to `tmp_dir`
    ///
    /// GitHub authentication is not required for this operation as release
    /// assets are downloaded from public URLs without Authorization headers.
    ///
    /// # Arguments
    ///
    /// * `progress_callback` - Function called with progress updates during download
    ///
    /// # Returns
    ///
    /// The downloaded bundle: xz in memory or on disk when the release ships one,
    /// otherwise the bare `KoboRoot.tgz` on disk.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in the configured temp directory
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::ArtifactsNotFound` - KoboRoot.tgz not found in latest release
    /// * `OtaError::Io` - Failed to write downloaded file to disk
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn download_stable_release_artifact<F>(
        &self,
        mut progress_callback: F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<DownloadedArtifact, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        if should_cancel.is_some_and(CancelFlag::is_cancelled) {
            return Err(OtaError::Cancelled);
        }

        check_disk_space(&self.tmp_dir)?;

        progress_callback(OtaProgress::FindingLatestBuild);
        tracing::info!("Starting stable release download");
        tracing::debug!("Finding latest stable release");

        let releases_url = "https://api.github.com/repos/ogkevin/cadmus/releases/latest";
        tracing::debug!(url = %releases_url, "Fetching latest release");

        let release: Release = self
            .github
            .api_get_unauthenticated(releases_url)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status_code(), error = %e, "Latest release fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(asset_count = release.assets.len(), "Found release assets");

        #[cfg(feature = "tracing")]
        for (idx, asset) in release.assets.iter().enumerate() {
            tracing::debug!(
                index = idx,
                name = %asset.name,
                size_bytes = asset.size,
                "Asset"
            );
        }

        let (asset, format) = select_release_asset(&release.assets).ok_or_else(|| {
            tracing::error!(
                target_asset = GZIP_RELEASE_ASSET,
                "Asset not found in latest release"
            );
            OtaError::ArtifactsNotFound(ArtifactSource::ReleaseAsset(GZIP_RELEASE_ASSET.to_owned()))
        })?;

        tracing::debug!(
            name = %asset.name,
            url = %asset.browser_download_url,
            size_bytes = asset.size,
            ?format,
            "Found release asset"
        );

        let downloaded = match format {
            PayloadFormat::Gzip => {
                let download_path = self.tmp_dir.join("cadmus-ota-stable-release.tgz");
                self.download_release_asset(
                    asset,
                    &download_path,
                    &mut progress_callback,
                    should_cancel,
                )
                .await?;
                DownloadedArtifact::Bundle(download_path)
            }
            PayloadFormat::Xz => {
                self.download_release_xz(asset, &mut progress_callback, should_cancel)
                    .await?
            }
        };

        progress_callback(OtaProgress::Complete);

        tracing::info!("Stable release download completed");
        Ok(downloaded)
    }

    /// Fetches the latest stable release version from GitHub.
    ///
    /// Retrieves and parses the version from the most recent stable release.
    /// Returns the version as a `GitVersion` struct for easy comparison and display.
    ///
    /// GitHub authentication is not required for this operation as releases are public.
    ///
    /// # Errors
    ///
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::VersionParse` - Failed to parse the release tag as a valid version
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cadmus_core::github::GithubClient;
    /// use cadmus_core::ota::OtaClient;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # rustls::crypto::ring::default_provider().install_default().ok();
    /// # let github = GithubClient::new(None)?;
    /// # let client = OtaClient::new(github, std::path::PathBuf::from("/tmp"));
    /// let version = client.fetch_latest_release_version().await?;
    /// println!("Latest version: {}", version);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn fetch_latest_release_version(&self) -> Result<GitVersion, OtaError> {
        let releases_url = "https://api.github.com/repos/ogkevin/cadmus/releases/latest";
        tracing::debug!(url = %releases_url, "Fetching latest release version");

        let release: Release = self
            .github
            .api_get_unauthenticated(releases_url)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status_code(), error = %e, "Latest release fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::info!(version = %release.tag_name, "Fetched latest release version");

        let version: GitVersion = release.tag_name.parse()?;
        Ok(version)
    }

    /// Deploys KoboRoot.tgz from the specified path directly without extraction.
    ///
    /// Used when the artifact is already in the correct format (e.g., stable releases
    /// that are distributed as bare KoboRoot.tgz files).
    ///
    /// On success, the source file is deleted as a best-effort cleanup step.
    ///
    /// # Arguments
    ///
    /// * `kobo_root_path` - Path to the KoboRoot.tgz file to deploy
    ///
    /// # Returns
    ///
    /// The deploy outcome, or an error if the bundle was not published.
    ///
    /// # Errors
    ///
    /// * `OtaError::Io` - Failed to read or write files before the bundle was renamed
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn deploy(
        &self,
        kobo_root_path: PathBuf,
        should_cancel: CancelFunc<'_>,
    ) -> Result<DeployOutcome, OtaError> {
        tracing::info!(path = ?kobo_root_path, "Deploying KoboRoot.tgz");

        let cancel = should_cancel;
        let src_path = kobo_root_path.clone();
        let (deploy_path, staging_path, mut unpublished, mut staging) =
            self.begin_staging().await?;
        let mut src = tokio::fs::File::open(&src_path).await?;
        copy_file_to_file(&mut src, &mut staging, cancel).await?;
        let outcome = self
            .commit_staging(
                deploy_path,
                staging_path,
                &mut unpublished,
                &mut staging,
                cancel,
            )
            .await?;

        if kobo_root_path != outcome.path()
            && let Err(e) = tokio::fs::remove_file(&kobo_root_path).await
        {
            tracing::error!(path = ?kobo_root_path, error = %e, "Failed to remove source file");
        }

        tracing::info!(path = ?outcome.path(), "Update deployed successfully");
        Ok(outcome)
    }

    /// Returns the platform-specific deployment path for KoboRoot.tgz.
    ///
    /// | Build context        | Path                                              |
    /// |----------------------|---------------------------------------------------|
    /// | During `cargo test`  | `{tmp_dir}/.kobo/KoboRoot.tgz`                    |
    /// | Emulator builds      | `/tmp/.kobo/KoboRoot.tgz`                         |
    /// | Kobo builds          | `{INTERNAL_CARD_ROOT}/.kobo/KoboRoot.tgz`         |
    pub(crate) fn deploy_path(&self) -> PathBuf {
        let path = cfg_select! {
            test => {
                self.tmp_dir.join(".kobo").join("KoboRoot.tgz")
            }
            feature = "emulator" => { PathBuf::from("/tmp/.kobo/KoboRoot.tgz") }
            _ => { PathBuf::from(format!("{}/.kobo/KoboRoot.tgz", INTERNAL_CARD_ROOT)) }
        };

        tracing::debug!(path = ?path, "Deploy destination");
        path
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub(crate) fn staging_path(&self) -> PathBuf {
        let deploy_path = self.deploy_path();
        let deploy_name = deploy_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "KoboRoot.tgz".to_owned());
        let staging_name = format!("{deploy_name}.{}.partial", uuid::Uuid::now_v7());
        let path = deploy_path.with_file_name(staging_name);
        tracing::debug!(path = ?path, "Staging destination");
        path
    }

    async fn begin_staging(
        &self,
    ) -> Result<
        (
            PathBuf,
            PathBuf,
            crate::fs::RemovePathOnDrop,
            tokio::fs::File,
        ),
        OtaError,
    > {
        let deploy_path = self.deploy_path();
        let staging_path = self.staging_path();
        self.ensure_deploy_dir(&deploy_path).await?;
        let unpublished = crate::fs::RemovePathOnDrop::file(staging_path.clone());
        let staging = tokio::fs::File::create(&staging_path).await?;
        Ok((deploy_path, staging_path, unpublished, staging))
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, unpublished, staging, should_cancel))
    )]
    async fn commit_staging(
        &self,
        deploy_path: PathBuf,
        staging_path: PathBuf,
        unpublished: &mut crate::fs::RemovePathOnDrop,
        staging: &mut tokio::fs::File,
        should_cancel: CancelFunc<'_>,
    ) -> Result<DeployOutcome, OtaError> {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }
        staging.sync_all().await?;
        if !should_cancel.try_commit() {
            return Err(OtaError::Cancelled);
        }
        tokio::fs::rename(&staging_path, &deploy_path).await?;
        #[cfg(test)]
        let inject_sync_failure = FAIL_SYNC_DEPLOY_PARENT.with(|flag| flag.replace(false));
        let sync_result = tokio::task::spawn_blocking({
            let deploy_path = deploy_path.clone();
            move || {
                #[cfg(test)]
                if inject_sync_failure {
                    return Err(OtaError::Io(std::io::Error::other(
                        "injected sync_deploy_parent failure",
                    )));
                }
                Self::sync_deploy_parent(&deploy_path)
            }
        })
        .await
        .map_err(|error| OtaError::Io(std::io::Error::other(error)))?;

        let result = match sync_result {
            Ok(()) => Ok(DeployOutcome::Durable(deploy_path.clone())),
            Err(error) => {
                tracing::warn!(
                    path = ?deploy_path,
                    error = %error,
                    "Parent directory sync failed after committing bundle"
                );
                Ok(DeployOutcome::CommittedNotDurable {
                    path: deploy_path.clone(),
                    error,
                })
            }
        };

        if result.is_ok() {
            unpublished.disarm();
        }

        result
    }

    /// Drives the staging sequence for a test that only cares about the
    /// commit, not how the bytes get there.
    ///
    /// Test-only on purpose: the boxed higher-ranked closure would otherwise
    /// dictate the shape of the production deploy paths, which cannot use it
    /// because they have to observe cancellation mid-write.
    #[cfg(test)]
    async fn write_staging_then_rename<F>(
        &self,
        should_cancel: CancelFunc<'_>,
        write: F,
    ) -> Result<DeployOutcome, OtaError>
    where
        F: for<'a> FnOnce(
            &'a mut tokio::fs::File,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), OtaError>> + Send + 'a>,
        >,
    {
        let (deploy_path, staging_path, mut unpublished, mut staging) =
            self.begin_staging().await?;
        write(&mut staging).await?;
        self.commit_staging(
            deploy_path,
            staging_path,
            &mut unpublished,
            &mut staging,
            should_cancel,
        )
        .await
    }

    fn sync_deploy_parent(deploy_path: &Path) -> Result<(), OtaError> {
        let Some(parent) = deploy_path.parent() else {
            return Ok(());
        };

        #[cfg(unix)]
        {
            use std::fs::OpenOptions;
            use std::os::unix::fs::OpenOptionsExt;

            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY)
                .open(parent)?
                .sync_all()?;
        }

        #[cfg(not(unix))]
        {
            let _ = parent;
        }

        Ok(())
    }

    async fn ensure_deploy_dir(&self, deploy_path: &Path) -> Result<(), OtaError> {
        #[cfg(any(
            test,
            feature = "emulator",
            all(
                feature = "deviceless",
                not(any(feature = "kobo", feature = "emulator"))
            )
        ))]
        {
            if let Some(parent) = deploy_path.parent() {
                tracing::debug!(directory = ?parent, "Creating parent directory");
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let _ = deploy_path;
        Ok(())
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, data, should_cancel), fields(bytes = data.len()))
    )]
    async fn deploy_bytes(
        &self,
        data: &[u8],
        should_cancel: CancelFunc<'_>,
    ) -> Result<DeployOutcome, OtaError> {
        let byte_count = data.len();
        let cancel = should_cancel;
        let (deploy_path, staging_path, mut unpublished, mut staging) =
            self.begin_staging().await?;
        use tokio::io::AsyncWriteExt;
        for chunk in data.chunks(64 * 1024) {
            if cancel.is_cancelled() {
                return Err(OtaError::Cancelled);
            }
            staging.write_all(chunk).await?;
        }
        let outcome = self
            .commit_staging(
                deploy_path,
                staging_path,
                &mut unpublished,
                &mut staging,
                cancel,
            )
            .await?;

        tracing::debug!(bytes = byte_count, path = ?outcome.path(), "Deployment complete");
        tracing::info!(path = ?outcome.path(), "Update deployed successfully");

        Ok(outcome)
    }

    /// Extracts KoboRoot.tgz from an on-disk artifact ZIP and deploys it.
    ///
    /// Used by the streaming fallback when the artifact is too large to buffer in
    /// memory. The entry is copied straight into the staging file, so the
    /// decompressed bundle is written to flash exactly once. On success, the
    /// source artifact ZIP is deleted as a best-effort cleanup step.
    ///
    /// # Arguments
    ///
    /// * `zip_path` - Path to the downloaded artifact ZIP file
    ///
    /// # Returns
    ///
    /// The deploy outcome, or an error if the bundle was not published.
    ///
    /// # Errors
    ///
    /// * `OtaError::ZipError` - Failed to open or read the ZIP archive, or its CRC32 did not match
    /// * `OtaError::DeploymentError` - KoboRoot.tgz not found in archive
    /// * `OtaError::Io` - Failed to write deployment file before the bundle was renamed
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub async fn extract_and_deploy(
        &self,
        zip_path: PathBuf,
        should_cancel: CancelFunc<'_>,
    ) -> Result<DeployOutcome, OtaError> {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }

        tracing::info!(path = ?zip_path, "Extracting and deploying update");

        let file = tokio::fs::File::open(&zip_path).await?;
        let mut reader = ZipArchiveReader::open(tokio::io::BufReader::new(file).compat())
            .await
            .map_err(|error| OtaError::ZipError(error.to_string()))?;
        let (mut entry, _) =
            open_kobo_root_entry(&mut reader, kobo_root_gzip_name(), should_cancel).await?;

        let (deploy_path, staging_path, mut unpublished, mut staging) =
            self.begin_staging().await?;
        copy_entry_to_file(&mut entry, &mut staging, should_cancel).await?;
        tokio::io::AsyncWriteExt::flush(&mut staging).await?;

        let outcome = self
            .commit_staging(
                deploy_path,
                staging_path,
                &mut unpublished,
                &mut staging,
                should_cancel,
            )
            .await?;

        if let Err(e) = tokio::fs::remove_file(&zip_path).await {
            tracing::error!(path = ?zip_path, error = %e, "Failed to remove source file");
        }

        Ok(outcome)
    }

    /// Extracts KoboRoot.tgz from an in-memory artifact ZIP and deploys it.
    ///
    /// The ZIP never touches flash: the `KoboRoot` entry is decompressed into a
    /// buffer and the bundle is published with a single staged write.
    ///
    /// # Returns
    ///
    /// The deploy outcome, or an error if the bundle was not published.
    ///
    /// # Errors
    ///
    /// * `OtaError::ZipError` - Failed to read the in-memory ZIP, or its CRC32 did not match
    /// * `OtaError::DeploymentError` - KoboRoot.tgz not found in archive
    /// * `OtaError::Io` - Failed to write deployment file before the bundle was renamed
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, zip_bytes, should_cancel), fields(zip_bytes = zip_bytes.len()))
    )]
    pub(crate) async fn extract_and_deploy_bytes(
        &self,
        zip_bytes: Vec<u8>,
        should_cancel: CancelFunc<'_>,
    ) -> Result<DeployOutcome, OtaError> {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }

        tracing::info!(
            bytes = zip_bytes.len(),
            "Extracting in-memory artifact and deploying"
        );

        let mut reader = ZipArchiveReader::open(Cursor::new(zip_bytes))
            .await
            .map_err(|error| OtaError::ZipError(error.to_string()))?;
        let (mut entry, uncompressed) =
            open_kobo_root_entry(&mut reader, kobo_root_gzip_name(), should_cancel).await?;
        let mut kobo_root_data = Vec::with_capacity(usize::try_from(uncompressed).unwrap_or(0));
        copy_cancellable(&mut entry, &mut kobo_root_data, should_cancel).await?;
        tracing::debug!(bytes = kobo_root_data.len(), "Extracted file into memory");

        self.deploy_bytes(&kobo_root_data, should_cancel).await
    }

    /// Decodes an xz payload into `KoboRoot.tgz` and deploys it.
    ///
    /// The xz stream is decoded and re-compressed with gzip in a single pass
    /// straight into the staging file, so the only flash write is the final
    /// `KoboRoot.tgz`. The CPU-bound codec runs on the blocking pool one chunk at
    /// a time while reads and writes stay async. `progress` receives
    /// [`OtaProgress::Decompressing`]. On success an on-disk source is deleted as
    /// a best-effort cleanup step.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientDeploySpace` - The deploy filesystem cannot hold
    ///   the bundle
    /// * `OtaError::Decompress` - The xz stream is corrupt or too demanding
    /// * `OtaError::ZipError` - The wrapping ZIP is invalid or fails its CRC32
    /// * `OtaError::DeploymentError` - No `KoboRoot` xz member in the ZIP
    /// * `OtaError::Cancelled` - The user cancelled
    /// * `OtaError::Io` - Reading the payload or writing the staging file failed
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, source, progress, should_cancel))
    )]
    pub async fn transcode_and_deploy<F>(
        &self,
        source: XzSource,
        mut progress: F,
        should_cancel: CancelFunc<'_>,
    ) -> Result<DeployOutcome, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }

        tracing::info!("Decoding xz bundle and deploying");

        let (deploy_path, staging_path, mut unpublished, mut staging) =
            self.begin_staging().await?;

        match xz::uncompressed_size(&source).await {
            Some(raw_size) => check_deploy_space(&deploy_path, xz::gzip_bound(raw_size))?,
            None => tracing::warn!("xz index unreadable; skipping the deploy space check"),
        }

        xz::transcode_to_gzip(&source, &mut staging, should_cancel, &mut progress).await?;

        let outcome = self
            .commit_staging(
                deploy_path,
                staging_path,
                &mut unpublished,
                &mut staging,
                should_cancel,
            )
            .await?;

        if let Some(path) = source.path()
            && let Err(e) = tokio::fs::remove_file(path).await
        {
            tracing::error!(path = ?path, error = %e, "Failed to remove source file");
        }

        Ok(outcome)
    }

    /// Queries the GitHub API for the repository's default branch name.
    async fn fetch_default_branch(&self) -> Result<String, OtaError> {
        let repo_url = "https://api.github.com/repos/ogkevin/cadmus";
        tracing::debug!(url = %repo_url, "Fetching repository metadata");

        let repo: Repository = self
            .github
            .api_get(repo_url)
            .send().await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status_code(), error = %e, "Repository metadata fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(default_branch = %repo.default_branch, "Resolved default branch");
        Ok(repo.default_branch)
    }

    /// Fetches artifacts for a workflow run and picks the OTA bundle.
    ///
    /// The xz artifact wins over the legacy gzip one when a run has both.
    async fn find_ota_artifact_in_run(
        &self,
        run_id: u64,
        prefixes: &ArtifactPrefixes,
    ) -> Result<(Artifact, PayloadFormat), OtaError> {
        let artifacts_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/runs/{}/artifacts?per_page=50",
            run_id
        );
        tracing::debug!(url = %artifacts_url, "Fetching artifacts");

        let artifacts: ArtifactsResponse = self
            .github
            .api_get(&artifacts_url)
            .send()
            .await?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(run_id, status = ?e.status_code(), error = %e, "Artifacts fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(count = artifacts.artifacts.len(), "Found artifacts");

        #[cfg(feature = "tracing")]
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (idx, artifact) in artifacts.artifacts.iter().enumerate() {
                tracing::debug!(
                    index = idx,
                    name = %artifact.name,
                    id = artifact.id,
                    size_bytes = artifact.size_in_bytes,
                    "Artifact"
                );
            }
        }

        select_artifact(artifacts.artifacts, prefixes).ok_or_else(|| {
            tracing::error!(run_id, xz = %prefixes.xz, legacy = %prefixes.legacy, "No matching artifact found");
            OtaError::ArtifactsNotFound(ArtifactSource::WorkflowRun(prefixes.legacy.clone()))
        })
    }

    /// Downloads an artifact ZIP, buffering it in memory when it fits.
    ///
    /// The in-memory path never writes the ZIP to flash. When it does not fit,
    /// the ZIP is downloaded to `download_path` and the caller streams the
    /// bundle entry from disk. `format` selects the memory heuristic and the
    /// returned variant.
    ///
    /// GitHub authentication is required for this operation.
    async fn download_artifact<F>(
        &self,
        artifact: &Artifact,
        format: PayloadFormat,
        download_path: &PathBuf,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<DownloadedArtifact, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        let download_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/artifacts/{}/zip",
            artifact.id
        );
        let mut progress = |downloaded, total| {
            progress_callback(OtaProgress::DownloadingArtifact { downloaded, total })
        };

        if payload_fits_in_memory(format, artifact.size_in_bytes) {
            tracing::debug!(
                artifact_id = artifact.id,
                size_bytes = artifact.size_in_bytes,
                "Buffering artifact in memory"
            );
            let bytes = self
                .github
                .download_to_vec(
                    &download_url,
                    Some(artifact.size_in_bytes),
                    |url| self.github.get(url),
                    &mut progress,
                    should_cancel,
                )
                .await?;
            return Ok(match format {
                PayloadFormat::Gzip => DownloadedArtifact::InMemory(bytes),
                PayloadFormat::Xz => DownloadedArtifact::Xz(XzSource::ZipBytes(bytes)),
            });
        }

        self.github
            .download(
                &download_url,
                Some(artifact.size_in_bytes),
                download_path,
                |url| self.github.get(url),
                &mut progress,
                should_cancel,
            )
            .await?;
        Ok(match format {
            PayloadFormat::Gzip => DownloadedArtifact::Zipped(download_path.clone()),
            PayloadFormat::Xz => DownloadedArtifact::Xz(XzSource::ZipFile(download_path.clone())),
        })
    }

    /// Downloads a release asset to the specified path with chunked transfer and progress reporting.
    ///
    /// GitHub authentication is not required for this operation as release
    /// assets are downloaded from public URLs.
    #[inline]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, progress_callback))
    )]
    async fn download_release_asset<F>(
        &self,
        asset: &ReleaseAsset,
        download_path: &PathBuf,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<(), OtaError>
    where
        F: FnMut(OtaProgress),
    {
        self.github
            .download(
                &asset.browser_download_url,
                Some(asset.size),
                download_path,
                |url| self.github.get_unauthenticated(url),
                &mut |downloaded, total| {
                    progress_callback(OtaProgress::DownloadingArtifact { downloaded, total })
                },
                should_cancel,
            )
            .await?;
        Ok(())
    }

    /// Downloads the `KoboRoot.tar.xz` release asset, into memory when it fits.
    async fn download_release_xz<F>(
        &self,
        asset: &ReleaseAsset,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<DownloadedArtifact, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        if xz::fits_in_memory(asset.size) {
            tracing::debug!(size_bytes = asset.size, "Buffering release asset in memory");
            let bytes = self
                .github
                .download_to_vec(
                    &asset.browser_download_url,
                    Some(asset.size),
                    |url| self.github.get_unauthenticated(url),
                    &mut |downloaded, total| {
                        progress_callback(OtaProgress::DownloadingArtifact { downloaded, total })
                    },
                    should_cancel,
                )
                .await?;
            return Ok(DownloadedArtifact::Xz(XzSource::RawBytes(bytes)));
        }

        let download_path = self.tmp_dir.join("cadmus-ota-stable-release.tar.xz");
        self.download_release_asset(asset, &download_path, progress_callback, should_cancel)
            .await?;
        Ok(DownloadedArtifact::Xz(XzSource::RawFile(download_path)))
    }
}

async fn copy_file_to_file(
    src: &mut tokio::fs::File,
    dst: &mut tokio::fs::File,
    should_cancel: CancelFunc<'_>,
) -> Result<(), OtaError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;

    loop {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }

        let read = src.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        dst.write_all(&buf[..read]).await?;
        total += read as u64;
    }

    tracing::debug!(bytes = total, "Streamed file to staging path");
    Ok(())
}

fn is_ota_candidate_run(run: &WorkflowRun) -> bool {
    run.name == "Cargo"
        && matches!(run.status.as_str(), "completed" | "in_progress")
        && !matches!(
            run.conclusion.as_deref(),
            Some("cancelled") | Some("skipped")
        )
}

/// Verifies that the GitHub token has all scopes required for OTA operations.
///
/// Delegates to [`GithubClient::verify_token_scopes`], which reads the
/// `X-OAuth-Scopes` header from a lightweight `/user` request and compares
/// against [`crate::github::REQUIRED_SCOPES`].
///
/// Returns `Ok(())` if all scopes are present, or an `OtaError` that is
/// either a transport failure or missing scopes, so the caller can trigger
/// re-authentication.
async fn verify_scopes(github: &crate::github::GithubClient) -> Result<(), OtaError> {
    github
        .verify_token_scopes()
        .await
        .map_err(scope_check_error)
}

fn scope_check_error(error: crate::github::VerifyScopesError) -> OtaError {
    match error {
        crate::github::VerifyScopesError::Request(error) => api_error(error),
        crate::github::VerifyScopesError::HttpStatus { status } => {
            api_error(crate::github::ApiStatusError::from_status(status))
        }
        crate::github::VerifyScopesError::Transport(message) => OtaError::Api(message),
        crate::github::VerifyScopesError::InsufficientScopes(error) => {
            OtaError::InsufficientScopes(error)
        }
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_SYNC_DEPLOY_PARENT: Cell<bool> = const { Cell::new(false) };
}

trait HttpFailure {
    fn http_status(&self) -> Option<http::StatusCode>;
}

impl HttpFailure for reqwest::Error {
    fn http_status(&self) -> Option<http::StatusCode> {
        self.status()
    }
}

impl HttpFailure for crate::github::ApiStatusError {
    fn http_status(&self) -> Option<http::StatusCode> {
        Some(self.status_code())
    }
}

/// Maps a failed HTTP response to the appropriate `OtaError`.
///
/// A 401 Unauthorized response means the saved token has been revoked or
/// expired — the caller should re-authenticate via device flow rather than
/// treating this as a generic API error.
fn api_error(error: impl std::fmt::Display + HttpFailure) -> OtaError {
    if error.http_status() == Some(http::StatusCode::UNAUTHORIZED) {
        tracing::warn!("GitHub API returned 401 — token invalid or revoked");
        OtaError::Unauthorized
    } else {
        OtaError::Api(error.to_string())
    }
}

fn check_disk_space(path: &Path) -> Result<(), OtaError> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs(path)?;
    let available_mb =
        (stat.blocks_available() as u64 * stat.fragment_size() as u64) / (1024 * 1024);
    tracing::debug!(path = ?path, available_mb, "Checking disk space");

    if available_mb < 100 {
        tracing::error!(
            path = ?path,
            available_mb,
            required_mb = 100,
            "Insufficient disk space"
        );
        return Err(OtaError::InsufficientSpace(available_mb));
    }
    Ok(())
}

/// Fails when the deploy filesystem cannot hold `needed_bytes`.
///
/// [`check_disk_space`] only covers `tmp_dir`; the staged `KoboRoot.tgz` is
/// written next to the deploy path, which can be a different filesystem.
fn check_deploy_space(deploy_path: &Path, needed_bytes: u64) -> Result<(), OtaError> {
    use nix::sys::statvfs::statvfs;

    let directory = deploy_path.parent().unwrap_or(deploy_path);
    let stat = statvfs(directory)?;
    let available = stat.blocks_available() as u64 * stat.fragment_size() as u64;
    tracing::debug!(path = ?directory, available, needed_bytes, "Checking deploy space");

    if available < needed_bytes {
        const MIB: u64 = 1024 * 1024;
        tracing::error!(path = ?directory, available, needed_bytes, "Insufficient deploy space");
        return Err(OtaError::InsufficientDeploySpace {
            needed_mb: needed_bytes / MIB,
            available_mb: available / MIB,
        });
    }
    Ok(())
}

/// Returns whether an artifact ZIP of `zip_size` bytes should be buffered in RAM.
///
/// Peak memory on the in-memory path is the ZIP plus the decompressed entry. The
/// entry is itself a gzip-compressed `KoboRoot.tgz`, so its decompressed size is
/// close to its stored size; doubling the ZIP size bounds both buffers. When the
/// doubled size does not fit, the caller streams the entry from disk instead.
fn artifact_fits_in_memory(zip_size: u64) -> bool {
    matches!(
        crate::memory::choose_buffer_path(zip_size.saturating_mul(2)),
        crate::memory::BufferPath::InMemory
    )
}

/// Which compression an OTA payload uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadFormat {
    /// A gzip `KoboRoot.tgz`, deployed as is.
    Gzip,
    /// An xz `KoboRoot.tar.xz`, decoded into a gzip bundle on the device.
    Xz,
}

/// Release asset holding the gzip bundle. Never dropped: USB installs and
/// firmware that predates xz support depend on it.
const GZIP_RELEASE_ASSET: &str = "KoboRoot.tgz";

/// Release asset holding the xz bundle. Stable releases ship no test variant,
/// so the name does not depend on the `test` feature.
const XZ_RELEASE_ASSET: &str = "KoboRoot.tar.xz";

/// Returns whether a downloaded payload of `size` bytes can be held in RAM.
fn payload_fits_in_memory(format: PayloadFormat, size: u64) -> bool {
    match format {
        PayloadFormat::Gzip => artifact_fits_in_memory(size),
        PayloadFormat::Xz => xz::fits_in_memory(size),
    }
}

/// Artifact name prefixes for one build, xz first.
///
/// The xz name carries its marker before the `pr<N>` / `<sha7>` part so the
/// legacy prefix, which firmware without xz support still searches for, can
/// never match it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactPrefixes {
    xz: String,
    legacy: String,
}

impl ArtifactPrefixes {
    fn for_pr(pr_number: u32) -> Self {
        Self::for_suffix(&format!("pr{pr_number}"))
    }

    fn for_commit(short_sha: &str) -> Self {
        Self::for_suffix(short_sha)
    }

    fn for_suffix(suffix: &str) -> Self {
        cfg_select! {
            feature = "test" => {
                Self {
                    xz: format!("cadmus-kobo-xz-test-{suffix}"),
                    legacy: format!("cadmus-kobo-test-{suffix}"),
                }
            }
            _ => {
                Self {
                    xz: format!("cadmus-kobo-xz-{suffix}"),
                    legacy: format!("cadmus-kobo-{suffix}"),
                }
            }
        }
    }
}

/// Picks the xz artifact when present, otherwise the legacy gzip one.
fn select_artifact(
    artifacts: Vec<Artifact>,
    prefixes: &ArtifactPrefixes,
) -> Option<(Artifact, PayloadFormat)> {
    let mut legacy = None;
    for artifact in artifacts.into_iter().filter(|a| !a.expired) {
        if artifact.name.starts_with(&prefixes.xz) {
            return Some((artifact, PayloadFormat::Xz));
        }
        if legacy.is_none() && artifact.name.starts_with(&prefixes.legacy) {
            legacy = Some(artifact);
        }
    }
    legacy.map(|artifact| (artifact, PayloadFormat::Gzip))
}

/// Picks the xz release asset when present, otherwise `KoboRoot.tgz`.
fn select_release_asset(assets: &[ReleaseAsset]) -> Option<(&ReleaseAsset, PayloadFormat)> {
    assets
        .iter()
        .find(|asset| asset.name == XZ_RELEASE_ASSET)
        .map(|asset| (asset, PayloadFormat::Xz))
        .or_else(|| {
            assets
                .iter()
                .find(|asset| asset.name == GZIP_RELEASE_ASSET)
                .map(|asset| (asset, PayloadFormat::Gzip))
        })
}

/// Name of the gzip bundle inside a CI artifact ZIP.
fn kobo_root_gzip_name() -> &'static str {
    cfg_select! {
        feature = "test" => { "KoboRoot-test.tgz" }
        _ => { "KoboRoot.tgz" }
    }
}

/// Locates the deployable bundle named `kobo_root_name` in an opened ZIP.
///
/// Returns the entry index and its uncompressed size. Directory entries are
/// skipped. The stored name may be the full path or only its final component.
///
/// # Errors
///
/// Returns [`OtaError::Cancelled`] when `should_cancel` trips during the scan,
/// [`OtaError::ZipError`] when an entry name is not UTF-8 or its size cannot be
/// read, and [`OtaError::DeploymentError`] when no matching member is present.
fn find_kobo_root_entry(
    cdrs: &[CDR],
    kobo_root_name: &str,
    should_cancel: CancelFunc<'_>,
) -> Result<(usize, u64), OtaError> {
    for (index, cdr) in cdrs.iter().enumerate() {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }
        let entry_name = cdr
            .insecure_file_name
            .as_str()
            .ok_or_else(|| OtaError::ZipError(ZipError::StringNotUtf8.to_string()))?;
        tracing::debug!(index, name = %entry_name, "Checking entry");
        if entry_name.ends_with('/') {
            continue;
        }
        if entry_name == kobo_root_name
            || Path::new(entry_name)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(kobo_root_name)
        {
            let uncompressed = cdr
                .uncompressed_size()
                .map_err(|error| OtaError::ZipError(error.to_string()))?;
            return Ok((index, uncompressed));
        }
    }

    tracing::error!(
        target_file = kobo_root_name,
        "Target file not found in artifact"
    );
    Err(OtaError::DeploymentError(format!(
        "{kobo_root_name} not found in artifact"
    )))
}

/// Opens the deployable bundle named `kobo_root_name` for reading.
///
/// Returns the entry reader and the member's uncompressed size. Cancellation is
/// checked while the central directory is scanned, before the member is opened.
///
/// # Errors
///
/// Returns [`OtaError::Cancelled`] when `should_cancel` trips during the scan,
/// [`OtaError::ZipError`] when the member cannot be opened or its name is not
/// UTF-8, and [`OtaError::DeploymentError`] when the archive has no matching member.
pub(super) async fn open_kobo_root_entry<'a, R>(
    reader: &'a mut ZipArchiveReader<R>,
    kobo_root_name: &str,
    should_cancel: CancelFunc<'_>,
) -> Result<(ZipFileReader<&'a mut R>, u64), OtaError>
where
    R: AsyncBufRead + AsyncSeek + Unpin,
{
    let (index, uncompressed) = find_kobo_root_entry(reader.cdrs(), kobo_root_name, should_cancel)?;
    let entry = reader
        .file(index)
        .await
        .map_err(|error| OtaError::ZipError(error.to_string()))?;
    Ok((entry, uncompressed))
}

/// Turns an entry-body read error into the message [`OtaError::ZipError`] carries.
///
/// `read1` reports a CRC mismatch as [`ZipError::CRC32CheckError`] inside a
/// [`std::io::Error`]. That case is rewritten to `CRC32 checksum mismatch`.
pub(super) fn zip_body_error(error: &Error) -> String {
    match error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ZipError>())
    {
        Some(ZipError::CRC32CheckError) => "CRC32 checksum mismatch".to_owned(),
        _ => error.to_string(),
    }
}

/// Copies an inflated ZIP entry into `buf`.
///
/// # Errors
///
/// Returns [`OtaError::Cancelled`] when `should_cancel` trips between chunks,
/// and [`OtaError::ZipError`] when the entry cannot be read.
async fn copy_cancellable<R>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    should_cancel: CancelFunc<'_>,
) -> Result<(), OtaError>
where
    R: AsyncRead + Unpin,
{
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }
        let n = reader
            .read(&mut chunk)
            .await
            .map_err(|error| OtaError::ZipError(zip_body_error(&error)))?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Copies an inflated ZIP entry into `out`.
///
/// # Errors
///
/// Returns [`OtaError::Cancelled`] when `should_cancel` trips between chunks,
/// [`OtaError::ZipError`] when the entry cannot be read, and [`OtaError::Io`]
/// when the staging file cannot be written.
async fn copy_entry_to_file<R>(
    reader: &mut R,
    out: &mut tokio::fs::File,
    should_cancel: CancelFunc<'_>,
) -> Result<(), OtaError>
where
    R: AsyncRead + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        if should_cancel.is_cancelled() {
            return Err(OtaError::Cancelled);
        }
        let n = reader
            .read(&mut chunk)
            .await
            .map_err(|error| OtaError::ZipError(zip_body_error(&error)))?;
        if n == 0 {
            return Ok(());
        }
        out.write_all(&chunk[..n]).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{ApiStatusError, GithubClient, ScopeError, VerifyScopesError};
    use http::StatusCode;
    use secrecy::SecretString;

    #[test]
    fn api_status_401_is_unauthorized() {
        let error = api_error(ApiStatusError::from_status(StatusCode::UNAUTHORIZED));
        assert!(matches!(error, OtaError::Unauthorized));
    }

    #[test]
    fn api_status_other_is_api_error() {
        let error = api_error(ApiStatusError::from_status(StatusCode::NOT_FOUND));
        assert!(matches!(error, OtaError::Api(_)));
    }

    #[test]
    fn scope_check_401_is_unauthorized() {
        let error = scope_check_error(VerifyScopesError::HttpStatus {
            status: StatusCode::UNAUTHORIZED,
        });
        assert!(matches!(error, OtaError::Unauthorized));
    }

    #[test]
    fn scope_check_missing_scopes_stay_insufficient() {
        let error = scope_check_error(VerifyScopesError::InsufficientScopes(ScopeError::new(
            vec!["public_repo".to_owned()],
        )));
        assert!(matches!(error, OtaError::InsufficientScopes(_)));
    }

    fn make_client(tmp_dir: PathBuf) -> OtaClient {
        crate::crypto::init_crypto_provider();
        let github =
            GithubClient::new(Some(SecretString::from("test_token"))).expect("client build");
        OtaClient::new(github, tmp_dir)
    }

    fn ota_test_tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("cadmus-ota-")
            .tempdir()
            .expect("failed to create temp dir")
    }

    fn assert_no_partial_staging(deploy_path: &Path) {
        let Some(parent) = deploy_path.parent() else {
            return;
        };
        let entries = match std::fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => panic!("read deploy directory: {e}"),
        };
        let leftovers: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
            .map(|entry| entry.path())
            .collect();
        assert!(
            leftovers.is_empty(),
            "unexpected staging leftovers: {leftovers:?}"
        );
    }
    fn no_cancel() -> CancelFunc<'static> {
        CancelFunc::never()
    }

    fn fixture_zip_bytes() -> Vec<u8> {
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/test_artifact.zip");
        std::fs::read(&fixture_path).expect("read fixture")
    }

    fn leftover_zips(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "zip"))
            .collect()
    }

    #[test]
    fn artifact_fits_in_memory_tracks_choose_buffer_path() {
        assert!(artifact_fits_in_memory(0));
        assert!(!artifact_fits_in_memory(u64::MAX / 4));
    }

    #[tokio::test]
    async fn test_extract_and_deploy_bytes_deploys_from_memory_without_a_zip_on_disk() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());

        let result = client
            .extract_and_deploy_bytes(fixture_zip_bytes(), no_cancel())
            .await;

        assert!(
            result.is_ok(),
            "Deployment should succeed: {:?}",
            result.err()
        );
        let deploy_path = result.unwrap().into_path();
        assert!(
            deploy_path.exists(),
            "Deployed file should exist at {deploy_path:?}"
        );
        let content = std::fs::read_to_string(&deploy_path).unwrap();
        assert!(content.contains("Mock KoboRoot.tgz"));
        assert_no_partial_staging(&deploy_path);
        assert!(
            leftover_zips(temp_dir.path()).is_empty(),
            "the in-memory path must not write the artifact ZIP to disk"
        );
    }

    #[tokio::test]
    async fn test_extract_and_deploy_bytes_cancelled_before_zip_walk() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let cancelled = AtomicBool::new(true);
        let cancel_check = || cancelled.load(Ordering::Relaxed);

        let result = client
            .extract_and_deploy_bytes(fixture_zip_bytes(), CancelFunc::new(&cancel_check))
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert!(
            !client.deploy_path().exists(),
            "cancelled in-memory extraction must not publish a bundle"
        );
        assert_no_partial_staging(&client.deploy_path());
    }

    #[tokio::test]
    async fn test_extract_and_deploy_bytes_rejects_crc_mismatch() {
        use async_zip::tokio::write::ZipFileWriter;
        use async_zip::{Compression, ZipEntryBuilder};

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let entry_name = if cfg!(feature = "test") {
            "KoboRoot-test.tgz"
        } else {
            "KoboRoot.tgz"
        };
        let payload = b"CRC-MISMATCH-PAYLOAD-0123456789";

        let mut zip_bytes = Vec::new();
        {
            let mut writer = ZipFileWriter::with_tokio(&mut zip_bytes);
            let builder = ZipEntryBuilder::new(entry_name.into(), Compression::Stored);
            writer.write_entry_whole(builder, payload).await.unwrap();
            writer.close().await.unwrap();
        }

        let offset = zip_bytes
            .windows(payload.len())
            .position(|window| window == payload)
            .expect("payload present in stored entry");
        zip_bytes[offset] ^= 0xFF;

        let result = client
            .extract_and_deploy_bytes(zip_bytes, no_cancel())
            .await;

        match result {
            Err(OtaError::ZipError(message)) => {
                assert!(message.contains("CRC32 checksum mismatch"), "{message}");
            }
            other => panic!("expected CRC32 rejection, got {other:?}"),
        }
        assert_no_partial_staging(&client.deploy_path());
    }

    struct FailSyncDeployParent;

    impl FailSyncDeployParent {
        fn arm() -> Self {
            FAIL_SYNC_DEPLOY_PARENT.with(|flag| flag.set(true));
            Self
        }
    }

    impl Drop for FailSyncDeployParent {
        fn drop(&mut self) {
            FAIL_SYNC_DEPLOY_PARENT.with(|flag| flag.set(false));
        }
    }

    #[tokio::test]
    async fn test_extract_and_deploy_success() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/test_artifact.zip");
        let artifact_path = temp_dir.path().join("test_artifact.zip");
        std::fs::copy(&fixture_path, &artifact_path).unwrap();

        let result = client
            .extract_and_deploy(artifact_path.clone(), no_cancel())
            .await;

        assert!(
            result.is_ok(),
            "Deployment should succeed: {:?}",
            result.err()
        );

        let deploy_path = result.unwrap().into_path();
        assert!(
            deploy_path.exists(),
            "Deployed file should exist at {:?}",
            deploy_path
        );

        let content = std::fs::read_to_string(&deploy_path).unwrap();
        assert!(
            content.contains("Mock KoboRoot.tgz"),
            "Deployed file should contain mock content"
        );
        assert_no_partial_staging(&deploy_path);
        assert!(
            !artifact_path.exists(),
            "Downloaded artifact should be removed after successful deployment"
        );
    }

    #[tokio::test]
    async fn test_atomic_deploy_success() {
        use tokio::io::AsyncWriteExt;

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let payload = b"new bundle content";

        let result = client
            .write_staging_then_rename(no_cancel(), |staging| {
                Box::pin(async {
                    staging.write_all(payload).await?;
                    Ok(())
                })
            })
            .await;

        assert!(
            result.is_ok(),
            "Deployment should succeed: {:?}",
            result.err()
        );
        let outcome = result.unwrap();
        assert!(matches!(outcome, DeployOutcome::Durable(_)));
        assert_eq!(outcome.path(), deploy_path.as_path());
        assert_eq!(std::fs::read(&deploy_path).unwrap(), payload);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_atomic_deploy_parent_sync_failure_is_committed_not_durable() {
        use tokio::io::AsyncWriteExt;

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let existing = b"existing bundle content";
        let payload = b"new bundle content";

        client.ensure_deploy_dir(&deploy_path).await.unwrap();
        std::fs::write(&deploy_path, existing).unwrap();

        let _fail_sync = FailSyncDeployParent::arm();
        let result = client
            .write_staging_then_rename(no_cancel(), |staging| {
                Box::pin(async {
                    staging.write_all(payload).await?;
                    Ok(())
                })
            })
            .await;

        match result {
            Ok(DeployOutcome::CommittedNotDurable { path, error }) => {
                assert_eq!(path, deploy_path);
                assert_eq!(
                    error.to_string(),
                    "I/O error: injected sync_deploy_parent failure"
                );
            }
            other => panic!("expected committed-not-durable outcome, got {other:?}"),
        }
        assert_eq!(std::fs::read(&deploy_path).unwrap(), payload);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_deploy_parent_sync_failure_still_removes_source() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let source_path = temp_dir.path().join("source-KoboRoot.tgz");
        let payload = b"source bundle content";

        std::fs::write(&source_path, payload).unwrap();
        let _fail_sync = FailSyncDeployParent::arm();

        let outcome = client
            .deploy(source_path.clone(), no_cancel())
            .await
            .expect("published bundle remains a successful deploy");

        assert!(matches!(outcome, DeployOutcome::CommittedNotDurable { .. }));
        assert_eq!(outcome.path(), deploy_path.as_path());
        assert_eq!(std::fs::read(&deploy_path).unwrap(), payload);
        assert!(
            !source_path.exists(),
            "source file should be removed after the bundle is committed"
        );
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_atomic_deploy_failed_write_preserves_existing_bundle() {
        use tokio::io::AsyncWriteExt;

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let existing = b"existing bundle content";

        client.ensure_deploy_dir(&deploy_path).await.unwrap();
        std::fs::write(&deploy_path, existing).unwrap();

        let result = client
            .write_staging_then_rename(no_cancel(), |staging| {
                Box::pin(async {
                    staging.write_all(b"partial data").await?;
                    Err(OtaError::Io(std::io::Error::other("simulated failure")))
                })
            })
            .await;

        assert!(result.is_err(), "Deployment should fail");
        assert_eq!(std::fs::read(&deploy_path).unwrap(), existing);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_atomic_deploy_failed_write_without_existing_bundle() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();

        let result = client
            .write_staging_then_rename(no_cancel(), |_| {
                Box::pin(async { Err(OtaError::Io(std::io::Error::other("simulated failure"))) })
            })
            .await;

        assert!(result.is_err(), "Deployment should fail");
        assert!(
            !deploy_path.exists(),
            "Final deploy path should not exist after failed deployment"
        );
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_atomic_deploy_cancelled_before_rename() {
        use crate::http::CancelFlag;
        use tokio::io::AsyncWriteExt;

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let existing = b"existing bundle content";
        let cancelled = CancelFlag::new();

        client.ensure_deploy_dir(&deploy_path).await.unwrap();
        std::fs::write(&deploy_path, existing).unwrap();
        cancelled.request_cancel();

        let result = client
            .write_staging_then_rename(CancelFunc::from_flag(&cancelled), |staging| {
                Box::pin(async {
                    staging.write_all(b"partial data").await?;
                    Ok(())
                })
            })
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert_eq!(std::fs::read(&deploy_path).unwrap(), existing);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_atomic_deploy_commit_ignores_late_cancel() {
        use crate::http::CancelFlag;
        use tokio::io::AsyncWriteExt;

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let payload = b"committed bundle content";
        let flag = CancelFlag::new();

        let result = client
            .write_staging_then_rename(CancelFunc::from_flag(&flag), |staging| {
                Box::pin(async {
                    staging.write_all(payload).await?;
                    Ok(())
                })
            })
            .await;

        assert!(
            result.is_ok(),
            "committed deploy should succeed: {result:?}"
        );
        flag.request_cancel();
        assert!(
            !flag.is_cancelled(),
            "cancel after commit must not flip cancelled state"
        );
        assert_eq!(std::fs::read(&deploy_path).unwrap(), payload);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_atomic_deploy_cancelled_after_staging_sync() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let existing = b"existing bundle content";
        let polls = AtomicUsize::new(0);

        client.ensure_deploy_dir(&deploy_path).await.unwrap();
        std::fs::write(&deploy_path, existing).unwrap();

        let cancel_check = || polls.fetch_add(1, Ordering::Relaxed) >= 1;
        let result = client
            .write_staging_then_rename(CancelFunc::new(&cancel_check), |staging| {
                Box::pin(async {
                    staging.write_all(b"partial data").await?;
                    Ok(())
                })
            })
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert_eq!(std::fs::read(&deploy_path).unwrap(), existing);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_deploy_bytes_cancelled_mid_write() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let existing = b"existing bundle content";
        let cancelled = AtomicBool::new(true);
        let payload = vec![0u8; 128 * 1024];

        client.ensure_deploy_dir(&deploy_path).await.unwrap();
        std::fs::write(&deploy_path, existing).unwrap();

        let cancel_check = || cancelled.load(Ordering::Relaxed);
        let result = client
            .deploy_bytes(&payload, CancelFunc::new(&cancel_check))
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert_eq!(std::fs::read(&deploy_path).unwrap(), existing);
        assert_no_partial_staging(&deploy_path);
    }

    #[tokio::test]
    async fn test_extract_and_deploy_cancelled_before_zip_walk() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/test_artifact.zip");
        let artifact_path = temp_dir.path().join("test_artifact.zip");
        std::fs::copy(&fixture_path, &artifact_path).unwrap();

        let cancelled = AtomicBool::new(true);
        let cancel_check = || cancelled.load(Ordering::Relaxed);
        let result = client
            .extract_and_deploy(artifact_path.clone(), CancelFunc::new(&cancel_check))
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert!(
            artifact_path.exists(),
            "Source artifact should be retained when extraction is cancelled"
        );
        assert_no_partial_staging(&client.deploy_path());
    }

    #[tokio::test]
    async fn test_extract_and_deploy_cancelled_during_zip_walk() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/test_artifact.zip");
        let artifact_path = temp_dir.path().join("test_artifact.zip");
        std::fs::copy(&fixture_path, &artifact_path).unwrap();

        let checks = AtomicUsize::new(0);
        let cancel_check = || checks.fetch_add(1, Ordering::Relaxed) >= 1;
        let result = client
            .extract_and_deploy(artifact_path.clone(), CancelFunc::new(&cancel_check))
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert!(
            artifact_path.exists(),
            "Source artifact should be retained when zip walk is cancelled"
        );
    }

    #[tokio::test]
    async fn test_extract_and_deploy_cancelled_during_entry_read() {
        use async_zip::tokio::write::ZipFileWriter;
        use async_zip::{Compression, ZipEntryBuilder};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let artifact_path = temp_dir.path().join("large_artifact.zip");
        let payload = vec![0u8; 128 * 1024];
        {
            let mut file = tokio::fs::File::create(&artifact_path).await.unwrap();
            let mut writer = ZipFileWriter::with_tokio(&mut file);
            for name in ["KoboRoot.tgz", "KoboRoot-test.tgz"] {
                let builder = ZipEntryBuilder::new(name.into(), Compression::Stored);
                writer.write_entry_whole(builder, &payload).await.unwrap();
            }
            writer.close().await.unwrap();
        }

        let checks = AtomicUsize::new(0);
        let cancel_check = || checks.fetch_add(1, Ordering::Relaxed) >= 3;
        let result = client
            .extract_and_deploy(artifact_path.clone(), CancelFunc::new(&cancel_check))
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)));
        assert!(
            artifact_path.exists(),
            "Source artifact should be retained when entry extraction is cancelled"
        );
        assert_no_partial_staging(&client.deploy_path());
    }

    #[test]
    fn test_staging_path_uses_partial_suffix() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let deploy_path = client.deploy_path();
        let staging_path = client.staging_path();
        let staging_name = staging_path
            .file_name()
            .expect("staging file name")
            .to_string_lossy();
        let deploy_name = deploy_path
            .file_name()
            .expect("deploy file name")
            .to_string_lossy();

        assert_eq!(staging_path.parent(), deploy_path.parent());
        assert!(
            staging_name.starts_with(&format!("{deploy_name}.")),
            "staging name {staging_name} should be derived from deploy name"
        );
        assert!(
            staging_name.ends_with(".partial"),
            "staging name {staging_name} should end with .partial"
        );
        assert_ne!(
            client.staging_path(),
            staging_path,
            "each staging path should be unique"
        );
    }

    #[tokio::test]
    async fn test_extract_and_deploy_missing_koboroot() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/empty_artifact.zip");
        let artifact_path = temp_dir.path().join("empty_artifact.zip");
        std::fs::copy(&fixture_path, &artifact_path).unwrap();

        let result = client
            .extract_and_deploy(artifact_path.clone(), no_cancel())
            .await;
        assert!(result.is_err(), "Should fail when KoboRoot.tgz is missing");

        if let Err(OtaError::DeploymentError(msg)) = result {
            assert!(
                msg.contains("not found in artifact"),
                "Error should mention missing file"
            );
        } else {
            panic!("Expected DeploymentError");
        }

        assert!(
            artifact_path.exists(),
            "Source artifact should be retained when deployment fails"
        );
    }

    #[test]
    fn test_check_disk_space_sufficient() {
        use tempfile::TempDir;
        let temp_dir = TempDir::new().unwrap();
        let result = check_disk_space(temp_dir.path());
        assert!(
            result.is_ok(),
            "Should have sufficient disk space in temp directory"
        );
    }

    fn cargo_run(id: u64, status: &str, conclusion: Option<&str>) -> WorkflowRun {
        WorkflowRun {
            name: "Cargo".to_owned(),
            id,
            status: status.to_owned(),
            conclusion: conclusion.map(str::to_owned),
            head_sha: None,
        }
    }

    #[test]
    fn test_ota_candidate_run_keeps_completed_success_and_failure() {
        assert!(is_ota_candidate_run(&cargo_run(
            1,
            "completed",
            Some("success")
        )));
        assert!(is_ota_candidate_run(&cargo_run(
            2,
            "completed",
            Some("failure")
        )));
    }

    #[test]
    fn test_ota_candidate_run_keeps_in_progress() {
        assert!(is_ota_candidate_run(&cargo_run(3, "in_progress", None)));
    }

    #[test]
    fn test_ota_candidate_run_drops_cancelled_skipped_and_queued() {
        assert!(!is_ota_candidate_run(&cargo_run(
            4,
            "completed",
            Some("cancelled")
        )));
        assert!(!is_ota_candidate_run(&cargo_run(
            5,
            "completed",
            Some("skipped")
        )));
        assert!(!is_ota_candidate_run(&cargo_run(6, "queued", None)));
    }

    #[test]
    fn test_ota_candidate_run_requires_cargo_name() {
        let run = WorkflowRun {
            name: "Docs".to_owned(),
            id: 7,
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
            head_sha: None,
        };
        assert!(!is_ota_candidate_run(&run));
    }

    fn create_external_client(tmp_dir: PathBuf) -> OtaClient {
        crate::crypto::init_crypto_provider();
        let token = std::env::var("GH_TOKEN").expect("GH_TOKEN must be set");
        let github = GithubClient::new(Some(SecretString::from(token))).expect("client build");
        OtaClient::new(github, tmp_dir)
    }

    #[tokio::test]
    #[ignore]
    async fn test_external_download_default_branch_and_deploy() {
        let temp_dir = ota_test_tempdir();
        let client = create_external_client(temp_dir.path().to_path_buf());
        let mut last_progress = None;

        let download_result = client
            .download_default_branch_artifact(
                |progress| {
                    last_progress = Some(format!("{:?}", progress));
                },
                None,
            )
            .await;

        assert!(
            download_result.is_ok(),
            "Default branch artifact download should succeed: {:?}",
            download_result.err()
        );

        let artifact = download_result.unwrap();
        let deploy_result = match artifact {
            DownloadedArtifact::InMemory(bytes) => {
                assert!(!bytes.is_empty(), "Downloaded ZIP should not be empty");
                client.extract_and_deploy_bytes(bytes, no_cancel()).await
            }
            DownloadedArtifact::Zipped(path) => {
                assert!(path.exists(), "Downloaded ZIP should exist at {path:?}");
                assert!(
                    path.metadata().unwrap().len() > 0,
                    "Downloaded ZIP should not be empty"
                );
                client.extract_and_deploy(path, no_cancel()).await
            }
            DownloadedArtifact::Xz(source) => {
                client
                    .transcode_and_deploy(source, |_| {}, no_cancel())
                    .await
            }
            DownloadedArtifact::Bundle(_) => {
                unreachable!("default branch artifacts are ZIP archives")
            }
        };

        assert!(
            deploy_result.is_ok(),
            "Deployment should succeed: {:?}",
            deploy_result.err()
        );

        let deploy_path = deploy_result.unwrap().into_path();
        assert!(
            deploy_path.exists(),
            "Deployed file should exist at {:?}",
            deploy_path
        );

        std::fs::remove_file(&deploy_path).ok();
    }

    #[tokio::test]
    #[ignore]
    async fn test_external_download_stable_release_and_deploy() {
        let temp_dir = ota_test_tempdir();
        let client = create_external_client(temp_dir.path().to_path_buf());
        let download_result = client.download_stable_release_artifact(|_| {}, None).await;

        assert!(
            download_result.is_ok(),
            "Stable release artifact download should succeed: {:?}",
            download_result.err()
        );

        let deploy_result = match download_result.unwrap() {
            DownloadedArtifact::Bundle(asset_path) => {
                assert!(
                    asset_path.exists(),
                    "Downloaded asset should exist at {:?}",
                    asset_path
                );
                assert!(
                    asset_path.metadata().unwrap().len() > 0,
                    "Downloaded asset should not be empty"
                );
                client.deploy(asset_path, no_cancel()).await
            }
            DownloadedArtifact::Xz(source) => {
                client
                    .transcode_and_deploy(source, |_| {}, no_cancel())
                    .await
            }
            _ => unreachable!("stable releases ship a bare bundle"),
        };

        assert!(
            deploy_result.is_ok(),
            "Deployment should succeed: {:?}",
            deploy_result.err()
        );

        let deploy_path = deploy_result.unwrap().into_path();
        assert!(
            deploy_path.exists(),
            "Deployed file should exist at {:?}",
            deploy_path
        );

        std::fs::remove_file(&deploy_path).ok();
    }

    fn artifact(name: &str, expired: bool) -> Artifact {
        Artifact {
            name: name.to_owned(),
            id: 1,
            size_in_bytes: 1,
            expired,
        }
    }

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_owned(),
            browser_download_url: format!("https://example.invalid/{name}"),
            size: 1,
        }
    }

    #[test]
    fn artifact_prefixes_for_pr_and_commit_never_overlap() {
        for (prefixes, suffix) in [
            (ArtifactPrefixes::for_pr(123), "-abc1234"),
            (ArtifactPrefixes::for_commit("abc1234"), ""),
        ] {
            let xz_name = format!("{}{suffix}", prefixes.xz);
            let legacy_name = format!("{}{suffix}", prefixes.legacy);
            assert!(
                !xz_name.starts_with(&prefixes.legacy),
                "firmware without xz support must never match {xz_name}"
            );
            assert!(
                !legacy_name.starts_with(&prefixes.xz),
                "{legacy_name} must not look like an xz artifact"
            );
        }
    }

    #[test]
    fn select_artifact_prefers_xz_in_any_order() {
        let prefixes = ArtifactPrefixes::for_pr(7);
        let xz = format!("{}-abc1234", prefixes.xz);
        let legacy = format!("{}-abc1234", prefixes.legacy);

        for names in [[&legacy, &xz], [&xz, &legacy]] {
            let artifacts = names.iter().map(|name| artifact(name, false)).collect();
            let (selected, format) = select_artifact(artifacts, &prefixes).unwrap();
            assert_eq!(format, PayloadFormat::Xz);
            assert_eq!(selected.name, xz);
        }
    }

    #[test]
    fn select_artifact_falls_back_to_legacy_and_skips_expired_and_other_bundles() {
        let prefixes = ArtifactPrefixes::for_pr(7);
        let legacy = format!("{}-abc1234", prefixes.legacy);

        let artifacts = vec![
            artifact(&format!("{}-abc1234", prefixes.xz), true),
            artifact("cadmus-kobo-nm-pr7-abc1234", false),
            artifact("cadmus-kobo-dist-pr7-abc1234", false),
            artifact(&legacy, false),
        ];
        let (selected, format) = select_artifact(artifacts, &prefixes).unwrap();
        assert_eq!(format, PayloadFormat::Gzip);
        assert_eq!(selected.name, legacy);

        let none = vec![
            artifact(&format!("{}-abc1234", prefixes.xz), true),
            artifact("cadmus-kobo-nm-pr7-abc1234", false),
        ];
        assert!(select_artifact(none, &prefixes).is_none());
    }

    #[test]
    fn select_release_asset_prefers_xz_then_falls_back_to_tgz() {
        let both = [
            asset("KoboRoot-nm.tgz"),
            asset(GZIP_RELEASE_ASSET),
            asset(XZ_RELEASE_ASSET),
        ];
        let (selected, format) = select_release_asset(&both).unwrap();
        assert_eq!(format, PayloadFormat::Xz);
        assert_eq!(selected.name, XZ_RELEASE_ASSET);

        let legacy_only = [asset("KoboRoot-nm.tgz"), asset(GZIP_RELEASE_ASSET)];
        let (selected, format) = select_release_asset(&legacy_only).unwrap();
        assert_eq!(format, PayloadFormat::Gzip);
        assert_eq!(selected.name, GZIP_RELEASE_ASSET);

        assert!(select_release_asset(&[asset("KoboRoot-nm.tgz")]).is_none());
    }

    #[test]
    fn check_deploy_space_rejects_requests_larger_than_the_filesystem() {
        let temp_dir = ota_test_tempdir();
        let deploy_path = temp_dir.path().join("KoboRoot.tgz");

        assert!(check_deploy_space(&deploy_path, 1).is_ok());
        assert!(matches!(
            check_deploy_space(&deploy_path, u64::MAX),
            Err(OtaError::InsufficientDeploySpace { .. })
        ));
    }

    fn xz_fixture(data: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let mut writer =
            lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(1)).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap()
    }

    fn zip_fixture(name: &str, data: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn bundle_payload() -> Vec<u8> {
        (0..200_000_u32)
            .flat_map(|n| n.wrapping_mul(2_654_435_761).to_le_bytes())
            .collect()
    }

    fn gunzip_file(path: &Path) -> Vec<u8> {
        use std::io::Read;

        let mut out = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap())
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    #[tokio::test]
    async fn transcode_and_deploy_publishes_a_gzip_bundle_and_reports_progress() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let payload = bundle_payload();
        let source =
            XzSource::ZipBytes(zip_fixture(xz::kobo_root_xz_name(), &xz_fixture(&payload)));
        let mut events = Vec::new();

        let outcome = client
            .transcode_and_deploy(source, |event| events.push(event), no_cancel())
            .await
            .unwrap();

        assert_eq!(gunzip_file(outcome.path()), payload);
        assert_no_partial_staging(outcome.path());
        assert!(matches!(
            events.last(),
            Some(OtaProgress::Decompressing { done, total }) if done == total
        ));
    }

    #[tokio::test]
    async fn transcode_and_deploy_removes_an_on_disk_source_after_publishing() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let payload = bundle_payload();
        let raw_path = temp_dir.path().join("cadmus-ota-stable-release.tar.xz");
        std::fs::write(&raw_path, xz_fixture(&payload)).unwrap();

        let outcome = client
            .transcode_and_deploy(XzSource::RawFile(raw_path.clone()), |_| {}, no_cancel())
            .await
            .unwrap();

        assert_eq!(gunzip_file(outcome.path()), payload);
        assert!(!raw_path.exists());
    }

    #[tokio::test]
    async fn transcode_and_deploy_rejects_corrupt_xz_without_publishing() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let mut xz = xz_fixture(&bundle_payload());
        let middle = xz.len() / 2;
        xz[middle] ^= 0xff;

        let result = client
            .transcode_and_deploy(XzSource::RawBytes(xz), |_| {}, no_cancel())
            .await;

        assert!(matches!(result, Err(OtaError::Decompress(_))), "{result:?}");
        assert!(!client.deploy_path().exists());
        assert_no_partial_staging(&client.deploy_path());
    }

    #[tokio::test]
    async fn transcode_and_deploy_cancelled_mid_stream_does_not_publish() {
        let temp_dir = ota_test_tempdir();
        let client = make_client(temp_dir.path().to_path_buf());
        let flag = CancelFlag::new();
        let source = XzSource::RawBytes(xz_fixture(&bundle_payload()));

        let result = client
            .transcode_and_deploy(
                source,
                |_| flag.request_cancel(),
                CancelFunc::from_flag(&flag),
            )
            .await;

        assert!(matches!(result, Err(OtaError::Cancelled)), "{result:?}");
        assert!(!client.deploy_path().exists());
        assert_no_partial_staging(&client.deploy_path());
    }
}
