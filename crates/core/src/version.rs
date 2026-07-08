//! Version comparison utility for git describe format version strings.
//!
//! Supports comparing versions like:
//! - `v0.9.46` (tagged releases)
//! - `v0.9.46-5-gabc123` (development builds with commits ahead)
//! - `v0.9.46-5-gabc123-dirty` (dirty working tree)
//!
//! When versions contain different git hashes, GitHub API is used to check
//! ancestry relationships. The API client is created internally with no
//! authentication for public repository access.

use crate::github::GithubClient;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Visitor};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};

include!(concat!(env!("OUT_DIR"), "/cargo_features.rs"));

/// Result of comparing two versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionComparison {
    /// The local version is newer than the remote.
    Newer,
    /// The local version is older than the remote.
    Older,
    /// Both versions are equal.
    Equal,
    /// Cannot determine order (divergent branches).
    Incomparable,
}

/// Errors that can occur during version parsing or comparison.
#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    /// Invalid version format.
    #[error("invalid version format: {0}")]
    InvalidFormat(String),
    /// GitHub API error.
    #[error("GitHub API error: {0}")]
    GitHubApi(String),
    /// Inconsistent version data (e.g., same hash but different commit counts).
    #[error("inconsistent version data: {0}")]
    InconsistentData(String),
}

/// Response from GitHub's compare API.
#[derive(Debug, Deserialize)]
struct CompareResponse {
    status: String,
}

/// A parsed git describe version string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitVersion {
    major: u64,
    minor: u64,
    patch: u64,
    commits_ahead: u64,
    hash: Option<String>,
    dirty: bool,
}

/// Ordering is based on semver components (major, minor, patch) followed by
/// the number of commits ahead of the tag. This is a purely local comparison
/// that does not require network access.
///
/// When two versions share the same semver and commit count but have different
/// git hashes (divergent branches), this ordering treats them as equal. Use
/// [`GitVersion::compare`] instead if you need GitHub-based ancestry checks to
/// distinguish divergent development builds.
///
/// # Dirty-flag ordering
///
/// A dirty build (`v0.10.0-dirty`) is treated as strictly newer than its clean
/// counterpart (`v0.10.0`). This means that switching from a dirty dev build
/// back to the clean release of the same version will be detected as a
/// **downgrade** by the version gate, triggering a backup restore. This is
/// intentional for production use but can be surprising during local
/// development.
///
/// # Examples
///
/// ```
/// use cadmus_core::version::GitVersion;
///
/// let v1: GitVersion = "v0.9.46".parse().unwrap();
/// let v2: GitVersion = "v0.10.0".parse().unwrap();
/// let v3: GitVersion = "v0.10.0-5-gabc123".parse().unwrap();
/// let v3_dirty: GitVersion = "v0.10.0-5-gabc123-dirty".parse().unwrap();
///
/// assert!(v1 < v2);
/// assert!(v2 < v3);
/// assert!(v1 < v3);
/// assert!(v3 < v3_dirty);
/// assert_eq!(v2, v2);
/// ```
impl PartialOrd for GitVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GitVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then_with(|| self.minor.cmp(&other.minor))
            .then_with(|| self.patch.cmp(&other.patch))
            .then_with(|| self.commits_ahead.cmp(&other.commits_ahead))
            .then_with(|| self.dirty.cmp(&other.dirty))
    }
}

/// Complete compile-time version metadata for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    git: GitVersion,
    pull_request: Option<PullRequestInfo>,
    build: BuildAttributes,
    features: BuildFeatures,
    build_kind: BuildKind,
}

/// Build flavor captured from Cargo features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildKind {
    /// Test build.
    Test,
    /// Standard build.
    Standard,
}

/// Pull request metadata captured at build time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PullRequestInfo {
    value: &'static str,
}

/// Cargo features enabled for this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildFeatures {
    features: &'static [&'static str],
}

/// Compile-time build provenance attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildAttributes {
    /// User that produced this build.
    pub user: &'static str,
    /// Host that produced this build.
    pub host: &'static str,
    /// Timestamp when this build was produced.
    pub timestamp: BuildTimestamp,
}

/// Build timestamp parsed from Unix epoch seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTimestamp {
    raw: &'static str,
    datetime: Option<DateTime<Utc>>,
}

