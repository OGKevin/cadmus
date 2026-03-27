//! HTTP download and checksum helpers.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Downloads `url` to `dest`, following redirects.
///
/// Creates parent directories of `dest` if they do not exist.
///
/// # Errors
///
/// Returns an error if the HTTP request fails, the server returns a non-success
/// status, or writing to `dest` fails.
pub fn download(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory for {}", dest.display()))?;
    }

    let response = reqwest::blocking::get(url)
        .with_context(|| format!("HTTP request failed for {url}"))?
        .error_for_status()
        .with_context(|| format!("server returned error status for {url}"))?;

    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read response body from {url}"))?;

    std::fs::write(dest, &bytes)
        .with_context(|| format!("failed to write downloaded file to {}", dest.display()))
}

/// Verifies the SHA-256 checksum of `file` against `expected`.
///
/// `expected` must be a lowercase hex string without any prefix.
///
/// # Errors
///
/// Returns an error if the file cannot be read or the checksum does not match.
pub fn verify_sha256(file: &Path, expected: &str) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| {
        format!(
            "failed to read {} for checksum verification",
            file.display()
        )
    })?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    if actual != expected {
        bail!(
            "SHA-256 checksum mismatch for {}\n  expected: {expected}\n  got:      {actual}",
            file.display()
        );
    }

    println!("Checksum verified.");

    Ok(())
}

/// Downloads `url` to a temporary file, verifies its SHA-256 checksum, then
/// moves it to `dest`.
///
/// # Errors
///
/// Returns an error if the download, checksum verification, or move fails.
pub fn download_verified(url: &str, dest: &Path, expected_sha256: &str) -> Result<()> {
    let tmp = dest.with_extension("tmp");
    download(url, &tmp)?;

    if let Err(e) = verify_sha256(&tmp, expected_sha256) {
        std::fs::remove_file(&tmp).ok();
        return Err(e);
    }

    std::fs::rename(&tmp, dest)
        .with_context(|| format!("failed to move download to {}", dest.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn verify_sha256_accepts_correct_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("data.bin");
        fs::write(&file, b"hello world").unwrap();

        let mut hasher = Sha256::new();
        hasher.update(b"hello world");
        let real_hash: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        assert!(verify_sha256(&file, &real_hash).is_ok());
    }

    #[test]
    fn verify_sha256_rejects_wrong_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("data.bin");
        fs::write(&file, b"hello world").unwrap();

        let result = verify_sha256(
            &file,
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("checksum mismatch"));
    }
}
