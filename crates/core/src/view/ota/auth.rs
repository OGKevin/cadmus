//! GitHub device-flow job owned by the OTA screen.
//!
//! The screen draws the code. This job only talks to GitHub and reports
//! through [`Event::Github`].

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::Instrument as _;

use crate::fl;
use crate::github::{GithubClient, TokenPollResult};
use crate::http::CancelFlag;
use crate::view::github::GithubEvent;
use crate::view::{Event, Generation, Hub};

/// Starts GitHub device-flow sign-in and polls until it finishes.
///
/// The job takes the Wi-Fi lease, asks GitHub for a user code, and sends
/// [`GithubEvent::DeviceAuthStarted`] so the screen can draw that code. It
/// then polls until the reader approves, the code expires, GitHub cancels,
/// or the screen drops the job. Each of those endings is one [`Event::Github`].
/// Every event carries `generation`, so the screen can tell this attempt
/// apart from a later one. This function does not draw the window.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn spawn_device_flow_job(
    hub: &Hub,
    generation: Generation,
    cancelled: Arc<CancelFlag>,
    wifi_session: Arc<crate::device::wifi::WifiSession>,
) -> crate::runtime::Job {
    let hub2 = hub.clone();
    let job_token = cancelled.cancellation_token();
    let parent = tracing::Span::current();
    let span = tracing::info_span!(parent: &parent, "device_flow");
    crate::runtime::Job::with_token(job_token, move |job_cancel| {
        async move {
            let client = match GithubClient::new(None) {
                Ok(client) => client,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to create device-flow client");
                    send_auth_error(&hub2, generation, e.to_string());
                    return;
                }
            };

            let _wifi = match tokio::select! {
                biased;
                () = job_cancel.cancelled() => {
                    tracing::info!("Device flow cancelled before Wi-Fi");
                    return;
                }
                lease = wifi_session.acquire("device-auth") => lease,
            } {
                Ok(lease) => lease,
                Err(e) => {
                    tracing::warn!(error = %e, "device flow could not acquire Wi-Fi");
                    send_auth_error(&hub2, generation, e.to_string());
                    return;
                }
            };

            let response = match unless_cancelled(&job_cancel, client.initiate_device_flow()).await
            {
                Some(Ok(response)) => response,
                Some(Err(e)) => {
                    tracing::error!(error = %e, "Device flow initiation failed");
                    send_auth_error(&hub2, generation, e.to_string());
                    return;
                }
                None => {
                    tracing::info!("Device flow initiation cancelled");
                    return;
                }
            };

            let device_code = response.device_code;
            let mut interval = Duration::from_secs(response.interval);
            let mut poll_errors = 0u32;
            const MAX_CONSECUTIVE_POLL_ERRORS: u32 = 8;

            hub2.send(
                (Event::Github(GithubEvent::DeviceAuthStarted {
                    generation,
                    verification_uri: response.verification_uri,
                    user_code: response.user_code,
                }))
                .into(),
            )
            .ok();

            loop {
                sleep_until_cancelled(&job_cancel, interval).await;
                if job_cancel.is_cancelled() {
                    tracing::info!("Device flow polling cancelled");
                    return;
                }

                let poll =
                    unless_cancelled(&job_cancel, client.poll_device_token(&device_code)).await;
                match poll {
                    None => {
                        tracing::info!("Device flow polling cancelled");
                        return;
                    }
                    Some(Ok(TokenPollResult::Pending)) => {
                        poll_errors = 0;
                    }
                    Some(Ok(TokenPollResult::SlowDown)) => {
                        poll_errors = 0;
                        interval += Duration::from_secs(5);
                    }
                    Some(Ok(TokenPollResult::Complete(token))) => {
                        tracing::info!("Device flow authorization complete");
                        hub2.send(
                            (Event::Github(GithubEvent::DeviceAuthComplete { generation, token }))
                                .into(),
                        )
                        .ok();
                        return;
                    }
                    Some(Ok(TokenPollResult::Expired)) => {
                        tracing::warn!("Device flow code expired");
                        hub2.send(
                            (Event::Github(GithubEvent::DeviceAuthExpired { generation })).into(),
                        )
                        .ok();
                        return;
                    }
                    Some(Ok(TokenPollResult::Cancelled)) => {
                        tracing::info!("Device flow cancelled by user on GitHub");
                        send_auth_error(&hub2, generation, fl!("device-auth-cancelled-on-github"));
                        return;
                    }
                    Some(Err(e)) => {
                        poll_errors += 1;
                        if poll_errors > MAX_CONSECUTIVE_POLL_ERRORS {
                            tracing::error!(error = %e, "Device flow poll error");
                            send_auth_error(&hub2, generation, e.to_string());
                            return;
                        }
                        tracing::warn!(
                            error = %e,
                            attempt = poll_errors,
                            "Device flow poll error, retrying"
                        );
                        interval = interval.saturating_add(Duration::from_secs(2));
                    }
                }
            }
        }
        .instrument(span)
    })
}

/// Tells the screen that this sign-in attempt failed.
fn send_auth_error(hub: &Hub, generation: Generation, message: String) {
    hub.send(
        (Event::Github(GithubEvent::DeviceAuthError {
            generation,
            message,
        }))
        .into(),
    )
    .ok();
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
async fn unless_cancelled<T>(
    cancel: &CancellationToken,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        () = cancel.cancelled() => None,
        value = future => Some(value),
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
async fn sleep_until_cancelled(cancel: &CancellationToken, duration: Duration) {
    tokio::select! {
        () = tokio::time::sleep(duration) => {}
        () = cancel.cancelled() => {}
    }
}
