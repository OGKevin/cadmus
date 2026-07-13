//! GitHub Releases API helpers.
//!
//! Fetches release metadata and returns typed asset information including the
//! download URL and optional SHA-256 digest for checksum verification.
//!
//! The `digest` field in the GitHub API response has the format
//! `sha256:<hex>`.  [`Asset::sha256`] strips the prefix so the hex string can
//! be passed directly to [`super::http::download_verified`].
//!
//! ## Authentication
//!
//! All requests use a shared [`reqwest::blocking::Client`] that sets the
//! required `User-Agent` header and, when `GH_TOKEN` or `GITHUB_TOKEN` is
//! present in the environment, an `Authorization: Bearer` header.  This
//! avoids 403 rate-limit errors in GitHub Actions where the token is always
//! available.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Builds a `reqwest` blocking client with the GitHub API `User-Agent` and,
/// when available, an `Authorization: Bearer` token from the environment.
///
/// Checks `GH_TOKEN` first, then `GITHUB_TOKEN`.
pub(crate) fn client() -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder().user_agent("cargo-xtask/cadmus");

    if let Some(token) = std::env::var("GH_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
    {
        let mut auth = reqwest::header::HeaderMap::new();
        auth.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .context("invalid token value for Authorization header")?,
        );
        builder = builder.default_headers(auth);
    }

    builder.build().context("failed to build HTTP client")
}

/// A single asset from a GitHub release.
#[derive(Debug, Deserialize)]
pub struct Asset {
    /// The direct download URL.
    pub browser_download_url: String,
    /// The asset filename.
    pub name: String,
    /// The digest in `sha256:<hex>` format, if provided by GitHub.
    pub digest: Option<String>,
}

impl Asset {
    /// Returns the SHA-256 hex digest, stripping the `sha256:` prefix.
    ///
    /// Returns `None` if the `digest` field is absent or has an unexpected
    /// format.
    pub fn sha256(&self) -> Option<&str> {
        self.digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
    }
}

/// The subset of a GitHub release response that we need.
#[derive(Debug, Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

/// Fetches the named asset from a GitHub release.
///
/// `repo` must be in `owner/name` format (e.g. `"pgaskin/NickelMenu"`).
/// `tag` must include the `v` prefix if the release uses one (e.g. `"v0.6.0"`).
///
/// # Errors
///
/// Returns an error if the HTTP request fails, the response cannot be parsed,
/// or no asset with the given name exists in the release.
pub fn fetch_release_asset(repo: &str, tag: &str, asset_name: &str) -> Result<Asset> {
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    fetch_release_from_url(&url)?
        .assets
        .into_iter()
        .find(|a| a.name == asset_name)
        .with_context(|| format!("asset '{asset_name}' not found in release {tag} of {repo}"))
}

/// Fetches the named asset from the latest GitHub release.
///
/// `repo` must be in `owner/name` format (e.g. `"OGKevin/cadmus"`).
///
/// Currently unused because asset directories are sourced from the Plato
/// release zip as a workaround (see [issue #64]).  When that issue is resolved
/// and Cadmus builds these directories from source, the entire
/// `download_assets` task will be removed and this function along with it.
///
/// [issue #64]: https://github.com/OGKevin/cadmus/issues/64
///
/// # Errors
///
/// Returns an error if the HTTP request fails, the response cannot be parsed,
/// or no asset with the given name exists in the latest release.
#[allow(dead_code)]
pub fn fetch_latest_release_asset(repo: &str, asset_name: &str) -> Result<Asset> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    fetch_release_from_url(&url)?
        .assets
        .into_iter()
        .find(|a| a.name == asset_name)
        .with_context(|| format!("asset '{asset_name}' not found in latest release of {repo}"))
}

/// Fetches the first asset whose name starts with `prefix` from the latest
/// GitHub release.
///
/// Useful when the asset name includes a version number
/// (e.g. `"plato-0.9.45.zip"`).
///
/// `repo` must be in `owner/name` format (e.g. `"baskerville/plato"`).
///
/// Currently unused — asset downloads use a pinned version via
/// [`fetch_release_asset`] instead.  Kept for potential future use.
///
/// # Errors
///
/// Returns an error if the HTTP request fails, the response cannot be parsed,
/// or no asset matching the prefix exists in the latest release.
#[allow(dead_code)]
pub fn fetch_latest_release_asset_by_prefix(repo: &str, prefix: &str) -> Result<Asset> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    fetch_release_from_url(&url)?
        .assets
        .into_iter()
        .find(|a| a.name.starts_with(prefix))
        .with_context(|| {
            format!("no asset with prefix '{prefix}' found in latest release of {repo}")
        })
}

