use super::super::cycle::{SuspendCycle, SuspendKind, SuspendPhase};
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
use std::path::Path;
use std::time::Duration;

async fn wake_node(path: &Path) -> String {
    tokio::fs::read_to_string(path)
        .await
        .expect("read wake node")
        .trim()
        .to_owned()
}

async fn wake_lock_held(
    paths: &crate::device::linux::soft_suspend::paths::SoftSuspendPaths,
) -> bool {
    wake_node(&paths.wake_lock).await == crate::device::linux::soft_suspend::WAKE_LOCK_NAME
}

async fn wake_unlock_written(
    paths: &crate::device::linux::soft_suspend::paths::SoftSuspendPaths,
) -> bool {
    wake_node(&paths.wake_unlock).await == crate::device::linux::soft_suspend::WAKE_LOCK_NAME
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_for_sleep_schedules_suspend_rtc() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.push_task(DeviceTaskId::PrepareSuspend);
    harness.context.settings.wifi = crate::settings::WifiMode::AlwaysOn;
    harness.context.online = true;
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await
    );
    assert!(!harness.context.online);
    assert!(
        harness
            .context
            .device
            .wifi_manager_for_test()
            .was_disable_called()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspend_awaits_display_wait_without_stalling_runtime() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);
    crate::framebuffer::set_wait_hook_for_test(move |token| {
        assert_eq!(token, 7);
        started_tx.send(()).expect("signal display wait");
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .expect("release display wait");
        Ok(1)
    });
    harness.updating.push(crate::view::UpdateData {
        token: 7,
        time: std::time::Instant::now(),
        rect: crate::geom::Rectangle::new(
            crate::geom::Point::new(0, 0),
            crate::geom::Point::new(1, 1),
        ),
    });
    let alarms = harness
        .context
        .alarm_manager
        .clone()
        .expect("alarm manager");
    let prepare = async {
        let mut runtime = crate::device::DeviceRuntime {
            view: &mut harness.view,
            history: &mut harness.history,
            tasks: &mut harness.tasks,
            updating: &mut harness.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        prepare_for_sleep(
            &harness.hub_tx,
            &mut harness.bus,
            &mut harness.rq,
            &mut harness.context,
            &mut runtime,
        )
        .await
    };
    let started =
        crate::runtime::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(5)));
    let prepare = Box::pin(prepare);
    let prepare = match futures_util::future::select(prepare, started).await {
        futures_util::future::Either::Left((outcome, _)) => {
            panic!("suspend bypassed the display wait: {outcome:?}")
        }
        futures_util::future::Either::Right((started, prepare)) => {
            started
                .expect("display wait join")
                .expect("display wait started");
            prepare
        }
    };

    let heartbeat = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let sleep_deferred = !alarms
            .lock()
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await;
        release_tx.send(()).expect("release display wait");
        sleep_deferred
    });
    let outcome = tokio::time::timeout(Duration::from_secs(5), prepare)
        .await
        .expect("the display wait must be released by the heartbeat");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), heartbeat)
            .await
            .expect("heartbeat join")
            .expect("heartbeat task"),
        "the runtime must keep ticking while the display wait holds suspend, before sleep is armed"
    );
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await,
        "sleep proceeds only after the display wait is released"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_for_sleep_turns_off_frontlight() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.frontlight = true;
    harness
        .context
        .device
        .frontlight_mut()
        .set_intensity(50.0.into())
        .await
        .unwrap();
    harness
        .context
        .device
        .frontlight_mut()
        .set_warmth(30.0.into())
        .await
        .unwrap();
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    let levels = harness.context.device.frontlight().levels();
    assert_eq!(levels.intensity, LightLevel::off());
    assert_eq!(levels.warmth, LightLevel::off());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_alarms_past_due_auto_power_off_exits() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_power_off = 1.0;
    lock_alarms(&mut harness)
        .await
        .schedule_in(AlarmType::AutoPowerOff, ChronoDuration::seconds(-10))
        .await
        .unwrap();
    let outcome = crate::poll_runtime_only!(harness, |context, runtime| {
        schedule_alarms_before_sleep(context, runtime)
    });
    assert_eq!(outcome, Some(EventOutcome::Exit(ExitStatus::PowerOff)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_alarms_calendar_when_intermission_calendar() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    let outcome = crate::poll_runtime_only!(harness, |context, runtime| {
        schedule_alarms_before_sleep(context, runtime)
    });
    assert!(outcome.is_none());
    assert!(
        lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::CalendarUpdate)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_post_wake_auto_power_off_exit() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let before = Local::now();
    {
        lock_alarms(&mut harness)
            .await
            .schedule_in(AlarmType::AutoPowerOff, ChronoDuration::minutes(5))
            .await
            .unwrap();
        if let Ok(rtc) = harness.context.device.rtc() {
            let wake_at = rtc.scheduled_wake_time().expect("hardware wake programmed");
            rtc.set_current_time(wake_at);
            rtc.simulate_alarm_fired();
        }
    }
    let after = before + ChronoDuration::minutes(5) + ChronoDuration::seconds(1);
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_post_wake(before, after, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Exit(ExitStatus::PowerOff));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classic_suspend_schedules_wake_debounce() {
    let mut harness = DeviceRuntimeHarness::new().await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    let power = harness.context.device.power_manager_for_test();
    assert!(power.was_suspend_called());
    assert!(power.was_resume_called());
    assert_eq!(power.suspend_call_count(), 1);
    assert_eq!(power.resume_call_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classic_suspend_unlocks_and_restores_thumbnail_wake_lock() {
    use crate::device::soft_suspend::SoftSuspendBackend as _;

    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    harness
        .context
        .inhibitor
        .set_mode(AutosleepMode::Off)
        .await
        .ok();
    let _thumbnail = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
        .await
        .unwrap();
    harness
        .context
        .device
        .power_manager_for_test()
        .set_wake_lock_probe(paths.wake_unlock.clone());
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(wake_lock_held(&paths).await);
    tokio::fs::write(&paths.wake_lock, "")
        .await
        .expect("clear lock");
    tokio::fs::write(&paths.wake_unlock, "")
        .await
        .expect("clear unlock");

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert_eq!(power.suspend_call_count(), 1);
    assert_eq!(
        power.wake_lock_at_suspend().as_deref(),
        Some(crate::device::linux::soft_suspend::WAKE_LOCK_NAME),
        "cadmus must be written to wake_unlock before the blocking state=mem write"
    );
    assert!(
        wake_lock_held(&paths).await,
        "resume must restore cadmus while the thumbnail lease remains"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_debounce_classic_reenters_via_enter_sleep() {
    use crate::view::common::locate;
    use crate::view::intermission::Intermission;

    let mut harness = DeviceRuntimeHarness::new().await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert_eq!(
        harness
            .context
            .device
            .power_manager_for_test()
            .suspend_call_count(),
        1
    );

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert_eq!(
        harness
            .tasks
            .iter()
            .filter(|task| task.id == DeviceTaskId::ClassicSuspend)
            .count(),
        0,
        "completed Classic commits must not leave task entries behind"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_rtc_auto_suspend_future_noop_when_not_fired_via_event() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 5.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_rtc_auto_suspend_fired_begins_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_rtc_auto_power_off_exits() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_rtc_auto_power_off_ignored_while_full_inhibit_active() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(
            &Event::RtcAlarmFired(AlarmType::AutoPowerOff),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert_eq!(outcome, EventOutcome::Handled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_rtc_auto_suspend_blocked_when_shared_reschedules() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    harness.context.shared = true;
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reschedule_auto_suspend_zero_cancels() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    assert!(
        lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::AutoSuspend)
    );
    harness.context.settings.auto_suspend = 0.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::AutoSuspend)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reschedule_auto_suspend_moves_deadline_forward() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    let first = lock_alarms(&mut harness)
        .await
        .time_until_alarm(AlarmType::AutoSuspend)
        .await
        .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    let second = lock_alarms(&mut harness)
        .await
        .time_until_alarm(AlarmType::AutoSuspend)
        .await
        .unwrap();
    assert!(second >= first);
    assert!((second - 30 * 60).abs() < 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reschedule_auto_suspend_sub_minute_clamps_nonzero() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 0.01;
    assert_eq!(
        (harness.context.settings.auto_suspend * 60.0) as i64,
        0,
        "precondition: whole-second cast truncates this setting to zero"
    );
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await,
        "sub-minute AutoSuspend must schedule a future alarm"
    );
    let until = lock_alarms(&mut harness)
        .await
        .time_until_alarm(AlarmType::AutoSuspend)
        .await
        .unwrap();
    assert!(until >= 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_cancels_auto_suspend_alarm() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::AutoSuspend)
    );
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_prepare_suspend_reschedules_auto_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::AutoSuspend)
    );
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_suspend_rtc_reschedules_auto_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    harness.tasks.clear();
    {
        let mut alarms = lock_alarms(&mut harness).await;
        alarms
            .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
            .await
            .unwrap();
    }
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::Suspend)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_ignored_while_prepare_pending() {
    let mut harness = DeviceRuntimeHarness::new().await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    if let Some(cycle) = harness.context.suspend.as_mut() {
        cycle.consecutive_deep_idle_timeouts = 2;
    }

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });

    let cycle = harness
        .context
        .suspend
        .as_ref()
        .expect("the in-flight cycle must survive");
    assert_eq!(cycle.kind, SuspendKind::Classic);
    assert_eq!(cycle.consecutive_deep_idle_timeouts, 2);
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_ignored_after_prepare_keeps_suspend_alarm() {
    let mut harness = DeviceRuntimeHarness::new().await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(harness.tasks.is_empty());
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await
    );
    if let Some(cycle) = harness.context.suspend.as_mut() {
        cycle.deep_idle_restore = Some(AutosleepMode::Off);
    }

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });

    assert!(
        harness.tasks.is_empty(),
        "no second PrepareSuspend may be scheduled"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await,
        "the in-flight cycle's Suspend alarm must stay armed"
    );
    let cycle = harness
        .context
        .suspend
        .as_ref()
        .expect("the in-flight cycle must survive");
    assert_eq!(cycle.kind, SuspendKind::Classic);
    assert_eq!(cycle.deep_idle_restore, Some(AutosleepMode::Off));
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_suspend_rtc_after_cancel_skips_hardware_sleep() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 30.0;
    harness.context.settings.auto_power_off = 1.0;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await
    );
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::Suspend)
    );
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::AutoPowerOff),
        "stale Suspend must not arm AutoPowerOff without sleeping"
    );
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::CalendarUpdate),
        "stale Suspend must not arm CalendarUpdate without sleeping"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finish_cycle_clears_auto_power_off_before_post_wake_can_see_it() {
    let mut harness = DeviceRuntimeHarness::new().await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let before = Local::now() - ChronoDuration::minutes(10);
    {
        lock_alarms(&mut harness)
            .await
            .schedule_in(AlarmType::AutoPowerOff, ChronoDuration::minutes(-5))
            .await
            .unwrap();
        if let Ok(rtc) = harness.context.device.rtc() {
            rtc.simulate_alarm_fired();
        }
    }
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::AutoPowerOff)
    );
    let after = Local::now();
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_post_wake(before, after, hub, bus, rq, context, runtime)
    });
    assert_eq!(
        outcome,
        EventOutcome::Handled,
        "finish_cycle cancels AutoPowerOff; post-wake must run first on deep idle"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classic_prepare_suspend_still_schedules_with_suspend_rtc() {
    let mut harness = DeviceRuntimeHarness::new().await;
    assert!(!harness.context.inhibitor.mode().is_armed());
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_start_cycle_acquires_cycle_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_prepare_suspend_enters_deep_idle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    pump_deep_idle_wake(&mut harness).await;
    assert!(!harness.context.inhibitor.has_holders());
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::Suspend)
    );
}

