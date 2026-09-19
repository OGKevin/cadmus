//! Startup handling for a committed update bundle awaiting the platform applicator.

use crate::device::AppContext;
#[cfg(feature = "kobo")]
use crate::device::DevicePaths as _;
use crate::device::inhibitor::Kind;
use crate::fl;
use crate::ota::{StartupAction, clear_restart_marker, reconcile_startup};
use crate::view::{EntryId, Event, Hub, NotificationEvent};
use std::path::Path;

/// Reconciles a leftover update-bundle restart marker at process start.
///
/// Returns `true` when a restart was requested so the caller can skip the rest
/// of startup. That happens when the marker is present and the package is
/// still at [`crate::device::DevicePaths::update_bundle_deploy_path`]. No-op
/// when this platform has no deploy path.
#[cfg(feature = "kobo")]
pub(crate) fn handle_startup_restart_marker(context: &AppContext, hub: &Hub) -> bool {
    let Some(deploy_path) = context.device.update_bundle_deploy_path() else {
        return false;
    };
    let data_dir = context.device.data_dir();
    match reconcile_startup(&data_dir, &deploy_path) {
        Ok(action) => apply_startup_action(action, &data_dir, context, hub),
        Err(error) => {
            tracing::warn!(error = %error, "OTA restart marker reconciliation failed");
            false
        }
    }
}

/// Applies [`StartupAction`]: reboot, notify unapplied, or continue.
///
/// Returns `true` only for [`StartupAction::RequestRestart`].
fn apply_startup_action(
    action: StartupAction,
    data_dir: &Path,
    context: &AppContext,
    hub: &Hub,
) -> bool {
    match action {
        StartupAction::Continue => false,
        StartupAction::RequestRestart => {
            request_reboot_releasing_ota(context, hub);
            true
        }
        StartupAction::ReportUnapplied => {
            if hub
                .send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-update-not-applied"))))
                        .into(),
                )
                .is_err()
            {
                tracing::error!(
                    "failed to queue unapplied OTA notification; keeping restart marker"
                );
                return false;
            }
            if let Err(error) = clear_restart_marker(data_dir) {
                tracing::error!(
                    error = %error,
                    "failed to clear OTA restart marker after queuing notification"
                );
            }
            false
        }
    }
}

/// Drops the `"ota"` Full inhibit, then posts reboot so firmware can apply the package.
fn request_reboot_releasing_ota(context: &AppContext, hub: &Hub) {
    hub.send((Event::ClearDeferredSuspend).into()).ok();
    drop(context.inhibitor.acquire(Kind::Full, "ota"));
    hub.send((Event::Select(EntryId::Reboot)).into()).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::ota::{
        RESTART_ATTEMPT_LIMIT, marker_path, write_attempts_for_test, write_restart_marker,
    };
    use std::fs;
    use std::sync::mpsc::channel;

    fn isolated_data_and_deploy() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let data_dir = tmp.path().join("data");
        let deploy_path = tmp.path().join(".kobo").join("KoboRoot.tgz");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(deploy_path.parent().unwrap()).unwrap();
        (tmp, data_dir, deploy_path)
    }

    #[test]
    fn test_startup_marker_with_bundle_sends_reboot_and_releases_inhibit() {
        let context = create_test_context();
        let (_tmp, data_dir, deploy_path) = isolated_data_and_deploy();
        write_restart_marker(&data_dir).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        let action = reconcile_startup(&data_dir, &deploy_path).unwrap();
        let (hub, rx) = channel();
        assert!(apply_startup_action(action, &data_dir, &context, &hub));
        assert!(!context.inhibitor.full_active());

        let events: Vec<Event> = rx.try_iter().map(|message| message.event).collect();
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, Event::Select(EntryId::Reboot)) })
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::ClearDeferredSuspend))
        );
    }

    #[test]
    fn test_startup_attempt_limit_notifies_and_does_not_reboot() {
        crate::i18n::init(None);
        let context = create_test_context();
        let (_tmp, data_dir, deploy_path) = isolated_data_and_deploy();
        write_attempts_for_test(&data_dir, RESTART_ATTEMPT_LIMIT).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        let action = reconcile_startup(&data_dir, &deploy_path).unwrap();
        let (hub, rx) = channel();
        assert!(!apply_startup_action(action, &data_dir, &context, &hub));
        assert!(!marker_path(&data_dir).exists());

        let events: Vec<Event> = rx.try_iter().map(|message| message.event).collect();
        assert!(
            !events
                .iter()
                .any(|event| { matches!(event, Event::Select(EntryId::Reboot)) })
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Event::Notification(NotificationEvent::Show(label))
                    if label == &fl!("ota-update-not-applied")
            )
        }));
    }

    #[test]
    fn test_startup_attempt_limit_keeps_marker_when_notice_is_not_queued() {
        crate::i18n::init(None);
        let context = create_test_context();
        let (_tmp, data_dir, deploy_path) = isolated_data_and_deploy();
        write_attempts_for_test(&data_dir, RESTART_ATTEMPT_LIMIT).unwrap();
        fs::write(&deploy_path, b"bundle").unwrap();

        let action = reconcile_startup(&data_dir, &deploy_path).unwrap();
        let (hub, rx) = channel();
        drop(rx);
        assert!(!apply_startup_action(action, &data_dir, &context, &hub));
        assert!(marker_path(&data_dir).exists());
    }
}
