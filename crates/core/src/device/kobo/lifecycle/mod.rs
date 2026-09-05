//! Suspend, power-off, and USB-share event handling.

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
use crate::device::power::PowerManager;
use crate::device::reschedule_auto_suspend_alarm;
use crate::device::schedule_device_task;
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::device::soft_suspend::mode::AutosleepMode;
use crate::device::suspend::{handle_event as handle_suspend_event, is_suspend_active};
use crate::device::{AppContext, DeviceRuntime, DeviceTaskId, EventOutcome, ExitStatus};
use crate::framebuffer::Framebuffer as _;
use crate::frontlight::Frontlight as _;
use crate::gesture::GestureEvent;
use crate::input::{ButtonCode, DeviceEvent};
use crate::view::{EntryId, Event, HubMessage};
use std::fs::File;
use std::sync::mpsc;
use std::thread;

/// Onboard path where a Nickel/OTA `KoboRoot.tgz` appears after USB mass storage.
///
/// After USB share ends, presence of this file triggers reboot instead of a
/// plain app restart so the firmware update can apply.
const KOBO_UPDATE_BUNDLE: &str = "/mnt/onboard/.kobo/KoboRoot.tgz";

/// Restores the display rotation observed at device init for non-gyro devices.
fn restore_boot_rotation_if_needed(context: &mut AppContext) {
    if context.device.has_gyroscope() {
        return;
    }

    let initial_rotation = context.device.boot_transformed_rotation();
    if context.display.rotation != initial_rotation {
        context.set_rotation(initial_rotation).ok();
    }
}

