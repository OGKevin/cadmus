//! USB mass-storage share preparation and session handling.

use crate::device::DeviceHardware as _;
use crate::device::schedule_device_task;
use crate::device::usb::UsbManager;
use crate::device::{
    AppContext, DeviceRuntime, DeviceTask, DeviceTaskId, EventOutcome, ExitStatus, HistoryItem,
};
use crate::device::{DeviceInput, DevicePaths as _, InputSource};
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::settings::IntermKind;
use crate::view::intermission::Intermission;
use crate::view::{
    Bus, Event, Hub, NotificationEvent, RenderData, RenderQueue, UpdateData, View, wait_for_all,
};
use std::env;
use std::path::Path;
use std::time::Duration;

/// Prepares the app for USB mass-storage sharing.
///
/// Unwinds the view stack to the root, persists settings, closes the library
/// and dictionary databases, disables frontlight and WiFi, shows a share
/// intermission, and sends [`Event::Share`]. Pending device tasks are cleared
/// because the USB session owns the process until unplug.
#[allow(clippy::too_many_arguments)]
async fn prepare_usb_share(
    context: &mut AppContext,
    view: &mut Box<dyn View>,
    history: &mut Vec<HistoryItem>,
    tasks: &mut Vec<DeviceTask>,
    settings_manager: &crate::settings::versioned::SettingsManager,
    updating: &mut Vec<UpdateData>,
    bus: &mut Bus,
    hub: &crate::view::Hub,
    rq: &mut RenderQueue,
    background_tasks: Option<&mut crate::task::TaskManager>,
) {
    tasks.clear();
    view.handle_event(&Event::Back, hub, bus, rq, context).await;
    while let Some(mut item) = history.pop() {
        item.view
            .handle_event(&Event::Back, hub, bus, rq, context)
            .await;
        if item.rotation != context.display.rotation {
            wait_for_all(updating, context).await;
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
        .await
        .map_err(|error| tracing::error!(error = %error, "Can't save settings"))
        .ok();
    context.database.close().await;
    if let Some(tasks) = background_tasks {
        tasks.close_dictionary_index().await;
    }

    if context.settings.frontlight {
        context.set_frontlight(false).await;
    }
    #[cfg(not(feature = "test"))]
    if context.settings.wifi != crate::settings::WifiMode::Off {
        if let Err(error) = context.wifi_session.disable_radio().await {
            tracing::error!(error = %error, "Failed to disable WiFi for USB share");
        }
        context.online = false;
    }

    let interm = Intermission::new(
        context.device.framebuffer().rect(),
        IntermKind::Share,
        context,
    )
    .await;
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        UpdateMode::Full,
    ));
    view.children_mut().push(Box::new(interm));
    hub.send((Event::Share).into()).ok();
}

/// Enables USB mass-storage mode.
///
/// Redirects logging to `/tmp`, exposes onboard storage via the USB manager,
/// and moves the working directory to `/tmp`.
///
/// On failure, shows a notification and then schedules [`Event::Exit`] with a
/// restart or reboot after a short delay.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(hub, tasks, context)))]
async fn enable_usb_share(
    context: &mut AppContext,
    hub: &crate::view::Hub,
    tasks: &mut Vec<DeviceTask>,
) {
    if let Err(error) = crate::logging::redirect_log_to_dir(
        Path::new("/tmp/cadmus-logs"),
        &context.settings.logging,
    ) {
        eprintln!("Failed to redirect logging to /tmp: {error}");

        hub.send(
            (Event::Notification(NotificationEvent::Show(
                "Failed to start USB session".to_string(),
            )))
            .into(),
        )
        .ok();
        schedule_device_task(
            DeviceTaskId::Exit,
            Event::Exit(ExitStatus::Restart),
            Duration::from_secs(3),
            hub,
            tasks,
        );

        return;
    }

    match context.device.usb_manager() {
        Ok(usb_manager) => match usb_manager.enable().await {
            Ok(()) => {
                context.shared = true;
                if let Err(error) = env::set_current_dir("/tmp") {
                    tracing::error!(error = %error, "failed to set working directory to /tmp before USB share");
                }
            }
            Err(error) => {
                tracing::error!(error = %error, "Failed to enable USB sharing");
                hub.send(
                    (Event::Notification(NotificationEvent::Show(
                        "Failed to start USB session".to_string(),
                    )))
                    .into(),
                )
                .ok();
                schedule_device_task(
                    DeviceTaskId::Exit,
                    Event::Exit(ExitStatus::Reboot),
                    Duration::from_secs(3),
                    hub,
                    tasks,
                );
            }
        },
        Err(error) => {
            tracing::error!(error = %error, "Failed to create USB manager");
            hub.send(
                (Event::Notification(NotificationEvent::Show(
                    "Failed to start USB session".to_string(),
                )))
                .into(),
            )
            .ok();
            schedule_device_task(
                DeviceTaskId::Exit,
                Event::Exit(ExitStatus::Restart),
                Duration::from_secs(3),
                hub,
                tasks,
            );
        }
    }
}

