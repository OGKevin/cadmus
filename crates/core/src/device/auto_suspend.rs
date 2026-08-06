//! Auto Suspend idle-deadline RTC scheduling.
//!
//! Shared by the Kobo suspend orchestrator, emulator lifecycle, and power
//! settings. Schedules [`crate::AlarmType::AutoSuspend`] from
//! `settings.auto_suspend` (minutes).

use crate::AlarmType;
use crate::chrono::Duration as ChronoDuration;
use crate::device::AppContext;

/// Converts Auto Suspend minutes to a chrono duration of at least one second.
fn auto_suspend_chrono_duration(minutes: f32) -> ChronoDuration {
    let secs = (minutes * 60.0).max(0.0);
    let duration = ChronoDuration::from_std(std::time::Duration::from_secs_f32(secs))
        .unwrap_or_else(|_| ChronoDuration::milliseconds(((secs * 1000.0) as i64).max(1)));
    if duration.num_seconds() < 1 {
        ChronoDuration::seconds(1)
    } else {
        duration
    }
}

/// Schedules or cancels [`AlarmType::AutoSuspend`] from the Auto Suspend setting.
///
/// When `auto_suspend` is `0`, cancels any pending alarm. Otherwise replaces the
/// alarm with a wake time of *now + timeout* (setting is minutes). Activity and
/// cycle end call this to keep the idle deadline aligned with real wall-clock idle.
pub(crate) fn reschedule_auto_suspend_alarm(context: &mut AppContext) {
    let Some(alarm_manager) = context.alarm_manager.as_ref() else {
        return;
    };
    let mut alarm_manager = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());

    let minutes = context.settings.auto_suspend;
    if minutes <= 0.0 {
        if let Err(error) = alarm_manager.cancel_alarm(AlarmType::AutoSuspend) {
            tracing::error!(error = %error, "failed to cancel AutoSuspend alarm");
        }
        return;
    }

    let duration = auto_suspend_chrono_duration(minutes);
    if let Err(error) = alarm_manager.schedule_in(AlarmType::AutoSuspend, duration) {
        tracing::error!(error = %error, "failed to schedule AutoSuspend alarm");
    }
}
