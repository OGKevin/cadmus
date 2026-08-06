//! Deep-idle wake detection: boottime − monotonic, with a test inject seam.
//!
//! Production must not treat realtime / NTP clock steps as wake. Prefer
//! `CLOCK_BOOTTIME` elapsed minus monotonic elapsed. Unit tests inject
//! [`PollResult`] via [`AppContext::deep_idle_poll_inject`] without advancing
//! clocks.

use super::cycle::DeepIdleWaitState;
use crate::device::AppContext;
use std::time::Duration;

/// Outcome of one deep-idle wait poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollResult {
    StillWaiting,
    Woke,
    TimedOut,
}

const WAKE_THRESHOLD: Duration = Duration::from_secs(1);
#[cfg_attr(test, allow(dead_code))]
const DEEP_IDLE_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Resolves deep-idle wait using the test inject queue when present, else
/// production boottime−monotonic detection.
pub(in crate::device::suspend) fn resolve_wait(
    context: &mut AppContext,
    state: &DeepIdleWaitState,
) -> PollResult {
    #[cfg(test)]
    {
        let _ = state;
        if let Some(injected) = context.deep_idle_poll_inject.pop_front() {
            return injected;
        }
        PollResult::Woke
    }
    #[cfg(not(test))]
    {
        let _ = context;
        production_resolve(state)
    }
}

fn production_resolve(state: &DeepIdleWaitState) -> PollResult {
    let mono = state.mono_anchor().elapsed();
    let (Some(boot_now), Some(boot_anchor)) = (clock_boottime(), state.boot_anchor()) else {
        if mono > DEEP_IDLE_WAIT_TIMEOUT {
            return PollResult::TimedOut;
        }
        return PollResult::StillWaiting;
    };
    let boot_elapsed = boot_now.saturating_sub(boot_anchor);
    if boot_elapsed > mono + WAKE_THRESHOLD {
        return PollResult::Woke;
    }
    if mono > DEEP_IDLE_WAIT_TIMEOUT {
        return PollResult::TimedOut;
    }
    PollResult::StillWaiting
}

/// Reads `CLOCK_BOOTTIME` as a duration since an unspecified epoch.
pub(in crate::device::suspend) fn clock_boottime() -> Option<Duration> {
    #[cfg(unix)]
    {
        use nix::time::{ClockId, clock_gettime};
        match clock_gettime(ClockId::CLOCK_BOOTTIME) {
            Ok(ts) => Some(Duration::new(ts.tv_sec() as u64, ts.tv_nsec() as u32)),
            Err(error) => {
                tracing::warn!(error = %error, "CLOCK_BOOTTIME read failed; deep-idle wake uses monotonic timeout only");
                None
            }
        }
    }
    #[cfg(not(unix))]
    {
        tracing::warn!(
            "CLOCK_BOOTTIME unavailable on this platform; deep-idle wake uses monotonic timeout only"
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use std::time::Instant;

    #[test]
    fn production_no_gap_still_waiting() {
        let state = DeepIdleWaitState::capture(Local::now());
        assert_eq!(production_resolve(&state), PollResult::StillWaiting);
    }

    #[test]
    fn production_boot_ahead_of_mono_is_wake() {
        let sleep_started_at = Local::now();
        let boot_now = clock_boottime().unwrap_or(Duration::from_secs(10));
        let synthetic = DeepIdleWaitState::synthetic(
            Some(boot_now.saturating_sub(Duration::from_secs(5))),
            Instant::now(),
            sleep_started_at,
        );
        assert_eq!(production_resolve(&synthetic), PollResult::Woke);
    }

    #[test]
    fn missing_boot_anchor_does_not_false_wake() {
        let synthetic = DeepIdleWaitState::synthetic(None, Instant::now(), Local::now());
        assert_eq!(production_resolve(&synthetic), PollResult::StillWaiting);
    }
}
