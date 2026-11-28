//! Suspend, power-off, and USB-share event handling.
//!
//! On startup, registers
//! [`Inhibitor::set_full_release_notifier`](crate::device::inhibitor::Inhibitor::set_full_release_notifier)
//! and
//! [`Inhibitor::set_sleep_release_notifier`](crate::device::inhibitor::Inhibitor::set_sleep_release_notifier)
//! so the last [`Kind::Full`] and [`Kind::Sleep`] drops post
//! [`Event::InhibitCleared`] naming their kind. Power and exit paths consult
//! [`Inhibitor::full_active`](crate::device::inhibitor::Inhibitor::full_active).

mod battery;
mod device_events;
mod frontlight;
mod power;
mod usb_share;
mod wifi;

use super::Device;
use super::input::BATTERY_REFRESH_INTERVAL;
use crate::device::DeviceCapabilities as _;
use crate::device::DeviceHardware as _;
use crate::device::DeviceLifecycle;
use crate::device::DeviceRotation as _;
use crate::device::battery::Battery as _;
use crate::device::inhibitor::{Kind, SoftSuspendName};
use crate::device::power::PowerManager;
use crate::device::reschedule_auto_suspend_alarm;
use crate::device::schedule_device_task;
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::device::suspend::handle_event as handle_suspend_event;
use crate::device::{
    AppContext, AppDevice, DeviceRuntime, DeviceTask, DeviceTaskId, EventOutcome, ExitStatus,
    ShutdownContext,
};
use crate::framebuffer::Framebuffer as _;
use crate::frontlight::Frontlight as _;
use crate::gesture::GestureEvent;
use crate::input::{ButtonCode, DeviceEvent};
use crate::view::{EntryId, Event, HubMessage};
use std::io;
use std::sync::Arc;

/// Onboard path where a Nickel/OTA `KoboRoot.tgz` appears after USB mass storage.
///
/// After USB share ends, presence of this file triggers reboot instead of a
/// plain app restart so the firmware update can apply.
const KOBO_UPDATE_BUNDLE: &str = "/mnt/onboard/.kobo/KoboRoot.tgz";

const RESTART_MARKER: &str = "/tmp/restart";
const REBOOT_MARKER: &str = "/tmp/reboot";
const POWER_OFF_MARKER: &str = "/tmp/power_off";
const RUN_COMMAND_MARKER: &str = "/tmp/run_command";

/// Restores the display rotation observed at device init for non-gyro devices.
fn restore_boot_rotation_if_needed(context: &mut ShutdownContext<'_, AppDevice>) {
    if context.device.has_gyroscope() {
        return;
    }

    let initial_rotation = context.device.boot_transformed_rotation();
    if context.display.rotation != initial_rotation {
        context.set_rotation(initial_rotation).ok();
    }
}

async fn write_exit_marker(status: ExitStatus) -> Result<(), io::Error> {
    match status {
        ExitStatus::Restart => {
            tokio::fs::File::create(RESTART_MARKER).await?;
        }
        ExitStatus::Reboot => {
            tokio::fs::File::create(REBOOT_MARKER).await?;
        }
        ExitStatus::PowerOff => {
            tokio::fs::File::create(POWER_OFF_MARKER).await?;
        }
        ExitStatus::RunCommand(command) => {
            tokio::fs::write(RUN_COMMAND_MARKER, command.to_string_lossy().as_bytes()).await?;
        }
        ExitStatus::Quit => {}
    }
    Ok(())
}

impl Device {
    /// Rewrites a dequeued hub event before any main-loop consumer sees it.
    ///
    /// A [`Event::WifiReconciled`] completion carries the reconcile generation
    /// that produced it. Resolving it here, at the event's original FIFO slot,
    /// keeps a later user `Off` from being overtaken by a re-queued `NetUp`:
    /// a current generation becomes [`DeviceEvent::NetUp`] in place, a stale
    /// generation is dropped (`None`), and every other event passes through
    /// unchanged. The caller owns any hub lease attached to the message and
    /// must keep holding it across this call.
    ///
    /// Must run on the main-loop thread against the live [`AppContext`], so the
    /// generation check and the consumers observe the same session state.
    pub fn normalize_main_loop_event(context: &AppContext, event: Event) -> Option<Event> {
        match event {
            Event::WifiReconciled(generation) => {
                if context
                    .wifi_session
                    .is_current_reconcile_generation(generation)
                {
                    Some(Event::Device(DeviceEvent::NetUp))
                } else {
                    tracing::debug!(generation, "ignoring stale wifi reconciliation");
                    None
                }
            }
            other => Some(other),
        }
    }
}

