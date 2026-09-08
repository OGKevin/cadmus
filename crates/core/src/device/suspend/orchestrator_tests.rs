use super::super::cycle::{SuspendCycle, SuspendKind};
use super::wake::PollResult;
use super::*;
use crate::device::inhibitor::{Kind, SoftSuspendName};
use crate::device::reschedule_auto_suspend_alarm;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::device::suspend::test_support::{
    install_armed_soft_suspend, intermission_count, lock_alarms, pump_deep_idle_wake,
};
use crate::device::suspend::{cancel_suspend_if_pending, has_task};
use crate::device::test_harness::DeviceRuntimeHarness;
use crate::frontlight::{Frontlight, LightLevel};
use crate::settings::IntermissionDisplay;
use chrono::TimeZone;
use std::time::Duration;

#[test]
fn prepare_for_sleep_schedules_suspend_rtc() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.push_task(DeviceTaskId::PrepareSuspend);
    harness.context.settings.wifi = crate::settings::WifiMode::AlwaysOn;
    harness.context.online = true;
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::Suspend));
    assert!(!harness.context.online);
    assert!(
        harness
            .context
            .device
            .wifi_manager_for_test()
            .was_disable_called()
    );
}

#[test]
fn prepare_for_sleep_turns_off_frontlight() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.frontlight = true;
    harness
        .context
        .device
        .frontlight_mut()
        .set_intensity(50.0.into())
        .unwrap();
    harness
        .context
        .device
        .frontlight_mut()
        .set_warmth(30.0.into())
        .unwrap();
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    let levels = harness.context.device.frontlight().levels();
    assert_eq!(levels.intensity, LightLevel::off());
    assert_eq!(levels.warmth, LightLevel::off());
}

#[test]
fn schedule_alarms_past_due_auto_power_off_exits() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_power_off = 1.0;
    lock_alarms(&mut harness)
        .schedule_in(AlarmType::AutoPowerOff, ChronoDuration::seconds(-10))
        .unwrap();
    let outcome = harness.with_runtime_only(schedule_alarms_before_sleep);
    assert_eq!(outcome, Some(EventOutcome::Exit(ExitStatus::PowerOff)));
}

#[test]
fn schedule_alarms_calendar_when_intermission_calendar() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    let outcome = harness.with_runtime_only(schedule_alarms_before_sleep);
    assert!(outcome.is_none());
    assert!(lock_alarms(&mut harness).has_alarm(AlarmType::CalendarUpdate));
}

#[test]
fn handle_post_wake_auto_power_off_exit() {
    let mut harness = DeviceRuntimeHarness::new();
    let before = Local::now();
    {
        lock_alarms(&mut harness)
            .schedule_in(AlarmType::AutoPowerOff, ChronoDuration::minutes(5))
            .unwrap();
        if let Ok(rtc) = harness.context.device.rtc() {
            let wake_at = rtc.scheduled_wake_time().expect("hardware wake programmed");
            rtc.set_current_time(wake_at);
            rtc.simulate_alarm_fired();
        }
    }
    let after = before + ChronoDuration::minutes(5) + ChronoDuration::seconds(1);
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_post_wake(before, after, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
}

#[test]
fn perform_suspend_resume_schedules_wake_debounce() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_before, _after) = harness.with_parts(|hub, _bus, _rq, context, runtime| {
        perform_suspend_resume(hub, context, runtime)
    });
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    let power = harness.context.device.power_manager_for_test();
    assert!(power.was_suspend_called());
    assert!(power.was_resume_called());
    assert_eq!(power.suspend_call_count(), 1);
    assert_eq!(power.resume_call_count(), 1);
}

#[test]
fn wake_debounce_classic_reenters_via_enter_sleep() {
    use crate::view::common::locate;
    use crate::view::intermission::Intermission;

    let mut harness = DeviceRuntimeHarness::new();
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime);
    });
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::Suspend),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert_eq!(
        harness
            .context
            .device
            .power_manager_for_test()
            .suspend_call_count(),
        1
    );

    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::WakeDebounce),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert_eq!(
        harness
            .view
            .children()
            .iter()
            .filter(|child| child.as_ref().is::<Intermission>())
            .count(),
        1
    );
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
    assert_eq!(
        harness
            .context
            .device
            .power_manager_for_test()
            .suspend_call_count(),
        2
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
}

#[test]
fn handle_rtc_auto_suspend_future_noop_when_not_fired_via_event() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 5.0;
    reschedule_auto_suspend_alarm(&mut harness.context);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[test]
fn handle_rtc_auto_suspend_fired_begins_suspend() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::AutoSuspend),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[test]
fn handle_rtc_auto_power_off_exits() {
    let mut harness = DeviceRuntimeHarness::new();
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::AutoPowerOff),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
}