/// Formats compile-time version metadata for display in the About dialog.
///
/// The output contains one section per line:
/// - Git describe version, prefixed with `Test` for test builds
/// - Optional pull-request metadata
/// - Build provenance (timestamp, user, host)
///
/// Cargo features are omitted. Use [`Version::to_string_with_features`] or
/// [`Version::write_as_html_to`] when the feature list is needed.
///
/// # Examples
///
/// ```no_run
/// use cadmus_core::version::get_version;
///
/// let output = get_version().to_string();
/// assert!(output.lines().next().is_some_and(|line| {
///     line.starts_with('v') || line.starts_with("Test v")
/// }));
/// ```
///
/// A test build with pull-request metadata renders like:
///
/// ```text
/// Test v0.9.46-5-gabc123
/// PR #12 (abc1234)
/// Built 2009-02-13 23:31:30 UTC
/// By builder@host
/// ```
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.build_kind {
            BuildKind::Test => write!(f, "Test {}", self.git)?,
            BuildKind::Standard => write!(f, "{}", self.git)?,
        }

        if let Some(pull_request) = self.pull_request {
            write!(f, "\n{}", pull_request)?;
        }

        write!(f, "\n{}", self.build)
    }
}

impl Version {
    /// Returns the git describe version.
    pub fn git(&self) -> &GitVersion {
        &self.git
    }

    /// Returns the pull request metadata, if present.
    pub fn pull_request(&self) -> Option<PullRequestInfo> {
        self.pull_request
    }

    /// Returns the build provenance attributes.
    pub fn build(&self) -> &BuildAttributes {
        &self.build
    }

    /// Returns the build flavor.
    pub fn build_kind(&self) -> BuildKind {
        self.build_kind
    }

    /// Returns the Cargo features enabled for this build.
    pub fn features(&self) -> BuildFeatures {
        self.features
    }

    /// Formats version metadata including the enabled Cargo feature list.
    pub fn to_string_with_features(&self) -> String {
        if self.features.is_empty() {
            self.to_string()
        } else {
            format!("{self}\n{}", self.features)
        }
    }

    /// Appends build-metadata rows to a System Info HTML table buffer.
    pub fn write_as_html_to(&self, buf: &mut String) {
        let version_line = match self.build_kind {
            BuildKind::Test => format!("Cadmus Test {}", self.git),
            BuildKind::Standard => format!("Cadmus {}", self.git),
        };
        push_sysinfo_row(buf, "Version", &version_line);

        if let Some(pull_request) = self.pull_request {
            push_sysinfo_row(buf, "Pull request", pull_request.as_str());
        }

        push_sysinfo_row(buf, "Built", &self.build.timestamp.to_string());
        push_sysinfo_row(
            buf,
            "Built by",
            &format!("{}@{}", self.build.user, self.build.host),
        );

        if !self.features.is_empty() {
            buf.push_str("\t\t\t<tr>\n");
            buf.push_str("\t\t\t\t<td class=\"key\">Features</td>\n");
            buf.push_str("\t\t\t\t<td class=\"value\"><ul>\n");
            for feature in self.features.as_slice() {
                buf.push_str(&format!("\t\t\t\t\t<li>{feature}</li>\n"));
            }
            buf.push_str("\t\t\t\t</ul></td>\n");
            buf.push_str("\t\t\t</tr>\n");
        }

        buf.push_str("\t\t\t<tr class=\"sep\"></tr>\n");
    }
}

fn push_sysinfo_row(buf: &mut String, key: &str, value: &str) {
    buf.push_str("\t\t\t<tr>\n");
    buf.push_str(&format!("\t\t\t\t<td class=\"key\">{key}</td>\n"));
    buf.push_str(&format!("\t\t\t\t<td class=\"value\">{value}</td>\n"));
    buf.push_str("\t\t\t</tr>\n");
}

impl std::fmt::Display for PullRequestInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.value)
    }
}

impl PullRequestInfo {
    /// Returns the raw pull request metadata.
    pub fn as_str(&self) -> &'static str {
        self.value
    }
}

impl std::fmt::Display for BuildFeatures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::fl!(
            "build-features",
            features = self.features.join(", ")
        ))
    }
}

impl BuildFeatures {
    /// Returns whether any Cargo features were enabled for this build.
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Returns the enabled Cargo features.
    pub fn as_slice(&self) -> &'static [&'static str] {
        self.features
    }
}

