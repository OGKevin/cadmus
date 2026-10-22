use crate::github::types::{
    Artifact, ArtifactsResponse, Release, ReleaseAsset, Repository, WorkflowRunsResponse,
};
use crate::github::{GithubClient, OtaProgress};
use crate::http::{CancelFlag, CancelFunc, ChunkedDownloadError};
use crate::ota::pr_status::{
    ArtifactSnapshot, OpenPull, RunSnapshot, artifact_name_matches_any_pr, pending_pulls,
    project_pulls, unknown_unless_ready,
};
use crate::ota::{CheckedPull, PrBuild};
use crate::version::GitVersion;
use async_zip::base::read1::ZipFileReader;
use async_zip::base::read1::seek::ZipArchiveReader;
use async_zip::error::ZipError;
use async_zip::spec::constructs::CDR;
use futures_lite::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncSeek, Cursor};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::collections::HashSet;
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
/// on disk ([`Self::Bundle`]).
#[derive(Debug)]
pub enum DownloadedArtifact {
    /// The artifact ZIP, held entirely in memory.
    InMemory(Vec<u8>),
    /// Path to the artifact ZIP on disk, used when it does not fit in RAM.
    Zipped(PathBuf),
    /// Path to a bare `KoboRoot.tgz` release asset.
    Bundle(PathBuf),
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
            crate::github::GithubRequestError::Unauthorized => Self::Unauthorized,
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

