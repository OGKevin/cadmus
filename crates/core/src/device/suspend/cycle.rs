//! Explicit suspend cycle kind and phase.
//!
//! An active cycle is stored as [`Option`]`<`[`SuspendCycle`]`>` on
//! [`crate::device::AppContext`]. Interactive use is `None`. Kind is chosen
//! once in [`super::orchestrator::start_cycle`] and must not be re-selected by
//! mid-cycle `is_armed()` probes.

use super::orchestrator::{ClassicCommit, DeferredSuspend};
use crate::device::inhibitor::InhibitorGuard;
use crate::device::inhibitor::soft_suspend::SuppressionEpoch;
#[cfg(target_os = "linux")]
use crate::device::linux::wake_telemetry::InterruptSnapshot;
use crate::device::rtc::PreSleepAlarms;
use crate::device::soft_suspend::mode::AutosleepMode;
#[cfg(target_os = "linux")]
use std::sync::Arc;
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
    /// Whether the sleep cover was closed when the wait began.
    ///
    /// A wake with this set and the cover open was caused by opening the cover.
    covered_before_sleep: bool,
    /// Alarm state captured immediately before deep-idle entry.
    pre_sleep_alarms: PreSleepAlarms,
    #[cfg(target_os = "linux")]
    interrupt_baseline: Option<Arc<InterruptSnapshot>>,
}

impl DeepIdleWaitState {
    /// Samples boottime and monotonic clocks at deep-idle wait start.
    pub(in crate::device::suspend) fn capture(
        pre_sleep_alarms: PreSleepAlarms,
        covered: bool,
        #[cfg(target_os = "linux")] interrupt_baseline: Option<Arc<InterruptSnapshot>>,
    ) -> Self {
        Self {
            boot_anchor: super::wake::clock_boottime(),
            mono_anchor: Instant::now(),
            covered_before_sleep: covered,
            pre_sleep_alarms,
            #[cfg(target_os = "linux")]
            interrupt_baseline,
        }
    }

    pub(in crate::device::suspend) fn boot_anchor(&self) -> Option<Duration> {
        self.boot_anchor
    }

    pub(in crate::device::suspend) fn mono_anchor(&self) -> Instant {
        self.mono_anchor
    }

    pub(in crate::device::suspend) fn pre_sleep_alarms(&self) -> PreSleepAlarms {
        self.pre_sleep_alarms
    }

    pub(in crate::device::suspend) fn covered_before_sleep(&self) -> bool {
        self.covered_before_sleep
    }

    #[cfg(target_os = "linux")]
    pub(in crate::device::suspend) fn interrupt_baseline(&self) -> Option<Arc<InterruptSnapshot>> {
        self.interrupt_baseline.clone()
    }

    #[cfg(test)]
    pub(in crate::device::suspend) fn synthetic(
        boot_anchor: Option<Duration>,
        mono_anchor: Instant,
    ) -> Self {
        Self {
            boot_anchor,
            mono_anchor,
            covered_before_sleep: false,
            pre_sleep_alarms: PreSleepAlarms::default(),
            #[cfg(target_os = "linux")]
            interrupt_baseline: None,
        }
    }
}

/// Single source of truth for an in-progress explicit suspend.
pub(crate) struct SuspendCycle {
    pub(in crate::device::suspend) kind: SuspendKind,
    /// Whether this cycle came from Auto Suspend or an explicit request.
    /// Kept so a Full abort can resume the same intent instead of inventing an
    /// explicit one.
    pub(in crate::device::suspend) origin: DeferredSuspend,
    pub(in crate::device::suspend) phase: SuspendPhase,
    /// DeepIdle lease while preparing / before wait; `None` during `InSleep`.
    pub(in crate::device::suspend) cycle_lease: Option<InhibitorGuard>,
    /// Autosleep mode to restore after a DeepIdle Mem force.
    pub(in crate::device::suspend) deep_idle_restore: Option<AutosleepMode>,
    /// Epoch of the wake-lock suppression owned by the current sleep leg.
    /// The first exit (wake, timeout, cancel, or failure) consumes it and
    /// restores the lock, so a later cycle finish does not restore twice.
    /// Poll also uses it to detect a transient Full interruption.
    pub(in crate::device::suspend) wake_lock_epoch: Option<SuppressionEpoch>,
    /// Shared completion for the owned Classic commit, when one is in flight.
    /// Cleanup drains this before touching peripherals or disarming hardware.
    pub(in crate::device::suspend) classic_commit: Option<std::sync::Arc<ClassicCommit>>,
    /// Consecutive deep-idle wait timeouts in this cycle; caps autosleep
    /// retries before the Classic fallback.
    pub(in crate::device::suspend) consecutive_deep_idle_timeouts: u32,
}

impl SuspendCycle {
    /// New cycle in [`SuspendPhase::Preparing`] with no lease yet.
    pub(in crate::device::suspend) fn new(kind: SuspendKind, origin: DeferredSuspend) -> Self {
        Self {
            kind,
            origin,
            phase: SuspendPhase::Preparing,
            cycle_lease: None,
            deep_idle_restore: None,
            wake_lock_epoch: None,
            classic_commit: None,
            consecutive_deep_idle_timeouts: 0,
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