#[cfg(target_os = "linux")]
mod irq_deltas {
    use super::*;
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    const BEFORE: &str = "           CPU0       CPU1
 90:        10          0      MTK   90 Level      bd71828_irq
195:         2          8      bd71828   195 Edge      bd71828_pwrkey
213:         4          4      bd71828   213 Edge      rtc-alarm
";
    const AFTER: &str = "           CPU0       CPU1
 90:        12          1      MTK   90 Level      bd71828_irq
195:         3         10      bd71828   195 Edge      bd71828_pwrkey
213:         4          4      bd71828   213 Edge      rtc-alarm
";
    const AFTER_PARTIAL: &str = "           CPU0       CPU1
 90:        12          1      MTK   90 Level      bd71828_irq
195:         3         10      bd71828   195 Edge      bd71828_pwrkey
";

    #[derive(Clone, Debug, Default)]
    struct LogBuffer(Arc<Mutex<Vec<WakeEvent>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogBuffer {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut fields = WakeFields::default();
            event.record(&mut fields);
            self.0.lock().unwrap().push(WakeEvent { fields });
        }
    }

    #[derive(Debug)]
    struct WakeEvent {
        fields: WakeFields,
    }

    #[derive(Debug, Default)]
    struct WakeFields {
        message: String,
        pre_hardware_alarm: String,
        post_hardware_alarm: String,
        pre_cadmus_alarm_type: serde_json::Value,
        pre_cadmus_alarm: serde_json::Value,
        unmanaged_alarm_candidate: bool,
        irq_delta_status: String,
        irq_deltas_json: String,
        irq_delta_reasons_json: String,
    }

    impl WakeFields {
        fn assign(&mut self, name: &str, value: String) {
            match name {
                "message" => self.message = value,
                "pre_hardware_alarm" => self.pre_hardware_alarm = value,
                "post_hardware_alarm" => self.post_hardware_alarm = value,
                "pre_cadmus_alarm_type" => {
                    self.pre_cadmus_alarm_type = serde_json::Value::String(value);
                }
                "pre_cadmus_alarm" => self.pre_cadmus_alarm = serde_json::Value::String(value),
                "irq_delta_status" => self.irq_delta_status = value,
                "irq_deltas_json" => self.irq_deltas_json = value,
                "irq_delta_reasons_json" => self.irq_delta_reasons_json = value,
                _ => {}
            }
        }
    }