    /// Fills in which of `pulls` can be installed.
    ///
    /// `emit` runs once a live package is found, and again when the check
    /// finishes or stops early. `failed` is true only on that early stop.
    /// A row stays pending until then, unless it is already ready.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, pulls, emit), fields(pulls = pulls.len()))
    )]
    pub async fn resolve_install_status(
        &self,
        pulls: &[OpenPull],
        mut emit: impl FnMut(Vec<CheckedPull>, bool),
    ) -> Result<(), OtaError> {
        let runs = match self.github.list_cargo_pull_request_runs().await {
            Ok(runs) => runs,
            Err(error) => {
                tracing::warn!(error = %error, step = "cargo_runs", "install check failed");
                emit(unknown_unless_ready(pending_pulls(pulls)), true);
                return Err(error.into());
            }
        };
        let run_snapshots: Vec<RunSnapshot<'_>> = runs
            .iter()
            .map(|run| RunSnapshot {
                head_sha: run.head_sha.as_str(),
                name: run.name.as_str(),
                status: run.status.as_str(),
                conclusion: run.conclusion.as_deref(),
            })
            .collect();
        let heads: Vec<&str> = pulls.iter().map(|pull| pull.head_sha.as_str()).collect();
        let queries = crate::github::install_artifact_runs(&runs, &heads);
        let mut artifacts = Vec::new();
        let mut announced = 0;
        let mut failed_heads = Vec::new();
        let mut failure = None;
        for run in &queries {
            let listed = match self.github.list_run_artifacts(run.id).await {
                Ok(listed) => listed,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        run_id = run.id,
                        step = "run_artifacts",
                        "install check failed"
                    );
                    failed_heads.push(run.head_sha.as_str());
                    failure = Some(error);
                    continue;
                }
            };
            artifacts.extend(
                listed
                    .into_iter()
                    .filter(|artifact| artifact_name_matches_any_pr(&artifact.name)),
            );
            let rows = project_pulls(
                pulls,
                &run_snapshots,
                &artifact_snapshots(&artifacts),
                false,
            );
            let ready = rows
                .iter()
                .filter(|pull| pull.build == PrBuild::Ready && pull.artifact.is_some())
                .count();
            if ready > announced {
                announced = ready;
                emit(rows, false);
            }
        }
        let mut rows = project_pulls(pulls, &run_snapshots, &artifact_snapshots(&artifacts), true);
        if !failed_heads.is_empty() {
            let failed: HashSet<&str> = failed_heads.iter().copied().collect();
            rows = rows
                .into_iter()
                .zip(pulls)
                .map(|(mut row, pull)| {
                    if row.build != PrBuild::Ready && failed.contains(pull.head_sha.as_str()) {
                        row.build = PrBuild::Unknown;
                        row.artifact = None;
                    }
                    row
                })
                .collect();
        }
        let all_failed = !queries.is_empty() && failed_heads.len() == queries.len();
        emit(rows, all_failed);
        tracing::debug!(
            pulls = pulls.len(),
            runs = run_snapshots.len(),
            artifacts = artifacts.len(),
            ready = announced,
            failed = failed_heads.len(),
            "install check finished"
        );
        if all_failed && let Some(error) = failure {
            return Err(error.into());
        }
        Ok(())
    }

    /// Downloads a pull-request artifact that the picker already resolved.
    ///
    /// Skips the run lookup. Scope checks, disk space, and cancellation match
    /// the other downloads.
    pub async fn download_pr_artifact_from<F>(
        &self,
        pr_number: u64,
        artifact_id: u64,
        size_in_bytes: u64,
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

        let download_path = self.tmp_dir.join(format!("cadmus-ota-{pr_number}.zip"));
        let downloaded = self
            .download_artifact(
                artifact_id,
                size_in_bytes,
                &download_path,
                &mut progress_callback,
                should_cancel,
            )
            .await?;
        progress_callback(OtaProgress::Complete);
        Ok(downloaded)
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

        let artifact_name_prefix = cfg_select! {
            feature = "test" => { format!("cadmus-kobo-test-{}", short_sha) }
            _ => { format!("cadmus-kobo-{}", short_sha) }
        };

        tracing::debug!(pattern = %artifact_name_prefix, "Looking for artifact");

        progress_callback(OtaProgress::FindingWorkflow);

        let artifact = self
            .find_artifact_in_run(cargo_run.id, &artifact_name_prefix)
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
                artifact.id,
                artifact.size_in_bytes,
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
    /// 3. Locates the `KoboRoot.tgz` asset in the release
    /// 4. Downloads the file to `tmp_dir/cadmus-ota-stable-release.tgz`
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
    /// The downloaded `KoboRoot.tgz` bundle, always on disk.
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

        let asset_name = "KoboRoot.tgz";

        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| {
                tracing::error!(
                    target_asset = asset_name,
                    "Asset not found in latest release"
                );
                OtaError::ArtifactsNotFound(ArtifactSource::ReleaseAsset(asset_name.to_owned()))
            })?;

        tracing::debug!(
            name = %asset.name,
            url = %asset.browser_download_url,
            size_bytes = asset.size,
            "Found release asset"
        );

        let download_path = self.tmp_dir.join("cadmus-ota-stable-release.tgz");

        self.download_release_asset(asset, &download_path, &mut progress_callback, should_cancel)
            .await?;

        progress_callback(OtaProgress::Complete);

        tracing::info!("Stable release download completed");
        Ok(DownloadedArtifact::Bundle(download_path))
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
        let (mut entry, _) = open_kobo_root_entry(&mut reader, should_cancel).await?;

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
        let (mut entry, uncompressed) = open_kobo_root_entry(&mut reader, should_cancel).await?;
        let mut kobo_root_data = Vec::with_capacity(usize::try_from(uncompressed).unwrap_or(0));
        copy_cancellable(&mut entry, &mut kobo_root_data, should_cancel).await?;
        tracing::debug!(bytes = kobo_root_data.len(), "Extracted file into memory");

        self.deploy_bytes(&kobo_root_data, should_cancel).await
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

    /// Fetches artifacts for a workflow run and finds one matching the given prefix.
    async fn find_artifact_in_run(
        &self,
        run_id: u64,
        name_prefix: &str,
    ) -> Result<Artifact, OtaError> {
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

        tracing::debug!(pattern = %name_prefix, "Looking for artifact");

        artifacts
            .artifacts
            .into_iter()
            .find(|a| !a.expired && a.name.starts_with(name_prefix))
            .ok_or_else(|| {
                tracing::error!(run_id, pattern = %name_prefix, "No matching artifact found");
                OtaError::ArtifactsNotFound(ArtifactSource::WorkflowRun(name_prefix.to_owned()))
            })
    }

    /// Downloads an artifact ZIP, buffering it in memory when it fits.
    ///
    /// The in-memory path never writes the ZIP to flash. When it does not fit,
    /// the ZIP is downloaded to `download_path` and the caller streams the
    /// `KoboRoot` entry from disk.
    ///
    /// GitHub authentication is required for this operation.
    async fn download_artifact<F>(
        &self,
        artifact_id: u64,
        size_in_bytes: u64,
        download_path: &PathBuf,
        progress_callback: &mut F,
        should_cancel: Option<&CancelFlag>,
    ) -> Result<DownloadedArtifact, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        let download_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/artifacts/{artifact_id}/zip",
        );
        let mut progress = |downloaded, total| {
            progress_callback(OtaProgress::DownloadingArtifact { downloaded, total })
        };

        if artifact_fits_in_memory(size_in_bytes) {
            tracing::debug!(
                artifact_id,
                size_bytes = size_in_bytes,
                "Buffering artifact in memory"
            );
            let bytes = self
                .github
                .download_to_vec(
                    &download_url,
                    Some(size_in_bytes),
                    |url| self.github.get(url),
                    &mut progress,
                    should_cancel,
                )
                .await?;
            return Ok(DownloadedArtifact::InMemory(bytes));
        }

        self.github
            .download(
                &download_url,
                Some(size_in_bytes),
                download_path,
                |url| self.github.get(url),
                &mut progress,
                should_cancel,
            )
            .await?;
        Ok(DownloadedArtifact::Zipped(download_path.clone()))
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