impl DeviceLifecycle for Device {
    fn should_skip_main_loop_soft_suspend_lease(context: &AppContext, event: &Event) -> bool {
        context
            .suspend
            .as_ref()
            .is_some_and(|cycle| cycle.should_skip_main_loop_lease(event))
    }

    /// Initializes cores, inhibitor callbacks, and startup Wi-Fi.
    ///
    /// The spawned radio reconcile reads the live session mode rather than a
    /// snapshot captured before spawn, so a mode change queued in between
    /// cannot power the radio the other way.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, hub, runtime), level = tracing::Level::TRACE
    ))]
    async fn on_startup(
        context: &mut AppContext,
        hub: &crate::view::Hub,
        runtime: &mut DeviceRuntime<'_>,
    ) -> Result<(), anyhow::Error> {
        if let Ok(power) = context.device.power_manager()
            && let Err(error) = power.init_cores().await
        {
            tracing::error!(error = %error, "Failed to initialize CPU cores");
        }

        let wants_on = context.settings.wifi.wants_radio_at_rest();
        context.wifi_session.set_mode(context.settings.wifi).await;
        if let Err(error) = context
            .inhibitor
            .apply_settings(
                context.settings.autosleep_mode,
                context.settings.indicate_autosleep_led,
                std::time::Duration::from_secs_f32(context.settings.autosleep_grace.max(0.0)),
            )
            .await
        {
            tracing::error!(error = %error, "failed to apply soft-suspend settings on startup");
        }
        let hub_full = hub.clone();
        context
            .inhibitor
            .set_full_release_notifier(Arc::new(move || {
                hub_full.send(Event::InhibitCleared(Kind::Full).into()).ok();
            }));
        let hub_sleep = hub.clone();
        context
            .inhibitor
            .set_sleep_release_notifier(Arc::new(move || {
                hub_sleep
                    .send(Event::InhibitCleared(Kind::Sleep).into())
                    .ok();
            }));
        if !wants_on {
            context.online = false;
        }
        crate::device::spawn_wifi_reconcile(hub, &context.wifi_session, wants_on, runtime.tasks);

        context.plugged = context
            .device
            .battery()
            .status()
            .await
            .is_ok_and(|v| v[0].is_wired());
        context
            .device
            .framebuffer_mut()
            .set_inverted(context.settings.inverted);
        context.set_frontlight(context.settings.frontlight).await;
        schedule_device_task(
            DeviceTaskId::CheckBattery,
            Event::CheckBattery,
            BATTERY_REFRESH_INTERVAL,
            hub,
            runtime.tasks,
        );
        hub.send((Event::WakeUp).into()).ok();
        reschedule_auto_suspend_alarm(context).await;
        if let Some(alarm_manager) = context.alarm_manager.clone() {
            let hub = hub.clone();
            let inhibitor = Arc::clone(&context.inhibitor);
            crate::device::rtc::AlarmManager::start_irq_listener(
                &alarm_manager,
                move |alarm_type| {
                    let hub = hub.clone();
                    let inhibitor = Arc::clone(&inhibitor);
                    async move {
                        hub.send(
                            HubMessage::try_with_soft_suspend(
                                &inhibitor,
                                SoftSuspendName::Rtc,
                                Event::RtcAlarmFired(alarm_type),
                            )
                            .await,
                        )
                        .ok();
                    }
                },
            )
            .await;
        }
        if let Some(job) = wifi::spawn_wifi_idle_poller(
            hub,
            context.settings.wifi_idle_timeout,
            &context.wifi_session,
        ) {
            runtime.tasks.push(DeviceTask {
                id: DeviceTaskId::WifiIdlePoller,
                job,
            });
        }
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, status, tasks), level = tracing::Level::TRACE
    ))]
    async fn on_shutdown(
        context: &mut ShutdownContext<'_, AppDevice>,
        status: ExitStatus,
        tasks: &[DeviceTask],
    ) -> Result<(), anyhow::Error> {
        if let Err(error) = context.inhibitor.set_mode(AutosleepMode::Off).await {
            tracing::error!(error = %error, "failed to disarm autosleep on shutdown");
        }

        if status == ExitStatus::Quit {
            restore_boot_rotation_if_needed(context);
        }

        if !context.is_suspend_active(tasks).await && context.settings.frontlight {
            context.settings.frontlight_levels = context.device.frontlight().levels();
        }

        if let Ok(power) = context.device.power_manager()
            && let Err(error) = power.restore_cores().await
        {
            tracing::error!(error = %error, "Failed to restore CPU cores on exit");
        }

        if let Err(error) = write_exit_marker(status.clone()).await {
            tracing::error!(error = %error, ?status, "Failed to write exit marker");
        }

        if status == ExitStatus::Quit
            && let Err(error) = context.wifi_session.disable_radio().await
        {
            tracing::error!(error = %error, "Failed to disable WiFi on exit");
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(event, hub, bus, rq, context, runtime), level = tracing::Level::TRACE, ret(level = tracing::Level::TRACE)
    ))]
    async fn handle_event(
        event: &Event,
        hub: &crate::view::Hub,
        bus: &mut crate::view::Bus,
        rq: &mut crate::view::RenderQueue,
        context: &mut AppContext,
        runtime: &mut DeviceRuntime<'_>,
    ) -> EventOutcome {
        match event {
            Event::Device(_) => {
                device_events::handle_event(event, hub, bus, rq, context, runtime).await
            }
            Event::SetWifiMode(_)
            | Event::Select(EntryId::SetWifiMode(_))
            | Event::MightDisableWifi => wifi::handle_event(event, hub, context).await,
            Event::PrepareSuspend
            | Event::Suspend
            | Event::PollDeepIdleWait
            | Event::RtcAlarmFired(_)
            | Event::InhibitCleared(_)
            | Event::ClearDeferredSuspend => {
                handle_suspend_event(event, hub, bus, rq, context, runtime).await
            }
            Event::PrepareShare | Event::Share => {
                usb_share::handle_event(event, hub, bus, rq, context, runtime).await
            }
            Event::CheckBattery => battery::handle_event(hub, rq, context, runtime).await,
            Event::ToggleFrontlight
            | Event::SetFrontlightLevels(_)
            | Event::UpdateAutoFrontlight => {
                frontlight::handle_event(event, hub, bus, rq, context, runtime).await
            }
            Event::Exit(status) => EventOutcome::Exit(status.clone()),
            Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power))
            | Event::Select(EntryId::PowerOff)
            | Event::Select(EntryId::Restart)
            | Event::Select(EntryId::Reboot)
            | Event::Select(EntryId::Quit)
            | Event::Select(EntryId::Suspend)
            | Event::Select(EntryId::SwitchInstall) => {
                power::handle_event(event, hub, bus, rq, context, runtime).await
            }
            _ => EventOutcome::Unhandled,
        }
    }
}