#[test]
fn handle_rtc_auto_suspend_blocked_when_shared_reschedules() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    harness.context.shared = true;
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::AutoSuspend),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[test]
fn reschedule_auto_suspend_zero_cancels() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    reschedule_auto_suspend_alarm(&mut harness.context);
    assert!(lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
    harness.context.settings.auto_suspend = 0.0;
    reschedule_auto_suspend_alarm(&mut harness.context);
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
}

#[test]
fn reschedule_auto_suspend_moves_deadline_forward() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    reschedule_auto_suspend_alarm(&mut harness.context);
    let first = lock_alarms(&mut harness)
        .time_until_alarm(AlarmType::AutoSuspend)
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    reschedule_auto_suspend_alarm(&mut harness.context);
    let second = lock_alarms(&mut harness)
        .time_until_alarm(AlarmType::AutoSuspend)
        .unwrap();
    assert!(second >= first);
    assert!((second - 30 * 60).abs() < 2);
}

#[test]
fn reschedule_auto_suspend_sub_minute_clamps_nonzero() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 0.01;
    assert_eq!(
        (harness.context.settings.auto_suspend * 60.0) as i64,
        0,
        "precondition: whole-second cast truncates this setting to zero"
    );
    reschedule_auto_suspend_alarm(&mut harness.context);
    assert!(
        lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend),
        "sub-minute AutoSuspend must schedule a future alarm"
    );
    let until = lock_alarms(&mut harness)
        .time_until_alarm(AlarmType::AutoSuspend)
        .unwrap();
    assert!(until >= 0);
}

#[test]
fn start_cycle_cancels_auto_suspend_alarm() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    reschedule_auto_suspend_alarm(&mut harness.context);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[test]
fn cancel_prepare_suspend_reschedules_auto_suspend() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoSuspend));
    harness.with_parts(|hub, _bus, rq, context, runtime| {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[test]
fn cancel_suspend_rtc_reschedules_auto_suspend() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.tasks.clear();
    {
        let mut alarms = lock_alarms(&mut harness);
        alarms
            .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
            .unwrap();
    }
    harness.with_parts(|hub, _bus, rq, context, runtime| {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
}

#[test]
fn stale_suspend_rtc_after_cancel_skips_hardware_sleep() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.auto_suspend = 30.0;
    harness.context.settings.auto_power_off = 1.0;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime);
    });
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::Suspend));
    harness.with_parts(|hub, _bus, rq, context, runtime| {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::Suspend),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    let power = harness.context.device.power_manager_for_test();
    assert!(
        !power.was_suspend_called(),
        "stale Suspend after cancel must not call power.suspend"
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
    assert!(
        !lock_alarms(&mut harness).has_alarm(AlarmType::AutoPowerOff),
        "stale Suspend must not arm AutoPowerOff without sleeping"
    );
    assert!(
        !lock_alarms(&mut harness).has_alarm(AlarmType::CalendarUpdate),
        "stale Suspend must not arm CalendarUpdate without sleeping"
    );
}

#[test]
fn finish_cycle_clears_auto_power_off_before_post_wake_can_see_it() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    let before = Local::now() - ChronoDuration::minutes(10);
    {
        lock_alarms(&mut harness)
            .schedule_in(AlarmType::AutoPowerOff, ChronoDuration::minutes(-5))
            .unwrap();
        if let Ok(rtc) = harness.context.device.rtc() {
            rtc.simulate_alarm_fired();
        }
    }
    harness.with_parts(|hub, _bus, rq, context, runtime| {
        finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::AutoPowerOff));
    let after = Local::now();
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_post_wake(before, after, hub, bus, rq, context, runtime)
    });
    assert_eq!(
        outcome,
        EventOutcome::Handled,
        "finish_cycle cancels AutoPowerOff; post-wake must run first on deep idle"
    );
}

#[test]
fn classic_prepare_suspend_still_schedules_with_suspend_rtc() {
    let mut harness = DeviceRuntimeHarness::new();
    assert!(!harness.context.inhibitor.mode().is_armed());
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::Suspend));
}

#[test]
fn soft_start_cycle_acquires_cycle_lease() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_some_and(|c| c.holds_cycle_lease())
    );
    assert!(harness.context.inhibitor.has_holders());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[test]
fn soft_prepare_suspend_enters_deep_idle() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    pump_deep_idle_wake(&mut harness);
    assert!(!harness.context.inhibitor.has_holders());
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
}

#[test]
fn soft_deep_idle_has_no_holders_when_cycle_lease_dropped() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    assert!(harness.context.inhibitor.has_holders());
    if let Some(c) = harness.context.suspend.as_mut() {
        c.cycle_lease = None;
    }
    assert!(
        !harness.context.inhibitor.has_holders(),
        "cycle lease must be the last soft-suspend holder before deep-idle wait"
    );
}