impl std::fmt::Display for BuildAttributes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let timestamp = self.timestamp.to_string();
        f.write_str(&crate::fl!(
            "build-attributes",
            timestamp = timestamp.as_str(),
            user = self.user,
            host = self.host,
        ))
    }
}

impl std::fmt::Display for BuildTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(datetime) = self.datetime {
            return write!(f, "{}", datetime.format("%Y-%m-%d %H:%M:%S %Z"));
        }

        f.write_str(self.raw)
    }
}

impl BuildTimestamp {
    /// Parses a build timestamp from Unix epoch seconds.
    ///
    /// Invalid values are preserved so the About dialog can still show the raw
    /// build metadata instead of failing to render version information.
    pub fn parse(raw: &'static str) -> Self {
        Self {
            raw,
            datetime: raw
                .parse::<i64>()
                .ok()
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0)),
        }
    }
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)?;

        if self.commits_ahead > 0 {
            if let Some(ref hash) = self.hash {
                write!(f, "-{}-g{}", self.commits_ahead, hash)?;
            }
        }

        if self.dirty {
            write!(f, "-dirty")?;
        }

        Ok(())
    }
}

impl std::str::FromStr for GitVersion {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl GitVersion {
    /// Parses a version string in git describe format.
    ///
    /// Supported formats:
    /// - `v0.9.46` - Tagged release
    /// - `v0.9.46-5-gabc123` - Development build with commits ahead
    /// - `v0.9.46-5-gabc123-dirty` - Dirty working tree
    ///
    /// # Errors
    ///
    /// Returns `VersionError::InvalidFormat` if the version string cannot be parsed.
    ///
    /// # Examples
    ///
    /// ```
    /// use cadmus_core::version::GitVersion;
    ///
    /// let v = GitVersion::parse("v0.9.46").unwrap();
    /// assert_eq!(v.major(), 0);
    /// assert_eq!(v.minor(), 9);
    /// assert_eq!(v.patch(), 46);
    /// ```
    pub fn parse(version: &str) -> Result<Self, VersionError> {
        let original = version.to_string();
        let (version, dirty) = version
            .strip_suffix("-dirty")
            .map_or((version, false), |v| (v, true));

        let parts: Vec<&str> = version.split('-').collect();

        if parts.is_empty() {
            return Err(VersionError::InvalidFormat(original));
        }

        let semver = parts[0];
        let (major, minor, patch) = parse_semver(semver)?;

        let (commits_ahead, hash) = if parts.len() == 3 {
            let ahead = parts[1]
                .parse::<u64>()
                .map_err(|_| VersionError::InvalidFormat(original.clone()))?;
            let hash = parts[2]
                .strip_prefix('g')
                .ok_or_else(|| VersionError::InvalidFormat(original.clone()))?
                .to_string();
            (ahead, Some(hash))
        } else if parts.len() == 1 {
            (0, None)
        } else {
            return Err(VersionError::InvalidFormat(original));
        };

        Ok(GitVersion {
            major,
            minor,
            patch,
            commits_ahead,
            hash,
            dirty,
        })
    }

    /// Returns the major version number.
    pub fn major(&self) -> u64 {
        self.major
    }

    /// Returns the minor version number.
    pub fn minor(&self) -> u64 {
        self.minor
    }

    /// Returns the patch version number.
    pub fn patch(&self) -> u64 {
        self.patch
    }

    /// Returns the number of commits ahead of the tag.
    pub fn commits_ahead(&self) -> u64 {
        self.commits_ahead
    }

    /// Returns the git hash if present.
    pub fn hash(&self) -> Option<&str> {
        self.hash.as_deref()
    }

    /// Returns true if the working tree was dirty.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Returns true if this is a tagged release (no commits ahead).
    pub fn is_tagged_release(&self) -> bool {
        self.commits_ahead == 0
    }