    impl tracing::field::Visit for WakeFields {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.assign(field.name(), format!("{value:?}"));
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            if field.name() == "unmanaged_alarm_candidate" {
                self.unmanaged_alarm_candidate = value;
            }
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.assign(field.name(), value.to_owned());
        }
    }

    async fn wake_with_snapshots(snapshots: Vec<Option<String>>) -> (Vec<WakeEvent>, String) {
        let logs = LogBuffer::default();
        capture_wake_events(&logs, async move {
            let mut harness = DeviceRuntimeHarness::new().await;
            let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
            harness
                .context
                .deep_idle_interrupts_inject
                .extend(snapshots);
            {
                crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
                    start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
                });
                let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
                    handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
                });
                assert_eq!(outcome, EventOutcome::Handled);
            }
            pump_deep_idle_wake(&mut harness).await;
            assert!(
                lock_alarms(&mut harness)
                    .await
                    .is_alarm_scheduled(AlarmType::WakeDebounce)
                    .await
            );
            assert!(
                harness.context.deep_idle_interrupts_inject.is_empty(),
                "wake telemetry did not consume every injected snapshot"
            );
        })
        .await
    }

    async fn capture_wake_events(
        logs: &LogBuffer,
        scenario: impl Future<Output = ()>,
    ) -> (Vec<WakeEvent>, String) {
        let subscriber = tracing_subscriber::registry().with(logs.clone());
        scenario.with_subscriber(subscriber).await;
        wake_events_from_logs(logs)
    }

    fn wake_events_from_logs(logs: &LogBuffer) -> (Vec<WakeEvent>, String) {
        let records = std::mem::take(&mut *logs.0.lock().unwrap());
        let raw = format!("{records:?}");
        let events = records
            .into_iter()
            .filter(|event| event.fields.message == "Primary deep-idle wake")
            .collect();
        (events, raw)
    }

    #[tokio::test]
    async fn deep_idle_wake_logs_changed_parent_and_child_counters() {
        let (events, raw) =
            wake_with_snapshots(vec![Some(BEFORE.to_owned()), Some(AFTER.to_owned())]).await;
        assert_eq!(events.len(), 1, "unexpected wake events:\n{raw}");
        let fields = &events[0].fields;
        assert_eq!(fields.irq_delta_status, "complete");
        assert_eq!(fields.irq_delta_reasons_json, "[]");
        assert_eq!(fields.pre_hardware_alarm, "unavailable");
        assert_eq!(fields.post_hardware_alarm, "unavailable");
        assert_eq!(fields.pre_cadmus_alarm_type, "Some(AutoPowerOff)");
        assert!(
            fields
                .pre_cadmus_alarm
                .as_str()
                .is_some_and(|value| value.starts_with("Some("))
        );
        assert!(!fields.unmanaged_alarm_candidate);
        let deltas: serde_json::Value = serde_json::from_str(&fields.irq_deltas_json).unwrap();
        assert_eq!(
            deltas,
            serde_json::json!([
                {"irq": "195", "name": "bd71828_pwrkey", "delta": 3},
                {"irq": "90", "name": "bd71828_irq", "delta": 3},
            ])
        );
    }

    #[tokio::test]
    async fn deep_idle_wake_logs_partial_reasons() {
        let (events, raw) = wake_with_snapshots(vec![
            Some(BEFORE.to_owned()),
            Some(AFTER_PARTIAL.to_owned()),
        ])
        .await;
        assert_eq!(events.len(), 1, "unexpected wake events:\n{raw}");
        let fields = &events[0].fields;
        assert_eq!(fields.irq_delta_status, "partial");
        assert_eq!(
            fields.irq_delta_reasons_json,
            r#"[{"irq":"213","reason":"missing_after"}]"#
        );
        let deltas: serde_json::Value = serde_json::from_str(&fields.irq_deltas_json).unwrap();
        assert_eq!(
            deltas,
            serde_json::json!([
                {"irq": "195", "name": "bd71828_pwrkey", "delta": 3},
                {"irq": "90", "name": "bd71828_irq", "delta": 3},
            ])
        );
    }

    #[tokio::test]
    async fn deep_idle_wake_keeps_handling_when_baseline_unavailable() {
        let (events, raw) = wake_with_snapshots(vec![None, Some(AFTER.to_owned())]).await;
        assert_eq!(events.len(), 1, "unexpected wake events:\n{raw}");
        assert_eq!(events[0].fields.irq_delta_status, "unavailable");
        assert_eq!(events[0].fields.irq_deltas_json, "[]");
        assert_eq!(events[0].fields.irq_delta_reasons_json, "[]");
    }

    #[tokio::test]
    async fn deep_idle_wake_reports_unavailable_when_post_wake_snapshot_missing() {
        let (events, raw) = wake_with_snapshots(vec![Some(BEFORE.to_owned()), None]).await;
        assert_eq!(events.len(), 1, "unexpected wake events:\n{raw}");
        assert_eq!(events[0].fields.irq_delta_status, "unavailable");
        assert_eq!(events[0].fields.irq_deltas_json, "[]");
        assert_eq!(events[0].fields.irq_delta_reasons_json, "[]");
    }

    #[tokio::test]
    async fn timeout_reentry_replaces_interrupt_baseline() {
        let logs = LogBuffer::default();
        let (event, raw) = capture_wake_events(&logs, async {
            let mut harness = DeviceRuntimeHarness::new().await;
            let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
            harness.context.settings.intermissions[IntermKind::Suspend] =
                IntermissionDisplay::Calendar;
            for fixture in [BEFORE, AFTER, AFTER, AFTER] {
                harness
                    .context
                    .deep_idle_interrupts_inject
                    .push_back(Some(fixture.to_owned()));
            }
            crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
                start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
            });
            crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
                handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
            });
            harness
                .context
                .deep_idle_poll_inject
                .push_back(PollResult::TimedOut);
            let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
                handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
            });
            assert_eq!(outcome, EventOutcome::Handled);
            pump_deep_idle_wake(&mut harness).await;
            assert!(
                harness.context.deep_idle_interrupts_inject.is_empty(),
                "re-entry did not consume every injected snapshot"
            );
        })
        .await;
        assert_eq!(event.len(), 1, "unexpected wake events:\n{raw}");
        assert_eq!(event[0].fields.irq_delta_status, "complete");
        assert_eq!(event[0].fields.irq_delta_reasons_json, "[]");
        let deltas: serde_json::Value =
            serde_json::from_str(&event[0].fields.irq_deltas_json).unwrap();
        assert_eq!(
            deltas,
            serde_json::json!([]),
            "the re-entered wait must compare against its fresh baseline"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_has_no_holders_when_cycle_lease_dropped() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_forces_mem_without_state_mem_write() {
    use std::fs;

    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    pump_deep_idle_wake(&mut harness).await;
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
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::Suspend)
    );
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
    let autosleep = fs::read_to_string(&paths.autosleep).expect("autosleep");
    assert!(autosleep.trim() == "freeze" || autosleep.trim() == "Freeze");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_schedules_wake_debounce_alarm() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::Suspend),
        "deep-idle prepare enters sleep immediately; no Suspend RTC"
    );
    pump_deep_idle_wake(&mut harness).await;
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(
        !lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_wake_debounce_fired_begins_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    pump_deep_idle_wake(&mut harness).await;
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert_eq!(intermission_count(harness.view.as_ref()), 1);

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::WakeDebounce)
    );
    assert_eq!(intermission_count(harness.view.as_ref()), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_calendar_wake_keeps_suspend_intermission() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    pump_deep_idle_wake(&mut harness).await;
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());

    let before = Local::now() - ChronoDuration::minutes(10);
    {
        lock_alarms(&mut harness)
            .await
            .schedule_in(AlarmType::CalendarUpdate, ChronoDuration::minutes(-5))
            .await
            .unwrap();
        if let Ok(rtc) = harness.context.device.rtc() {
            rtc.simulate_alarm_fired();
        }
    }
    let after = Local::now();
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::CalendarUpdate)
            .await,
        "next CalendarUpdate must be re-armed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_timeout_retries_without_finishing_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::CalendarUpdate)
            .await
    );

    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::TimedOut);
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::CalendarUpdate)
            .await,
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
        !lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await,
        "AutoSuspend must stay cancelled while retrying"
    );

    pump_deep_idle_wake(&mut harness).await;
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
    assert!(locate::<Intermission>(harness.view.as_ref()).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_idle_timeout_cap_falls_back_to_classic_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });

    for _ in 0..3 {
        harness
            .context
            .deep_idle_poll_inject
            .push_back(PollResult::TimedOut);
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    let power = harness.context.device.power_manager_for_test();
    assert_eq!(
        power.arm_deep_idle_call_count(),
        4,
        "three timeouts must produce three deep-idle entries plus the Classic preparation"
    );
    assert_eq!(
        power.disarm_deep_idle_call_count(),
        4,
        "each deep-idle leg must leave once and the Classic resume must disarm once"
    );
    assert_eq!(
        power.suspend_call_count(),
        1,
        "the retry cap must hand over to one classic suspend attempt"
    );
    assert!(
        !has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait),
        "no fourth deep-idle entry may be scheduled"
    );
    assert_eq!(
        harness.context.suspend.as_ref().map(|c| c.kind),
        Some(SuspendKind::Classic),
        "the cycle must stay on the classic path after the fallback"
    );
    assert_eq!(
        harness.context.inhibitor.mode(),
        AutosleepMode::Off,
        "the classic fallback must run with autosleep disarmed"
    );
    assert_eq!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_restore),
        Some(AutosleepMode::Freeze),
        "the fallback must stash the armed mode for finish_cycle"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );

    lock_alarms(&mut harness)
        .await
        .cancel_alarm(AlarmType::WakeDebounce)
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.inhibitor.mode(),
        AutosleepMode::Freeze,
        "finish_cycle must restore the stashed autosleep mode"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_idle_timeout_counter_resets_after_wake() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });

    for expected in 1..=2 {
        harness
            .context
            .deep_idle_poll_inject
            .push_back(PollResult::TimedOut);
        crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
        });
        assert_eq!(
            harness
                .context
                .suspend
                .as_ref()
                .map(|c| c.consecutive_deep_idle_timeouts),
            Some(expected),
            "timeouts must count up within one cycle"
        );
    }

    pump_deep_idle_wake(&mut harness).await;
    assert_eq!(
        harness
            .context
            .suspend
            .as_ref()
            .map(|c| c.consecutive_deep_idle_timeouts),
        Some(0),
        "a detected wake must reset the consecutive timeout counter"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classic_fallback_refusal_retries_via_wake_debounce() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    harness
        .context
        .device
        .power_manager_for_test()
        .set_suspend_fails(true);

    for _ in 0..3 {
        harness
            .context
            .deep_idle_poll_inject
            .push_back(PollResult::TimedOut);
        crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
        });
    }

    assert_eq!(
        harness.context.suspend.as_ref().map(|c| c.kind),
        Some(SuspendKind::Classic),
        "the fallback must keep the cycle on the classic path"
    );
    assert!(
        !has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait),
        "a refused classic fallback must not re-enter deep idle"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await,
        "a refused classic attempt must stay on the classic retry path"
    );
    assert_eq!(
        harness.context.inhibitor.mode(),
        AutosleepMode::Off,
        "the classic fallback must run with autosleep disarmed"
    );
    assert_eq!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_restore),
        Some(AutosleepMode::Freeze),
        "the fallback must stash the armed mode for finish_cycle"
    );
    assert_eq!(
        harness
            .context
            .device
            .power_manager_for_test()
            .suspend_call_count(),
        1
    );

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert_eq!(
        harness
            .context
            .device
            .power_manager_for_test()
            .suspend_call_count(),
        2,
        "WakeDebounce must retry the classic attempt"
    );
    assert_eq!(
        harness.context.suspend.as_ref().map(|c| c.kind),
        Some(SuspendKind::Classic)
    );
    assert_eq!(
        harness.context.inhibitor.mode(),
        AutosleepMode::Off,
        "the retried attempt must also run disarmed so EBUSY is the real blocker"
    );
    assert!(!has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_deep_idle_wait_succeeds_with_input_lease_holders() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let _input_leases: Vec<_> = futures_util::future::join_all((0..8).map(|_| async {
        harness
            .context
            .inhibitor
            .acquire(Kind::SoftSuspend, SoftSuspendName::Input)
            .await
            .unwrap()
    }))
    .await;
    assert!(
        !harness.context.inhibitor.is_empty(),
        "input leases must pin wake_lock like a hub backlog"
    );
    pump_deep_idle_wake(&mut harness).await;
    assert!(
        harness.context.suspend.is_none()
            || locate::<Intermission>(harness.view.as_ref()).is_some(),
        "wait completion must not give up solely because input leases were held"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_soft_suspend_input_lease_keeps_holders() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    assert!(harness.context.suspend.is_none());
    let _lease = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Input)
        .await
        .unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calendar_rearm_schedules_relative_from_system_boundary() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;

    schedule_next_calendar_update(&harness.context).await;

    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::CalendarUpdate)
            .await,
        "system boundary duration must schedule CalendarUpdate via schedule_in"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_rtc_calendar_update_rearms_and_reenters() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    pump_deep_idle_wake(&mut harness).await;
    lock_alarms(&mut harness)
        .await
        .cancel_alarm(AlarmType::WakeDebounce)
        .await
        .unwrap();
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::WakeDebounce)
    );

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::CalendarUpdate)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_calendar_update_during_insleep_preserves_deep_idle_restore() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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

    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.inhibitor.mode(),
        AutosleepMode::Freeze,
        "finish must restore Freeze, not leave Mem"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classic_rtc_calendar_update_rearms_and_reenters() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness.context.suspend = Some(SuspendCycle::new(
        SuspendKind::Classic,
        DeferredSuspend::Explicit,
    ));
    crate::poll_parts!(harness, |_hub, _bus, _rq, context, runtime| async {
        let interm = Intermission::new(
            context.device.framebuffer().rect(),
            IntermKind::Suspend,
            context,
        )
        .await;
        runtime.view.children_mut().push(Box::new(interm));
    });

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::CalendarUpdate)
            .await
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
            || lock_alarms(&mut harness)
                .await
                .is_alarm_scheduled(AlarmType::Suspend)
                .await
            || harness
                .context
                .device
                .power_manager_for_test()
                .was_suspend_called(),
        "classic CalendarUpdate must re-enter sleep"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_armed_classic_suspend_refused_without_cycle_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    if let Some(c) = harness.context.suspend.as_mut() {
        c.cycle_lease = None;
    }

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::Suspend)
    );
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_cancel_suspend_drops_cycle_lease_and_restores_mode() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    harness.tasks.clear();
    {
        let mut alarms = lock_alarms(&mut harness).await;
        alarms
            .schedule_in(AlarmType::Suspend, ChronoDuration::seconds(15))
            .await
            .unwrap();
    }
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_none_or(|c| !c.holds_cycle_lease())
    );
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_idle_reentry_preserves_frontlight_levels() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.frontlight = true;
    harness.context.settings.auto_suspend = 30.0;
    harness.context.settings.intermissions[IntermKind::Suspend] = IntermissionDisplay::Calendar;
    harness
        .context
        .device
        .frontlight_mut()
        .set_intensity(50.0.into())
        .await
        .unwrap();
    harness
        .context
        .device
        .frontlight_mut()
        .set_warmth(30.0.into())
        .await
        .unwrap();

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let off = harness.context.device.frontlight().levels();
    assert_eq!(off.intensity, LightLevel::off());
    assert_eq!(off.warmth, LightLevel::off());
    assert_eq!(
        harness.context.settings.frontlight_levels.intensity,
        LightLevel::from(50.0)
    );

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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

    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });

    assert!(harness.context.suspend.is_none());
    let levels = harness.context.device.frontlight().levels();
    assert_eq!(levels.intensity, LightLevel::from(50.0));
    assert_eq!(levels.warmth, LightLevel::from(30.0));
    assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Freeze);
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeat_suspend_then_cancel_restores_peripherals() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.frontlight = true;
    harness.context.settings.auto_suspend = 30.0;
    harness.context.settings.wifi = crate::settings::WifiMode::AlwaysOn;
    harness.context.online = true;
    harness
        .context
        .device
        .frontlight_mut()
        .set_intensity(50.0.into())
        .await
        .unwrap();
    harness
        .context
        .device
        .frontlight_mut()
        .set_warmth(30.0.into())
        .await
        .unwrap();

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(
        harness.context.device.frontlight().levels().intensity,
        LightLevel::off(),
        "prepare turns the frontlight off"
    );
    assert!(
        harness
            .context
            .device
            .wifi_manager_for_test()
            .was_disable_called()
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await
    );

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(
        !has_task(&harness.tasks, DeviceTaskId::PrepareSuspend),
        "a second start_cycle during an active cycle must not schedule another PrepareSuspend"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await,
        "the in-flight cycle's Suspend alarm must stay armed"
    );
    assert_eq!(
        harness.context.suspend.as_ref().map(|c| c.kind),
        Some(SuspendKind::Classic),
        "the in-flight cycle must survive the ignored second start_cycle"
    );
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .is_some_and(|c| matches!(c.phase, SuspendPhase::ArmingSleep)),
        "the ignored second start_cycle must not replace the prepared cycle with a fresh one"
    );
    assert_eq!(
        intermission_count(harness.view.as_ref()),
        1,
        "exactly one intermission belongs to the one cycle"
    );

    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });

    assert!(harness.context.suspend.is_none());
    let levels = harness.context.device.frontlight().levels();
    assert_eq!(
        levels.intensity,
        LightLevel::from(50.0),
        "cancel after an ignored repeat Suspend must restore the frontlight the first prepare turned off"
    );
    assert_eq!(levels.warmth, LightLevel::from(30.0));
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(!has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
    {
        let alarms = lock_alarms(&mut harness).await;
        assert!(!alarms.has_alarm(AlarmType::Suspend));
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
        assert!(alarms.is_alarm_scheduled(AlarmType::AutoSuspend).await);
    }
    assert_eq!(intermission_count(harness.view.as_ref()), 0);
    let wifi = harness.context.device.wifi_manager_for_test().clone();
    tokio::time::timeout(Duration::from_secs(5), async move {
        while wifi.enable_call_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancel after a repeat Suspend must re-enable WiFi at rest");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_restores_radio_for_live_auto_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.wifi = crate::settings::WifiMode::Auto;
    harness
        .context
        .wifi_session
        .set_mode(crate::settings::WifiMode::Auto)
        .await;
    let wifi = harness.context.device.wifi_manager_for_test().clone();
    wifi.set_network_info(Ok(None));
    harness.context.online = true;
    harness.context.wifi_session.notify_online();
    let lease = harness
        .context
        .wifi_session
        .acquire("live-demand")
        .await
        .expect("an online Auto lease is granted without touching the radio");
    assert!(harness.context.wifi_session.has_holders());
    assert_eq!(wifi.enabled(), Some(true));
    assert_eq!(wifi.enable_call_count(), 0);
    assert_eq!(wifi.disable_call_count(), 0);

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(
        wifi.enabled(),
        Some(false),
        "prepare must power the radio down"
    );
    assert_eq!(wifi.disable_call_count(), 1);
    assert_eq!(wifi.enable_call_count(), 0);
    assert!(
        harness.context.wifi_session.has_holders(),
        "the lease outlives preparation"
    );

    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });

    assert!(harness.context.suspend.is_none());
    let index = harness
        .tasks
        .iter()
        .position(|task| task.id == DeviceTaskId::WifiReconcile)
        .expect("cleanup must own a WiFi reconcile job");
    let reconcile = harness.tasks.remove(index);
    assert!(
        matches!(
            reconcile.job.join(Duration::from_secs(5)).await,
            crate::runtime::JobOutcome::Finished(())
        ),
        "the owned WiFi reconcile must finish in bounded time"
    );
    assert_eq!(
        wifi.enable_call_count(),
        1,
        "cleanup must re-enable the radio for the live Auto lease"
    );
    assert_eq!(wifi.enabled(), Some(true));
    assert_eq!(wifi.disable_call_count(), 1);
    assert!(harness.context.wifi_session.has_holders());
    drop(lease);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspend_during_deep_idle_wait_does_not_finish_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_idle_timeout_cannot_rearm_finishes_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    harness
        .context
        .inhibitor
        .set_mode(AutosleepMode::Off)
        .await
        .ok();
    if let Some(cycle) = harness.context.suspend.as_mut() {
        cycle.deep_idle_restore = Some(AutosleepMode::Off);
    }
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::TimedOut);
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(harness.context.suspend.is_none());
    assert!(locate::<Intermission>(harness.view.as_ref()).is_none());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wake_detect_inject_woke_without_realtime_step() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::Woke);
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_defers_while_full_inhibit_active() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_full_flushes_deferred_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_deferred_suspend_before_full_release_prevents_flush() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    handle_clear_deferred_suspend(&mut harness.context);
    assert!(harness.context.deferred_suspend.is_none());
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reschedule_auto_suspend_clears_deferred_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.deferred_suspend = Some(DeferredSuspend::Auto);
    harness.context.settings.auto_suspend = 5.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    assert!(harness.context.deferred_suspend.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_from_auto_defers_while_sleep_inhibit_active() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_runs_while_sleep_inhibit_active() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_sleep_flushes_deferred_auto() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "companion")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto)
    );
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    drop(sleep);
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto),
        "the intent waits for the release event, not the lease drop"
    );
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Sleep,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_sleep_after_touch_starts_no_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 5.0;
    let sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "companion")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto)
    );
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    assert!(harness.context.deferred_suspend.is_none());
    drop(sleep);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Sleep,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(harness.context.suspend.is_none());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await,
        "the touch restarts the idle delay"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_sleep_with_full_held_redefers_then_full_flushes() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "companion")
        .await
        .unwrap();
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto)
    );
    drop(sleep);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Sleep,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto),
        "the Full lease holds the Deferred Auto Suspend"
    );
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_sleep_keeps_deferred_explicit_for_full() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "companion")
        .await
        .unwrap();
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    drop(sleep);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Sleep,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit),
        "the Sleep release leaves a deferred Explicit suspend for the Full release"
    );
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_sleep_during_cycle_starts_no_second_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    harness.context.settings.auto_suspend = 5.0;
    reschedule_auto_suspend_alarm(&mut harness.context).await;
    let sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "companion")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto)
    );
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto),
        "starting the cycle does not clear the intent"
    );
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(harness.tasks.is_empty());
    assert!(
        harness.context.suspend.is_some(),
        "the explicit start must leave a cycle in flight"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await
    );

    drop(sleep);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Sleep,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(
        harness.context.suspend.is_some(),
        "the in-flight cycle must survive"
    );
    assert!(
        harness.tasks.is_empty(),
        "no second PrepareSuspend may be scheduled"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::Suspend)
            .await,
        "the in-flight cycle's Suspend alarm must stay armed"
    );
    assert!(
        !lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::AutoSuspend)
            .await,
        "no AutoSuspend alarm may wake the device during the cycle"
    );
}

