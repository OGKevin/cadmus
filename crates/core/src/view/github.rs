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
    /// A GitHub API call returned 401 or 403 — the token in use is invalid,
    /// revoked, or missing required scopes.
    ///
    /// [`super::ota::OtaView`] drops the rejected token (environment tokens
    /// are ignored for the rest of the session; saved tokens are deleted) and
    /// either retries with a remaining credential or re-triggers device flow.
    TokenInvalid,
}
