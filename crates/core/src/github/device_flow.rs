use secrecy::{ExposeSecret, SecretString};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;

/// Subdirectory and filename appended to the card root for token storage.
///
/// Uses `.adds/cadmus/` for normal builds and `.adds/cadmus-tst/` for test
/// builds.
#[cfg(not(feature = "test"))]
const TOKEN_RELATIVE_PATH: &str = ".adds/cadmus/.github_token";

#[cfg(feature = "test")]
const TOKEN_RELATIVE_PATH: &str = ".adds/cadmus-tst/.github_token";

/// Persists a GitHub OAuth token to disk for reuse across app restarts.
///
/// Writes to `{card_root}/.adds/cadmus/.github_token` with `0600` permissions.
///
/// # Errors
///
/// Returns an error string if directory creation or file write fails.
pub fn save_token(token: &SecretString) -> Result<(), String> {
    let path = token_path();
    tracing::debug!(path = %path.display(), "Saving GitHub token");

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create token dir: {}", e))?;
    }

    let mut file =
        File::create(&path).map_err(|e| format!("Failed to create token file: {}", e))?;
    file.write_all(token.expose_secret().as_bytes())
        .map_err(|e| format!("Failed to write token: {}", e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to set token file permissions: {}", e))?;
    }

    tracing::info!("GitHub token saved");
    Ok(())
}

/// Loads a previously saved GitHub OAuth token from disk.
///
/// Returns `None` if no token file exists (first-time setup).
///
/// # Errors
///
/// Returns an error string if the file exists but cannot be read.
pub fn load_token() -> Result<Option<SecretString>, String> {
    let path = token_path();
    tracing::debug!(path = %path.display(), "Loading GitHub token");

    if !path.exists() {
        tracing::debug!("No saved token found");
        return Ok(None);
    }

    let mut contents = String::new();
    File::open(&path)
        .map_err(|e| format!("Failed to open token file: {}", e))?
        .read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read token file: {}", e))?;

    let token = contents.trim().to_owned();
    if token.is_empty() {
        tracing::warn!("Token file exists but is empty");
        return Ok(None);
    }

    tracing::info!("GitHub token loaded from disk");
    Ok(Some(SecretString::from(token)))
}

/// Deletes the saved GitHub OAuth token from disk.
///
/// Called when a token is found to be invalid or revoked, so the next
/// authentication attempt starts fresh via device flow.
///
/// Returns `Ok(())` if the file was deleted or did not exist.
///
/// # Errors
///
/// Returns an error string if the file exists but cannot be removed.
pub fn delete_token() -> Result<(), String> {
    let path = token_path();
    tracing::debug!(path = %path.display(), "Deleting GitHub token");

    if !path.exists() {
        return Ok(());
    }

    fs::remove_file(&path).map_err(|e| format!("Failed to delete token file: {}", e))?;
    tracing::info!("GitHub token deleted");
    Ok(())
}

fn token_path() -> PathBuf {
    #[cfg(test)]
    return std::env::temp_dir()
        .join("cadmus-test")
        .join(TOKEN_RELATIVE_PATH);

    #[cfg(all(not(test), feature = "emulator"))]
    return PathBuf::from("/tmp").join(TOKEN_RELATIVE_PATH);

    #[cfg(all(not(test), not(feature = "emulator")))]
    return PathBuf::from(crate::settings::INTERNAL_CARD_ROOT).join(TOKEN_RELATIVE_PATH);
}
