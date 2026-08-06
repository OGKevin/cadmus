//! Explicit suspend cycle kind and phase.
//!
//! An active cycle is stored as [`Option`]`<`[`SuspendCycle`]`>` on
//! [`crate::device::AppContext`]. Interactive use is `None`. Kind is chosen
//! once in [`super::orchestrator::start_cycle`] and must not be re-selected by
//! mid-cycle `is_armed()` probes.

use crate::device::soft_suspend::{AutosleepMode, SoftSuspendLease};
use chrono::{DateTime, Local};
use std::time::{Duration, Instant};

/// Sleep backend for one explicit suspend cycle.
///
/// Selected when the cycle starts from soft-suspend armedness (`DeepIdle` when
/// armed, otherwise `Classic`) and kept for WakeDebounce / CalendarUpdate
/// re-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::device::suspend) enum SuspendKind {
    /// Blocking [`crate::device::power::PowerManager::suspend`] / `resume`.
    Classic,
    /// Autosleep `mem` + vendor deep-idle prep + non-blocking wait poll.
    DeepIdle,
}

/// Progress through an explicit suspend cycle.
///
/// Transitions are driven by [`super::orchestrator`]:
/// `Preparing` → `ArmingSleep` → (`InSleep` for DeepIdle) → `PostWakeDebounce`.
#[derive(Debug, Clone)]
pub(in crate::device::suspend) enum SuspendPhase {
    /// Intermission shown; waiting for [`crate::view::Event::PrepareSuspend`].
    Preparing,
    /// Teardown done; Classic waits on Suspend RTC, DeepIdle enters sleep next.
    ArmingSleep,
    /// DeepIdle non-blocking wait (lease dropped; poll for wake/timeout).
    InSleep { wait: DeepIdleWaitState },
    /// Left sleep; WakeDebounce / CalendarUpdate may re-enter or user cancels.
    PostWakeDebounce,
}

/// Suspend-aware anchors for a non-blocking deep-idle wait.
///
/// Uses boot-time and monotonic clocks (not realtime) so NTP steps do not look
/// like wakes. See [`super::wake`].
#[derive(Debug, Clone)]
pub(in crate::device::suspend) struct DeepIdleWaitState {
    /// `CLOCK_BOOTTIME` sample when the wait begins; `None` if the sample failed.
    boot_anchor: Option<Duration>,
    /// Monotonic clock sample captured when the deep-idle wait begins.
    mono_anchor: Instant,
    /// Wall-clock time when sleep began; used for post-wake RTC alarm claims.
    sleep_started_at: DateTime<Local>,
}

impl DeepIdleWaitState {
    /// Samples boottime and monotonic clocks at deep-idle wait start.
    pub(in crate::device::suspend) fn capture(sleep_started_at: DateTime<Local>) -> Self {
        Self {
            boot_anchor: super::wake::clock_boottime(),
            mono_anchor: Instant::now(),
            sleep_started_at,
        }
    }

    pub(in crate::device::suspend) fn boot_anchor(&self) -> Option<Duration> {
        self.boot_anchor
    }

    pub(in crate::device::suspend) fn mono_anchor(&self) -> Instant {
        self.mono_anchor
    }

    pub(in crate::device::suspend) fn sleep_started_at(&self) -> DateTime<Local> {
        self.sleep_started_at
    }

    #[cfg(test)]
    pub(in crate::device::suspend) fn synthetic(
        boot_anchor: Option<Duration>,
        mono_anchor: Instant,
        sleep_started_at: DateTime<Local>,
    ) -> Self {
        Self {
            boot_anchor,
            mono_anchor,
            sleep_started_at,
        }
    }
}

/// Single source of truth for an in-progress explicit suspend.
pub(crate) struct SuspendCycle {
    pub(in crate::device::suspend) kind: SuspendKind,
    pub(in crate::device::suspend) phase: SuspendPhase,
    /// DeepIdle lease while preparing / before wait; `None` during `InSleep`.
    pub(in crate::device::suspend) cycle_lease: Option<SoftSuspendLease>,
    /// Autosleep mode to restore after a DeepIdle Mem force.
    pub(in crate::device::suspend) deep_idle_restore: Option<AutosleepMode>,
}

impl SuspendCycle {
    /// New cycle in [`SuspendPhase::Preparing`] with no lease yet.
    pub(in crate::device::suspend) fn new(kind: SuspendKind) -> Self {
        Self {
            kind,
            phase: SuspendPhase::Preparing,
            cycle_lease: None,
            deep_idle_restore: None,
        }
    }

    /// True while Suspend events must be ignored (already asleep or debounce).
    pub(in crate::device::suspend) fn is_in_sleep_or_debounce(&self) -> bool {
        matches!(
            self.phase,
            SuspendPhase::InSleep { .. } | SuspendPhase::PostWakeDebounce
        )
    }

    /// DeepIdle wait anchors when phase is [`SuspendPhase::InSleep`].
    pub(in crate::device::suspend) fn deep_idle_wait(&self) -> Option<&DeepIdleWaitState> {
        match &self.phase {
            SuspendPhase::InSleep { wait } => Some(wait),
            _ => None,
        }
    }

    /// Whether the DeepIdle cycle wake lock is currently held.
    pub(in crate::device::suspend) fn holds_cycle_lease(&self) -> bool {
        self.cycle_lease.is_some()
    }

    /// Whether the main loop should skip a nested soft-suspend lease for `event`.
    pub(crate) fn should_skip_main_loop_lease(&self, event: &crate::view::Event) -> bool {
        matches!(self.phase, SuspendPhase::InSleep { .. })
            || (self.holds_cycle_lease()
                && matches!(
                    event,
                    crate::view::Event::PrepareSuspend
                        | crate::view::Event::Suspend
                        | crate::view::Event::PollDeepIdleWait
                ))
    }
}
