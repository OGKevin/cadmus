//! USB mass-storage share preparation and session handling.

use super::{KOBO_UPDATE_BUNDLE, schedule_device_task};
use crate::device::DeviceHardware as _;
use crate::device::usb::UsbManager;
#[cfg(not(feature = "test"))]
use crate::device::wifi::WifiManager;
use crate::device::{
    AppContext, DeviceRuntime, DeviceTask, DeviceTaskId, EventOutcome, HistoryItem,
};
use crate::device::{DeviceInput, DevicePaths as _, InputSource};
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::settings::IntermKind;
use crate::view::intermission::Intermission;
use crate::view::{
    Bus, EntryId, Event, Hub, NotificationEvent, RenderData, RenderQueue, UpdateData, View,
    wait_for_all,
};
use std::env;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Prepares the app for USB mass-storage sharing.
///
/// Unwinds the view stack to the root, persists settings, closes the
/// database, disables frontlight and WiFi, shows a share intermission, and
/// sends [`Event::Share`]. Pending device tasks are cleared because the USB
/// session owns the process until unplug.
#[allow(clippy::too_many_arguments)]
fn prepare_usb_share(
    context: &mut AppContext,
    view: &mut Box<dyn View>,
    history: &mut Vec<HistoryItem>,
    tasks: &mut Vec<DeviceTask>,
    settings_manager: &crate::settings::versioned::SettingsManager,
    updating: &mut Vec<UpdateData>,
    bus: &mut Bus,
    hub: &Sender<Event>,
    rq: &mut RenderQueue,
) {
    tasks.clear();
    view.handle_event(&Event::Back, hub, bus, rq, context);
    while let Some(mut item) = history.pop() {
        item.view.handle_event(&Event::Back, hub, bus, rq, context);
        if item.rotation != context.display.rotation {
            wait_for_all(updating, context);
            if context.set_rotation(item.rotation).is_ok() {
                context
                    .device
                    .input()
                    .send_raw(crate::input::display_rotate_event(item.rotation));
            }
        }
        *view = item.view;
    }
    settings_manager
        .save(&context.settings)
        .map_err(|error| tracing::error!(error = %error, "Can't save settings"))
        .ok();
    context.database.close();

    if context.settings.frontlight {
        context.set_frontlight(false);
    }
    #[cfg(not(feature = "test"))]
    if context.settings.wifi {
        if let Ok(wifi) = context.device.wifi_manager()
            && let Err(error) = wifi.disable()
        {
            tracing::error!(error = %error, "Failed to disable WiFi for USB share");
        }
        context.online = false;
    }

    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        IntermKind::Share,
        context,
    );
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        UpdateMode::Full,
    ));
    view.children_mut().push(Box::new(interm));
    hub.send(Event::Share).ok();
}

/// Enables USB mass-storage mode.
///
/// Redirects logging to `/tmp`, exposes onboard storage via the USB manager,
/// and moves the working directory to `/tmp`.
///
/// On failure, shows a notification and schedules a restart or reboot.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(hub, tasks, context)))]
fn enable_usb_share(context: &mut AppContext, hub: &Sender<Event>, tasks: &mut Vec<DeviceTask>) {
    if let Err(error) = crate::logging::redirect_log_to_dir(
        Path::new("/tmp/cadmus-logs"),
        &context.settings.logging,
    ) {
        eprintln!("Failed to redirect logging to /tmp: {error}");

        hub.send(Event::Notification(NotificationEvent::Show(
            "Failed to start USB session".to_string(),
        )))
        .ok();
        schedule_device_task(
            DeviceTaskId::Exit,
            Event::Select(EntryId::Restart),
            Duration::from_secs(3),
            hub,
            tasks,
        );

        return;
    }

    match context.device.usb_manager() {
        Ok(usb_manager) => match usb_manager.enable() {
            Ok(()) => {
                context.shared = true;
                if let Err(error) = env::set_current_dir("/tmp") {
                    tracing::error!(error = %error, "failed to set working directory to /tmp before USB share");
                }
            }
            Err(error) => {
                tracing::error!(error = %error, "Failed to enable USB sharing");
                hub.send(Event::Notification(NotificationEvent::Show(
                    "Failed to start USB session".to_string(),
                )))
                .ok();
                schedule_device_task(
                    DeviceTaskId::Exit,
                    Event::Select(EntryId::Reboot),
                    Duration::from_secs(3),
                    hub,
                    tasks,
                );
            }
        },
        Err(error) => {
            tracing::error!(error = %error, "Failed to create USB manager");
            hub.send(Event::Notification(NotificationEvent::Show(
                "Failed to start USB session".to_string(),
            )))
            .ok();
            schedule_device_task(
                DeviceTaskId::Exit,
                Event::Select(EntryId::Restart),
                Duration::from_secs(3),
                hub,
                tasks,
            );
        }
    }
}

