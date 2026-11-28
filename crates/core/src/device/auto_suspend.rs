//! Auto Suspend idle-deadline RTC scheduling.
//!
//! Shared by the Kobo suspend orchestrator, emulator lifecycle, and power
//! settings. Schedules [`crate::AlarmType::AutoSuspend`] from
//! `settings.auto_suspend` (minutes).

use crate::AlarmType;
use crate::chrono::Duration as ChronoDuration;
use crate::device::AppContext;
#[cfg(any(feature = "kobo", docsrs))]
use crate::device::suspend::clear_deferred_suspend;

/// Converts a second count to a chrono duration of at least one second.
pub(crate) fn secs_to_chrono_duration(secs: f32) -> ChronoDuration {
    let secs = secs.max(0.0);
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
///
/// On Kobo this also clears a queued deferred explicit suspend without
/// starting a cycle — user activity means that deferred sleep is stale.
pub(crate) async fn reschedule_auto_suspend_alarm(context: &mut AppContext) {
    #[cfg(any(feature = "kobo", docsrs))]
    clear_deferred_suspend(context, "auto_suspend_reschedule");
    let Some(alarm_manager) = context.alarm_manager.as_ref().cloned() else {
        return;
    };
    let minutes = context.settings.auto_suspend;
    crate::device::rtc::alarm_transaction(&alarm_manager, async move |mut alarm_manager| {
        if minutes <= 0.0 {
            if let Err(error) = alarm_manager.cancel_alarm(AlarmType::AutoSuspend).await {
                tracing::error!(error = %error, "failed to cancel AutoSuspend alarm");
            }
            return;
        }

        let duration = secs_to_chrono_duration(minutes * 60.0);
        if let Err(error) = alarm_manager
            .schedule_in(AlarmType::AutoSuspend, duration)
            .await
        {
            tracing::error!(error = %error, "failed to schedule AutoSuspend alarm");
        }
    })
    .await;
}
