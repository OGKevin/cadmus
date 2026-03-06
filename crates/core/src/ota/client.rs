use crate::github::types::{
    Artifact, ArtifactsResponse, Release, ReleaseAsset, Repository, WorkflowRunsResponse,
};
use crate::github::{GithubClient, OtaProgress};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use zip::ZipArchive;

#[cfg(all(not(test), not(feature = "emulator")))]
use crate::settings::INTERNAL_CARD_ROOT;

const MIN_CHUNK_SIZE: usize = 256 * 1024;
const MAX_CHUNK_SIZE: usize = 10 * 1024 * 1024;
const INITIAL_CHUNK_SIZE: usize = 1024 * 1024;
/// Target 80% of the HTTP timeout to leave headroom for throughput variance.
const TARGET_CHUNK_SECS: f64 = crate::github::CLIENT_TIMEOUT_SECS as f64 * 0.8;

/// Maximum number of retry attempts for failed chunks
const MAX_RETRIES: usize = 3;

/// Downloads and deploys OTA updates from GitHub.
///
/// Delegates all HTTP communication to [`GithubClient`] and focuses solely on
/// the OTA-specific workflow: finding artifacts, chunked downloading, ZIP
/// extraction, and deploying `KoboRoot.tgz` to the Kobo device.
pub struct OtaClient {
    github: GithubClient,
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
    ZipError(#[from] zip::result::ZipError),

    /// Deployment process failed after successful download
    #[error("Deployment error: {0}")]
    DeploymentError(String),
}

impl OtaClient {
    /// Creates a new OTA client wrapping the provided GitHub client.
    ///
    /// # Errors
    ///
    /// Returns `OtaError::TlsConfig` if the underlying HTTP client fails to build.
    pub fn new(github: GithubClient) -> Self {
        Self { github }
    }

    /// Downloads the build artifact from a GitHub pull request.
    ///
    /// This performs the complete download workflow:
    /// 1. Verifies sufficient disk space (100MB required)
    /// 2. Fetches PR metadata to get the commit SHA
    /// 3. Finds the associated "Cargo" workflow run
    /// 4. Locates artifacts matching "cadmus-kobo-pr*" pattern
    /// 5. Downloads the artifact ZIP file to `/tmp/cadmus-ota-{pr_number}.zip`
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
    /// The path to the downloaded ZIP file on success.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in /tmp
    /// * `OtaError::NoToken` - GitHub token not configured
    /// * `OtaError::PrNotFound` - PR number doesn't exist in repository
    /// * `OtaError::ArtifactsNotFound` - No matching build artifacts found for the PR
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::Io` - Failed to write downloaded file to disk
    pub fn download_pr_artifact<F>(
        &self,
        pr_number: u32,
        mut progress_callback: F,
    ) -> Result<PathBuf, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        check_disk_space("/tmp")?;
        verify_scopes(&self.github)?;

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
            .get(&pr_url)
            .send()?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(pr_number, status = ?e.status(), error = %e, "PR fetch failed");
                if e.status() == Some(reqwest::StatusCode::UNAUTHORIZED) {
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
            .get(&runs_url)
            .send()?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(head_sha = %head_sha, status = ?e.status(), error = %e, "Workflow runs fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(count = runs.workflow_runs.len(), "Found workflow runs");

        #[cfg(feature = "otel")]
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (idx, run) in runs.workflow_runs.iter().enumerate() {
                tracing::debug!(
                    index = idx,
                    name = %run.name,
                    id = run.id,
                    "Workflow run"
                );
            }
        }

        let run = runs
            .workflow_runs
            .iter()
            .find(|r| r.name == "Cargo")
            .ok_or_else(|| {
                tracing::error!(pr_number, "No Cargo workflow run found");
                OtaError::ArtifactsNotFound(ArtifactSource::PullRequest(pr_number))
            })?;

        tracing::debug!(run_id = run.id, "Found Cargo workflow run");

        #[cfg(feature = "test")]
        let artifact_name_pattern = format!("cadmus-kobo-test-pr{}", pr_number);
        #[cfg(not(feature = "test"))]
        let artifact_name_pattern = format!("cadmus-kobo-pr{}", pr_number);

        let artifact = self
            .find_artifact_in_run(run.id, &artifact_name_pattern)
            .map_err(|e| match e {
                OtaError::ArtifactsNotFound(ArtifactSource::WorkflowRun(_)) => {
                    OtaError::ArtifactsNotFound(ArtifactSource::PullRequest(pr_number))
                }
                other => other,
            })?;

        tracing::debug!(
            name = %artifact.name,
            id = artifact.id,
            size_bytes = artifact.size_in_bytes,
            "Found artifact"
        );

        let download_path = PathBuf::from(format!("/tmp/cadmus-ota-{}.zip", pr_number));

        self.download_artifact_to_path(&artifact, &download_path, &mut progress_callback)?;

        progress_callback(OtaProgress::Complete {
            path: download_path.clone(),
        });

        tracing::info!(pr_number, "PR build download completed");
        Ok(download_path)
    }

    /// Downloads the latest build artifact from the default branch.
    ///
    /// This performs the complete download workflow for default branch builds:
    /// 1. Verifies sufficient disk space (100MB required)
    /// 2. Queries GitHub API for the latest successful `cargo.yml` workflow run on the default branch
    /// 3. Locates artifacts matching "cadmus-kobo-{sha}" pattern (or "cadmus-kobo-test-{sha}" with `test` feature)
    /// 4. Downloads the artifact ZIP file to `/tmp/cadmus-ota-{sha}.zip`
    ///
    /// GitHub authentication is required for this operation.
    ///
    /// # Arguments
    ///
    /// * `progress_callback` - Function called with progress updates during download
    ///
    /// # Returns
    ///
    /// The path to the downloaded ZIP file on success.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in /tmp
    /// * `OtaError::NoToken` - GitHub token not configured
    /// * `OtaError::ArtifactsNotFound` - No matching build artifacts found for default branch
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::Io` - Failed to write downloaded file to disk
    pub fn download_default_branch_artifact<F>(
        &self,
        mut progress_callback: F,
    ) -> Result<PathBuf, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        check_disk_space("/tmp")?;
        verify_scopes(&self.github)?;

