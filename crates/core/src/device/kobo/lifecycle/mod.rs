//! Suspend, power-off, and USB-share event handling.
//!
//! On startup, registers
//! [`Inhibitor::set_full_release_notifier`](crate::device::inhibitor::Inhibitor::set_full_release_notifier)
//! so the last [`Kind::Full`](crate::device::inhibitor::Kind::Full) drop posts
//! [`Event::FullInhibitCleared`]. Power and exit paths consult
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
use crate::device::inhibitor::SoftSuspendName;
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
            && let Err(error) = power.init_cores()
        {
            tracing::error!(error = %error, "Failed to initialize CPU cores");
        }

        let wants_on = context.settings.wifi.wants_radio_at_rest();
        context.wifi_session.set_mode(context.settings.wifi);
        context.inhibitor.apply_settings(
            context.settings.autosleep_mode,
            context.settings.indicate_autosleep_led,
            std::time::Duration::from_secs_f32(context.settings.autosleep_grace.max(0.0)),
        );
        let hub_notify = hub.clone();
        context
            .inhibitor
            .set_full_release_notifier(Arc::new(move || {
                hub_notify.send(Event::FullInhibitCleared.into()).ok();
            }));
        if !wants_on {
            context.online = false;
        }
        let wifi_session = context.wifi_session.clone();
        let hub_wifi = hub.clone();
        let startup_job = crate::runtime::Job::spawn(move |cancel| async move {
            if cancel.is_cancelled() {
                return;
            }
            let wants_on = wifi_session.mode().wants_radio_at_rest();
            if wants_on {
                match wifi_session.enable_radio().await {
                    Ok(connected) => {
                        let enabled = wifi_session.wifi_manager().is_enabled().await;
                        tracing::info!(wants_on, enabled, connected, "wifi startup reconcile");
                        if connected {
                            hub_wifi
                                .send((Event::Device(DeviceEvent::NetUp)).into())
                                .ok();
                        }
                    }
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            wants_on,
                            "Failed to configure WiFi on startup"
                        );
                    }
                }
            } else {
                let result = wifi_session.disable_radio().await;
                let enabled = wifi_session.wifi_manager().is_enabled().await;
                tracing::info!(wants_on, enabled, "wifi startup reconcile");
                if let Err(error) = result {
                    tracing::error!(
                        error = %error,
                        wants_on,
                        "Failed to configure WiFi on startup"
                    );
                }
            }
        });

        context.plugged = context
            .device
            .battery()
            .status()
            .is_ok_and(|v| v[0].is_wired());
        context
            .device
            .framebuffer_mut()
            .set_inverted(context.settings.inverted);
        context.set_frontlight(context.settings.frontlight);
        schedule_device_task(
            DeviceTaskId::CheckBattery,
            Event::CheckBattery,
            BATTERY_REFRESH_INTERVAL,
            hub,
            runtime.tasks,
        );
        hub.send((Event::WakeUp).into()).ok();
        reschedule_auto_suspend_alarm(context);
        if let Some(alarm_manager) = context.alarm_manager.clone() {
            let hub = hub.clone();
            let inhibitor = Arc::clone(&context.inhibitor);
            crate::device::rtc::AlarmManager::start_irq_listener(
                &alarm_manager,
                move |alarm_type| {
                    hub.send(HubMessage::try_with_soft_suspend(
                        &inhibitor,
                        SoftSuspendName::Rtc,
                        Event::RtcAlarmFired(alarm_type),
                    ))
                    .ok();
                },
            );
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
        context.inhibitor.set_mode(AutosleepMode::Off);

        if status == ExitStatus::Quit {
            restore_boot_rotation_if_needed(context);
        }

        if !context.is_suspend_active(tasks) && context.settings.frontlight {
            context.settings.frontlight_levels = context.device.frontlight().levels();
        }

        if let Ok(power) = context.device.power_manager()
            && let Err(error) = power.restore_cores()
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
            | Event::MightDisableWifi => wifi::handle_event(event, hub, context),
            Event::PrepareSuspend
            | Event::Suspend
            | Event::PollDeepIdleWait
            | Event::RtcAlarmFired(_)
            | Event::FullInhibitCleared
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
        harness.with_parts(|hub, _bus, _rq, context, runtime| {
            crate::runtime::block_on(Device::on_startup(context, hub, runtime)).unwrap()
        });
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
        harness.with_parts(|hub, _bus, _rq, context, runtime| {
            crate::runtime::block_on(Device::on_startup(context, hub, runtime)).unwrap()
        });
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
            events
                .iter()
                .any(|e| matches!(e, Event::Device(DeviceEvent::NetUp))),
            "expected NetUp when AlwaysOn and associated, got {events:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_event_device_delegates() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let event = Event::Device(DeviceEvent::Button {
            code: ButtonCode::Light,
            status: ButtonStatus::Pressed,
            time: 0.0,
        });
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            crate::runtime::block_on(Device::handle_event(&event, hub, bus, rq, context, runtime))
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_event_check_battery_delegates() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            crate::runtime::block_on(Device::handle_event(
                &Event::CheckBattery,
                hub,
                bus,
                rq,
                context,
                runtime,
            ))
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_event_set_wifi_delegates() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            crate::runtime::block_on(Device::handle_event(
                &Event::SetWifiMode(crate::settings::WifiMode::AlwaysOn),
                hub,
                bus,
                rq,
                context,
                runtime,
            ))
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
        harness.context.inhibitor.set_mode(AutosleepMode::Mem);

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
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::WakeDebounce, chrono::Duration::seconds(15))
                .unwrap();
            alarms
                .schedule_in(AlarmType::AutoPowerOff, chrono::Duration::hours(1))
                .unwrap();
        }
        let rtc = harness.context.device.rtc().unwrap();
        let _ = std::fs::remove_file("/tmp/power_off");

        shutdown_rtc(&harness.context).await;
        harness
            .run_on_shutdown::<Device>(ExitStatus::PowerOff, |_| {})
            .await;

        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
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
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::AutoSuspend, chrono::Duration::minutes(10))
                .unwrap();
        }
        let rtc = harness.context.device.rtc().unwrap();

        shutdown_rtc(&harness.context).await;
        harness
            .run_on_shutdown::<Device>(ExitStatus::Quit, |_| {})
            .await;

        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(!alarms.has_alarm(AlarmType::AutoSuspend));
        assert!(!rtc.alarm_enabled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_shutdown_completes_when_rtc_alarm_disable_fails() {
        let mut harness = DeviceRuntimeHarness::new().await;
        {
            let mut alarms = harness
                .context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            alarms
                .schedule_in(AlarmType::WakeDebounce, chrono::Duration::seconds(15))
                .unwrap();
        }
        let rtc = harness.context.device.rtc().unwrap();
        rtc.set_fail_disable(true);
        let _ = std::fs::remove_file("/tmp/restart");

        shutdown_rtc(&harness.context).await;
        harness
            .run_on_shutdown::<Device>(ExitStatus::Restart, |_| {})
            .await;

        let alarms = harness
            .context
            .alarm_manager
            .as_ref()
            .unwrap()
            .lock()
            .unwrap();
        assert!(!alarms.has_alarm(AlarmType::WakeDebounce));
        assert!(
            rtc.alarm_enabled(),
            "hardware alarm should stay armed when disable_alarm fails"
        );
        assert!(std::path::Path::new("/tmp/restart").exists());
        let _ = std::fs::remove_file("/tmp/restart");
    }
}