/// Downloads a release asset to `dest`, using GitHub authentication when available.
///
/// Uses the same authenticated client as the API calls so that assets from
/// repositories that require authentication can be downloaded without a
/// separate token setup.
///
/// # Errors
///
/// Returns an error if the download or write fails.
pub fn download_asset(asset: &Asset, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for {}", dest.display()))?;
    }

    let temp = {
        let mut path = dest.as_os_str().to_os_string();
        path.push(".part");
        PathBuf::from(path)
    };

    let bytes = client()?
        .get(&asset.browser_download_url)
        .send()
        .with_context(|| format!("HTTP request failed for {}", asset.browser_download_url))?
        .error_for_status()
        .with_context(|| {
            format!(
                "server returned error status for {}",
                asset.browser_download_url
            )
        })?
        .bytes()
        .with_context(|| {
            format!(
                "failed to read response body from {}",
                asset.browser_download_url
            )
        })?;

    std::fs::write(&temp, &bytes)
        .with_context(|| format!("failed to write downloaded file to {}", temp.display()))?;
    std::fs::rename(&temp, dest).with_context(|| {
        format!(
            "failed to move downloaded file from {} to {}",
            temp.display(),
            dest.display()
        )
    })
}

fn fetch_release_from_url(url: &str) -> Result<Release> {
    client()?
        .get(url)
        .send()
        .with_context(|| format!("HTTP request failed for {url}"))?
        .error_for_status()
        .with_context(|| format!("server returned error status for {url}"))?
        .json()
        .with_context(|| format!("failed to parse GitHub API response from {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_release_json(name: &str, digest: Option<&str>) -> String {
        let digest_field = match digest {
            Some(d) => format!(r#", "digest": "{d}""#),
            None => String::new(),
        };
        format!(
            r#"{{
                "assets": [
                    {{
                        "browser_download_url": "https://github.com/example/releases/download/v1.0/{name}",
                        "name": "{name}"{digest_field}
                    }}
                ]
            }}"#
        )
    }

    #[test]
    fn asset_sha256_strips_prefix() {
        let asset = Asset {
            browser_download_url: String::new(),
            name: String::new(),
            digest: Some("sha256:deadbeef".to_owned()),
        };
        assert_eq!(asset.sha256(), Some("deadbeef"));
    }

    #[test]
    fn asset_sha256_returns_none_when_absent() {
        let asset = Asset {
            browser_download_url: String::new(),
            name: String::new(),
            digest: None,
        };
        assert!(asset.sha256().is_none());
    }

    #[test]
    fn asset_sha256_returns_none_for_unexpected_prefix() {
        let asset = Asset {
            browser_download_url: String::new(),
            name: String::new(),
            digest: Some("md5:deadbeef".to_owned()),
        };
        assert!(asset.sha256().is_none());
    }

    #[test]
    fn release_deserializes_asset_with_digest() {
        let json = sample_release_json("foo.tgz", Some("sha256:abc123"));
        let release: Release = serde_json::from_str(&json).unwrap();
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].name, "foo.tgz");
        assert_eq!(release.assets[0].sha256(), Some("abc123"));
    }

    #[test]
    fn release_deserializes_asset_without_digest() {
        let json = sample_release_json("foo.tgz", None);
        let release: Release = serde_json::from_str(&json).unwrap();
        assert!(release.assets[0].sha256().is_none());
    }

    #[test]
    fn release_deserializes_download_url() {
        let json = sample_release_json("foo.tgz", None);
        let release: Release = serde_json::from_str(&json).unwrap();
        assert!(release.assets[0].browser_download_url.contains("foo.tgz"));
    }

    #[test]
    fn release_deserializes_empty_assets() {
        let json = r#"{"assets": []}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert!(release.assets.is_empty());
    }
}
