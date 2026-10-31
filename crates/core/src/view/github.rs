use secrecy::SecretString;

use super::Generation;

/// The background attempt a rejected token came from.
///
/// The list and install counters advance separately and start at the same
/// value, so a bare [`Generation`] can match the wrong attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenAttempt {
    /// The pull-request list and its install check.
    List(Generation),
    /// A download.
    Install(Generation),
}

/// Events emitted by GitHub authentication and API interactions.
#[derive(Debug, Clone)]
pub enum GithubEvent {
    /// Device flow completed successfully; carries the new access token.
    DeviceAuthComplete {
        /// The sign-in attempt this token belongs to.
        generation: Generation,
        /// The token GitHub issued.
        token: SecretString,
    },
    /// Device flow code expired before the user authorized.
    DeviceAuthExpired {
        /// The sign-in attempt that expired.
        generation: Generation,
    },
    /// Device flow failed with an error message.
    DeviceAuthError {
        /// The sign-in attempt that failed.
        generation: Generation,
        /// What went wrong.
        message: String,
    },
    /// A GitHub API call returned 401, or the token lacks a scope OTA needs.
    ///
    /// [`super::ota::OtaView`] drops the rejected token (environment tokens
    /// are ignored for the rest of the session; saved tokens are deleted) and
    /// either retries with a remaining credential or re-triggers device flow.
    TokenInvalid {
        /// The attempt the rejection belongs to.
        attempt: TokenAttempt,
    },
    /// The device-flow code is available, so the view can show it.
    DeviceAuthStarted {
        /// The sign-in attempt the code belongs to.
        generation: Generation,
        /// Where the user must go to authorize.
        verification_uri: String,
        /// The short code the user must enter.
        user_code: String,
    },
    /// The stable-release version check finished; carries the outcome.
    ///
    /// The check is a view-owned job holding a Wi-Fi lease, so it reports back
    /// here rather than blocking the main loop for the retry budget.
    StableReleaseCheck {
        /// The install attempt the check belongs to.
        generation: Generation,
        /// What the check concluded.
        outcome: StableReleaseCheckOutcome,
    },
}

/// What the stable-release version check concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StableReleaseCheckOutcome {
    /// The remote version is newer; a download should start.
    Older,
    /// Local and remote match.
    Equal,
    /// Local is newer than the published release.
    Newer,
    /// Versions could not be compared.
    Incomparable,
}
