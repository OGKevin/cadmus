//! Kobo lifecycle unit tests: power-button handling during deep-idle suspend.

use crate::AlarmType;
use crate::device::suspend::PollResult;
use crate::device::suspend::has_task;
use crate::device::suspend::test_support::{
    install_armed_soft_suspend, intermission_count, lock_alarms, pump_deep_idle_wake,
};
use crate::device::suspend::{handle_event, start_cycle};
use crate::device::test_harness::DeviceRuntimeHarness;
use crate::device::{DeviceLifecycle, DeviceTaskId, EventOutcome};
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};
use crate::view::Event;
use crate::view::common::locate;
use crate::view::intermission::Intermission;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_release_during_wake_debounce_cancels_and_restores_auto_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.auto_suspend = 30.0;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    pump_deep_idle_wake(&mut harness).await;
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::WakeDebounce));
    assert!(harness.context.suspend.is_some());
    assert_eq!(intermission_count(harness.view.as_ref()), 1);

    let wake_release = Event::Device(DeviceEvent::Button {
        code: ButtonCode::Power,
        status: ButtonStatus::Released,
        time: 0.0,
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        super::Device::handle_event(&wake_release, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(!lock_alarms(&mut harness).has_alarm(AlarmType::WakeDebounce));
    assert!(!has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(harness.context.suspend.is_none());
    assert!(locate::<Intermission>(harness.view.as_ref()).is_none());
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));

    let intentional = Event::Device(DeviceEvent::Button {
        code: ButtonCode::Power,
        status: ButtonStatus::Released,
        time: 1.0,
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        super::Device::handle_event(&intentional, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(has_task(&harness.tasks, DeviceTaskId::PrepareSuspend));
    assert!(harness.context.suspend.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn power_release_after_deep_idle_timeout_retry_finishes_and_restores_auto_suspend() {
    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    harness.context.settings.auto_suspend = 30.0;
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
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.suspend.is_some());
    assert!(has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));

    let wake_release = Event::Device(DeviceEvent::Button {
        code: ButtonCode::Power,
        status: ButtonStatus::Released,
        time: 0.0,
    });
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        super::Device::handle_event(&wake_release, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(harness.context.suspend.is_none());
    assert!(!has_task(&harness.tasks, DeviceTaskId::PollDeepIdleWait));
    assert!(locate::<Intermission>(harness.view.as_ref()).is_none());
    assert!(lock_alarms(&mut harness).is_alarm_scheduled(AlarmType::AutoSuspend));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_shutdown_preserves_frontlight_levels_during_deep_idle() {
    use crate::device::DeviceHardware as _;
    use crate::device::ExitStatus;
    use crate::frontlight::{Frontlight as _, LightLevel, LightLevels};

    let mut harness = DeviceRuntimeHarness::new().await;
    let (_dir, _paths) = install_armed_soft_suspend(&mut harness);
    let expected = LightLevels {
        intensity: LightLevel::from(42.0),
        warmth: LightLevel::from(17.0),
    };
    harness.context.settings.frontlight = true;
    harness.context.settings.frontlight_levels = expected;
    harness
        .context
        .device
        .frontlight_mut()
        .set_intensity(expected.intensity)
        .unwrap();
    harness
        .context
        .device
        .frontlight_mut()
        .set_warmth(expected.warmth)
        .unwrap();

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        start_cycle(context, runtime.view.as_mut(), hub, bus, rq, runtime.tasks)
    });
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.suspend.is_some());
    assert_eq!(
        harness.context.device.frontlight().levels().intensity,
        LightLevel::off()
    );
    assert_eq!(
        harness.context.settings.frontlight_levels.intensity,
        expected.intensity
    );

    harness
        .run_on_shutdown::<super::Device>(ExitStatus::PowerOff, |_| {})
        .await;
    assert_eq!(
        harness.context.settings.frontlight_levels.intensity,
        expected.intensity
    );
    assert_eq!(
        harness.context.settings.frontlight_levels.warmth,
        expected.warmth
    );
}