    /// Compares this version with another.
    ///
    /// If both versions contain different git hashes, this method will
    /// use the GitHub API to check ancestry relationships.
    ///
    /// For local-only comparison without network access, use the [`Ord`]
    /// implementation which compares semantic version numbers and commit count.
    ///
    /// # Examples
    ///
    /// ```
    /// use cadmus_core::version::{GitVersion, VersionComparison};
    ///
    /// // Local is newer than remote (higher semver)
    /// let local: GitVersion = "v0.9.46".parse().unwrap();
    /// let remote: GitVersion = "v0.9.45".parse().unwrap();
    /// let result = local.compare(&remote).unwrap();
    /// assert_eq!(result, VersionComparison::Newer);
    ///
    /// // Local is older than remote (lower semver)
    /// let local: GitVersion = "v0.9.44".parse().unwrap();
    /// let remote: GitVersion = "v0.9.45".parse().unwrap();
    /// let result = local.compare(&remote).unwrap();
    /// assert_eq!(result, VersionComparison::Older);
    ///
    /// // Local equals remote (same version)
    /// let local: GitVersion = "v0.9.46".parse().unwrap();
    /// let remote: GitVersion = "v0.9.46".parse().unwrap();
    /// let result = local.compare(&remote).unwrap();
    /// assert_eq!(result, VersionComparison::Equal);
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, other), fields(local = %self, remote = %other))
    )]
    pub fn compare(&self, other: &GitVersion) -> Result<VersionComparison, VersionError> {
        tracing::debug!(local = %self, remote = %other, "Comparing versions");

        let semver_cmp = compare_semver(self, other);
        if semver_cmp != std::cmp::Ordering::Equal {
            tracing::debug!(result = ?semver_cmp, "Semver comparison determined order");
            return Ok(match semver_cmp {
                std::cmp::Ordering::Greater => VersionComparison::Newer,
                std::cmp::Ordering::Less => VersionComparison::Older,
                std::cmp::Ordering::Equal => unreachable!(),
            });
        }

        match (
            self.commits_ahead(),
            other.commits_ahead(),
            self.hash(),
            other.hash(),
        ) {
            (0, 0, _, _) => {
                tracing::debug!("Both versions are tagged releases with same semver");
                Ok(VersionComparison::Equal)
            }

            (0, remote_ahead, _, Some(_)) => {
                tracing::debug!(
                    remote_ahead,
                    "Local is tagged release, remote has commits ahead"
                );
                Ok(VersionComparison::Older)
            }

            (local_ahead, 0, Some(_), _) => {
                tracing::debug!(
                    local_ahead,
                    "Local has commits ahead, remote is tagged release"
                );
                Ok(VersionComparison::Newer)
            }

            (local_ahead, remote_ahead, Some(local_hash), Some(remote_hash)) => {
                tracing::debug!(
                    local_ahead,
                    remote_ahead,
                    local_hash,
                    remote_hash,
                    "Both versions have commits ahead, checking ancestry"
                );

                if local_hash == remote_hash {
                    if local_ahead != remote_ahead {
                        return Err(VersionError::InconsistentData(format!(
                            "same hash '{}' but different commits ahead: {} vs {}",
                            local_hash, local_ahead, remote_ahead
                        )));
                    }
                    tracing::debug!("Same hash and same commit count");
                    return Ok(VersionComparison::Equal);
                }

                let github =
                    GithubClient::new(None).map_err(|e| VersionError::GitHubApi(e.to_string()))?;
                check_ancestry(&github, local_hash, remote_hash)
            }

            _ => {
                tracing::warn!("Unexpected version comparison state");
                Ok(VersionComparison::Incomparable)
            }
        }
    }
}

impl Serialize for GitVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GitVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GitVersionVisitor;

        impl Visitor<'_> for GitVersionVisitor {
            type Value = GitVersion;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a git version string (e.g., 'v1.2.3' or 'v1.2.3-5-gabc123')")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                GitVersion::parse(value).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(GitVersionVisitor)
    }
}

impl sqlx::Type<Sqlite> for GitVersion {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, Sqlite> for GitVersion {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        self.to_string().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for GitVersion {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        s.parse().map_err(Into::into)
    }
}

/// Returns the current application version from compile-time environment.
///
/// On the emulator path this function panics if the version string cannot be parsed,
/// catching build issues early during development. On the app path it logs a warning
/// and falls back to `v0.0.0` so a bad build descriptor does not crash the device.
pub fn get_current_version() -> GitVersion {
    let version_str = env!("GIT_VERSION");

    cfg_select! {
        feature = "emulator" => {
            version_str.parse().unwrap_or_else(|e| {
                panic!("compile-time GIT_VERSION is not a valid git-describe string: {e}")
            })
        }
        _ => {
            match version_str.parse() {
                Ok(version) => version,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        version = version_str,
                        "Failed to parse compile-time GIT_VERSION; falling back to v0.0.0"
                    );
                    "v0.0.0"
                        .parse()
                        .expect("v0.0.0 is always a valid version string")
                }
            }
        }
    }
}

