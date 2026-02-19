use secrecy::SecretString;

/// Events emitted by GitHub authentication and API interactions.
#[derive(Debug, Clone)]
pub enum GithubEvent {
    /// Device flow completed successfully; carries the new access token.
    DeviceAuthComplete(SecretString),
    /// Device flow code expired before the user authorized.
    DeviceAuthExpired,
    /// Device flow failed with an error message.
    DeviceAuthError(String),
    /// A GitHub API call returned 401 or 403 — the saved token is invalid,
    /// revoked, or missing required scopes.
    ///
    /// `OtaView` handles this by deleting the stale token, clearing its
    /// in-memory token, and re-triggering device flow for the pending download.
    TokenInvalid,
}
