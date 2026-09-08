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
//!     │ finish_cycle() | cancel_prepare() | start_cycle() again
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
//! | `cancel_prepare` | Abort during PrepareSuspend only |
//!
//! [`Event::Suspend`] / Suspend RTC mean **enter sleep now** (after prepare),
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
//! # Full inhibit
//!
//! [`crate::device::inhibitor::Kind::Full`] does **not** cancel an in-flight
//! cycle. It blocks **starting** one: [`start_cycle`] sets
//! [`Context::deferred_suspend`](crate::context::Context::deferred_suspend)
//! and returns. When the last Full holder drops, Kobo posts
//! [`Event::FullInhibitCleared`](crate::view::Event::FullInhibitCleared) and
//! the orchestrator flushes that intent. OTA success sends
//! [`Event::ClearDeferredSuspend`](crate::view::Event::ClearDeferredSuspend)
//! first so reboot is not raced by a flush.
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
pub(crate) use orchestrator::{
    clear_deferred_suspend, handle_event, is_suspend_rtc_pending, show_power_off_intermission,
    start_cycle,
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