fn holds_sleep_inhibit(harness: &DeviceRuntimeHarness) -> bool {
    harness
        .context
        .inhibitor
        .holders()
        .iter()
        .any(|name| name.as_str() == "sleep-inhibit")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_keeps_sleep_inhibit_until_kernel_sleep() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    assert!(holds_sleep_inhibit(&harness));
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.inhibitor.sleep_active());
    assert!(
        holds_sleep_inhibit(&harness),
        "preparation and intermission must keep the Sleep wake lease"
    );
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_keeps_other_soft_suspend_holders() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    let _wifi = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Wifi)
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.inhibitor.sleep_active());
    assert!(
        holds_sleep_inhibit(&harness),
        "the kernel-sleep boundary, not start_cycle, releases the shared lock"
    );
    assert!(
        harness
            .context
            .inhibitor
            .holders()
            .iter()
            .any(|name| name.as_str() == SoftSuspendName::Wifi.as_str()),
        "other SoftSuspend holders stay tracked across the cycle"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finish_cycle_keeps_sleep_inhibit_while_sleep_held() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        finish_cycle(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });
    assert!(harness.context.inhibitor.sleep_active());
    assert!(holds_sleep_inhibit(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_deep_idle_unlocks_for_thumbnail_and_relocks_on_wake() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    let _thumbnail = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(wake_lock_held(&paths).await);

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });

    assert!(harness.context.inhibitor.has_holders());
    assert!(
        wake_unlock_written(&paths).await,
        "kernel sleep must release cadmus even with a live thumbnail lease"
    );
    tokio::fs::write(&paths.wake_lock, "")
        .await
        .expect("clear lock");

    pump_deep_idle_wake(&mut harness).await;
    assert!(
        wake_lock_held(&paths).await,
        "wake must restore cadmus while the thumbnail lease remains"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_deep_idle_proceeds_with_only_thumbnail_held() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    let _thumbnail = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(intermission_count(harness.view.as_ref()), 1);

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });

    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|cycle| cycle.deep_idle_wait())
            .is_some()
    );
    assert!(wake_unlock_written(&paths).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deep_idle_wake_restores_sleep_inhibit_if_sleep_held() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.inhibitor.sleep_active());
    assert!(
        holds_sleep_inhibit(&harness),
        "the Sleep lease stays tracked while its kernel lock is suppressed"
    );
    assert!(wake_unlock_written(&paths).await);
    tokio::fs::write(&paths.wake_lock, "")
        .await
        .expect("clear lock");

    pump_deep_idle_wake(&mut harness).await;
    assert!(harness.context.inhibitor.sleep_active());
    assert!(
        holds_sleep_inhibit(&harness),
        "resume must retain the nested sleep-inhibit lease while Sleep holders remain"
    );
    assert!(
        wake_lock_held(&paths).await,
        "resume must re-acquire cadmus for the remaining Sleep holder"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_deep_idle_restores_thumbnail_wake_lock() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    let _thumbnail = harness
        .context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(wake_unlock_written(&paths).await);
    tokio::fs::write(&paths.wake_lock, "")
        .await
        .expect("clear lock");

    crate::poll_parts!(harness, |hub, _bus, rq, context, runtime| async {
        cancel_suspend_if_pending(context, runtime.tasks, runtime.view.as_mut(), hub, rq).await;
    });

    assert!(harness.context.suspend.is_none());
    assert!(
        wake_lock_held(&paths).await,
        "cancelling during the wait must restore cadmus for the live thumbnail lease"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_acquired_before_deep_idle_sleep_finishes_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let _full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    tokio::fs::write(&paths.wake_unlock, "")
        .await
        .expect("clear unlock");

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });

    assert_eq!(outcome, EventOutcome::Handled);
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    assert!(
        !wake_unlock_written(&paths).await,
        "pause must not suppress the lock while Full is active"
    );
    assert!(wake_lock_held(&paths).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_acquired_before_classic_suspend_finishes_cycle() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    harness
        .context
        .inhibitor
        .set_mode(AutosleepMode::Off)
        .await
        .ok();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let _full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    tokio::fs::write(&paths.wake_unlock, "")
        .await
        .expect("clear unlock");
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });

    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
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
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    assert!(
        !wake_unlock_written(&paths).await,
        "pause must not suppress the lock while Full is active"
    );
    assert!(wake_lock_held(&paths).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_acquired_during_deep_idle_aborts_kernel_sleep() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(wake_unlock_written(&paths).await);
    tokio::fs::write(&paths.wake_lock, "")
        .await
        .expect("clear lock");

    let _full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    assert!(
        wake_lock_held(&paths).await,
        "Full acquisition must restore cadmus before the next poll"
    );
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::StillWaiting);
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });

    assert_eq!(outcome, EventOutcome::Handled);
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    assert!(
        wake_lock_held(&paths).await,
        "Full must restore cadmus instead of sleeping over OTA"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_held_across_deep_idle_poll_replays_explicit_suspend_after_release() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::StillWaiting);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    assert!(
        !has_task(&harness.tasks, DeviceTaskId::PrepareSuspend),
        "the deferred Explicit suspend waits for the Full release"
    );

    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });

    assert!(harness.context.deferred_suspend.is_none());
    assert!(
        harness.context.suspend.is_some(),
        "the Full release runs the deferred Explicit suspend"
    );
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_cycle_from_auto_with_sleep_does_not_show_intermission() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    assert_eq!(intermission_count(harness.view.as_ref()), 0);
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_full_does_not_flush_auto_while_sleep_held() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_some());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_full_before_first_poll_restarts_explicit_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.suspend.as_ref().is_some_and(|cycle| {
        cycle.deep_idle_wait().is_some() && cycle.wake_lock_epoch.is_some()
    }));

    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());

    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::StillWaiting);
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });

    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness.context.suspend.is_some(),
        "an earlier Full release must not strand explicit suspend"
    );
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_wake_after_unlock_classifies_as_wake() {
    use crate::device::battery::FakeBattery;
    use crate::device::inhibitor::Inhibitor;
    use crate::device::linux::soft_suspend::paths::{SoftSuspendPaths, write_sysfs_blocking};
    use crate::device::soft_suspend::SoftSuspendBackend as _;

    const UNLOCK_DELAY: Duration = Duration::from_millis(400);

    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = SoftSuspendPaths::test_fixture();
    let delay_unlock = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook_delay = Arc::clone(&delay_unlock);
    let writer_clocks = Arc::new(std::sync::Mutex::new(
        None::<(std::time::Instant, Option<Duration>)>,
    ));
    let hook_clocks = Arc::clone(&writer_clocks);
    let inhibitor = Inhibitor::with_paths_and_writer(
        paths.clone(),
        None,
        Arc::new(FakeBattery::new()),
        Some(Arc::new(move |path, value| {
            if path.ends_with("wake_unlock") && hook_delay.load(std::sync::atomic::Ordering::SeqCst)
            {
                if let Ok(mut clocks) = hook_clocks.lock() {
                    clocks
                        .get_or_insert((std::time::Instant::now(), super::wake::clock_boottime()));
                }
                std::thread::sleep(UNLOCK_DELAY);
            }
            write_sysfs_blocking(path, value)
        })),
    );
    inhibitor
        .set_mode(AutosleepMode::Freeze)
        .await
        .expect("arm freeze autosleep");
    harness
        .context
        .wifi_session
        .set_inhibitor(Arc::clone(&inhibitor))
        .await;
    harness.context.inhibitor = inhibitor;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    delay_unlock.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(wake_unlock_written(&paths).await);
    let unlock_mtime = tokio::fs::metadata(&paths.wake_unlock)
        .await
        .expect("wake_unlock metadata")
        .modified()
        .expect("wake_unlock mtime");
    let wait = harness
        .context
        .suspend
        .as_ref()
        .and_then(|cycle| cycle.deep_idle_wait().cloned())
        .expect("deep-idle wait armed");
    assert!(
        wait.sleep_started_at() + chrono::Duration::milliseconds(150)
            <= chrono::DateTime::<chrono::Local>::from(unlock_mtime),
        "sleep anchor must be captured before wake_unlock is written"
    );
    let (writer_instant, writer_boot) = (*writer_clocks.lock().expect("writer clocks lock"))
        .expect("wake_unlock writer must have observed the physical write");
    assert!(
        wait.mono_anchor() <= writer_instant,
        "monotonic anchor must precede the wake_unlock write"
    );
    let boot_anchor = wait
        .boot_anchor()
        .expect("CLOCK_BOOTTIME anchor is required on Linux");
    let writer_boot = writer_boot.expect("CLOCK_BOOTTIME sample required in writer hook");
    assert!(
        boot_anchor <= writer_boot,
        "boottime anchor must precede the wake_unlock write"
    );
    harness
        .context
        .deep_idle_poll_inject
        .push_back(PollResult::Woke);

    pump_deep_idle_wake(&mut harness).await;

    let cycle = harness
        .context
        .suspend
        .as_ref()
        .expect("cycle survives wake");
    assert!(matches!(cycle.phase, SuspendPhase::PostWakeDebounce));
    assert!(
        cycle.wake_lock_epoch.is_none(),
        "wake consumes the owned suppression epoch exactly once"
    );
    assert!(
        lock_alarms(&mut harness)
            .await
            .is_alarm_scheduled(AlarmType::WakeDebounce)
            .await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_during_classic_prepare_blocks_mem_write() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let power = harness.context.device.power_manager_for_test().clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    power.set_on_prepare_suspend(move || {
        started_tx.send(()).expect("signal prepare start");
        release_rx.recv().expect("wait for prepare release");
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });

    let inhibitor = Arc::clone(&harness.context.inhibitor);
    let full_task = crate::runtime::spawn(async move {
        crate::runtime::spawn_blocking(move || started_rx.recv())
            .await
            .expect("prepare wait worker join")
            .expect("prepare started");
        let full = inhibitor.acquire(Kind::Full, "ota").await.unwrap();
        release_tx.send(()).expect("release preparation");
        full
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    let full = full_task.await.expect("full acquisition task");

    assert_eq!(outcome, EventOutcome::Handled);
    assert_eq!(power.suspend_call_count(), 0);
    assert_eq!(
        power.arm_deep_idle_call_count(),
        1,
        "preparation arms exactly once"
    );
    assert!(
        power.disarm_deep_idle_call_count() >= 1,
        "the aborted cycle must disarm the deep-idle backend at least once"
    );
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    drop(full);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_request_during_classic_pause_backs_off_mem_write() {
    use crate::device::battery::FakeBattery;
    use crate::device::inhibitor::Inhibitor;
    use crate::device::linux::soft_suspend::paths::{SoftSuspendPaths, write_sysfs_blocking};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, paths) = SoftSuspendPaths::test_fixture();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let paused = AtomicBool::new(false);
    let hook: crate::device::linux::soft_suspend::wake::WakeWriteHook =
        Arc::new(move |path: &Path, value: &str| {
            if path.ends_with("wake_unlock") && !paused.swap(true, Ordering::SeqCst) {
                started_tx.send(()).expect("signal pause start");
                release_rx
                    .lock()
                    .unwrap()
                    .recv()
                    .expect("wait for pause release");
            }
            write_sysfs_blocking(path, value)
        });
    let inhibitor = Inhibitor::with_paths_and_writer(
        paths.clone(),
        None,
        Arc::new(FakeBattery::new()),
        Some(hook),
    );
    harness
        .context
        .wifi_session
        .set_inhibitor(Arc::clone(&inhibitor))
        .await;
    harness.context.inhibitor = Arc::clone(&inhibitor);
    let power = harness.context.device.power_manager_for_test().clone();
    let _thumbnail = inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });

    let event = async {
        let mut runtime = crate::device::DeviceRuntime {
            view: &mut harness.view,
            history: &mut harness.history,
            tasks: &mut harness.tasks,
            updating: &mut harness.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        handle_event(
            &Event::Suspend,
            &harness.hub_tx,
            &mut harness.bus,
            &mut harness.rq,
            &mut harness.context,
            &mut runtime,
        )
        .await
    };
    let started = crate::runtime::spawn_blocking(move || started_rx.recv());
    let event = Box::pin(event);
    let event = match futures_util::future::select(event, started).await {
        futures_util::future::Either::Left((outcome, _)) => {
            panic!("suspend finished before the wake lock was paused: {outcome:?}")
        }
        futures_util::future::Either::Right((started, event)) => {
            started.expect("pause worker join").expect("pause started");
            event
        }
    };

    let full_inhibitor = Arc::clone(&inhibitor);
    let full =
        crate::runtime::spawn(async move { full_inhibitor.acquire(Kind::Full, "ota").await });
    let mut requested = false;
    for _ in 0..200 {
        if inhibitor.full_active() {
            requested = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    release_tx.send(()).expect("release pause");
    assert!(
        requested,
        "the Full request must wait on the gate the commit holds"
    );

    let outcome = tokio::time::timeout(Duration::from_secs(5), event)
        .await
        .expect("suspend must finish once the pause returns");
    let full = tokio::time::timeout(Duration::from_secs(5), full)
        .await
        .expect("Full acquisition must complete after the commit backs off")
        .expect("full worker join")
        .unwrap();

    assert_eq!(outcome, EventOutcome::Handled);
    assert_eq!(
        power.suspend_call_count(),
        0,
        "a Full request waiting on the gate must stop the mem write"
    );
    assert!(!power.was_resume_called());
    assert_eq!(power.arm_deep_idle_call_count(), 1);
    assert!(
        power.disarm_deep_idle_call_count() >= 1,
        "the backed-off mem write must disarm the deep-idle backend at least once"
    );
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(
        wake_lock_held(&paths).await,
        "cadmus stays held for the Full lease after the back-off"
    );

    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(
        has_task(&harness.tasks, DeviceTaskId::PrepareSuspend),
        "the deferred Explicit suspend runs after the release"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_waiter_drains_committed_classic_sleep_before_full() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let power = harness.context.device.power_manager_for_test().clone();
    let (resume_started_rx, resume_release_tx) = power.set_resume_barrier();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    power.set_on_suspend(move || {
        started_tx.send(()).expect("signal in-flight write");
        release_rx.recv().expect("wait for write release");
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });

    let event = async {
        let mut runtime = crate::device::DeviceRuntime {
            view: &mut harness.view,
            history: &mut harness.history,
            tasks: &mut harness.tasks,
            updating: &mut harness.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        handle_event(
            &Event::Suspend,
            &harness.hub_tx,
            &mut harness.bus,
            &mut harness.rq,
            &mut harness.context,
            &mut runtime,
        )
        .await
    };
    let started = crate::runtime::spawn_blocking(move || started_rx.recv());
    let event = Box::pin(event);
    let event = match futures_util::future::select(event, started).await {
        futures_util::future::Either::Left((outcome, _)) => {
            panic!("suspend finished before its waiter was dropped: {outcome:?}")
        }
        futures_util::future::Either::Right((started, event)) => {
            started.expect("wait worker join").expect("write started");
            event
        }
    };
    drop(event);

    let full_inhibitor = Arc::clone(&harness.context.inhibitor);
    let full =
        crate::runtime::spawn(async move { full_inhibitor.acquire(Kind::Full, "ota").await });

    let cleanup = Box::pin(async {
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
    });
    let settle = Box::pin(tokio::time::sleep(Duration::from_millis(300)));
    let cleanup = match futures_util::future::select(cleanup, settle).await {
        futures_util::future::Either::Left(_) => {
            panic!("cleanup finished while the committed Classic write was still in flight")
        }
        futures_util::future::Either::Right((_, cleanup)) => cleanup,
    };
    assert!(
        !power.was_resume_called(),
        "cleanup must not run peripheral resume ahead of the in-flight commit"
    );
    assert!(
        !full.is_finished(),
        "Full must wait for the active Classic commit gate"
    );
    assert_eq!(
        power.suspend_call_count(),
        1,
        "exactly one Classic kernel write may be in flight"
    );

    release_tx.send(()).expect("release in-flight write");
    tokio::time::timeout(Duration::from_secs(5), resume_started_rx)
        .await
        .expect("resume must start once the committed write returns")
        .expect("resume barrier start signal");
    let settle = Box::pin(tokio::time::sleep(Duration::from_millis(300)));
    let cleanup = match futures_util::future::select(cleanup, settle).await {
        futures_util::future::Either::Left(_) => {
            panic!("cleanup finished while Classic resume was still outstanding")
        }
        futures_util::future::Either::Right((_, cleanup)) => cleanup,
    };
    assert_eq!(power.resume_call_count(), 1);
    assert_eq!(
        power.disarm_deep_idle_call_count(),
        0,
        "resume has not yet reached its peripheral disarm"
    );
    assert!(
        !full.is_finished(),
        "Full must wait until Classic resume releases the commit gate"
    );

    resume_release_tx.send(()).expect("release resume");
    tokio::time::timeout(Duration::from_secs(5), cleanup)
        .await
        .expect("cleanup must complete once resume is released");
    let full = tokio::time::timeout(Duration::from_secs(5), full)
        .await
        .expect("Full acquisition must complete after the commit gate drops")
        .expect("full worker join")
        .unwrap();

    assert_eq!(power.suspend_call_count(), 1);
    assert_eq!(power.arm_deep_idle_call_count(), 1);
    assert!(
        power.disarm_deep_idle_call_count() >= 1,
        "the completed resume must disarm the deep-idle backend at least once"
    );
    assert!(harness.context.suspend.is_none());
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::WakeDebounce),
        "dropping the waiter must not fabricate post-wake handling"
    );
    drop(full);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_before_gate_waits_for_prepare_then_skips_mem_write() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let power = harness.context.device.power_manager_for_test().clone();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    power.set_on_prepare_suspend(move || {
        started_tx.send(()).expect("signal prepare start");
        release_rx.recv().expect("wait for prepare release");
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });

    let event = async {
        let mut runtime = crate::device::DeviceRuntime {
            view: &mut harness.view,
            history: &mut harness.history,
            tasks: &mut harness.tasks,
            updating: &mut harness.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        handle_event(
            &Event::Suspend,
            &harness.hub_tx,
            &mut harness.bus,
            &mut harness.rq,
            &mut harness.context,
            &mut runtime,
        )
        .await
    };
    let started = crate::runtime::spawn_blocking(move || started_rx.recv());
    let event = Box::pin(event);
    let event = match futures_util::future::select(event, started).await {
        futures_util::future::Either::Left((outcome, _)) => {
            panic!("suspend finished before its waiter was dropped: {outcome:?}")
        }
        futures_util::future::Either::Right((started, event)) => {
            started.expect("wait worker join").expect("prepare started");
            event
        }
    };
    drop(event);

    let cleanup = Box::pin(async {
        cancel_suspend_if_pending(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        )
        .await;
    });
    let settle = Box::pin(tokio::time::sleep(Duration::from_millis(300)));
    let cleanup = match futures_util::future::select(cleanup, settle).await {
        futures_util::future::Either::Left(_) => {
            panic!("cleanup finished while Classic preparation was still blocked")
        }
        futures_util::future::Either::Right((_, cleanup)) => cleanup,
    };
    assert_eq!(power.arm_deep_idle_call_count(), 1);
    assert_eq!(
        power.disarm_deep_idle_call_count(),
        0,
        "nothing may disarm before the in-flight preparation returns"
    );

    release_tx.send(()).expect("release preparation");
    tokio::time::timeout(Duration::from_secs(5), cleanup)
        .await
        .expect("cleanup must complete once preparation is released");

    assert_eq!(
        power.suspend_call_count(),
        0,
        "cancel precedes the mem write"
    );
    assert!(!power.was_resume_called());
    assert_eq!(power.arm_deep_idle_call_count(), 1);
    assert!(
        power.disarm_deep_idle_call_count() >= 1,
        "the cancelled commit must disarm the deep-idle backend at least once"
    );
    assert!(harness.context.suspend.is_none());
    assert!(!has_task(&harness.tasks, DeviceTaskId::ClassicSuspend));
    assert!(
        !lock_alarms(&mut harness)
            .await
            .has_alarm(AlarmType::WakeDebounce)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumed_classic_commit_does_not_block_cycle_cleanup() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let commit = ClassicCommit::new();
    let mut cycle = SuspendCycle::new(SuspendKind::Classic, DeferredSuspend::Explicit);
    cycle.classic_commit = Some(Arc::clone(&commit));
    harness.context.suspend = Some(cycle);
    commit.finish(ClassicSuspendOutcome::NotSlept).await;

    assert!(matches!(
        commit.take_result().await,
        Some(ClassicSuspendOutcome::NotSlept)
    ));

    tokio::time::timeout(
        Duration::from_secs(1),
        finish_cycle(
            &mut harness.context,
            &mut harness.tasks,
            harness.view.as_mut(),
            &harness.hub_tx,
            &mut harness.rq,
        ),
    )
    .await
    .expect("cleanup must not wait for a second result");
    assert!(harness.context.suspend.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_abort_keeps_auto_origin_deferred_while_sleep_held() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.suspend.is_some());
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.suspend.is_none());
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto),
        "a Full-aborted Auto cycle must stay Auto"
    );

    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto),
        "Auto origin rechecks Sleep instead of becoming Explicit"
    );
    assert!(harness.context.suspend.is_none());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newer_explicit_request_wins_after_auto_full_abort() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle_from_auto(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Suspend, hub, bus, rq, context, runtime)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Auto)
    );

    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert_eq!(
        harness.context.deferred_suspend,
        Some(DeferredSuspend::Explicit)
    );
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(
        harness.context.suspend.is_some(),
        "queued Explicit must flush even while Sleep is held"
    );
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inhibit_cleared_full_flushes_explicit_while_sleep_held() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let _sleep = harness
        .context
        .inhibitor
        .acquire(Kind::Sleep, "main-menu")
        .await
        .unwrap();
    let full = harness
        .context
        .inhibitor
        .acquire(Kind::Full, "ota")
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    assert!(harness.context.deferred_suspend.is_some());
    drop(full);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_inhibit_cleared(
            Kind::Full,
            context,
            runtime.view.as_mut(),
            hub,
            bus,
            rq,
            runtime.tasks,
        )
    });
    assert!(harness.context.deferred_suspend.is_none());
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
}
