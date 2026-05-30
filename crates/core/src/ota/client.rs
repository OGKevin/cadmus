use crate::github::types::{
    Artifact, ArtifactsResponse, Release, ReleaseAsset, Repository, WorkflowRunsResponse,
};
use crate::github::{GithubClient, OtaProgress};
use crate::http::ChunkedDownloadError;
use crate::version::GitVersion;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

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
    ZipError(#[from] zip::result::ZipError),

    /// Deployment process failed after successful download
    #[error("Deployment error: {0}")]
    DeploymentError(String),

    /// Failed to parse version string
    #[error(transparent)]
    VersionParse(#[from] crate::version::VersionError),
}

impl From<ChunkedDownloadError> for OtaError {
    fn from(e: ChunkedDownloadError) -> Self {
        match e {
            ChunkedDownloadError::Request(r) if r.status().is_some() => api_error(r),
            ChunkedDownloadError::Request(r) => OtaError::Request(r),
            ChunkedDownloadError::Io(e) => OtaError::Io(e),
        }
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
    /// 4. Locates artifacts matching "cadmus-kobo-pr*" pattern
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
    /// The path to the downloaded ZIP file on success.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in the configured temp directory
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
        check_disk_space(&self.tmp_dir)?;
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

        #[cfg(feature = "tracing")]
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

        let artifact_name_pattern = cfg_select! {
            feature = "test" => { format!("cadmus-kobo-test-pr{}", pr_number) }
            _ => { format!("cadmus-kobo-pr{}", pr_number) }
        };

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

        let download_path = self.tmp_dir.join(format!("cadmus-ota-{}.zip", pr_number));

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
    /// The path to the downloaded ZIP file on success.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in the configured temp directory
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
        check_disk_space(&self.tmp_dir)?;
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

        let artifact_name_prefix = cfg_select! {
            feature = "test" => { format!("cadmus-kobo-test-{}", short_sha) }
            _ => { format!("cadmus-kobo-{}", short_sha) }
        };

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

        let download_path = self.tmp_dir.join(format!("cadmus-ota-{}.zip", short_sha));

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
    /// The path to the downloaded KoboRoot.tgz file on success.
    ///
    /// # Errors
    ///
    /// * `OtaError::InsufficientSpace` - Less than 100MB available in the configured temp directory
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::ArtifactsNotFound` - KoboRoot.tgz not found in latest release
    /// * `OtaError::Io` - Failed to write downloaded file to disk
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn download_stable_release_artifact<F>(
        &self,
        mut progress_callback: F,
    ) -> Result<PathBuf, OtaError>
    where
        F: FnMut(OtaProgress),
    {
        check_disk_space(&self.tmp_dir)?;

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

        self.download_release_asset(asset, &download_path, &mut progress_callback)?;

        progress_callback(OtaProgress::Complete {
            path: download_path.clone(),
        });

        tracing::info!("Stable release download completed");
        Ok(download_path)
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
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # rustls::crypto::ring::default_provider().install_default().ok();
    /// # let github = GithubClient::new(None)?;
    /// # let client = OtaClient::new(github, std::path::PathBuf::from("/tmp"));
    /// let version = client.fetch_latest_release_version()?;
    /// println!("Latest version: {}", version);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn fetch_latest_release_version(&self) -> Result<GitVersion, OtaError> {
        let releases_url = "https://api.github.com/repos/ogkevin/cadmus/releases/latest";
        tracing::debug!(url = %releases_url, "Fetching latest release version");

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
    /// The path where the file was deployed, or an error if deployment fails.
    ///
    /// # Errors
    ///
    /// * `OtaError::Io` - Failed to read or write files
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn deploy(&self, kobo_root_path: PathBuf) -> Result<PathBuf, OtaError> {
        tracing::info!(path = ?kobo_root_path, "Deploying KoboRoot.tgz");

        let deploy_path = self.deploy_path();
        self.ensure_deploy_dir(&deploy_path)?;

        let mut src = File::open(&kobo_root_path)?;
        let mut dst = File::create(&deploy_path)?;
        let bytes_copied = std::io::copy(&mut src, &mut dst)?;

        tracing::debug!(
            bytes = bytes_copied,
            src = ?kobo_root_path,
            dst = ?deploy_path,
            "Streamed KoboRoot.tgz to deploy path"
        );

        if kobo_root_path != deploy_path {
            if let Err(e) = std::fs::remove_file(&kobo_root_path) {
                tracing::error!(path = ?kobo_root_path, error = %e, "Failed to remove source file");
            }
        }

        tracing::info!(path = ?deploy_path, "Update deployed successfully");
        Ok(deploy_path)
    }

    /// Returns the platform-specific deployment path for KoboRoot.tgz.
    ///
    /// | Build context        | Path                                              |
    /// |----------------------|---------------------------------------------------|
    /// | During `cargo test`  | `<temp_dir>/test-kobo-deployment/KoboRoot.tgz`    |
    /// | Emulator builds      | `/tmp/.kobo/KoboRoot.tgz`                         |
    /// | Kobo builds          | `{INTERNAL_CARD_ROOT}/.kobo/KoboRoot.tgz`         |
    fn deploy_path(&self) -> PathBuf {
        let path = cfg_select! {
            test => {
                std::env::temp_dir()
                    .join("test-kobo-deployment")
                    .join("KoboRoot.tgz")
            }
            feature = "emulator" => { PathBuf::from("/tmp/.kobo/KoboRoot.tgz") }
            _ => { PathBuf::from(format!("{}/.kobo/KoboRoot.tgz", INTERNAL_CARD_ROOT)) }
        };

        tracing::debug!(path = ?path, "Deploy destination");
        path
    }

    fn ensure_deploy_dir(&self, deploy_path: &Path) -> Result<(), OtaError> {
        #[cfg(any(test, feature = "emulator"))]
        {
            if let Some(parent) = deploy_path.parent() {
                tracing::debug!(directory = ?parent, "Creating parent directory");
                std::fs::create_dir_all(parent)?;
            }
        }

        let _ = deploy_path;
        Ok(())
    }

    fn deploy_bytes(&self, data: &[u8]) -> Result<PathBuf, OtaError> {
        let deploy_path = self.deploy_path();
        self.ensure_deploy_dir(&deploy_path)?;

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
    /// On success, the source artifact ZIP is deleted as a best-effort cleanup step.
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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub fn extract_and_deploy(&self, zip_path: PathBuf) -> Result<PathBuf, OtaError> {
        tracing::info!(path = ?zip_path, "Extracting and deploying update");
        tracing::debug!(path = ?zip_path, "Starting extraction");

        let file = File::open(&zip_path)?;
        let mut archive = ZipArchive::new(file)?;

        tracing::debug!(file_count = archive.len(), "Opened ZIP archive");

        let mut kobo_root_data = Vec::new();
        let mut found = false;

        let kobo_root_name = cfg_select! {
            feature = "test" => { "KoboRoot-test.tgz" }
            _ => { "KoboRoot.tgz" }
        };

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

        let deploy_path = self.deploy_bytes(&kobo_root_data)?;
        if let Err(e) = std::fs::remove_file(&zip_path) {
            tracing::error!(path = ?zip_path, error = %e, "Failed to remove source file");
        }

        Ok(deploy_path)
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
            .find(|a| a.name.starts_with(name_prefix))
            .ok_or_else(|| {
                tracing::error!(run_id, pattern = %name_prefix, "No matching artifact found");
                OtaError::ArtifactsNotFound(ArtifactSource::WorkflowRun(name_prefix.to_owned()))
            })
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

        self.github.download(
            &download_url,
            artifact.size_in_bytes,
            download_path,
            |url| self.github.get(url),
            &mut |downloaded, total| {
                progress_callback(OtaProgress::DownloadingArtifact { downloaded, total })
            },
        )?;
        Ok(())
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
    fn download_release_asset<F>(
        &self,
        asset: &ReleaseAsset,
        download_path: &PathBuf,
        progress_callback: &mut F,
    ) -> Result<(), OtaError>
    where
        F: FnMut(OtaProgress),
    {
        self.github.download(
            &asset.browser_download_url,
            asset.size,
            download_path,
            |url| self.github.get_unauthenticated(url),
            &mut |downloaded, total| {
                progress_callback(OtaProgress::DownloadingArtifact { downloaded, total })
            },
        )?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GithubClient;
    use secrecy::SecretString;

    fn make_client(tmp_dir: PathBuf) -> OtaClient {
        crate::crypto::init_crypto_provider();
        let github =
            GithubClient::new(Some(SecretString::from("test_token"))).expect("client build");
        OtaClient::new(github, tmp_dir)
    }

    #[test]
    fn test_extract_and_deploy_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = make_client(temp_dir.path().to_path_buf());
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/test_artifact.zip");
        let artifact_path = temp_dir.path().join("test_artifact.zip");
        std::fs::copy(&fixture_path, &artifact_path).unwrap();

        let result = client.extract_and_deploy(artifact_path.clone());

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
        assert!(
            !artifact_path.exists(),
            "Downloaded artifact should be removed after successful deployment"
        );
    }

    #[test]
    fn test_extract_and_deploy_missing_koboroot() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = make_client(temp_dir.path().to_path_buf());
        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/ota/tests/fixtures/empty_artifact.zip");
        let artifact_path = temp_dir.path().join("empty_artifact.zip");
        std::fs::copy(&fixture_path, &artifact_path).unwrap();

        let result = client.extract_and_deploy(artifact_path.clone());
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

    #[test]
    #[ignore]
    fn test_external_download_default_branch_and_deploy() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = create_external_client(temp_dir.path().to_path_buf());
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

        std::fs::remove_file(&deploy_path).ok();
    }

    #[test]
    #[ignore]
    fn test_external_download_stable_release_and_deploy() {
        let temp_dir = tempfile::tempdir().unwrap();
        let client = create_external_client(temp_dir.path().to_path_buf());
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

        std::fs::remove_file(&deploy_path).ok();
    }
}
