use reqwest::blocking::Client;
use rustls::RootCertStore;
use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use zip::ZipArchive;

/// Size of each download chunk in bytes (10 MB)
const CHUNK_SIZE: usize = 10 * 1024 * 1024;

/// Timeout for each chunk download attempt in seconds
const CHUNK_TIMEOUT_SECS: u64 = 30;

/// Maximum number of retry attempts for failed chunks
const MAX_RETRIES: usize = 3;

/// HTTP client for downloading GitHub Actions artifacts from pull requests.
///
/// This client handles the complete OTA update workflow:
/// - Fetching PR information from GitHub API
/// - Finding associated workflow runs
/// - Downloading build artifacts
/// - Extracting and deploying updates
///
/// Requires a GitHub personal access token with appropriate permissions.
pub struct OtaClient {
    client: Client,
    token: String,
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

    /// No build artifacts matching the expected pattern were found for the PR
    #[error("No build artifacts found for PR #{0}")]
    NoArtifacts(u32),

    /// GitHub token was not provided in configuration
    #[error("GitHub token not configured")]
    NoToken,

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

/// Progress states during an OTA download operation.
///
/// Used with progress callbacks to track download status.
#[derive(Debug, Clone)]
pub enum OtaProgress {
    /// Verifying the pull request exists and fetching its metadata
    CheckingPr,
    /// Searching for the associated GitHub Actions workflow run
    FindingWorkflow,
    /// Actively downloading the artifact with optional progress tracking
    DownloadingArtifact { downloaded: u64, total: u64 },
    /// Download completed successfully, artifact saved to disk
    Complete { path: PathBuf },
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    head: PrHead,
}

#[derive(Debug, Deserialize)]
struct PrHead {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowRunsResponse {
    workflow_runs: Vec<WorkflowRun>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    name: String,
    id: u64,
}

#[derive(Debug, Deserialize)]
struct ArtifactsResponse {
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    name: String,
    id: u64,
    size_in_bytes: u64,
}

impl OtaClient {
    /// Creates a new OTA client with GitHub authentication.
    ///
    /// Initializes an HTTP client with TLS configured using webpki-roots
    /// certificates for secure communication with GitHub's API.
    ///
    /// # Arguments
    ///
    /// * `github_token` - Personal access token for GitHub API authentication
    ///
    /// # Errors
    ///
    /// Returns `OtaError::TlsConfig` if the HTTP client fails to initialize
    /// with the provided TLS configuration.
    pub fn new(github_token: String) -> Result<Self, OtaError> {
        println!("[OTA] Initializing OTA client with webpki-roots certificates");

        let root_store = create_webpki_root_store();
        println!(
            "[OTA] Created root certificate store with {} certificates",
            root_store.len()
        );

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        println!("[OTA] Built TLS configuration");

        let client = Client::builder()
            .use_preconfigured_tls(tls_config)
            .user_agent("cadmus-ota")
            .timeout(Duration::from_secs(CHUNK_TIMEOUT_SECS))
            .build()
            .map_err(|e| OtaError::TlsConfig(format!("Failed to build HTTP client: {}", e)))?;

        println!("[OTA] Successfully initialized OTA client with reqwest");

        Ok(Self {
            client,
            token: github_token,
        })
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
    /// * `OtaError::PrNotFound` - PR number doesn't exist in repository
    /// * `OtaError::NoArtifacts` - No matching build artifacts found for the PR
    /// * `OtaError::Api` - GitHub API request failed
    /// * `OtaError::Request` - Network communication failed
    /// * `OtaError::Io` - Failed to write downloaded file to disk
    pub fn download_pr_artifact<F>(
        &self,
        pr_number: u32,
        progress_callback: F,
    ) -> Result<PathBuf, OtaError>
    where
        F: Fn(OtaProgress),
    {
        check_disk_space()?;

        progress_callback(OtaProgress::CheckingPr);
        println!("[OTA] Checking PR #{}", pr_number);

        let pr_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/pulls/{}",
            pr_number
        );

        let pr: PullRequest = self
            .client
            .get(&pr_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()?
            .error_for_status()
            .map_err(|_| OtaError::PrNotFound(pr_number))?
            .json()?;

        let head_sha = pr.head.sha;
        println!("[OTA] PR #{} head SHA: {}", pr_number, head_sha);

        progress_callback(OtaProgress::FindingWorkflow);
        println!("[OTA] Finding workflow runs for SHA: {}", head_sha);

        let runs_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/runs?head_sha={}&event=pull_request",
            head_sha
        );

        let runs: WorkflowRunsResponse = self
            .client
            .get(&runs_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()?
            .error_for_status()
            .map_err(|e| OtaError::Api(format!("Failed to fetch workflow runs: {}", e)))?
            .json()?;

        println!("[OTA] Found {} workflow runs", runs.workflow_runs.len());

        let run = runs
            .workflow_runs
            .iter()
            .find(|r| r.name == "Cargo")
            .ok_or(OtaError::NoArtifacts(pr_number))?;

        println!("[OTA] Found Cargo workflow run with ID: {}", run.id);

        let artifacts_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/runs/{}/artifacts",
            run.id
        );

        let artifacts: ArtifactsResponse = self
            .client
            .get(&artifacts_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()?
            .error_for_status()
            .map_err(|e| OtaError::Api(format!("Failed to fetch artifacts: {}", e)))?
            .json()?;

        println!("[OTA] Found {} artifacts", artifacts.artifacts.len());

        let artifact = artifacts
            .artifacts
            .iter()
            .find(|a| a.name.starts_with("cadmus-kobo-pr"))
            .ok_or(OtaError::NoArtifacts(pr_number))?;

        println!(
            "[OTA] Found artifact: {} (ID: {}, size: {} bytes)",
            artifact.name, artifact.id, artifact.size_in_bytes
        );

        progress_callback(OtaProgress::DownloadingArtifact {
            downloaded: 0,
            total: artifact.size_in_bytes,
        });

        let download_url = format!(
            "https://api.github.com/repos/ogkevin/cadmus/actions/artifacts/{}/zip",
            artifact.id
        );

        println!("[OTA] Downloading artifact from: {}", download_url);

        let download_path = PathBuf::from(format!("/tmp/cadmus-ota-{}.zip", pr_number));
        let mut file = File::create(&download_path)?;

        let mut downloaded = 0u64;
        let total_size = artifact.size_in_bytes;

        println!(
            "[OTA] Starting chunked download ({} MB chunks)",
            CHUNK_SIZE / (1024 * 1024)
        );

        while downloaded < total_size {
            let chunk_start = downloaded;
            let chunk_end = std::cmp::min(downloaded + CHUNK_SIZE as u64 - 1, total_size - 1);

            println!(
                "[OTA] Downloading chunk: bytes {}-{} of {}",
                chunk_start, chunk_end, total_size
            );

            let chunk_data =
                self.download_chunk_with_retries(&download_url, chunk_start, chunk_end)?;

            file.write_all(&chunk_data)?;
            downloaded += chunk_data.len() as u64;

            progress_callback(OtaProgress::DownloadingArtifact {
                downloaded,
                total: total_size,
            });

            println!(
                "[OTA] Progress: {}/{} bytes ({:.1}%)",
                downloaded,
                total_size,
                (downloaded as f64 / total_size as f64) * 100.0
            );
        }

        println!("[OTA] Download complete: {} bytes", downloaded);
        println!("[OTA] Saved artifact to: {:?}", download_path);

        progress_callback(OtaProgress::Complete {
            path: download_path.clone(),
        });

        Ok(download_path)
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
    pub fn extract_and_deploy(&self, zip_path: PathBuf) -> Result<PathBuf, OtaError> {
        println!("[OTA] Starting extraction of artifact: {:?}", zip_path);

        let file = File::open(&zip_path)?;
        let mut archive = ZipArchive::new(file)?;

        println!("[OTA] Opened ZIP archive with {} files", archive.len());

        let mut kobo_root_data = Vec::new();
        let mut found = false;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let entry_name = entry.name().to_string();

            println!("[OTA] Checking entry: {}", entry_name);

            if entry_name.eq("KoboRoot.tgz") {
                println!("[OTA] Found KoboRoot.tgz at: {}", entry_name);
                entry.read_to_end(&mut kobo_root_data)?;
                found = true;
                break;
            }
        }

        if !found {
            return Err(OtaError::DeploymentError(
                "KoboRoot.tgz not found in artifact".to_string(),
            ));
        }

        println!(
            "[OTA] Extracted {} bytes from KoboRoot.tgz",
            kobo_root_data.len()
        );

        let deploy_path = PathBuf::from("/mnt/onboard/.kobo/KoboRoot.tgz");
        let mut file = File::create(&deploy_path)?;
        file.write_all(&kobo_root_data)?;

        println!("[OTA] Deployment complete: {:?}", deploy_path);

        Ok(deploy_path)
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
    ) -> Result<Vec<u8>, OtaError> {
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.download_chunk(url, start, end) {
                Ok(data) => {
                    if attempt > 1 {
                        println!(
                            "[OTA] Chunk download succeeded on attempt {}/{}",
                            attempt, MAX_RETRIES
                        );
                    }
                    return Ok(data);
                }
                Err(e) => {
                    println!(
                        "[OTA] Chunk download failed (attempt {}/{}): {}",
                        attempt, MAX_RETRIES, e
                    );
                    last_error = Some(e);

                    if attempt < MAX_RETRIES {
                        let backoff_ms = 1000 * (2u64.pow(attempt as u32 - 1));
                        println!("[OTA] Retrying after {} ms...", backoff_ms);
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
    ///
    /// # Returns
    ///
    /// The downloaded chunk data as a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if the download fails or times out.
    fn download_chunk(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>, OtaError> {
        let range_header = format!("bytes={}-{}", start, end);

        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Range", range_header)
            .send()?
            .error_for_status()
            .map_err(|e| OtaError::Api(format!("Failed to download chunk: {}", e)))?;

        let bytes = response.bytes()?;
        Ok(bytes.to_vec())
    }
}

/// Verifies sufficient disk space is available in /tmp for download.
///
/// Requires at least 100MB of free space for artifact download and extraction.
///
/// # Errors
///
/// Returns `OtaError::InsufficientSpace` if less than 100MB is available.
fn check_disk_space() -> Result<(), OtaError> {
    use nix::sys::statvfs::statvfs;

    let stat = statvfs("/tmp")?;
    let available_mb = (stat.blocks_available() * stat.block_size()) / (1024 * 1024);
    println!("[OTA] Available disk space in /tmp: {} MB", available_mb);

    if available_mb < 100 {
        return Err(OtaError::InsufficientSpace(available_mb as u64));
    }
    Ok(())
}

/// Creates a root certificate store with Mozilla's trusted CA certificates.
///
/// Uses the webpki-roots crate which embeds Mozilla's CA certificate bundle
/// for verifying HTTPS connections to GitHub's API.
fn create_webpki_root_store() -> RootCertStore {
    println!("[OTA] Loading Mozilla root certificates from webpki-roots");
    let mut root_store = RootCertStore::empty();

    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    println!(
        "[OTA] Loaded {} root certificates from webpki-roots",
        root_store.len()
    );
    root_store
}
