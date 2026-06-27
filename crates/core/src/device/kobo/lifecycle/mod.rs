//! Suspend, power-off, and USB-share event handling.

mod battery;
mod device_events;
mod frontlight;
mod helpers;
mod power;
mod suspend;
#[cfg(test)]
mod test_helpers;
mod usb_share;
mod wifi;

use super::Device;
use super::input::BATTERY_REFRESH_INTERVAL;
use crate::battery::Battery as _;
use crate::device::DeviceCapabilities as _;
use crate::device::DeviceHardware as _;
use crate::device::DeviceLifecycle;
use crate::device::DeviceRotation as _;
use crate::device::power::PowerManager;
use crate::device::rtc::AlarmType;
use crate::device::wifi::WifiManager;
use crate::device::{
    AppContext, DeviceRuntime, DeviceTask, DeviceTaskId, EventOutcome, ExitStatus, HistoryItem,
};
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::frontlight::Frontlight as _;
use crate::gesture::GestureEvent;
use crate::input::ButtonCode;
use crate::view::common::locate;
use crate::view::intermission::Intermission;
use crate::view::{EntryId, Event, RenderData, View, wait_for_all};
use std::fs::File;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

pub(super) const PREPARE_SUSPEND_WAIT_DELAY: Duration = Duration::from_secs(3);
pub(super) const SUSPEND_WAIT_DELAY: Duration = Duration::from_secs(15);
pub(super) const KOBO_UPDATE_BUNDLE: &str = "/mnt/onboard/.kobo/KoboRoot.tgz";

/// Schedules a delayed [`Event`] and tracks it in `tasks`.
///
/// Replaces any existing task with the same [`DeviceTaskId`]. The spawned
/// thread is dropped when the receiver side is closed, for example when the
/// task is superseded or cleared.
fn schedule_device_task(
    id: DeviceTaskId,
    event: Event,
    delay: Duration,
    hub: &Sender<Event>,
    tasks: &mut Vec<DeviceTask>,
) {
    let (ty, ry) = mpsc::channel();
    let hub2 = hub.clone();
    tasks.retain(|task| task.id != id);
    tasks.push(DeviceTask { id, _chan: ry });
    thread::spawn(move || {
        thread::sleep(delay);
        if ty.send(()).is_ok() {
            hub2.send(event).ok();
        }
    });
}

/// Aborts an in-progress suspend and restores UI and hardware state.
///
/// When cancelling [`DeviceTaskId::Suspend`], re-enables frontlight and
/// WiFi and clears alarms that should not fire after a manual wake. For
/// either suspend task, removes the intermission overlay and refreshes clock
/// and battery widgets.
fn cancel_suspend(
    context: &mut AppContext,
    id: DeviceTaskId,
    tasks: &mut Vec<DeviceTask>,
    view: &mut dyn View,
    hub: &Sender<Event>,
    rq: &mut crate::view::RenderQueue,
) {
    if id == DeviceTaskId::Suspend {
        tasks.retain(|task| task.id != DeviceTaskId::Suspend);
        context.set_frontlight(context.settings.frontlight);
        if context.settings.wifi
            && let Ok(wifi) = context.device.wifi_manager()
        {
            thread::spawn(move || {
                if let Err(error) = wifi.enable() {
                    tracing::error!(error = %error, "Failed to enable WiFi on resume");
                }
            });
        }
        if let Some(alarm_manager) = context.alarm_manager.as_mut() {
            for alarm in AlarmType::alarms_to_cancel_after_resume() {
                if let Err(error) = alarm_manager.cancel_alarm(alarm) {
                    tracing::error!(error = ?error, alarm = ?alarm, "failed to cancel alarm after resume");
                }
            }
        }
    }

    if id == DeviceTaskId::Suspend || id == DeviceTaskId::PrepareSuspend {
        tasks.retain(|task| task.id != DeviceTaskId::PrepareSuspend);
        if let Some(index) = locate::<Intermission>(view) {
            let rect = *view.child(index).rect();
            view.children_mut().remove(index);
            rq.add(RenderData::expose(rect, UpdateMode::Full));
        } else {
            tracing::warn!("resume called but no intermission view found to remove");
        }
        hub.send(Event::ClockTick).ok();
        hub.send(Event::BatteryTick).ok();
    }
}

/// Restores the display rotation observed at device init for non-gyro devices.
pub(super) fn restore_boot_rotation_if_needed(context: &mut AppContext) {
    if context.device.has_gyroscope() {
        return;
    }

    let initial_rotation = context.device.boot_transformed_rotation();
    if context.display.rotation != initial_rotation {
        context.set_rotation(initial_rotation).ok();
    }
}