#[test]
fn soft_deep_idle_forces_mem_without_state_mem_write() {
    use std::fs;

    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    pump_deep_idle_wake(&mut harness);
    let power = harness.context.device.power_manager_for_test();
    assert!(!power.was_suspend_called());
    assert!(power.arm_deep_idle_call_count() >= 1);
    assert!(power.disarm_deep_idle_call_count() >= 1);
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
    let autosleep = fs::read_to_string(&paths.autosleep).expect("autosleep");
    assert!(autosleep.trim() == "freeze" || autosleep.trim() == "Freeze");
}

#[test]
fn soft_deep_idle_schedules_wake_debounce_alarm() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.auto_suspend = 30.0;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(
        !lock_alarms(&mut harness).has_alarm(AlarmType::Suspend),
        "deep-idle prepare enters sleep immediately; no Suspend RTC"
    );
    pump_deep_idle_wake(&mut harness);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(
        !lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend),
        "AutoSuspend stays cancelled until valid-wake finish_cycle"
    );
    assert!(
        !harness
            .context
            .device
            .power_manager_for_test()
            .was_suspend_called()
    );
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
}

#[test]
fn soft_deep_idle_wake_debounce_fired_begins_suspend() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.auto_suspend = 30.0;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    pump_deep_idle_wake(&mut harness);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert_eq!(intermission_count(harness.view.as_ref()), 1);

    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::WakeDebounce),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::WakeDebounce));
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
}

#[test]
fn soft_deep_idle_calendar_wake_keeps_suspend_intermission() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    pump_deep_idle_wake(&mut harness);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());

    let before = Local::now() - ChronoDuration::minutes(10);
    {
        lock_alarms(&mut harness)
            .schedule_in(AlarmType::CalendarUpdate, ChronoDuration::minutes(-5))
            .unwrap();
        if let Ok(rtc) = harness.context.device.rtc() {
            rtc.simulate_alarm_fired();
        }
    }
    let after = Local::now();
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_post_wake(before, after, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        locate::<Intermission>(harness.view.as_ref()).is_some(),
        "calendar refresh must keep suspend intermission; finish_cycle would remove it"
    );
    assert!(
        !has_task(&harness.tasks, DeviceTaskId::PrepareSuspend),
        "CalendarUpdate must re-enter sleep without a second prepare"
    );
    assert!(
        has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait),
        "CalendarUpdate must re-enter deep-idle wait"
    );
    assert!(
        lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::CalendarUpdate),
        "next CalendarUpdate must be re-armed"
    );
}

#[test]
fn soft_deep_idle_timeout_retries_without_finishing_cycle() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.context.settings.auto_suspend = 30.0;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::CalendarUpdate));

    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::TimedOut);
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness.context.suspend.is_some(),
        "timeout must retry, not finish the suspend cycle"
    );
    assert!(
        locate::<Intermission>(harness.view.as_ref()).is_some(),
        "intermission must survive timeout retry"
    );
    assert!(
        lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::CalendarUpdate),
        "CalendarUpdate must remain scheduled across retry"
    );
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    assert!(
        !lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend),
        "AutoSuspend must stay cancelled while retrying"
    );

    pump_deep_idle_wake(&mut harness);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
}

#[test]
fn soft_deep_idle_wait_succeeds_with_input_lease_holders() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let _input_leases: Vec<_> = (0..8)
        .map(|_| {
            harness
                .context
                .inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::Input)
        })
        .collect();
    assert!(
        !harness.context.inhibitor.is_empty(),
        "input leases must pin wake_lock like a hub backlog"
    );
    pump_deep_idle_wake(&mut harness);
    assert!(
        harness.context.suspend.is_none()
            || locate::<Intermission>(harness.view.as_ref()).is_some(),
        "wait completion must not give up solely because input leases were held"
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
}

#[test]
fn idle_soft_suspend_input_lease_keeps_holders() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    assert!(harness.context.suspend.is_none());
    let _lease = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Input);
    assert!(
        harness.context.inhibitor.has_holders(),
        "interactive input leases must still block opportunistic soft-suspend"
    );
}

#[test]
fn five_minute_boundary_mid_window() {
    let now = Local
        .with_ymd_and_hms(2026, 8, 11, 10, 12, 30)
        .single()
        .expect("valid local time");
    assert_eq!(seconds_until_next_five_minute_boundary(&now), 151);
}

#[test]
fn five_minute_boundary_near_end() {
    let now = Local
        .with_ymd_and_hms(2026, 8, 11, 10, 14, 59)
        .single()
        .expect("valid local time");
    assert_eq!(seconds_until_next_five_minute_boundary(&now), 2);
}