/// Disables USB mass-storage mode after the host unplugs.
///
/// Restores logging and the original working directory, then triggers a
/// reboot if `KoboRoot.tgz` is present on onboard storage or an app restart
/// otherwise.
pub(super) fn disable_usb_share(
    context: &AppContext,
    startup_cwd: Option<&Path>,
    logging_settings: &crate::settings::LoggingSettings,
    hub: &Sender<Event>,
) {
    tracing::info!("USB unplugged after sharing; disabling USB mass storage");

    match context.device.usb_manager() {
        Ok(usb_manager) => match usb_manager.disable() {
            Ok(()) => {
                tracing::info!("USB mass storage disabled successfully");
                if startup_cwd.is_some() {
                    let log_dir = context.device.data_path(&logging_settings.directory);
                    if let Err(error) =
                        crate::logging::redirect_log_to_dir(&log_dir, logging_settings)
                    {
                        eprintln!("Failed to restore logging after USB unshare: {error}");
                    }
                }
            }
            Err(error) => {
                tracing::error!(error = %error, "Failed to disable USB sharing, triggering reboot");
                hub.send(Event::Select(EntryId::Reboot)).ok();
                return;
            }
        },
        Err(error) => {
            tracing::error!(error = %error, "Failed to create USB manager, triggering reboot");
            hub.send(Event::Select(EntryId::Reboot)).ok();
            return;
        }
    }

    if let Some(cwd) = startup_cwd
        && let Err(error) = env::set_current_dir(cwd)
    {
        tracing::error!(error = %error, original_cwd = %cwd.display(), "failed to restore working directory after USB share");
    }

    let update_bundle_exists = Path::new(KOBO_UPDATE_BUNDLE).exists();
    tracing::info!(update_bundle_exists, "filesystem state after USB disable");

    if update_bundle_exists {
        tracing::info!("KoboRoot.tgz detected; triggering reboot");
        hub.send(Event::Select(EntryId::Reboot)).ok();
    } else {
        tracing::info!("triggering app restart");
        hub.send(Event::Select(EntryId::Restart)).ok();
    }
}

/// Handles [`Event::PrepareShare`] by unwinding the UI and queuing share setup.
fn handle_prepare_share(
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.shared {
        return EventOutcome::Handled;
    }

    let Some(settings_manager) = runtime.settings_manager else {
        tracing::error!("PrepareShare requires a settings manager");
        return EventOutcome::Error;
    };

    prepare_usb_share(
        context,
        runtime.view,
        runtime.history,
        runtime.tasks,
        settings_manager,
        runtime.updating,
        bus,
        hub,
        rq,
    );

    EventOutcome::Handled
}

/// Handles [`Event::Share`] by enabling USB mass-storage mode.
fn handle_share(
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.shared {
        return EventOutcome::Handled;
    }

    enable_usb_share(context, hub, runtime.tasks);

    EventOutcome::Handled
}

/// Dispatches USB-share lifecycle events.
pub(super) fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::PrepareShare => handle_prepare_share(hub, bus, rq, context, runtime),
        Event::Share => handle_share(hub, context, runtime),
        _ => EventOutcome::Unhandled,
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::view::EntryId;

    #[test]
    fn handle_prepare_share_returns_error_without_settings_manager() {
        let mut harness = LifecycleHarness::new();
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            runtime.settings_manager = None;
            handle_prepare_share(hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Error);
    }

    #[test]
    fn handle_prepare_share_early_return_when_shared() {
        let mut harness = LifecycleHarness::new();
        harness.context.shared = true;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareShare, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn handle_share_early_return_when_shared() {
        let mut harness = LifecycleHarness::new();
        harness.context.shared = true;
        let hub = harness.hub_tx.clone();
        let outcome = harness.with_runtime_only(|context, runtime| {
            handle_event(
                &Event::Share,
                &hub,
                &mut Bus::new(),
                &mut RenderQueue::new(),
                context,
                runtime,
            )
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[test]
    fn handle_share_enables_usb_mass_storage() {
        let mut harness = LifecycleHarness::new();
        let hub = harness.hub_tx.clone();
        let outcome =
            harness.with_runtime_only(|context, runtime| handle_share(&hub, context, runtime));
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.context.shared);
        assert_eq!(
            harness
                .context
                .device
                .usb_manager_for_test()
                .enable_call_count(),
            1
        );
        assert_eq!(
            harness.context.device.usb_manager_for_test().enabled(),
            Some(true)
        );
    }

    #[test]
    fn disable_usb_share_disables_mass_storage() {
        let harness = LifecycleHarness::new();
        disable_usb_share(
            &harness.context,
            None,
            &harness.context.settings.logging,
            &harness.hub_tx,
        );
        let usb = harness.context.device.usb_manager_for_test();
        assert_eq!(usb.disable_call_count(), 1);
        assert_eq!(usb.enabled(), Some(false));
        assert!(
            harness
                .drain_hub()
                .iter()
                .any(|e| matches!(e, Event::Select(EntryId::Restart)))
        );
    }
}
