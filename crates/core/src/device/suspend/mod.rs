//! Shared explicit-suspend orchestrator.
//!
//! Owns [`SuspendCycle`] (kind + phase), Classic / DeepIdle sleep entry,
//! deep-idle wake detect, and the cycle API below. Kobo
//! [`crate::device::kobo`] lifecycle forwards suspend events here. Emulator
//! builds keep a short UI-only suspend path and do not drive this module.
//!
//! # Workflow (phase state machine)
//!
//! Callers that mean “go to sleep” (power button, AutoSuspend RTC, sleep cover)
//! invoke [`start_cycle`], **not** sleep entry. Sleep is a later phase.
//!
//! ```text
//! Interactive
//!     │ start_cycle()          // kind Classic|DeepIdle fixed here
//!     ▼
//! Preparing                    // intermission up; PrepareSuspend task
//!     │ prepare_for_sleep()    // teardown wifi/frontlight/settings
//!     ▼
//! ArmingSleep
//!     │ Classic: Suspend RTC → enter_sleep()
//!     │ DeepIdle: enter_sleep() immediately
//!     ▼
//! InSleep                      // Classic: blocked in power.suspend()
//!     │ DeepIdle: PollDeepIdleWait until woke|timeout
//!     ▼
//! PostWakeDebounce             // WakeDebounce / CalendarUpdate may reenter
//!     │ finish_cycle() | start_cycle() again
//!     ▼
//! Interactive
//! ```
//!
//! | Function | Role |
//! |----------|------|
//! | [`start_cycle`] | Begin a cycle: UI + schedule prepare |
//! | `prepare_for_sleep` | Shared teardown; then arm classic RTC or enter DeepIdle |
//! | `enter_sleep` | Actually sleep (classic `power.suspend` or deep-idle wait) |
//! | `finish_cycle` | Tear down cycle → interactive |
//!
//! [`Event::Suspend`](crate::view::Event::Suspend) / Suspend RTC mean **enter sleep now** (after prepare),
//! not “start a new cycle”. Those handlers live in [`orchestrator`] and are
//! reached via [`handle_event`].
//!
//! Auto Suspend idle scheduling lives in [`crate::device::reschedule_auto_suspend_alarm`].
//!
//! # Kind vs opportunistic soft nap
//!
//! Soft suspend **freeze** (or settings `mem`) is the light opportunistic nap
//! between UI events while Cadmus holds named leases. **Deep idle** is the
//! explicit Auto Suspend / power-button / sleep-cover path once soft suspend is
//! armed. Classic hard suspend remains when soft suspend is off.
//!
//! # Inhibitor kinds and deferred suspend
//!
//! [`crate::device::inhibitor::Kind::Full`] blocks **starting** a cycle:
//! [`start_cycle`] sets
//! [`Context::deferred_suspend`](crate::context::Context::deferred_suspend)
//! and returns. If a worker acquires Full after a cycle has started, the
//! orchestrator aborts before kernel sleep during DeepIdle or before Classic
//! gate acquisition. After a Classic gate is taken, Full acquisition waits
//! until wake and cleanup before the cycle aborts. The abort preserves the
//! cycle's origin and any newer intent, with Explicit winning over Auto, and
//! flushes it immediately if Full is already gone. When the last Full holder
//! otherwise drops, Kobo posts
//! [`Event::InhibitCleared`](crate::view::Event::InhibitCleared) and the
//! orchestrator flushes that intent: Auto origin rechecks Sleep via
//! [`start_cycle_from_auto`](orchestrator::start_cycle_from_auto); Explicit
//! origin still runs `start_cycle`. OTA success sends
//! [`Event::ClearDeferredSuspend`](crate::view::Event::ClearDeferredSuspend)
//! first so reboot is not raced by a flush.
//!
//! [`crate::device::inhibitor::Kind::Sleep`] defers only the Auto Suspend path
//! ([`start_cycle_from_auto`](orchestrator::start_cycle_from_auto)). Explicit
//! `start_cycle` still runs. The last Sleep release flushes a deferred Auto
//! through that path, which re-defers while a Full or Sleep inhibition lease is held; a
//! deferred Explicit waits for the Full release.
//!
//! [`crate::device::reschedule_auto_suspend_alarm`] also clears deferred
//! intent (user activity) without starting a cycle.

mod cycle;
mod helpers;
mod orchestrator;
mod wake;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use cycle::SuspendCycle;
#[cfg(test)]
pub(crate) use helpers::has_task;
pub(crate) use helpers::{cancel_suspend_if_pending, is_suspend_active};
pub use orchestrator::drain_committed_suspend;
pub(crate) use orchestrator::{
    DeferredSuspend, clear_deferred_suspend, handle_event, is_suspend_rtc_pending,
    is_suspend_rtc_pending_in, show_power_off_intermission, start_cycle,
};
#[cfg(test)]
pub(crate) use wake::PollResult;

use std::time::Duration;

/// Delay before [`crate::view::Event::PrepareSuspend`] on the classic hard-suspend path.
/// Soft deep idle skips this and schedules prepare immediately.
const PREPARE_SUSPEND_WAIT_DELAY: Duration = Duration::from_secs(3);

/// Delay for [`crate::AlarmType::Suspend`] after prepare and for
/// [`crate::AlarmType::WakeDebounce`] after leave-sleep.
const SUSPEND_WAIT_DELAY: Duration = Duration::from_secs(15);