/// Begins the suspend flow.
///
/// Suspends the current view and shows the suspend intermission immediately,
/// so the device already appears asleep to the user. A
/// [`DeviceTaskId::PrepareSuspend`] task is scheduled to send
/// [`Event::PrepareSuspend`] after [`PREPARE_SUSPEND_WAIT_DELAY`].
fn begin_suspend(
    context: &mut AppContext,
    view: &mut dyn View,
    hub: &Sender<Event>,
    bus: &mut crate::view::Bus,
    rq: &mut crate::view::RenderQueue,
    tasks: &mut Vec<DeviceTask>,
) {
    view.handle_event(&Event::Suspend, hub, bus, rq, context);
    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        crate::settings::IntermKind::Suspend,
        context,
    );
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        UpdateMode::Full,
    ));
    schedule_device_task(
        DeviceTaskId::PrepareSuspend,
        Event::PrepareSuspend,
        PREPARE_SUSPEND_WAIT_DELAY,
        hub,
        tasks,
    );
    view.children_mut().push(Box::new(interm));
}

/// Tears down the view stack and renders the power-off intermission.
///
/// Called on every power-off path so the device shows a final screen before
/// the process exits with [`ExitStatus::PowerOff`].
fn show_power_off_intermission(
    context: &mut AppContext,
    view: &mut dyn View,
    history: &mut Vec<HistoryItem>,
    updating: &mut Vec<crate::view::UpdateData>,
) {
    let (tx, _rx) = mpsc::channel();
    view.handle_event(
        &Event::Back,
        &tx,
        &mut crate::view::Bus::new(),
        &mut crate::view::RenderQueue::new(),
        context,
    );
    while let Some(mut item) = history.pop() {
        item.view.handle_event(
            &Event::Back,
            &tx,
            &mut crate::view::Bus::new(),
            &mut crate::view::RenderQueue::new(),
            context,
        );
    }
    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        crate::settings::IntermKind::PowerOff,
        context,
    );
    wait_for_all(updating, context);
    interm.render(context, *interm.rect());
    context
        .device
        .framebuffer_mut()
        .update(interm.rect(), UpdateMode::Full)
        .ok();
}

impl DeviceLifecycle for Device {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, hub, runtime), level = tracing::Level::TRACE))]
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

        if let Ok(wifi) = context.device.wifi_manager() {
            let wifi_enabled = context.settings.wifi;
            thread::spawn(move || {
                let result = if wifi_enabled {
                    wifi.enable()
                } else {
                    wifi.disable()
                };
                if let Err(error) = result {
                    tracing::error!(error = %error, wifi_enabled, "Failed to configure WiFi on startup");
                }
            });
        }

        context.plugged = context
            .device
            .battery_mut()
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
        hub.send(Event::WakeUp).ok();
        suspend::spawn_auto_suspend_poller(hub, context.settings.auto_suspend);
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(context, status, runtime), level = tracing::Level::TRACE))]
    fn on_shutdown(
        context: &mut AppContext,
        status: ExitStatus,
        runtime: &mut DeviceRuntime<'_>,
    ) -> Result<(), anyhow::Error> {
        if status == ExitStatus::Quit {
            restore_boot_rotation_if_needed(context);
        }

        if runtime
            .tasks
            .iter()
            .all(|task| task.id != DeviceTaskId::Suspend)
            && context.settings.frontlight
        {
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
            ExitStatus::Quit => {
                if let Ok(wifi) = context.device.wifi_manager()
                    && let Err(error) = wifi.disable()
                {
                    tracing::error!(error = %error, "Failed to disable WiFi on exit");
                }
            }
        }

        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(event, hub, bus, rq, context, runtime), level = tracing::Level::TRACE, ret(level = tracing::Level::TRACE)))]
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
            Event::SetWifi(_) | Event::Select(EntryId::ToggleWifi) => {
                wifi::handle_event(event, context)
            }
            Event::PrepareSuspend | Event::Suspend | Event::MightSuspend => {
                suspend::handle_event(event, hub, bus, rq, context, runtime)
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
            | Event::Select(EntryId::Suspend) => {
                power::handle_event(event, hub, bus, rq, context, runtime)
            }
            _ => EventOutcome::Unhandled,
        }
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};

    #[test]
    fn handle_event_device_delegates() {
        let mut harness = LifecycleHarness::new();
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
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            Device::handle_event(&Event::CheckBattery, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn handle_event_set_wifi_delegates() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            Device::handle_event(&Event::SetWifi(true), hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn restore_boot_rotation_if_needed_noop_when_rotation_matches() {
        let mut harness = LifecycleHarness::new();
        let boot_rotation = harness.context.device.boot_transformed_rotation();
        harness.context.display.rotation = boot_rotation;

        restore_boot_rotation_if_needed(&mut harness.context);

        assert_eq!(harness.context.display.rotation, boot_rotation);
    }
}