impl DeviceLifecycle for Device {
    fn should_skip_main_loop_soft_suspend_lease(context: &AppContext, event: &Event) -> bool {
        context
            .suspend
            .as_ref()
            .is_some_and(|cycle| cycle.should_skip_main_loop_lease(event))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, hub, runtime), level = tracing::Level::TRACE
    ))]
    fn on_startup(
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
        context.soft_suspend_session.apply_settings(
            context.settings.autosleep_mode,
            context.settings.indicate_autosleep_led,
            std::time::Duration::from_secs_f32(context.settings.autosleep_grace.max(0.0)),
        );
        if !wants_on {
            context.online = false;
        }
        let wifi_session = context.wifi_session.clone();
        let hub_wifi = hub.clone();
        thread::spawn(move || {
            if wants_on {
                match wifi_session.enable_radio() {
                    Ok(connected) => {
                        let enabled = wifi_session.wifi_manager().is_enabled();
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
                let result = wifi_session.disable_radio();
                let enabled = wifi_session.wifi_manager().is_enabled();
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
            let soft_suspend = context.soft_suspend_session.clone();
            crate::device::rtc::AlarmManager::start_irq_listener(
                &alarm_manager,
                move |alarm_type| {
                    hub.send(HubMessage::with_soft_suspend(
                        Event::RtcAlarmFired(alarm_type),
                        soft_suspend.acquire("rtc"),
                    ))
                    .ok();
                },
            );
        }
        let (idle_wake_tx, idle_wake_rx) = mpsc::channel();
        context.wifi_session.set_idle_wake_sender(idle_wake_tx);
        wifi::spawn_wifi_idle_poller(hub, context.settings.wifi_idle_timeout, idle_wake_rx);
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, status, runtime), level = tracing::Level::TRACE
    ))]
    fn on_shutdown(
        context: &mut AppContext,
        status: ExitStatus,
        runtime: &mut DeviceRuntime<'_>,
    ) -> Result<(), anyhow::Error> {
        context.soft_suspend_session.set_mode(AutosleepMode::Off);

        if status == ExitStatus::Quit {
            restore_boot_rotation_if_needed(context);
        }

        if !is_suspend_active(context, runtime.tasks) && context.settings.frontlight {
            context.settings.frontlight_levels = context.device.frontlight().levels();
        }

        if let Ok(power) = context.device.power_manager()
            && let Err(error) = power.restore_cores()
        {
            tracing::error!(error = %error, "Failed to restore CPU cores on exit");
        }

        match status {
            ExitStatus::Restart => {
                File::create("/tmp/restart").ok();
            }
            ExitStatus::Reboot => {
                File::create("/tmp/reboot").ok();
            }
            ExitStatus::PowerOff => {
                File::create("/tmp/power_off").ok();
            }
            ExitStatus::RunCommand(command) => {
                if let Err(error) =
                    std::fs::write("/tmp/run_command", command.to_string_lossy().as_bytes())
                {
                    tracing::error!(
                        error = %error,
                        command = %command.display(),
                        "Failed to write run_command marker"
                    );
                }
            }
            ExitStatus::Quit => {
                if let Err(error) = context.wifi_session.disable_radio() {
                    tracing::error!(error = %error, "Failed to disable WiFi on exit");
                }
            }
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(event, hub, bus, rq, context, runtime), level = tracing::Level::TRACE, ret(level = tracing::Level::TRACE)
    ))]
    fn handle_event(
        event: &Event,
        hub: &crate::view::Hub,
        bus: &mut crate::view::Bus,
        rq: &mut crate::view::RenderQueue,
        context: &mut AppContext,
        runtime: &mut DeviceRuntime<'_>,
    ) -> EventOutcome {
        match event {
            Event::Device(_) => device_events::handle_event(event, hub, bus, rq, context, runtime),
            Event::SetWifiMode(_)
            | Event::Select(EntryId::SetWifiMode(_))
            | Event::MightDisableWifi => wifi::handle_event(event, hub, context),
            Event::PrepareSuspend
            | Event::Suspend
            | Event::PollDeepIdleWait
            | Event::RtcAlarmFired(_) => {
                handle_suspend_event(event, hub, bus, rq, context, runtime)
            }
            Event::PrepareShare | Event::Share => {
                usb_share::handle_event(event, hub, bus, rq, context, runtime)
            }
            Event::CheckBattery => battery::handle_event(hub, rq, context, runtime),
            Event::ToggleFrontlight
            | Event::SetFrontlightLevels(_)
            | Event::UpdateAutoFrontlight => {
                frontlight::handle_event(event, hub, bus, rq, context, runtime)
            }
            Event::Gesture(GestureEvent::HoldButtonLong(ButtonCode::Power))
            | Event::Select(EntryId::PowerOff)
            | Event::Select(EntryId::Restart)
            | Event::Select(EntryId::Reboot)
            | Event::Select(EntryId::Quit)
            | Event::Select(EntryId::Suspend)
            | Event::Select(EntryId::SwitchInstall) => {
                power::handle_event(event, hub, bus, rq, context, runtime)
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

    #[test]
    fn on_startup_auto_disables_without_netup() {
        let mut harness = DeviceRuntimeHarness::new();
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
            Device::on_startup(context, hub, runtime).unwrap();
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

    #[test]
    fn on_startup_always_on_sends_netup_when_connected() {
        let mut harness = DeviceRuntimeHarness::new();
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
            Device::on_startup(context, hub, runtime).unwrap();
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

    #[test]
    fn handle_event_device_delegates() {
        let mut harness = DeviceRuntimeHarness::new();
        let event = Event::Device(DeviceEvent::Button {
            code: ButtonCode::Light,
            status: ButtonStatus::Pressed,
            time: 0.0,
        });
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            Device::handle_event(&event, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn handle_event_check_battery_delegates() {
        let mut harness = DeviceRuntimeHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            Device::handle_event(&Event::CheckBattery, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn handle_event_set_wifi_delegates() {
        let mut harness = DeviceRuntimeHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
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

    #[test]
    fn restore_boot_rotation_if_needed_noop_when_rotation_matches() {
        let mut harness = DeviceRuntimeHarness::new();
        let boot_rotation = harness.context.device.boot_transformed_rotation();
        harness.context.display.rotation = boot_rotation;

        restore_boot_rotation_if_needed(&mut harness.context);

        assert_eq!(harness.context.display.rotation, boot_rotation);
    }

    #[test]
    fn on_shutdown_disarms_soft_suspend_without_changing_settings() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.autosleep_mode = AutosleepMode::Mem;
        harness
            .context
            .soft_suspend_session
            .set_mode(AutosleepMode::Mem);

        harness.with_runtime_only(|context, runtime| {
            Device::on_shutdown(context, ExitStatus::Quit, runtime).unwrap();
        });

        assert_eq!(harness.context.settings.autosleep_mode, AutosleepMode::Mem);
        assert_eq!(
            harness.context.soft_suspend_session.mode(),
            AutosleepMode::Off
        );
    }

    #[test]
    fn on_shutdown_clears_scheduled_alarms_for_power_off() {
        let mut harness = DeviceRuntimeHarness::new();
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

        harness.with_runtime_only(|context, runtime| {
            shutdown_rtc(context);
            Device::on_shutdown(context, ExitStatus::PowerOff, runtime).unwrap();
        });

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

    #[test]
    fn on_shutdown_clears_scheduled_alarms_for_quit() {
        let mut harness = DeviceRuntimeHarness::new();
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

        harness.with_runtime_only(|context, runtime| {
            shutdown_rtc(context);
            Device::on_shutdown(context, ExitStatus::Quit, runtime).unwrap();
        });

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

    #[test]
    fn on_shutdown_completes_when_rtc_alarm_disable_fails() {
        let mut harness = DeviceRuntimeHarness::new();
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

        harness.with_runtime_only(|context, runtime| {
            shutdown_rtc(context);
            Device::on_shutdown(context, ExitStatus::Restart, runtime).unwrap();
        });

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
