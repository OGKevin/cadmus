//! Background OTA download.
//!
//! The job reports progress with [`Event::OtaDownloadProgress`]. It leaves the
//! progress screen by sending [`OtaEvent::DownloadFinished`] instead of closing
//! a child view. A committed install still requests reboot after the Wi-Fi
//! lease and the suspend inhibitor drop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use secrecy::SecretString;
use tracing::{Instrument, error, info};

use crate::device::inhibitor::{Inhibitor, InhibitorError, Kind};
use crate::device::wifi::WifiSession;
use crate::fl;
use crate::github::GithubClient;
use crate::ota::{
    CancelFlag, CancelFunc, DeployOutcome, DownloadedArtifact, OtaClient, OtaError, OtaProgress,
    clean_bundled_files, cleanup_ota_cancel,
};
use crate::view::github::GithubEvent;
use crate::view::{EntryId, Event, Generation, Hub, NotificationEvent};

use super::OtaEvent;

/// What the download job tells the screen when it stops short of reboot.
///
/// Declared `pub` because [`OtaEvent::DownloadFinished`] carries it, and that
/// event travels through the public [`Event`]. The module is private, so this
/// type has no path outside the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadOutcome {
    /// The reader cancelled. Partial files are gone.
    Cancelled,
    /// The download or the version check failed. A notification was sent.
    Failed,
}

/// What is being installed.
#[derive(Clone, Copy)]
pub(super) enum OtaDownloadKind {
    /// A pull-request artifact the picker already resolved.
    Pr {
        /// Pull request number, used for the status text and the temp file name.
        number: u64,
        /// GitHub artifact id.
        artifact_id: u64,
        /// Size in bytes, so the download can pick memory or disk.
        size_in_bytes: u64,
    },
    /// Latest artifact from the repository default branch.
    DefaultBranch,
    /// Latest stable release asset.
    StableRelease,
}

impl OtaDownloadKind {
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    pub(super) fn progress_label(self, percent: u8) -> String {
        match self {
            Self::Pr { number, .. } => {
                fl!("ota-downloading-pr", pr_number = number, percent = percent)
            }
            Self::DefaultBranch => fl!("ota-downloading-default-branch", percent = percent),
            Self::StableRelease => fl!("ota-downloading-stable-release", percent = percent),
        }
    }
}

/// Parameters for a background OTA download started by [`run_ota_download`].
pub(super) struct OtaDownloadContext {
    pub(super) kind: OtaDownloadKind,
    /// The install attempt this download belongs to.
    pub(super) generation: Generation,
    pub(super) hub: Hub,
    pub(super) tmp_dir: PathBuf,
    pub(super) install_dir: PathBuf,
    pub(super) github_token: Option<SecretString>,
    pub(super) cancelled: Arc<CancelFlag>,
    pub(super) wifi_session: Arc<WifiSession>,
    pub(super) inhibitor: Arc<Inhibitor>,
}

/// Deletes partial download and staging files after user cancellation.
#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
async fn finish_ota_cancelled(tmp_dir: &Path, deploy_path: &Path) {
    cleanup_ota_cancel(tmp_dir, deploy_path).await;
}

/// Completes a published OTA install, including the committed-but-not-durable
/// case where parent-directory sync failed after rename.
#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
async fn finish_successful_deploy(hub: &Hub, install_dir: &Path, outcome: DeployOutcome) {
    if let DeployOutcome::CommittedNotDurable { path, error } = &outcome {
        tracing::warn!(
            path = ?path,
            error = %error,
            "OTA bundle committed but parent directory was not synced"
        );
    }

    if let Err(e) = clean_bundled_files(install_dir).await {
        tracing::warn!(path = ?install_dir, error = %e, "Failed to clean bundled OTA files");
    }
    send_ota_progress(hub, fl!("ota-installing-and-rebooting"), 100, false);
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
fn report_artifact_download_progress(hub: &Hub, kind: OtaDownloadKind, ota_progress: OtaProgress) {
    if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
        let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
        send_ota_progress(hub, kind.progress_label(percent), percent, true);
    }
}