        progress_callback(OtaProgress::FindingLatestBuild);
        tracing::info!("Starting main branch build download");
        tracing::debug!("Finding latest default branch build");

        let default_branch = self.fetch_default_branch()?;

        let encoded_branch = utf8_percent_encode(&default_branch, NON_ALPHANUMERIC);
        let runs_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/workflows/cargo.yml/runs?branch={}&event=push&status=success&per_page=1",
            encoded_branch
        );
        tracing::debug!(url = %runs_url, "Fetching Cargo workflow runs on default branch");

        let runs: WorkflowRunsResponse = self
            .github
            .get(&runs_url)
            .send()?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status(), error = %e, "Cargo workflow runs fetch failed");
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

        #[cfg(feature = "test")]
        let artifact_name_prefix = format!("cadmus-kobo-test-{}", short_sha);
        #[cfg(not(feature = "test"))]
        let artifact_name_prefix = format!("cadmus-kobo-{}", short_sha);

        tracing::debug!(pattern = %artifact_name_prefix, "Looking for artifact");

        progress_callback(OtaProgress::FindingWorkflow);

        let artifact = self
            .find_artifact_in_run(cargo_run.id, &artifact_name_prefix)
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

        let download_path = PathBuf::from(format!("/tmp/cadmus-ota-{}.zip", short_sha));

        self.download_artifact_to_path(&artifact, &download_path, &mut progress_callback)?;

        progress_callback(OtaProgress::Complete {
            path: download_path.clone(),
        });

        tracing::info!(sha = %short_sha, "Main branch build download completed");
        Ok(download_path)
    }

    /// Downloads the latest stable release artifact from GitHub releases.
    ///
    /// This performs the complete download workflow for stable releases:
    /// 1. Verifies sufficient disk space (100MB required)
    /// 2. Fetches the latest release from GitHub API
    /// 3. Locates the `KoboRoot.tgz` asset in the release
    /// 4. Downloads the file to `/tmp/cadmus-ota-stable-release.tgz`
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
    /// The path to the downloaded KoboRoot.tgz file on success.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in /tmp
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::ArtifactsNotFound` - KoboRoot.tgz not found in latest release
    /// * `OtaError::Io` - Failed to write downloaded file to disk
    #[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
    pub fn download_stable_release_artifact<F>(
        &self,
        mut progress_callback: F,
    ) -> Result<PathBuf, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        check_disk_space("/tmp")?;

        progress_callback(OtaProgress::FindingLatestBuild);
        tracing::info!("Starting stable release download");
        tracing::debug!("Finding latest stable release");

        let releases_url = "https://api.github.com/repos/ogkevin/cadmus/releases/latest";
        tracing::debug!(url = %releases_url, "Fetching latest release");

        let release: Release = self
            .github
            .get_unauthenticated(releases_url)
            .send()?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status(), error = %e, "Latest release fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(asset_count = release.assets.len(), "Found release assets");

        #[cfg(feature = "otel")]
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

        let download_path = PathBuf::from("/tmp/cadmus-ota-stable-release.tgz");

        self.download_release_asset(asset, &download_path, &mut progress_callback)?;

        progress_callback(OtaProgress::Complete {
            path: download_path.clone(),
        });

        tracing::info!("Stable release download completed");
        Ok(download_path)
    }

    /// Deploys KoboRoot.tgz from the specified path directly without extraction.
    ///
    /// Used when the artifact is already in the correct format (e.g., stable releases
    /// that are distributed as bare KoboRoot.tgz files).
    ///
    /// # Arguments
    ///
    /// * `kobo_root_path` - Path to the KoboRoot.tgz file to deploy
    ///
    /// # Returns
    ///
    /// The path where the file was deployed, or an error if deployment fails.
    ///
    /// # Errors
    ///
    /// * `OtaError::Io` - Failed to read or write files
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub fn deploy(&self, kobo_root_path: PathBuf) -> Result<PathBuf, OtaError> {
        tracing::info!(path = ?kobo_root_path, "Deploying KoboRoot.tgz");

        let mut kobo_root_data = Vec::new();
        {
            let mut file = File::open(&kobo_root_path)?;
            file.read_to_end(&mut kobo_root_data)?;
        }

        tracing::debug!(
            bytes = kobo_root_data.len(),
            path = ?kobo_root_path,
            "Read KoboRoot.tgz"
        );

        self.deploy_bytes(&kobo_root_data)
    }

    /// Deploys KoboRoot.tgz data to the appropriate location.
    ///
    /// Writes the provided data to the deployment path determined by the build configuration:
    /// - Test builds: temp directory
    /// - Emulator builds: /tmp/.kobo/KoboRoot.tgz
    /// - Production builds: {INTERNAL_CARD_ROOT}/.kobo/KoboRoot.tgz
    ///
    /// # Arguments
    ///
    /// * `data` - The KoboRoot.tgz file contents to deploy
    ///
    /// # Returns
    ///
    /// The deployment path where KoboRoot.tgz was written.
    ///
    /// # Errors
    ///
    /// * `OtaError::Io` - Failed to create directories or write deployment file
    fn deploy_bytes(&self, data: &[u8]) -> Result<PathBuf, OtaError> {
        #[cfg(test)]
        let deploy_path = {
            std::env::temp_dir()
                .join("test-kobo-deployment")
                .join("KoboRoot.tgz")
        };

        #[cfg(all(feature = "emulator", not(test)))]
        let deploy_path = PathBuf::from("/tmp/.kobo/KoboRoot.tgz");

        #[cfg(all(not(feature = "emulator"), not(test)))]
        let deploy_path = PathBuf::from(format!("{}/.kobo/KoboRoot.tgz", INTERNAL_CARD_ROOT));

        tracing::debug!(path = ?deploy_path, "Deploy destination");

        #[cfg(any(test, feature = "emulator"))]
        {
            if let Some(parent) = deploy_path.parent() {
                tracing::debug!(directory = ?parent, "Creating parent directory");
                std::fs::create_dir_all(parent)?;
            }
        }

        tracing::debug!(bytes = data.len(), path = ?deploy_path, "Writing file");
        let mut file = File::create(&deploy_path)?;
        file.write_all(data)?;

        tracing::debug!(path = ?deploy_path, "Deployment complete");
        tracing::info!(path = ?deploy_path, "Update deployed successfully");

        Ok(deploy_path)
    }

    /// Extracts KoboRoot.tgz from the artifact and deploys it for installation.
    ///
    /// Opens the downloaded ZIP archive, locates the `KoboRoot.tgz` file,
    /// extracts it, and writes it to `/mnt/onboard/.kobo/KoboRoot.tgz`
    /// where the Kobo device will automatically install it on next reboot.
    ///
    /// # Arguments
    ///
    /// * `zip_path` - Path to the downloaded artifact ZIP file
    ///
    /// # Returns
    ///
    /// The deployment path where KoboRoot.tgz was written.
    ///
    /// # Errors
    ///
    /// * `OtaError::ZipError` - Failed to open or read ZIP archive
    /// * `OtaError::DeploymentError` - KoboRoot.tgz not found in archive
    /// * `OtaError::Io` - Failed to write deployment file
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub fn extract_and_deploy(&self, zip_path: PathBuf) -> Result<PathBuf, OtaError> {
        tracing::info!(path = ?zip_path, "Extracting and deploying update");
        tracing::debug!(path = ?zip_path, "Starting extraction");

        let file = File::open(&zip_path)?;
        let mut archive = ZipArchive::new(file)?;

        tracing::debug!(file_count = archive.len(), "Opened ZIP archive");

        let mut kobo_root_data = Vec::new();
        let mut found = false;

        #[cfg(not(feature = "test"))]
        let kobo_root_name = "KoboRoot.tgz";
        #[cfg(feature = "test")]
        let kobo_root_name = "KoboRoot-test.tgz";

        tracing::debug!(target_file = kobo_root_name, "Looking for file");

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let entry_name = entry.name().to_string();

            tracing::debug!(index = i, name = %entry_name, "Checking entry");

            if entry_name.eq(kobo_root_name) {
                tracing::debug!(name = %entry_name, "Found target file");
                entry.read_to_end(&mut kobo_root_data)?;
                found = true;
                break;
            }
        }

        if !found {
            tracing::error!(
                target_file = kobo_root_name,
                "Target file not found in artifact"
            );
            return Err(OtaError::DeploymentError(format!(
                "{} not found in artifact",
                kobo_root_name
            )));
        }

        tracing::debug!(
            bytes = kobo_root_data.len(),
            file = kobo_root_name,
            "Extracted file"
        );

        self.deploy_bytes(&kobo_root_data)
    }

    /// Queries the GitHub API for the repository's default branch name.
    fn fetch_default_branch(&self) -> Result<String, OtaError> {
        let repo_url = "https://api.github.com/repos/ogkevin/cadmus";
        tracing::debug!(url = %repo_url, "Fetching repository metadata");

        let repo: Repository = self
            .github
            .get(repo_url)
            .send()?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(status = ?e.status(), error = %e, "Repository metadata fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(default_branch = %repo.default_branch, "Resolved default branch");
        Ok(repo.default_branch)
    }

    /// Fetches artifacts for a workflow run and finds one matching the given prefix.
    fn find_artifact_in_run(&self, run_id: u64, name_prefix: &str) -> Result<Artifact, OtaError> {
        let artifacts_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/runs/{}/artifacts",
            run_id
        );
        tracing::debug!(url = %artifacts_url, "Fetching artifacts");

        let artifacts: ArtifactsResponse = self
            .github
            .get(&artifacts_url)
            .send()?
            .error_for_status()
            .map_err(|e| {
                tracing::error!(run_id, status = ?e.status(), error = %e, "Artifacts fetch failed");
                api_error(e)
            })?
            .json()?;

        tracing::debug!(count = artifacts.artifacts.len(), "Found artifacts");

        #[cfg(feature = "otel")]
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
            .find(|a| a.name.starts_with(name_prefix))
            .ok_or_else(|| {
                tracing::error!(run_id, pattern = %name_prefix, "No matching artifact found");
                OtaError::ArtifactsNotFound(ArtifactSource::WorkflowRun(name_prefix.to_owned()))
            })
    }

    /// Downloads a file from a URL with chunked transfer and progress reporting.
    ///
    /// Uses HTTP Range headers to request the file in chunks for resilience
    /// against network interruptions.
    ///
    /// # Arguments
    ///
    /// * `url` - The complete download URL
    /// * `total_size` - Total file size in bytes
    /// * `download_path` - Path where the file should be saved
    /// * `progress_callback` - Function called with progress updates
    /// * `use_auth` - Whether to include Authorization header in requests
    ///
    /// # Returns
    ///
    /// Success if the file is written to disk, error otherwise.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, progress_callback)))]
    fn download_by_url_to_path<F>(
        &self,
        url: &str,
        total_size: u64,
        download_path: &PathBuf,
        progress_callback: &mut F,
        use_auth: bool,
    ) -> Result<(), OtaError>
    where
        F: FnMut(OtaProgress),
    {
        progress_callback(OtaProgress::DownloadingArtifact {
            downloaded: 0,
            total: total_size,
        });

        tracing::debug!(url = %url, "Downloading file");
        tracing::debug!(path = ?download_path, "Download destination");

        let mut file = File::create(download_path)?;

        let mut downloaded = 0u64;
        let mut chunk_size = INITIAL_CHUNK_SIZE;

        tracing::debug!(
            initial_chunk_size = INITIAL_CHUNK_SIZE,
            "Starting chunked download"
        );

        while downloaded < total_size {
            let chunk_start = downloaded;
            let chunk_end = std::cmp::min(downloaded + chunk_size as u64 - 1, total_size - 1);

            tracing::debug!(
                chunk_start,
                chunk_end,
                chunk_size,
                total_size,
                "Downloading chunk"
            );

            let start = std::time::Instant::now();
            let chunk_data =
                self.download_chunk_with_retries(url, chunk_start, chunk_end, use_auth)?;
            let elapsed_secs = start.elapsed().as_secs_f64();

            file.write_all(&chunk_data)?;
            downloaded += chunk_data.len() as u64;

            if elapsed_secs > 0.0 {
                let throughput = chunk_data.len() as f64 / elapsed_secs;
                chunk_size = ((throughput * TARGET_CHUNK_SECS) as usize)
                    .clamp(MIN_CHUNK_SIZE, MAX_CHUNK_SIZE);
                tracing::debug!(
                    elapsed_secs,
                    throughput_bytes_per_sec = throughput as u64,
                    next_chunk_size = chunk_size,
                    "Adjusted chunk size"
                );
            }

            progress_callback(OtaProgress::DownloadingArtifact {
                downloaded,
                total: total_size,
            });

            tracing::debug!(
                downloaded,
                total_size,
                progress_percent = (downloaded as f64 / total_size as f64) * 100.0,
                "Download progress"
            );
        }

        tracing::debug!(bytes = downloaded, "Download complete");
        tracing::debug!(path = ?download_path, "Saved file");

        Ok(())
    }

    /// Downloads an artifact ZIP to the specified path with chunked transfer and progress reporting.
    ///
    /// GitHub authentication is required for this operation.
    fn download_artifact_to_path<F>(
        &self,
        artifact: &Artifact,
        download_path: &PathBuf,
        progress_callback: &mut F,
    ) -> Result<(), OtaError>
    where
        F: FnMut(OtaProgress),
    {
        let download_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/artifacts/{}/zip",
            artifact.id
        );

        self.download_by_url_to_path(
            &download_url,
            artifact.size_in_bytes,
            download_path,
            progress_callback,
            true,
        )
    }

    /// Downloads a specific byte range of a file with automatic retry logic.
    ///
    /// Uses HTTP Range headers to request a specific chunk of the artifact.
    /// Implements exponential backoff retry strategy for failed downloads.
    ///
    /// # Arguments
    ///
    /// * `url` - The download URL
    /// * `start` - Starting byte offset (inclusive)
    /// * `end` - Ending byte offset (inclusive)
    ///
    /// # Returns
    ///
    /// The downloaded chunk data as a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if all retry attempts fail.
    fn download_chunk_with_retries(
        &self,
        url: &str,
        start: u64,
        end: u64,
        use_auth: bool,
    ) -> Result<Vec<u8>, OtaError> {
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.download_chunk(url, start, end, use_auth) {
                Ok(data) => {
                    if attempt > 1 {
                        tracing::debug!(
                            attempt,
                            max_retries = MAX_RETRIES,
                            "Chunk download succeeded after retry"
                        );
                    }
                    return Ok(data);
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        max_retries = MAX_RETRIES,
                        error = %e,
                        "Chunk download failed"
                    );
                    last_error = Some(e);

                    if attempt < MAX_RETRIES {
                        let backoff_ms = 1000 * (2u64.pow(attempt as u32 - 1));
                        tracing::debug!(backoff_ms, "Retrying after backoff");
                        std::thread::sleep(Duration::from_millis(backoff_ms));
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            OtaError::Api("Failed to download chunk after all retries".to_string())
        }))
    }

    /// Downloads a specific byte range from a URL using HTTP Range header.
    ///
    /// # Arguments
    ///
    /// * `url` - The download URL
    /// * `start` - Starting byte offset (inclusive)
    /// * `end` - Ending byte offset (inclusive)
    /// * `use_auth` - Whether to include Authorization header
    ///
    /// # Returns
    ///
    /// The downloaded chunk data as a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if the download fails or times out.
    fn download_chunk(
        &self,
        url: &str,
        start: u64,
        end: u64,
        use_auth: bool,
    ) -> Result<Vec<u8>, OtaError> {
        let range_header = format!("bytes={}-{}", start, end);

        let builder = if use_auth {
            self.github.get(url)
        } else {
            self.github.get_unauthenticated(url)
        };

        let bytes = builder
            .header("Range", range_header)
            .send()?
            .error_for_status()
            .map_err(api_error)?
            .bytes()?;

        Ok(bytes.to_vec())
    }

    /// Downloads a release asset to the specified path with chunked transfer and progress reporting.
    ///
    /// GitHub authentication is not required for this operation as release
    /// assets are downloaded from public URLs.
    #[inline]
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, progress_callback)))]
    fn download_release_asset<F>(
        &self,
        asset: &ReleaseAsset,
        download_path: &PathBuf,
        progress_callback: &mut F,
    ) -> Result<(), OtaError>
    where
        F: FnMut(OtaProgress),
    {
        self.download_by_url_to_path(
            &asset.browser_download_url,
            asset.size,
            download_path,
            progress_callback,
            false,
        )
    }
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
fn verify_scopes(github: &crate::github::GithubClient) -> Result<(), OtaError> {
    github.verify_token_scopes().map_err(|e| match e {
        crate::github::VerifyScopesError::Request(e) => api_error(e),
        crate::github::VerifyScopesError::InsufficientScopes(e) => OtaError::InsufficientScopes(e),
    })
}

/// Maps a failed `reqwest` response to the appropriate `OtaError`.
///
/// A 401 Unauthorized response means the saved token has been revoked or
/// expired — the caller should re-authenticate via device flow rather than
/// treating this as a generic API error.
fn api_error(e: reqwest::Error) -> OtaError {
    if e.status() == Some(reqwest::StatusCode::UNAUTHORIZED) {
        tracing::warn!("GitHub API returned 401 — token invalid or revoked");
        OtaError::Unauthorized
    } else {
        OtaError::Api(e.to_string())
    }
}

fn check_disk_space(path: &str) -> Result<(), OtaError> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs(path)?;
    let available_mb = (stat.blocks_available() as u64 * stat.block_size() as u64) / (1024 * 1024);
    tracing::debug!(path, available_mb, "Checking disk space");

    if available_mb < 100 {
        tracing::error!(
            path,
            available_mb,
            required_mb = 100,
            "Insufficient disk space"
        );
        return Err(OtaError::InsufficientSpace(available_mb));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GithubClient;
    use secrecy::SecretString;

    fn make_client() -> OtaClient {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let github =
            GithubClient::new(Some(SecretString::from("test_token"))).expect("client build");
        OtaClient::new(github)
    }

    #[test]
    fn test_extract_and_deploy_success() {
        let client = make_client();
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/test_artifact.zip");

        let result = client.extract_and_deploy(fixture_path);

        assert!(
            result.is_ok(),
            "Deployment should succeed: {:?}",
            result.err()
        );

        let deploy_path = result.unwrap();
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

        std::fs::remove_file(&deploy_path).ok();
    }

    #[test]
    fn test_extract_and_deploy_missing_koboroot() {
        let client = make_client();
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/empty_artifact.zip");

        let result = client.extract_and_deploy(fixture_path);
        assert!(result.is_err(), "Should fail when KoboRoot.tgz is missing");

        if let Err(OtaError::DeploymentError(msg)) = result {
            assert!(
                msg.contains("not found in artifact"),
                "Error should mention missing file"
            );
        } else {
            panic!("Expected DeploymentError");
        }
    }

    #[test]
    fn test_check_disk_space_sufficient() {
        use tempfile::TempDir;
        let temp_dir = TempDir::new().unwrap();
        let result = check_disk_space(temp_dir.path().to_str().unwrap());
        assert!(
            result.is_ok(),
            "Should have sufficient disk space in temp directory"
        );
    }

    fn external_test_enabled() -> bool {
        std::env::var("CADMUS_TEST_OTA_EXTERNAL").is_ok() && std::env::var("GH_TOKEN").is_ok()
    }

    fn create_external_client() -> OtaClient {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let token = std::env::var("GH_TOKEN").expect("GH_TOKEN must be set");
        let github = GithubClient::new(Some(SecretString::from(token))).expect("client build");
        OtaClient::new(github)
    }

    #[test]
    #[ignore]
    fn test_external_download_default_branch_and_deploy() {
        if !external_test_enabled() {
            return;
        }

        let client = create_external_client();
        let mut last_progress = None;

        let download_result = client.download_default_branch_artifact(|progress| {
            last_progress = Some(format!("{:?}", progress));
        });

        assert!(
            download_result.is_ok(),
            "Default branch artifact download should succeed: {:?}",
            download_result.err()
        );

        let zip_path = download_result.unwrap();
        assert!(
            zip_path.exists(),
            "Downloaded ZIP should exist at {:?}",
            zip_path
        );
        assert!(
            zip_path.metadata().unwrap().len() > 0,
            "Downloaded ZIP should not be empty"
        );

        let deploy_result = client.extract_and_deploy(zip_path.clone());

        assert!(
            deploy_result.is_ok(),
            "Deployment should succeed: {:?}",
            deploy_result.err()
        );

        let deploy_path = deploy_result.unwrap();
        assert!(
            deploy_path.exists(),
            "Deployed file should exist at {:?}",
            deploy_path
        );

        std::fs::remove_file(&zip_path).ok();
        std::fs::remove_file(&deploy_path).ok();
    }

    #[test]
    #[ignore]
    fn test_external_download_stable_release_and_deploy() {
        if !external_test_enabled() {
            return;
        }

        let client = create_external_client();
        let download_result = client.download_stable_release_artifact(|_| {});

        assert!(
            download_result.is_ok(),
            "Stable release artifact download should succeed: {:?}",
            download_result.err()
        );

        let asset_path = download_result.unwrap();
        assert!(
            asset_path.exists(),
            "Downloaded asset should exist at {:?}",
            asset_path
        );
        assert!(
            asset_path.metadata().unwrap().len() > 0,
            "Downloaded asset should not be empty"
        );

        let deploy_result = client.deploy(asset_path.clone());

        assert!(
            deploy_result.is_ok(),
            "Deployment should succeed: {:?}",
            deploy_result.err()
        );

        let deploy_path = deploy_result.unwrap();
        assert!(
            deploy_path.exists(),
            "Deployed file should exist at {:?}",
            deploy_path
        );

        std::fs::remove_file(&asset_path).ok();
        std::fs::remove_file(&deploy_path).ok();
    }
}
