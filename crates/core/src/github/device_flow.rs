use secrecy::{ExposeSecret, SecretString};
use std::fs::{self, File};
use std::io::{Read, Write};

/// Filename of the token written under [`crate::device::DevicePaths::install_dir()`].
const TOKEN_FILENAME: &str = ".github_token";

/// Debug-build environment variable read by [`token_from_env`].
///
/// Release builds never consult this value.
#[cfg(debug_assertions)]
const DEV_TOKEN_ENV: &str = "GH_TOKEN";

/// Returns a GitHub token from the [`DEV_TOKEN_ENV`] environment variable.
///
/// Only available in debug builds (`debug_assertions`). Release builds always
/// return `None` so PATs cannot be injected via the environment on device.
#[cfg(debug_assertions)]
pub fn token_from_env() -> Option<SecretString> {
    token_from_env_var(std::env::var(DEV_TOKEN_ENV))
}

#[cfg(debug_assertions)]
fn token_from_env_var(raw: Result<String, std::env::VarError>) -> Option<SecretString> {
    let token = raw.ok()?;
    let token = token.trim();
    if token.is_empty() {
        tracing::warn!(env = DEV_TOKEN_ENV, "Environment variable is set but empty");
        return None;
    }

    tracing::info!(env = DEV_TOKEN_ENV, "Using GitHub token from environment");
    Some(SecretString::from(token.to_owned()))
}

#[cfg(not(debug_assertions))]
pub fn token_from_env() -> Option<SecretString> {
    None
}

/// Origin of the token currently selected for GitHub OTA requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthOrigin {
    /// [`DEV_TOKEN_ENV`], captured by [`token_from_env`].
    Environment,
    /// Token from [`load_token`] / [`save_token`].
    Saved,
}

/// Combined OTA GitHub credentials: a saved token plus an optional debug
/// [`DEV_TOKEN_ENV`] override.
pub(crate) struct ResolvedAuth {
    /// Token persisted by [`save_token`] as [`TOKEN_FILENAME`] under
    /// [`crate::device::DevicePaths::install_dir()`].
    saved: Option<SecretString>,
    /// Snapshot of [`DEV_TOKEN_ENV`] taken by [`token_from_env`] when this
    /// session was loaded via [`Self::load`].
    ///
    /// Always `None` in release builds. Kept even after rejection so the
    /// original env value is not re-read mid-session.
    from_env: Option<SecretString>,
    /// When `true`, [`effective`](Self::effective) skips `from_env` and uses
    /// `saved` instead.
    ///
    /// Set by [`reject_effective`](Self::reject_effective) after GitHub
    /// rejects the [`AuthOrigin::Environment`] token, so a later retry can
    /// use the saved credential without deleting it. Starts `false`.
    ignore_env: bool,
}

impl ResolvedAuth {
    /// Loads the token saved under [`crate::device::DevicePaths::install_dir()`]
    /// and, in debug builds, a [`DEV_TOKEN_ENV`] override via [`token_from_env`].
    pub(crate) fn load(install_dir: &std::path::Path) -> Result<Self, String> {
        Self::load_with(install_dir, token_from_env())
    }

    fn load_with(
        install_dir: &std::path::Path,
        from_env: Option<SecretString>,
    ) -> Result<Self, String> {
        let saved = match load_token(install_dir) {
            Ok(token) => token,
            Err(e) if from_env.is_some() => {
                tracing::warn!(error = %e, "Failed to load saved GitHub token");
                None
            }
            Err(e) => return Err(e),
        };
        Ok(Self::from_parts(saved, from_env))
    }

    pub(crate) fn empty() -> Self {
        Self::from_parts(None, None)
    }

    fn from_parts(saved: Option<SecretString>, from_env: Option<SecretString>) -> Self {
        Self {
            saved,
            from_env,
            ignore_env: false,
        }
    }

    /// Token that OTA requests should send, preferring [`DEV_TOKEN_ENV`] until
    /// [`Self::reject_effective`] suppresses it for this session.
    pub(crate) fn effective(&self) -> Option<SecretString> {
        if self.ignore_env {
            self.saved.clone()
        } else {
            self.from_env.clone().or_else(|| self.saved.clone())
        }
    }

    /// Returns whether the effective token is from [`AuthOrigin::Environment`]
    /// or [`AuthOrigin::Saved`].
    pub(crate) fn origin(&self) -> Option<AuthOrigin> {
        if !self.ignore_env && self.from_env.is_some() {
            Some(AuthOrigin::Environment)
        } else if self.saved.is_some() {
            Some(AuthOrigin::Saved)
        } else {
            None
        }
    }

    pub(crate) fn set_saved(&mut self, token: SecretString) {
        self.saved = Some(token);
    }

    /// Marks the currently effective token as rejected.
    ///
    /// [`AuthOrigin::Environment`] tokens are ignored for the rest of this
    /// session so a saved credential can still be used. [`AuthOrigin::Saved`]
    /// tokens are cleared from memory; the caller must delete the on-disk file
    /// with [`delete_token`] only when this returns [`AuthOrigin::Saved`].
    pub(crate) fn reject_effective(&mut self) -> Option<AuthOrigin> {
        let origin = self.origin()?;
        match origin {
            AuthOrigin::Environment => self.ignore_env = true,
            AuthOrigin::Saved => self.saved = None,
        }
        Some(origin)
    }
}