/// Disables USB mass-storage mode after the host unplugs.
///
/// Returns [`ExitStatus::Reboot`] immediately if the USB manager cannot be
/// created or disabling fails. Otherwise it restores logging and the original
/// working directory, then returns [`ExitStatus::Reboot`] when `update_bundle`
/// is present on onboard storage and [`ExitStatus::Restart`] otherwise.
///
/// This sends no [`Event::Select`]: those exits carry user intent and the
/// device may refuse them while a USB share session or a Full lease is active.
/// Lifecycle code that must end the process returns [`EventOutcome::Exit`] with
/// the required [`ExitStatus`] instead, as [`enable_usb_share`] does by
/// scheduling [`Event::Exit`] when share start fails.
pub(super) async fn disable_usb_share(
    context: &AppContext,
    startup_cwd: Option<&Path>,
    logging_settings: &crate::settings::LoggingSettings,
    update_bundle: &Path,
) -> ExitStatus {
    tracing::info!("USB unplugged after sharing; disabling USB mass storage");

    match context.device.usb_manager() {
        Ok(usb_manager) => match usb_manager.disable().await {
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
                return ExitStatus::Reboot;
            }
        },
        Err(error) => {
            tracing::error!(error = %error, "Failed to create USB manager, triggering reboot");
            return ExitStatus::Reboot;
        }
    }

    if let Some(cwd) = startup_cwd
        && let Err(error) = env::set_current_dir(cwd)
    {
        tracing::error!(error = %error, original_cwd = %cwd.display(), "failed to restore working directory after USB share");
    }

    let update_bundle_exists = tokio::fs::try_exists(update_bundle).await.unwrap_or(false);
    tracing::info!(update_bundle_exists, "filesystem state after USB disable");

    if update_bundle_exists {
        tracing::info!("KoboRoot.tgz detected; triggering reboot");
        ExitStatus::Reboot
    } else {
        tracing::info!("triggering app restart");
        ExitStatus::Restart
    }
}

/// Handles [`Event::PrepareShare`] by unwinding the UI and queuing share setup.
async fn handle_prepare_share(
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
        runtime.background_tasks.as_deref_mut(),
    )
    .await;

    EventOutcome::Handled
}

/// Handles [`Event::Share`] by enabling USB mass-storage mode.
async fn handle_share(
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    if context.shared {
        return EventOutcome::Handled;
    }

    enable_usb_share(context, hub, runtime.tasks).await;

    EventOutcome::Handled
}

/// Dispatches USB-share lifecycle events.
pub(super) async fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::PrepareShare => handle_prepare_share(hub, bus, rq, context, runtime).await,
        Event::Share => handle_share(hub, context, runtime).await,
        _ => EventOutcome::Unhandled,
    }
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::test_harness::DeviceRuntimeHarness;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_prepare_share_returns_error_without_settings_manager() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            runtime.settings_manager = None;
            handle_prepare_share(hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Error);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_prepare_share_early_return_when_shared() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.shared = true;
        let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            handle_event(&Event::PrepareShare, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_share_early_return_when_shared() {
        let mut harness = DeviceRuntimeHarness::new().await;
        harness.context.shared = true;
        let hub = harness.hub_tx.clone();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let outcome = crate::poll_runtime_only!(harness, |context, runtime| {
            handle_event(&Event::Share, &hub, &mut bus, &mut rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Handled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handle_share_enables_usb_mass_storage() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let hub = harness.hub_tx.clone();
        let outcome = crate::poll_runtime_only!(harness, |context, runtime| {
            handle_share(&hub, context, runtime)
        });
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disable_usb_share_disables_mass_storage() {
        let harness = DeviceRuntimeHarness::new().await;
        let dir = tempfile::tempdir().expect("temp dir");
        let status = disable_usb_share(
            &harness.context,
            None,
            &harness.context.settings.logging,
            &dir.path().join("KoboRoot.tgz"),
        )
        .await;
        assert_eq!(status, ExitStatus::Restart);
        let usb = harness.context.device.usb_manager_for_test();
        assert_eq!(usb.disable_call_count(), 1);
        assert_eq!(usb.enabled(), Some(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disable_usb_share_reboots_when_update_bundle_present() {
        let harness = DeviceRuntimeHarness::new().await;
        let dir = tempfile::tempdir().expect("temp dir");
        let bundle = dir.path().join("KoboRoot.tgz");
        tokio::fs::write(&bundle, b"update")
            .await
            .expect("write bundle");
        let status = disable_usb_share(
            &harness.context,
            None,
            &harness.context.settings.logging,
            &bundle,
        )
        .await;
        assert_eq!(status, ExitStatus::Reboot);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disable_usb_share_reboots_when_disable_fails() {
        let harness = DeviceRuntimeHarness::new().await;
        let dir = tempfile::tempdir().expect("temp dir");
        harness
            .context
            .device
            .usb_manager_for_test()
            .set_fail_disable(true);
        let status = disable_usb_share(
            &harness.context,
            None,
            &harness.context.settings.logging,
            &dir.path().join("KoboRoot.tgz"),
        )
        .await;
        assert_eq!(status, ExitStatus::Reboot);
        assert_eq!(
            harness
                .context
                .device
                .usb_manager_for_test()
                .disable_call_count(),
            1
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_share_start_reboots_even_while_a_full_lease_is_held() {
        let mut harness = DeviceRuntimeHarness::new().await;
        let _full = harness
            .context
            .inhibitor
            .acquire(crate::device::inhibitor::Kind::Full, "ota")
            .expect("full lease");
        harness
            .context
            .device
            .usb_manager_for_test()
            .set_fail_enable(true);
        let hub = harness.hub_tx.clone();
        let mut tasks = Vec::new();

        enable_usb_share(&mut harness.context, &hub, &mut tasks).await;

        assert!(!harness.context.shared);
        assert!(tasks.iter().any(|task| task.id == DeviceTaskId::Exit));
        let immediate = harness.drain_hub();
        assert!(
            immediate
                .iter()
                .any(|event| matches!(event, Event::Notification(NotificationEvent::Show(_))))
        );
        assert!(
            !immediate
                .iter()
                .any(|event| matches!(event, Event::Exit(_)))
        );

        tokio::time::sleep(Duration::from_millis(3500)).await;
        let delayed = harness.drain_hub();
        assert_eq!(delayed.len(), 1);
        assert!(matches!(delayed[0], Event::Exit(ExitStatus::Reboot)));
    }
}