/// Sends an [`Event::OtaDownloadProgress`] update to the UI thread.
#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
fn send_ota_progress(hub: &Hub, label: String, percent: u8, cancelable: bool) {
    hub.send(
        (Event::OtaDownloadProgress {
            label,
            percent,
            cancelable,
        })
        .into(),
    )
    .ok();
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
fn finish(hub: &Hub, generation: Generation, outcome: DownloadOutcome) {
    hub.send(
        (Event::Ota(OtaEvent::DownloadFinished {
            generation,
            outcome,
        }))
        .into(),
    )
    .ok();
}

/// Runs an OTA download and deployment on the runtime.
///
/// On success, sends a final progress update ("Installing and rebooting…") and
/// requests reboot after the lease and the inhibitor drop. On cancel, deletes
/// partial files and tells the screen to leave progress. On a 401 or missing
/// scopes, sends [`GithubEvent::TokenInvalid`] and does not finish the screen,
/// so the screen can ask the reader to sign in again.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn run_ota_download(ctx: OtaDownloadContext) -> crate::runtime::Job {
    let OtaDownloadContext {
        kind,
        generation,
        hub,
        tmp_dir,
        install_dir,
        github_token,
        cancelled,
        wifi_session,
        inhibitor,
    } = ctx;

    let parent_span = tracing::Span::current();
    let span = match kind {
        OtaDownloadKind::Pr { number, .. } => {
            tracing::info_span!(parent: &parent_span, "pr_download_async", pr_number = number)
        }
        OtaDownloadKind::DefaultBranch => {
            tracing::info_span!(parent: &parent_span, "default_branch_download_async")
        }
        OtaDownloadKind::StableRelease => {
            tracing::info_span!(parent: &parent_span, "stable_release_download_async")
        }
    };
    let reboot_hub = hub.clone();
    let job_cancel = cancelled.cancellation_token();
    crate::runtime::Job::with_token(job_cancel, move |job_cancel| async move {
        let mut unwind_guard = crate::runtime::UnwindGuard::arm({
            let hub = hub.clone();
            move || close_ota_after_panic(&hub, generation)
        });
        let committed = async move {
            let hub2 = hub.clone();
            let should_cancel = CancelFunc::from_flag(&cancelled);
            let mut close_on_drop = crate::runtime::DropGuard::arm({
                let hub = hub2.clone();
                move || finish(&hub, generation, DownloadOutcome::Failed)
            });

            let _full_hold =
                match crate::runtime::spawn_blocking(move || inhibitor.acquire(Kind::Full, "ota"))
                    .await
                {
                    Ok(Ok(guard)) => guard,
                    Ok(Err(InhibitorError::BatteryTooLow)) => {
                        hub2.send(
                            (Event::Notification(NotificationEvent::Show(fl!(
                                "ota-battery-too-low"
                            ))))
                            .into(),
                        )
                        .ok();
                        return false;
                    }
                    Err(error) => {
                        error!(error = %error, "OTA inhibit task failed");
                        return false;
                    }
                };

            let _wifi = match tokio::select! {
                biased;
                _ = job_cancel.cancelled() => return false,
                lease = wifi_session.acquire("ota-download") => lease,
            } {
                Ok(lease) => lease,
                Err(e) => {
                    error!(error = %e, "Failed to acquire WiFi lease for OTA download");
                    hub2.send(
                        (Event::Notification(NotificationEvent::Show(fl!(
                            "notification-not-online"
                        ))))
                        .into(),
                    )
                    .ok();
                    return false;
                }
            };

            let github = match GithubClient::new(github_token) {
                Ok(c) => c,
                Err(e) => {
                    error!(error = %e, "Failed to create GitHub client");
                    hub2.send(
                        (Event::Notification(NotificationEvent::Show(fl!(
                            "ota-client-build-failed"
                        ))))
                        .into(),
                    )
                    .ok();
                    return false;
                }
            };

            let client = OtaClient::new(github, tmp_dir.clone());
            let deploy_path = client.deploy_path();

            send_ota_progress(&hub2, kind.progress_label(0), 0, true);

            let progress = |ota_progress| {
                report_artifact_download_progress(&hub2, kind, ota_progress);
            };
            let download_result = match kind {
                OtaDownloadKind::Pr {
                    number,
                    artifact_id,
                    size_in_bytes,
                } => {
                    crate::runtime::race_cancel(
                        &job_cancel,
                        client.download_pr_artifact_from(
                            number,
                            artifact_id,
                            size_in_bytes,
                            progress,
                            Some(cancelled.as_ref()),
                        ),
                    )
                    .await
                }
                OtaDownloadKind::DefaultBranch => {
                    crate::runtime::race_cancel(
                        &job_cancel,
                        client.download_default_branch_artifact(progress, Some(cancelled.as_ref())),
                    )
                    .await
                }
                OtaDownloadKind::StableRelease => {
                    crate::runtime::race_cancel(
                        &job_cancel,
                        client.download_stable_release_artifact(progress, Some(cancelled.as_ref())),
                    )
                    .await
                }
            };

            if matches!(download_result, Err(OtaError::Cancelled)) {
                finish_ota_cancelled(&tmp_dir, &deploy_path).await;
                finish(&hub2, generation, DownloadOutcome::Cancelled);
                close_on_drop.disarm();
                return false;
            }
            match download_result {
                Ok(artifact) => {
                    info!("Download completed, starting deployment");
                    let deploy_result = match artifact {
                        DownloadedArtifact::Bundle(path) => {
                            client.deploy(path, should_cancel).await
                        }
                        DownloadedArtifact::InMemory(bytes) => {
                            client.extract_and_deploy_bytes(bytes, should_cancel).await
                        }
                        DownloadedArtifact::Zipped(path) => {
                            client.extract_and_deploy(path, should_cancel).await
                        }
                    };

                    if matches!(deploy_result, Err(OtaError::Cancelled)) {
                        finish_ota_cancelled(&tmp_dir, &deploy_path).await;
                        finish(&hub2, generation, DownloadOutcome::Cancelled);
                        close_on_drop.disarm();
                        return false;
                    }

                    match deploy_result {
                        Ok(outcome) => {
                            hub2.send((Event::ClearDeferredSuspend).into()).ok();
                            finish_successful_deploy(&hub2, &install_dir, outcome).await;
                            close_on_drop.disarm();
                            return true;
                        }
                        Err(e) => {
                            error!(error = %e, "Deployment failed");
                            hub2.send(
                                (Event::Notification(NotificationEvent::Show(fl!(
                                    "ota-deployment-failed"
                                ))))
                                .into(),
                            )
                            .ok();
                        }
                    }
                }
                Err(OtaError::Unauthorized) | Err(OtaError::InsufficientScopes(_)) => {
                    tracing::warn!("GitHub token rejected — triggering re-auth");
                    hub2.send((Event::Github(GithubEvent::TokenInvalid)).into())
                        .ok();
                    close_on_drop.disarm();
                }
                Err(e) => {
                    error!(error = %e, "OTA download failed");
                    hub2.send(
                        (Event::Notification(NotificationEvent::Show(fl!("ota-download-failed"))))
                            .into(),
                    )
                    .ok();
                }
            }
            false
        }
        .instrument(span)
        .await;
        if committed {
            request_ota_reboot(reboot_hub).await;
        }
        unwind_guard.disarm();
    })
}