/// Returns complete compile-time version metadata for display.
pub fn get_version() -> Version {
    Version {
        git: get_current_version(),
        pull_request: option_env!("PR_INFO").map(|value| PullRequestInfo { value }),
        build: get_build_attributes(),
        features: BuildFeatures {
            features: BUILD_FEATURES,
        },
        build_kind: get_build_kind(),
    }
}

/// Returns compile-time build provenance attributes.
///
/// The timestamp is parsed from `BUILD_TIMESTAMP`, which is emitted by
/// `build.rs` as Unix epoch seconds.
pub fn get_build_attributes() -> BuildAttributes {
    BuildAttributes {
        user: env!("BUILD_USER"),
        host: env!("BUILD_HOST"),
        timestamp: BuildTimestamp::parse(env!("BUILD_TIMESTAMP")),
    }
}

fn get_build_kind() -> BuildKind {
    cfg_select! {
        feature = "test" => { BuildKind::Test }
        _ => { BuildKind::Standard }
    }
}

fn parse_semver(semver: &str) -> Result<(u64, u64, u64), VersionError> {
    let without_v = semver.strip_prefix('v').unwrap_or(semver);
    let nums: Vec<&str> = without_v.split('.').collect();

    if nums.len() != 3 {
        return Err(VersionError::InvalidFormat(semver.to_string()));
    }

    let major = nums[0]
        .parse::<u64>()
        .map_err(|_| VersionError::InvalidFormat(semver.to_string()))?;
    let minor = nums[1]
        .parse::<u64>()
        .map_err(|_| VersionError::InvalidFormat(semver.to_string()))?;
    let patch = nums[2]
        .parse::<u64>()
        .map_err(|_| VersionError::InvalidFormat(semver.to_string()))?;

    Ok((major, minor, patch))
}

/// Compares semantic versions (major, minor, patch) between two versions.
///
/// Returns `Ordering::Greater` if local has a higher semantic version,
/// `Ordering::Less` if remote has a higher semantic version,
/// or `Ordering::Equal` if both have the same semantic version.
///
/// # Examples
///
/// ```
/// use cadmus_core::version::{GitVersion, compare_semver};
/// use std::cmp::Ordering;
///
/// let v1: GitVersion = "v0.9.46".parse().unwrap();
/// let v2: GitVersion = "v0.9.45".parse().unwrap();
/// assert_eq!(compare_semver(&v1, &v2), Ordering::Greater);
///
/// let v1: GitVersion = "v0.9.44".parse().unwrap();
/// let v2: GitVersion = "v0.9.45".parse().unwrap();
/// assert_eq!(compare_semver(&v1, &v2), Ordering::Less);
///
/// let v1: GitVersion = "v0.9.46".parse().unwrap();
/// let v2: GitVersion = "v0.9.46".parse().unwrap();
/// assert_eq!(compare_semver(&v1, &v2), Ordering::Equal);
/// ```
pub fn compare_semver(local: &GitVersion, remote: &GitVersion) -> std::cmp::Ordering {
    local
        .major()
        .cmp(&remote.major())
        .then_with(|| local.minor().cmp(&remote.minor()))
        .then_with(|| local.patch().cmp(&remote.patch()))
}