/// Loads a GitHub token for OTA authentication.
///
/// In debug builds, checks [`DEV_TOKEN_ENV`] first via [`token_from_env`] so
/// local emulator runs can skip device flow when `GH_OAUTH_CLIENT_ID` was not
/// baked in at compile time. Otherwise falls back to the token saved under
/// [`crate::device::DevicePaths::install_dir()`].
pub fn resolve_auth_token(install_dir: &std::path::Path) -> Result<Option<SecretString>, String> {
    Ok(ResolvedAuth::load(install_dir)?.effective())
}

/// Persists a GitHub OAuth token to disk for reuse across app restarts.
///
/// Writes [`TOKEN_FILENAME`] under [`crate::device::DevicePaths::install_dir()`]
/// with `0600` permissions.
///
/// # Errors
///
/// Returns an error string if directory creation or file write fails.
pub fn save_token(token: &SecretString, install_dir: &std::path::Path) -> Result<(), String> {
    let path = install_dir.join(TOKEN_FILENAME);
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
/// Reads [`TOKEN_FILENAME`] under [`crate::device::DevicePaths::install_dir()`].
/// Returns `None` if no token file exists (first-time setup).
///
/// # Errors
///
/// Returns an error string if the file exists but cannot be read.
pub fn load_token(install_dir: &std::path::Path) -> Result<Option<SecretString>, String> {
    let path = install_dir.join(TOKEN_FILENAME);
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
/// Removes [`TOKEN_FILENAME`] under [`crate::device::DevicePaths::install_dir()`].
/// Called when a saved token is found to be invalid or revoked, so the next
/// authentication attempt starts fresh via device flow.
///
/// Returns `Ok(())` if the file was deleted or did not exist.
///
/// # Errors
///
/// Returns an error string if the file exists but cannot be removed.
pub fn delete_token(install_dir: &std::path::Path) -> Result<(), String> {
    let path = install_dir.join(TOKEN_FILENAME);
    tracing::debug!(path = %path.display(), "Deleting GitHub token");

    if !path.exists() {
        return Ok(());
    }

    fs::remove_file(&path).map_err(|e| format!("Failed to delete token file: {}", e))?;
    tracing::info!("GitHub token deleted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    #[cfg(debug_assertions)]
    fn token_from_env_var_returns_set_value() {
        let token = token_from_env_var(Ok("env-token".to_owned())).expect("token");
        assert_eq!(token.expose_secret(), "env-token");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn token_from_env_var_rejects_empty_or_whitespace() {
        assert!(token_from_env_var(Ok("   ".to_owned())).is_none());
    }

    #[test]
    #[cfg(debug_assertions)]
    fn token_from_env_var_returns_none_when_unset() {
        assert!(token_from_env_var(Err(std::env::VarError::NotPresent)).is_none());
    }

    #[test]
    fn resolve_auth_prefers_env_over_saved_file() {
        let auth = ResolvedAuth::from_parts(
            Some(SecretString::from("file-token".to_owned())),
            Some(SecretString::from("env-token".to_owned())),
        );

        assert_eq!(auth.origin(), Some(AuthOrigin::Environment));
        assert_eq!(
            auth.effective().expect("token").expose_secret(),
            "env-token"
        );
    }

    #[test]
    fn resolve_auth_falls_back_to_saved_file() {
        let auth =
            ResolvedAuth::from_parts(Some(SecretString::from("file-token".to_owned())), None);

        assert_eq!(auth.origin(), Some(AuthOrigin::Saved));
        assert_eq!(
            auth.effective().expect("token").expose_secret(),
            "file-token"
        );
    }

    #[test]
    fn reject_env_token_keeps_saved_credential() {
        let mut auth = ResolvedAuth::from_parts(
            Some(SecretString::from("file-token".to_owned())),
            Some(SecretString::from("env-token".to_owned())),
        );

        assert_eq!(auth.reject_effective(), Some(AuthOrigin::Environment));
        assert_eq!(auth.origin(), Some(AuthOrigin::Saved));
        assert_eq!(
            auth.effective().expect("token").expose_secret(),
            "file-token"
        );
    }

    #[test]
    fn reject_env_token_without_saved_credential_leaves_none() {
        let mut auth =
            ResolvedAuth::from_parts(None, Some(SecretString::from("env-token".to_owned())));

        assert_eq!(auth.reject_effective(), Some(AuthOrigin::Environment));
        assert_eq!(auth.origin(), None);
        assert!(auth.effective().is_none());
    }

    #[test]
    fn reject_saved_token_clears_memory_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        save_token(&SecretString::from("file-token".to_owned()), tmp.path()).unwrap();

        let mut auth =
            ResolvedAuth::from_parts(Some(SecretString::from("file-token".to_owned())), None);

        assert_eq!(auth.reject_effective(), Some(AuthOrigin::Saved));
        assert!(auth.effective().is_none());
        assert!(
            load_token(tmp.path())
                .expect("load")
                .is_some_and(|token| token.expose_secret() == "file-token")
        );
    }

    fn unreadable_token_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join(TOKEN_FILENAME)).expect("token path is a directory");
        tmp
    }

    #[test]
    fn load_with_env_token_ignores_saved_token_read_error() {
        let tmp = unreadable_token_dir();
        let auth =
            ResolvedAuth::load_with(tmp.path(), Some(SecretString::from("env-token".to_owned())))
                .expect("load");

        assert_eq!(auth.origin(), Some(AuthOrigin::Environment));
        assert_eq!(
            auth.effective().expect("token").expose_secret(),
            "env-token"
        );
    }

    #[test]
    fn load_without_env_token_preserves_saved_token_read_error() {
        let tmp = unreadable_token_dir();
        assert!(ResolvedAuth::load_with(tmp.path(), None).is_err());
    }
}