fn artifact_snapshots(artifacts: &[crate::github::ListedArtifact]) -> Vec<ArtifactSnapshot<'_>> {
    artifacts
        .iter()
        .map(|artifact| ArtifactSnapshot {
            name: artifact.name.as_str(),
            id: artifact.id,
            size_in_bytes: artifact.size_in_bytes,
            expired: artifact.expired,
        })
        .collect()
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
    let available_mb = (stat.blocks_available() as u64 * stat.block_size() as u64) / (1024 * 1024);
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

/// Locates the deployable `KoboRoot` entry in an opened ZIP.
///
/// Returns the entry index and its uncompressed size. Directory entries are
/// skipped. The stored name may be the full path or only its final component.
///
/// # Errors
///
/// Returns [`OtaError::Cancelled`] when `should_cancel` trips during the scan,
/// [`OtaError::ZipError`] when an entry name is not UTF-8 or its size cannot be
/// read, and [`OtaError::DeploymentError`] when no `KoboRoot` member is present.
fn find_kobo_root_entry(
    cdrs: &[CDR],
    should_cancel: CancelFunc<'_>,
) -> Result<(usize, u64), OtaError> {
    let kobo_root_name = cfg_select! {
        feature = "test" => { "KoboRoot-test.tgz" }
        _ => { "KoboRoot.tgz" }
    };

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

/// Opens the deployable `KoboRoot` member for reading.
///
/// Returns the entry reader and the member's uncompressed size. Cancellation is
/// checked while the central directory is scanned, before the member is opened.
///
/// # Errors
///
/// Returns [`OtaError::Cancelled`] when `should_cancel` trips during the scan,
/// [`OtaError::ZipError`] when the member cannot be opened or its name is not
/// UTF-8, and [`OtaError::DeploymentError`] when the archive has no `KoboRoot`.
async fn open_kobo_root_entry<'a, R>(
    reader: &'a mut ZipArchiveReader<R>,
    should_cancel: CancelFunc<'_>,
) -> Result<(ZipFileReader<&'a mut R>, u64), OtaError>
where
    R: AsyncBufRead + AsyncSeek + Unpin,
{
    let (index, uncompressed) = find_kobo_root_entry(reader.cdrs(), should_cancel)?;
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
fn zip_body_error(error: &Error) -> String {
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

        let asset_path = match download_result.unwrap() {
            DownloadedArtifact::Bundle(path) => path,
            _ => unreachable!("stable releases ship a bare KoboRoot.tgz"),
        };
        assert!(
            asset_path.exists(),
            "Downloaded asset should exist at {:?}",
            asset_path
        );
        assert!(
            asset_path.metadata().unwrap().len() > 0,
            "Downloaded asset should not be empty"
        );

        let deploy_result = client.deploy(asset_path, no_cancel()).await;

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
}