/// Tells the screen the download task panicked.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn close_ota_after_panic(hub: &Hub, generation: Generation) {
    error!("OTA download task panicked");
    finish(hub, generation, DownloadOutcome::Failed);
    hub.send((Event::Notification(NotificationEvent::Show(fl!("ota-download-failed")))).into())
        .ok();
}

/// After a committed OTA install, queues reboot on the hub.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn request_ota_reboot(hub: Hub) {
    if hub.send((Event::Select(EntryId::Reboot)).into()).is_err() {
        tracing::warn!("hub closed after OTA commit");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_tells_the_screen_the_download_failed() {
        let (hub, mut rx) = crate::view::hub_channel();
        let generation = Generation::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = crate::runtime::UnwindGuard::arm(move || {
                close_ota_after_panic(&hub, generation);
            });
            panic!("download task");
        }));
        assert!(result.is_err());

        let finished = rx.try_recv().expect("finished");
        assert!(matches!(
            finished.event,
            Event::Ota(OtaEvent::DownloadFinished {
                generation: reported,
                outcome: DownloadOutcome::Failed,
            }) if reported == generation
        ));
        let note = rx.try_recv().expect("notification");
        assert!(matches!(
            note.event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-download-failed")
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_after_commit_still_requests_reboot() {
        use std::time::Duration;

        let flag = CancelFlag::new();
        assert!(flag.try_commit());
        flag.request_cancel();
        assert!(!flag.is_cancelled());

        let (hub, mut rx) = crate::view::hub_channel();
        let job = crate::runtime::Job::spawn(move |_| request_ota_reboot(hub));
        crate::runtime::finish_within_deadline(vec![job], Duration::from_secs(5)).await;

        let message = rx.try_recv().expect("reboot request");
        assert!(matches!(message.event, Event::Select(EntryId::Reboot)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_committed_install_queues_its_reboot_for_the_shutdown_drain() {
        use std::time::Duration;

        let (hub, mut rx) = crate::view::hub_channel();
        let job = crate::runtime::Job::spawn(move |_| request_ota_reboot(hub));

        let premature: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|message| message.event)
            .collect();
        assert!(
            !premature
                .iter()
                .any(|event| matches!(event, Event::Select(EntryId::Reboot))),
            "reboot must not be visible before the download job is joined"
        );

        crate::runtime::finish_within_deadline(vec![job], Duration::from_secs(5)).await;

        let drained: Vec<Event> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|message| message.event)
            .collect();
        assert!(
            drained
                .iter()
                .any(|event| matches!(event, Event::Select(EntryId::Reboot))),
            "a committed install must leave a reboot request for the drain"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_download_removes_the_partial_artifact() {
        let tmp = tempfile::Builder::new()
            .prefix("cadmus-ota-cancel-")
            .tempdir()
            .expect("tempdir");
        let deploy_path = tmp.path().join("KoboRoot.tgz");
        let partial = tmp.path().join("cadmus-ota-42.zip");
        std::fs::write(&partial, b"partial").unwrap();

        finish_ota_cancelled(tmp.path(), &deploy_path).await;

        assert!(
            !partial.exists(),
            "a cancelled download must leave no partial artifact"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_during_the_api_phase_does_not_wait_for_the_request() {
        use std::time::Duration;

        let cancelled = Arc::new(CancelFlag::new());
        let token = cancelled.cancellation_token();
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();

        let request = {
            let cancelled = Arc::clone(&cancelled);
            async move {
                let _dropped = DroppedRequest(Some(dropped_tx));
                tokio::time::sleep(Duration::from_millis(50)).await;
                cancelled.request_cancel();
                tokio::time::sleep(Duration::from_secs(24)).await;
                Ok(7_u32)
            }
        };

        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            crate::runtime::race_cancel(&token, request),
        )
        .await
        .expect("cancel must not wait out the request timeout");

        assert!(matches!(outcome, Err(OtaError::Cancelled)));
        dropped_rx
            .await
            .expect("the in-flight request must be dropped, not awaited");
    }

    struct DroppedRequest(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for DroppedRequest {
        fn drop(&mut self) {
            if let Some(dropped) = self.0.take() {
                dropped.send(()).ok();
            }
        }
    }

    #[test]
    fn progress_label_names_the_kind() {
        assert_eq!(
            OtaDownloadKind::DefaultBranch.progress_label(12),
            fl!("ota-downloading-default-branch", percent = 12)
        );
        assert_eq!(
            OtaDownloadKind::StableRelease.progress_label(0),
            fl!("ota-downloading-stable-release", percent = 0)
        );
        assert_eq!(
            OtaDownloadKind::Pr {
                number: 7,
                artifact_id: 1,
                size_in_bytes: 2,
            }
            .progress_label(3),
            fl!("ota-downloading-pr", pr_number = 7, percent = 3)
        );
    }

    #[test]
    fn download_progress_is_posted_on_the_hub() {
        let (hub, mut rx) = crate::view::hub_channel();
        send_ota_progress(&hub, "x".into(), 40, true);
        assert!(matches!(
            rx.try_recv().expect("progress").event,
            Event::OtaDownloadProgress {
                percent: 40,
                cancelable: true,
                ..
            }
        ));

        report_artifact_download_progress(
            &hub,
            OtaDownloadKind::DefaultBranch,
            OtaProgress::DownloadingArtifact {
                downloaded: 50,
                total: 100,
            },
        );
        assert!(matches!(
            rx.try_recv().expect("artifact").event,
            Event::OtaDownloadProgress {
                percent: 50,
                cancelable: true,
                ..
            }
        ));

        report_artifact_download_progress(
            &hub,
            OtaDownloadKind::DefaultBranch,
            OtaProgress::Complete,
        );
        assert!(rx.try_recv().is_err());

        finish(&hub, Generation::default(), DownloadOutcome::Cancelled);
        assert!(matches!(
            rx.try_recv().expect("finished").event,
            Event::Ota(OtaEvent::DownloadFinished {
                outcome: DownloadOutcome::Cancelled,
                ..
            })
        ));
    }
}