#[cfg(all(test, feature = "kobo"))]
#[path = "suspend_tests.rs"]
mod suspend_tests;

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::inhibitor::Kind;
    use crate::device::rtc::{AlarmType, shutdown_rtc};
    use crate::device::test_harness::DeviceRuntimeHarness;
    use crate::device::wifi::{Essid, NetworkInfo};
    use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};
    use crate::settings::WifiMode;
    use std::time::Duration;

    fn wait_for_wifi_thread() {
        std::thread::sleep(Duration::from_millis(50));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_startup_auto_disables_without_netup() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.wifi = WifiMode::Auto;
        harness.context.online = true;
        harness
            .context
            .device
            .wifi_manager_for_test()
            .set_network_info(Ok(Some(NetworkInfo {
                ip: "192.168.1.1".parse().unwrap(),
                essid: Essid::new("test"),
            })));
        crate::poll_parts!(harness, |hub, _bus, _rq, context, runtime| {
            Device::on_startup(context, hub, runtime)
        })
        .unwrap();
        wait_for_wifi_thread();
        assert!(!harness.context.online);
        assert_eq!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .disable_call_count(),
            1
        );
        let events = harness.drain_hub();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Device(DeviceEvent::NetUp))),
            "Auto startup must not emit NetUp, got {events:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_startup_always_on_sends_netup_when_connected() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.wifi = WifiMode::AlwaysOn;
        harness
            .context
            .device
            .wifi_manager_for_test()
            .set_network_info(Ok(Some(NetworkInfo {
                ip: "192.168.1.1".parse().unwrap(),
                essid: Essid::new("test"),
            })));
        crate::poll_parts!(harness, |hub, _bus, _rq, context, runtime| {
            Device::on_startup(context, hub, runtime)
        })
        .unwrap();
        wait_for_wifi_thread();
        assert_eq!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .enable_call_count(),
            1
        );
        let events = harness.drain_hub();
        assert!(
            events.iter().any(|e| matches!(e, Event::WifiReconciled(_))),
            "expected a tagged association completion for AlwaysOn, got {events:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_resume_reconcile_is_rejected_and_radio_follows_latest_intent() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let manager = harness.context.device.wifi_manager_for_test().clone();
        manager.set_network_info(Ok(Some(NetworkInfo {
            ip: "192.168.1.1".parse().unwrap(),
            essid: Essid::new("test"),
        })));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        manager.set_on_enable(move || {
            started_tx.send(()).expect("signal radio enable");
            release_rx.recv().expect("hold radio enable");
        });
        let hub = harness.hub_tx.clone();
        let session = Arc::clone(&harness.context.wifi_session);
        let mut tasks = Vec::new();
        crate::device::spawn_wifi_reconcile(&hub, &session, true, &mut tasks);
        tokio::time::timeout(
            Duration::from_secs(5),
            crate::runtime::spawn_blocking(move || started_rx.recv()),
        )
        .await
        .expect("radio enable start")
        .expect("enable wait join")
        .expect("enable started");

        crate::device::spawn_wifi_reconcile(&hub, &session, false, &mut tasks);
        let (disable_started, release_disable) = manager.set_on_disable();
        release_tx.send(()).expect("release stale radio enable");
        tokio::time::timeout(Duration::from_secs(5), disable_started)
            .await
            .expect("disable must start")
            .expect("disable barrier must stay open");
        let stale_completion = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = harness
                    .drain_hub()
                    .into_iter()
                    .find(|event| matches!(event, Event::WifiReconciled(_)))
                {
                    break event;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stale completion must be emitted");
        let stale_generation = match stale_completion {
            Event::WifiReconciled(generation) => generation,
            other => panic!("unexpected stale reconcile result: {other:?}"),
        };
        release_disable
            .send(())
            .expect("release fake radio disable");
        assert!(!session.is_current_reconcile_generation(stale_generation));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !tasks.last().is_some_and(|task| task.job.is_finished()) {
            assert!(
                std::time::Instant::now() < deadline,
                "radio-off never completed"
            );
            tokio::task::yield_now().await;
        }
        assert_eq!(manager.enabled(), Some(false));

        assert!(
            Device::normalize_main_loop_event(&harness.context, stale_completion).is_none(),
            "stale completion must be dropped before any consumer sees it"
        );
        assert!(
            harness.drain_hub().is_empty(),
            "stale completion must not forward NetUp"
        );
        assert!(
            !harness.context.online,
            "stale NetUp never reaches consumers"
        );

        crate::device::spawn_wifi_reconcile(&hub, &session, true, &mut tasks);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !tasks.last().is_some_and(|task| task.job.is_finished()) {
            assert!(
                std::time::Instant::now() < deadline,
                "resume never completed"
            );
            tokio::task::yield_now().await;
        }
        let completion = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = harness
                    .drain_hub()
                    .into_iter()
                    .find(|event| matches!(event, Event::WifiReconciled(_)))
                {
                    break event;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("current association must complete");
        let generation = match completion {
            Event::WifiReconciled(generation) => generation,
            other => panic!("unexpected current reconcile result: {other:?}"),
        };
        assert!(session.is_current_reconcile_generation(generation));
        let net_up = Device::normalize_main_loop_event(&harness.context, completion)
            .expect("current reconciliation must normalize to NetUp");
        assert!(matches!(net_up, Event::Device(DeviceEvent::NetUp)));
        assert!(
            harness.drain_hub().is_empty(),
            "normalization must not queue a second NetUp"
        );
        crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(&net_up, hub, bus, rq, context, runtime)
        });
        assert!(
            harness.context.online,
            "ordinary resume reports association"
        );
    }

    /// Mirrors the main loop for one dequeued event: normalize, record what
    /// background consumers would see, then run the device handler.
    async fn dispatch_like_main_loop(
        harness: &mut DeviceRuntimeHarness,
        event: Event,
        background: &mut Vec<Event>,
    ) {
        let Some(event) = Device::normalize_main_loop_event(&harness.context, event) else {
            return;
        };
        background.push(event.clone());
        crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(&event, hub, bus, rq, context, runtime)
        });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn current_wifi_reconciled_is_processed_before_a_later_user_off() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.wifi = WifiMode::AlwaysOn;
        harness.context.online = false;
        let generation = harness.context.wifi_session.next_reconcile_generation();
        assert!(
            harness
                .hub_tx
                .send(Event::WifiReconciled(generation).into())
                .is_ok()
        );
        assert!(
            harness
                .hub_tx
                .send(Event::SetWifiMode(WifiMode::Off).into())
                .is_ok()
        );

        let queued = harness.drain_hub();
        assert_eq!(queued.len(), 2);
        let mut background = Vec::new();
        let mut queued = queued.into_iter();

        dispatch_like_main_loop(&mut harness, queued.next().unwrap(), &mut background).await;
        assert!(
            harness.context.online,
            "current reconcile completion must set online at its own FIFO slot"
        );
        dispatch_like_main_loop(&mut harness, queued.next().unwrap(), &mut background).await;

        assert!(
            !harness.context.online,
            "user Off dequeued after reconcile completion must win"
        );
        assert_eq!(
            background.len(),
            2,
            "unexpected consumer events: {background:?}"
        );
        assert!(
            matches!(background[0], Event::Device(DeviceEvent::NetUp)),
            "NetUp must reach consumers first, got {background:?}"
        );
        assert!(
            matches!(background[1], Event::SetWifiMode(WifiMode::Off)),
            "Off must reach consumers second, got {background:?}"
        );
        assert!(
            !harness.drain_hub().iter().any(|e| matches!(
                e,
                Event::Device(DeviceEvent::NetUp) | Event::WifiReconciled(_)
            )),
            "no second NetUp or WifiReconciled may be queued behind Off"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_wifi_reconciled_reaches_no_consumer_and_leaves_offline() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.online = false;
        let stale = harness.context.wifi_session.next_reconcile_generation();
        let current = harness.context.wifi_session.next_reconcile_generation();
        assert_ne!(stale, current);
        assert!(
            harness
                .hub_tx
                .send(Event::WifiReconciled(stale).into())
                .is_ok()
        );

        let mut background = Vec::new();
        for event in harness.drain_hub() {
            dispatch_like_main_loop(&mut harness, event, &mut background).await;
        }

        assert!(
            background.is_empty(),
            "stale completion must reach no consumer, got {background:?}"
        );
        assert!(!harness.context.online);
        assert!(harness.drain_hub().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_event_device_delegates() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let event = Event::Device(DeviceEvent::Button {
            code: ButtonCode::Light,
            status: ButtonStatus::Pressed,
            time: 0.0,
        });
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(&event, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_event_check_battery_delegates() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(&Event::CheckBattery, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_event_set_wifi_delegates() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(
                &Event::SetWifiMode(crate::settings::WifiMode::AlwaysOn),
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
    async fn restore_boot_rotation_if_needed_noop_when_rotation_matches() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let boot_rotation = harness.context.device.boot_transformed_rotation();
        harness.context.display.rotation = boot_rotation;

        restore_boot_rotation_if_needed(&mut harness.context.shutdown());

        assert_eq!(harness.context.display.rotation, boot_rotation);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_shutdown_disarms_soft_suspend_without_changing_settings() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.settings.autosleep_mode = AutosleepMode::Mem;
        harness
            .context
            .inhibitor
            .set_mode(AutosleepMode::Mem)
            .await
            .ok();

        harness
            .run_on_shutdown::<Device>(ExitStatus::Quit, |_| {})
            .await;

        assert_eq!(harness.context.settings.autosleep_mode, AutosleepMode::Mem);
        assert_eq!(harness.context.inhibitor.mode(), AutosleepMode::Off);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_shutdown_clears_scheduled_alarms_for_power_off() {
        let mut harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::WakeDebounce, chrono::Duration::seconds(15))
                .await
                .unwrap();
            alarms
                .schedule_in(AlarmType::AutoPowerOff, chrono::Duration::hours(1))
                .await
                .unwrap();
        }
        let rtc = harness.context.device.rtc().unwrap();
        let _ = std::fs::remove_file("/tmp/power_off");

        shutdown_rtc(&harness.context).await;
        harness
            .run_on_shutdown::<Device>(ExitStatus::PowerOff, |_| {})
            .await;

        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
        assert!(!alarms.has_alarm(AlarmType::AutoPowerOff));
        assert!(!rtc.alarm_enabled());
        assert!(rtc.is_released());
        assert!(std::path::Path::new("/tmp/power_off").exists());
        let _ = std::fs::remove_file("/tmp/power_off");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_shutdown_clears_scheduled_alarms_for_quit() {
        let mut harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::AutoSuspend, chrono::Duration::minutes(10))
                .await
                .unwrap();
        }
        let rtc = harness.context.device.rtc().unwrap();

        shutdown_rtc(&harness.context).await;
        harness
            .run_on_shutdown::<Device>(ExitStatus::Quit, |_| {})
            .await;

        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(!alarms.has_alarm(AlarmType::AutoSuspend));
        assert!(!rtc.alarm_enabled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_shutdown_completes_when_rtc_alarm_disable_fails() {
        let mut harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
            alarms
                .schedule_in(AlarmType::WakeDebounce, chrono::Duration::seconds(15))
                .await
                .unwrap();
        }
        let rtc = harness.context.device.rtc().unwrap();
        rtc.set_fail_disable(true);
        let _ = std::fs::remove_file("/tmp/restart");

        shutdown_rtc(&harness.context).await;
        harness
            .run_on_shutdown::<Device>(ExitStatus::Restart, |_| {})
            .await;

        let alarms = harness.context.alarm_manager.as_ref().unwrap().lock().await;
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
        assert!(
            rtc.alarm_enabled(),
            "hardware alarm should stay armed when disable_alarm fails"
        );
        assert!(std::path::Path::new("/tmp/restart").exists());
        let _ = std::fs::remove_file("/tmp/restart");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_reboot_ignores_full_lease() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let _guard = harness
            .context
            .inhibitor
            .acquire(Kind::Full, "ota")
            .await
            .unwrap();
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(
                &Event::Exit(ExitStatus::Reboot),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::Reboot));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_restart_ignores_full_lease() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let _guard = harness
            .context
            .inhibitor
            .acquire(Kind::Full, "ota")
            .await
            .unwrap();
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(
                &Event::Exit(ExitStatus::Restart),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::Restart));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_reboot_ignores_shared() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.shared = true;
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(
                &Event::Exit(ExitStatus::Reboot),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::Reboot));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exit_restart_ignores_shared() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.shared = true;
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            Device::handle_event(
                &Event::Exit(ExitStatus::Restart),
                hub,
                bus,
                rq,
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Exit(ExitStatus::Restart));
    }
}