/// Checks commit ancestry using GitHub's compare API.
///
/// Makes a request to GitHub's compare endpoint to determine if `local_hash`
/// is ahead of, behind, or diverged from `remote_hash`.
///
/// # Arguments
///
/// * `github` - GitHub client for making API requests
/// * `local_hash` - The local commit hash to compare
/// * `remote_hash` - The remote commit hash to compare against
///
/// # Errors
///
/// Returns `VersionError::GitHubApi` if:
/// - The HTTP request fails
/// - GitHub returns a non-success status code
/// - The response cannot be parsed
fn check_ancestry(
    github: &GithubClient,
    local_hash: &str,
    remote_hash: &str,
) -> Result<VersionComparison, VersionError> {
    let url = format!(
        "https://api.github.com/repos/ogkevin/cadmus/compare/{}...{}",
        remote_hash, local_hash
    );

    tracing::debug!(url = %url, "Checking commit ancestry via GitHub API");

    let response = github
        .get_unauthenticated(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| {
            tracing::error!(error = %e, "GitHub API request failed");
            VersionError::GitHubApi(e.to_string())
        })?;

    if !response.status().is_success() {
        let status = response.status();
        tracing::error!(status = ?status, "GitHub API returned error");
        return Err(VersionError::GitHubApi(format!(
            "HTTP {}",
            response.status()
        )));
    }

    let compare: CompareResponse = response.json().map_err(|e| {
        tracing::error!(error = %e, "Failed to parse GitHub response");
        VersionError::GitHubApi(e.to_string())
    })?;

    tracing::debug!(status = %compare.status, "GitHub compare result received");

    match compare.status.as_str() {
        "ahead" => Ok(VersionComparison::Newer),
        "behind" => Ok(VersionComparison::Older),
        "identical" => Ok(VersionComparison::Equal),
        "diverged" => Ok(VersionComparison::Incomparable),
        other => {
            tracing::warn!(status = other, "Unknown compare status from GitHub");
            Err(VersionError::GitHubApi(format!(
                "Unknown compare status: {}",
                other
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_display_includes_metadata() {
        let version = Version {
            git: GitVersion::parse("v0.9.46-5-gabc123").unwrap(),
            pull_request: Some(PullRequestInfo {
                value: "PR #12 (abc1234)",
            }),
            build: BuildAttributes {
                user: "builder",
                host: "host",
                timestamp: BuildTimestamp::parse("1234567890"),
            },
            features: BuildFeatures {
                features: &["kobo", "test"],
            },
            build_kind: BuildKind::Test,
        };
        let build_attributes = crate::fl!(
            "build-attributes",
            timestamp = "2009-02-13 23:31:30 UTC",
            user = "builder",
            host = "host",
        );

        assert_eq!(
            version.to_string(),
            format!("Test v0.9.46-5-gabc123\nPR #12 (abc1234)\n{build_attributes}")
        );
    }

    #[test]
    fn test_version_to_string_with_features() {
        let version = Version {
            git: GitVersion::parse("v0.9.46").unwrap(),
            pull_request: None,
            build: BuildAttributes {
                user: "builder",
                host: "host",
                timestamp: BuildTimestamp::parse("1234567890"),
            },
            features: BuildFeatures {
                features: &["kobo", "tracing"],
            },
            build_kind: BuildKind::Standard,
        };
        let build_attributes = crate::fl!(
            "build-attributes",
            timestamp = "2009-02-13 23:31:30 UTC",
            user = "builder",
            host = "host",
        );
        let build_features = crate::fl!("build-features", features = "kobo, tracing");

        assert_eq!(
            version.to_string_with_features(),
            format!("v0.9.46\n{build_attributes}\n{build_features}")
        );
    }

    #[test]
    fn test_version_to_string_with_features_without_features() {
        let version = Version {
            git: GitVersion::parse("v0.9.46").unwrap(),
            pull_request: None,
            build: BuildAttributes {
                user: "builder",
                host: "host",
                timestamp: BuildTimestamp::parse("1234567890"),
            },
            features: BuildFeatures { features: &[] },
            build_kind: BuildKind::Standard,
        };

        assert_eq!(version.to_string_with_features(), version.to_string());
    }

    #[test]
    fn test_version_write_as_html_to_includes_feature_list() {
        let version = Version {
            git: GitVersion::parse("v0.9.46").unwrap(),
            pull_request: None,
            build: BuildAttributes {
                user: "builder",
                host: "host",
                timestamp: BuildTimestamp::parse("1234567890"),
            },
            features: BuildFeatures {
                features: &["kobo", "tracing"],
            },
            build_kind: BuildKind::Standard,
        };
        let mut html = String::new();
        version.write_as_html_to(&mut html);

        assert!(html.contains("<td class=\"key\">Version</td>"));
        assert!(html.contains("Cadmus v0.9.46"));
        assert!(html.contains("<li>kobo</li>"));
        assert!(html.contains("<li>tracing</li>"));
        assert!(
            html.find("<li>kobo</li>").unwrap() < html.find("<li>tracing</li>").unwrap(),
            "features should be sorted alphabetically"
        );
        assert!(html.contains("<tr class=\"sep\"></tr>"));
    }

    #[test]
    fn test_version_display_without_pull_request() {
        let version = Version {
            git: GitVersion::parse("v0.9.46").unwrap(),
            pull_request: None,
            build: BuildAttributes {
                user: "builder",
                host: "host",
                timestamp: BuildTimestamp::parse("1234567890"),
            },
            features: BuildFeatures {
                features: &["kobo"],
            },
            build_kind: BuildKind::Standard,
        };
        let build_attributes = crate::fl!(
            "build-attributes",
            timestamp = "2009-02-13 23:31:30 UTC",
            user = "builder",
            host = "host",
        );

        assert_eq!(version.to_string(), format!("v0.9.46\n{build_attributes}"));
    }

    #[test]
    fn test_build_timestamp_display_falls_back_to_raw_value() {
        assert_eq!(BuildTimestamp::parse("unknown").to_string(), "unknown");
    }

    #[test]
    fn test_parse_release_version() {
        let v = GitVersion::parse("v0.9.46").unwrap();
        assert_eq!(v.major(), 0);
        assert_eq!(v.minor(), 9);
        assert_eq!(v.patch(), 46);
        assert_eq!(v.commits_ahead(), 0);
        assert!(v.hash().is_none());
        assert!(!v.is_dirty());
        assert!(v.is_tagged_release());
    }

    #[test]
    fn test_parse_development_version() {
        let v = GitVersion::parse("v0.9.46-5-gabc123").unwrap();
        assert_eq!(v.major(), 0);
        assert_eq!(v.minor(), 9);
        assert_eq!(v.patch(), 46);
        assert_eq!(v.commits_ahead(), 5);
        assert_eq!(v.hash(), Some("abc123"));
        assert!(!v.is_dirty());
        assert!(!v.is_tagged_release());
    }

    #[test]
    fn test_parse_dirty_version() {
        let v = GitVersion::parse("v0.9.46-5-gabc123-dirty").unwrap();
        assert_eq!(v.major(), 0);
        assert_eq!(v.minor(), 9);
        assert_eq!(v.patch(), 46);
        assert_eq!(v.commits_ahead(), 5);
        assert_eq!(v.hash(), Some("abc123"));
        assert!(v.is_dirty());
    }

    #[test]
    fn test_parse_without_v_prefix() {
        let v = GitVersion::parse("0.9.46").unwrap();
        assert_eq!(v.major(), 0);
        assert_eq!(v.minor(), 9);
        assert_eq!(v.patch(), 46);
    }

    #[test]
    fn test_parse_invalid_version() {
        assert!(GitVersion::parse("invalid").is_err());
        assert!(GitVersion::parse("v1.2").is_err());
        assert!(GitVersion::parse("v1.2.3.4").is_err());
        assert!(GitVersion::parse("v1.2.3-abc").is_err());
    }

    #[test]
    fn test_ord_dirty_is_newer_than_clean() {
        let clean: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        let dirty: GitVersion = "v0.9.46-5-gabc123-dirty".parse().unwrap();

        assert!(dirty > clean);
        assert!(clean < dirty);

        let clean_release: GitVersion = "v0.9.46".parse().unwrap();
        let dirty_release: GitVersion = "v0.9.46-dirty".parse().unwrap();

        assert!(dirty_release > clean_release);
    }

    #[test]
    fn test_ord_by_semver_and_commits_ahead() {
        let v094: GitVersion = "v0.9.46".parse().unwrap();
        let v095: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        let v100: GitVersion = "v0.10.0".parse().unwrap();
        let v100_dev: GitVersion = "v0.10.0-3-gdef456".parse().unwrap();

        assert!(v094 < v095);
        assert!(v095 < v100);
        assert!(v100 < v100_dev);
        assert!(v094 < v100_dev);
        assert_eq!(v094.cmp(&v094), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_different_semver() {
        let local1: GitVersion = "v0.9.46".parse().unwrap();
        let remote1: GitVersion = "v0.9.45".parse().unwrap();
        assert_eq!(local1.compare(&remote1).unwrap(), VersionComparison::Newer);

        let local2: GitVersion = "v0.9.45".parse().unwrap();
        let remote2: GitVersion = "v0.9.46".parse().unwrap();
        assert_eq!(local2.compare(&remote2).unwrap(), VersionComparison::Older);

        let local3: GitVersion = "v0.9.46".parse().unwrap();
        let remote3: GitVersion = "v0.9.46".parse().unwrap();
        assert_eq!(local3.compare(&remote3).unwrap(), VersionComparison::Equal);

        let local4: GitVersion = "v0.10.0".parse().unwrap();
        let remote4: GitVersion = "v0.9.46".parse().unwrap();
        assert_eq!(local4.compare(&remote4).unwrap(), VersionComparison::Newer);

        let local5: GitVersion = "v1.0.0".parse().unwrap();
        let remote5: GitVersion = "v0.9.46".parse().unwrap();
        assert_eq!(local5.compare(&remote5).unwrap(), VersionComparison::Newer);
    }

    #[test]
    fn test_compare_tagged_vs_development() {
        let local1: GitVersion = "v0.9.46".parse().unwrap();
        let remote1: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        assert_eq!(local1.compare(&remote1).unwrap(), VersionComparison::Older);

        let local2: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        let remote2: GitVersion = "v0.9.46".parse().unwrap();
        assert_eq!(local2.compare(&remote2).unwrap(), VersionComparison::Newer);
    }

    #[test]
    fn test_compare_same_hash_different_ahead() {
        let local: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        let remote: GitVersion = "v0.9.46-3-gabc123".parse().unwrap();
        let result = local.compare(&remote);
        assert!(matches!(result, Err(VersionError::InconsistentData(_))));
    }

    #[test]
    fn test_compare_same_hash_same_ahead() {
        let local: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        let remote: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        assert_eq!(local.compare(&remote).unwrap(), VersionComparison::Equal);
    }

    #[test]
    #[ignore = "requires network access to GitHub API"]
    fn test_compare_different_hashes_needs_github() {
        let local: GitVersion = "v0.9.46-5-gabc123".parse().unwrap();
        let remote: GitVersion = "v0.9.46-3-gdef456".parse().unwrap();
        // This will attempt to create a GitHub client and call the API
        // Since abc123 and def456 are not real commits, it will fail
        let result = local.compare(&remote);
        assert!(result.is_err());
    }

    #[test]
    fn test_git_version_serde_roundtrip() {
        let versions = vec!["v0.9.46", "v0.9.46-5-gabc123", "v0.9.46-5-gabc123-dirty"];

        for version_str in versions {
            let version: GitVersion = version_str.parse().unwrap();
            let serialized = serde_json::to_string(&version).unwrap();
            let deserialized: GitVersion = serde_json::from_str(&serialized).unwrap();
            assert_eq!(version, deserialized);
            assert_eq!(serialized, format!("\"{}\"", version_str));
        }
    }

    #[test]
    fn test_git_version_deserialize_from_string() {
        let json = "\"v0.9.46-5-gabc123\"";
        let version: GitVersion = serde_json::from_str(json).unwrap();
        assert_eq!(version.major(), 0);
        assert_eq!(version.minor(), 9);
        assert_eq!(version.patch(), 46);
        assert_eq!(version.commits_ahead(), 5);
        assert_eq!(version.hash(), Some("abc123"));
    }

    #[test]
    #[ignore = "requires network access to GitHub API"]
    fn test_check_ancestry_ahead() {
        crate::crypto::init_crypto_provider();
        let github = GithubClient::new(None).expect("client build");

        let result = check_ancestry(&github, "HEAD", "v0.9.46");
        assert!(
            result.is_ok(),
            "Ancestry check should succeed: {:?}",
            result.err()
        );

        let comparison = result.unwrap();
        assert_eq!(
            comparison,
            VersionComparison::Newer,
            "HEAD should be ahead of v0.9.46"
        );
    }

    #[test]
    #[ignore = "requires network access to GitHub API"]
    fn test_check_ancestry_same_commit() {
        crate::crypto::init_crypto_provider();
        let github = GithubClient::new(None).expect("client build");

        let result = check_ancestry(&github, "HEAD", "HEAD");
        assert!(
            result.is_ok(),
            "Same commit comparison should succeed: {:?}",
            result.err()
        );
        assert_eq!(result.unwrap(), VersionComparison::Equal);
    }
}