#[test]
fn calendar_rearm_schedules_relative_from_system_boundary() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;

    schedule_next_calendar_update(&mut harness.context);

    assert!(
        lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::CalendarUpdate),
        "system boundary duration must schedule CalendarUpdate via schedule_in"
    );
}

#[test]
fn soft_rtc_calendar_update_rearms_and_reenters() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    pump_deep_idle_wake(&mut harness);
    lock_alarms(&mut harness)
        .cancel_alarm(AlarmType::WakeDebounce)
        .unwrap();
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::WakeDebounce));

    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::CalendarUpdate),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::CalendarUpdate));
}

#[test]
fn soft_calendar_update_during_insleep_preserves_deep_idle_restore() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Mem);
    assert_eq!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_restore),
        Some(AutosleepMode::Freeze)
    );

    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::CalendarUpdate),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
    assert_eq!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_restore),
        Some(AutosleepMode::Freeze),
        "CalendarUpdate reentry must keep the pre-Mem restore mode"
    );
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Mem);

    harness.with_parts(|hub, _bus, rq, context, runtime| {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.inhibitor.mode(),
        AutosleepMode::Freeze,
        "finish must restore Freeze, not leave Mem"
    );
}

#[test]
fn classic_rtc_calendar_update_rearms_and_reenters() {
    let mut harness = DeviceRuntimeHarness::new();
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.context.suspend = Some(SuspendCycle::new(SuspendKind::Classic));
    harness.with_parts(|hub, bus, rq, context, runtime| {
        let interm = Intermission::new(
            context.device.framebuffer().rect(),
            IntermKind::Suspend,
            context,
        );
        runtime.view.children_mut().push(Box::new(interm));
        let _ = (hub, bus, rq);
    });

    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::CalendarUpdate),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::CalendarUpdate));
    assert!(
        lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce)
            || lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::Suspend)
            || harness
                .context
                .device
                .power_manager_for_test()
                .was_suspend_called(),
        "classic CalendarUpdate must re-enter sleep"
    );
}

#[test]
fn soft_armed_classic_suspend_refused_without_cycle_lease() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.auto_suspend = 30.0;
    if let Some(c) = harness.context.suspend.as_mut() {
        c.cycle_lease = None;
    }

    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        !harness
            .context
            .device
            .power_manager_for_test()
            .was_suspend_called()
    );
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::Suspend));
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[test]
fn soft_cancel_suspend_drops_cycle_lease_and_restores_mode() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.auto_suspend = 30.0;
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.tasks.clear();
    {
        let mut alarms = lock_alarms(&mut harness);
        alarms
            .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
            .unwrap();
    }
    harness.with_parts(|hub, _bus, rq, context, runtime| {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[test]
fn deep_idle_reentry_preserves_frontlight_levels() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.frontlight = true;
    harness.context.settings.auto_suspend = 30.0;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness
        .context
        .device
        .frontlight_mut()
        .set_intensity(50.0.into())
        .unwrap();
    harness
        .context
        .device
        .frontlight_mut()
        .set_warmth(30.0.into())
        .unwrap();

    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let off = harness.context.device.frontlight().levels();
    assert_eq!(off.intensity, LightLevel::off());
    assert_eq!(off.warmth, LightLevel::off());
    assert_eq!(
        harness.context.settings.frontlight_levels.intensity,
        LightLevel::from(50.0)
    );

    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::CalendarUpdate),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert_eq!(
        harness.context.settings.frontlight_levels.intensity,
        LightLevel::from(50.0),
        "reentry must not overwrite saved frontlight levels with off"
    );

    harness.with_parts(|hub, _bus, rq, context, runtime| {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq);
    });

    assert!(harness.context.suspend.is_none());
    let levels = harness.context.device.frontlight().levels();
    assert_eq!(levels.intensity, LightLevel::from(50.0));
    assert_eq!(levels.warmth, LightLevel::from(30.0));
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[test]
fn suspend_during_deep_idle_wait_does_not_finish_cycle() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(harness.context.suspend.is_some());
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some()
    );
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
}

#[test]
fn deep_idle_timeout_cannot_rearm_finishes_cycle() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    harness.context.inhibitor.set_mode(AutosleepMode::Off);
    if let Some(cycle) = harness.context.suspend.as_mut() {
        cycle.deep_idle_restore = Some(AutosleepMode::Off);
    }
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::TimedOut);
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(harness.context.suspend.is_none());
    assert!(locate::<Intermission>(harness.view.as_ref()).is_none());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
}

#[test]
fn wake_detect_inject_woke_without_realtime_step() {
    let mut harness = DeviceRuntimeHarness::new();
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.with_parts(|hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks);
    });
    harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::Woke);
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_none()
    );
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
}
