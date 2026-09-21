//! Device flow authentication view for GitHub OAuth.
//!
//! Displays the user code and verification URL, then polls GitHub on the
//! runtime until the user authorizes (or the code expires).
//!
//! On success, sends [`Event::Github`] with [`GithubEvent::DeviceAuthComplete`].
//! On expiry, sends [`Event::Github`] with [`GithubEvent::DeviceAuthExpired`].
//! On error, sends [`Event::Github`] with [`GithubEvent::DeviceAuthError`].
//! On cancel, the polling task is stopped via a shared cancel flag.

use super::button::Button;
use super::filler::Filler;
use super::label::Label;
use super::{Align, Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View, ViewId};
use crate::color::WHITE;
use crate::device::AppContext;
use crate::device::DeviceIdentity as _;
use crate::font::{NORMAL_STYLE, font_from_style};
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::github::{GithubClient, TokenPollResult};
use crate::http::CancelFlag;
use crate::unit::scale_by_dpi;
use crate::view::github::GithubEvent;
use crate::view::ota::OtaViewId;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Displays the GitHub device auth flow user code and polls for authorization.
///
/// Shows two lines of text:
/// - The verification URL (`github.com/login/device`)
/// - The user code to enter (e.g. `WDJB-MJHT`)
///
/// A Cancel button stops the background polling task and closes the view.
/// A runtime task polls GitHub at the required interval. When the user
/// authorizes, [`Event::Github`] with [`GithubEvent::DeviceAuthComplete`] is sent through the hub.
pub struct DeviceAuthView {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    view_id: ViewId,
    /// Shared cancel gate for the device-flow job.
    cancelled: Arc<CancelFlag>,
    /// Initiation *and* polling. Both talk to GitHub, so both are the same
    /// view-owned job; the code arrives through
    /// [`GithubEvent::DeviceAuthStarted`] once initiation succeeds.
    ///
    /// Dropping the view cancels it, so neither a late code nor a late
    /// `DeviceAuthComplete` can land in a view that no longer exists.
    poll_job: Option<crate::runtime::Job>,
    /// Index of the label showing the verification URL.
    url_label_index: usize,
    /// Index of the label showing the user code.
    code_label_index: usize,
}

/// Runs the whole device flow — initiate, report the code, then poll — as one
/// view-owned job.
///
/// Initiation is a GitHub request that can sit in the retry middleware for the
/// client timeout, so the view constructor cannot await it: that would park the
/// caller's runtime worker and freeze input while the user watches a blank
/// overlay. Both halves hold a Wi-Fi lease, for the same reason: without one they
/// can race a radio teardown.
async fn unless_cancelled<T>(
    cancel: &CancellationToken,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        () = cancel.cancelled() => None,
        value = future => Some(value),
    }
}

/// Waits for `duration` unless the device-flow job is cancelled first.
///
/// The token is the view's shared [`CancelFlag`], so [`View::stop_jobs`] and
/// dropping the view-owned [`crate::runtime::Job`] both end the poll loop
/// without sleeping through a full interval.
async fn sleep_until_cancelled(cancel: &CancellationToken, duration: Duration) {
    tokio::select! {
        () = tokio::time::sleep(duration) => {}
        () = cancel.cancelled() => {}
    }
}

fn spawn_device_flow_job(
    hub: &Hub,
    cancelled: Arc<CancelFlag>,
    wifi_session: Arc<crate::device::wifi::WifiSession>,
) -> crate::runtime::Job {
    let hub2 = hub.clone();
    let job_token = cancelled.cancellation_token();
    crate::runtime::Job::with_token(job_token, move |job_cancel| async move {
        let client = match GithubClient::new(None) {
            Ok(client) => client,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create device-flow client");
                hub2.send((Event::Github(GithubEvent::DeviceAuthError(e.to_string()))).into())
                    .ok();
                return;
            }
        };

        let _wifi = match wifi_session.acquire("device-auth").await {
            Ok(lease) => lease,
            Err(e) => {
                tracing::warn!(error = %e, "device flow could not acquire Wi-Fi");
                hub2.send((Event::Github(GithubEvent::DeviceAuthError(e.to_string()))).into())
                    .ok();
                return;
            }
        };

        let response = match unless_cancelled(&job_cancel, client.initiate_device_flow()).await {
            Some(Ok(response)) => response,
            Some(Err(e)) => {
                tracing::error!(error = %e, "Device flow initiation failed");
                hub2.send((Event::Github(GithubEvent::DeviceAuthError(e.to_string()))).into())
                    .ok();
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
        const MAX_POLL_TRANSIENT_ERRORS: u32 = 8;

        hub2.send(
            (Event::Github(GithubEvent::DeviceAuthStarted {
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

            let poll = unless_cancelled(&job_cancel, client.poll_device_token(&device_code)).await;
            match poll {
                None => {
                    tracing::info!("Device flow polling cancelled");
                    return;
                }
                Some(Ok(TokenPollResult::Pending)) => {
                    tracing::debug!("Authorization pending, continuing to poll");
                }
                Some(Ok(TokenPollResult::SlowDown)) => {
                    interval += Duration::from_secs(5);
                    tracing::debug!(interval_secs = interval.as_secs(), "Slowing down poll");
                }
                Some(Ok(TokenPollResult::Complete(token))) => {
                    tracing::info!("Device flow authorization complete");
                    hub2.send((Event::Github(GithubEvent::DeviceAuthComplete(token))).into())
                        .ok();
                    return;
                }
                Some(Ok(TokenPollResult::Expired)) => {
                    tracing::warn!("Device flow code expired");
                    hub2.send((Event::Github(GithubEvent::DeviceAuthExpired)).into())
                        .ok();
                    return;
                }
                Some(Ok(TokenPollResult::Cancelled)) => {
                    tracing::info!("Device flow cancelled by user on GitHub");
                    hub2.send(
                        (Event::Github(GithubEvent::DeviceAuthError(fl!(
                            "device-auth-cancelled-on-github"
                        ))))
                        .into(),
                    )
                    .ok();
                    return;
                }
                Some(Err(e)) => {
                    poll_errors += 1;
                    if poll_errors > MAX_POLL_TRANSIENT_ERRORS {
                        tracing::error!(error = %e, "Device flow poll error");
                        hub2.send(
                            (Event::Github(GithubEvent::DeviceAuthError(e.to_string()))).into(),
                        )
                        .ok();
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
    })
}

impl DeviceAuthView {
    /// Creates the device-auth overlay and starts the view-owned device-flow job.
    ///
    /// Initiation and polling run in a background job; the user code arrives
    /// later through [`GithubEvent::DeviceAuthStarted`]. Failures are reported
    /// through [`GithubEvent::DeviceAuthError`].
    ///
    /// # Arguments
    ///
    /// * `hub` - Event hub used to send auth result events
    /// * `context` - Application context for font metrics
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn new(hub: &Hub, context: &mut AppContext) -> Self {
        let id = ID_FEEDER.next();
        let view_id = ViewId::Ota(OtaViewId::DeviceAuth);
        let (width, height) = context.device.dims();
        let full_rect = rect![0, 0, width as i32, height as i32];
        let cancelled = Arc::new(CancelFlag::new());

        let mut children: Vec<Box<dyn View>> = Vec::new();
        children.push(Box::new(Filler::new(full_rect, WHITE)));

        let dpi = context.device.dpi();
        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;

        let center_y = height as i32 / 2;
        let line_height = 3 * x_height;

        let url_rect = rect![
            padding,
            center_y - line_height - padding / 2,
            width as i32 - padding,
            center_y - padding / 2
        ];
        // Filled in when the device-flow job reports the code.
        let url_label_index = children.len();
        children.push(Box::new(Label::new(
            url_rect,
            fl!("device-auth-connecting"),
            Align::Center,
        )));

        let code_rect = rect![
            padding,
            center_y + padding / 2,
            width as i32 - padding,
            center_y + line_height + padding / 2
        ];
        let code_label_index = children.len();
        children.push(Box::new(Label::new(
            code_rect,
            String::new(),
            Align::Center,
        )));

        let button_width = scale_by_dpi(200.0, dpi) as i32;
        let button_height = scale_by_dpi(40.0, dpi) as i32;
        let button_x = (width as i32 - button_width) / 2;
        let button_y = center_y + line_height + 2 * padding;
        let cancel_rect = rect![
            button_x,
            button_y,
            button_x + button_width,
            button_y + button_height
        ];
        children.push(Box::new(Button::new(
            cancel_rect,
            Event::Close(view_id),
            "Cancel".to_owned(),
        )));

        let poll = spawn_device_flow_job(hub, Arc::clone(&cancelled), context.wifi_session.clone());

        Self {
            id,
            rect: full_rect,
            children,
            view_id,
            cancelled,
            poll_job: Some(poll),
            url_label_index,
            code_label_index,
        }
    }

    /// Stops the background polling job.
    fn cancel_polling(&self) {
        self.cancelled.request_cancel();
        if let Some(job) = &self.poll_job {
            job.cancel();
        }
    }
}

#[async_trait::async_trait(?Send)]
impl View for DeviceAuthView {
    fn stop_jobs(&self) {
        self.cancel_polling();
    }

    fn take_background_jobs(&mut self, jobs: &mut Vec<crate::runtime::Job>) {
        if let Some(job) = self.poll_job.take() {
            jobs.push(job);
        }
    }

    /// Handles events for the device auth view.
    ///
    /// Captures all tap gestures within the view to prevent parent views from
    /// handling them (which would close the modal). The user must use the
    /// Cancel button to close this view and return to the parent.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, _hub, bus, rq, _context),
            fields(event = ?evt),
            ret(level = tracing::Level::TRACE)
        )
    )]
    async fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut AppContext,
    ) -> bool {
        match evt {
            Event::Close(id) if *id == self.view_id => {
                self.cancel_polling();
                bus.push_back(Event::Close(ViewId::Ota(OtaViewId::Main)));
                true
            }
            Event::Github(GithubEvent::DeviceAuthStarted {
                verification_uri,
                user_code,
            }) => {
                if let Some(label) = self.children[self.url_label_index].downcast_mut::<Label>() {
                    label.update(
                        &fl!("device-auth-go-to-uri", uri = verification_uri.as_str()),
                        rq,
                    );
                }
                if let Some(label) = self.children[self.code_label_index].downcast_mut::<Label>() {
                    label.update(
                        &fl!("device-auth-enter-code", code = user_code.as_str()),
                        rq,
                    );
                }
                true
            }
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(*center) => true,
            _ => false,
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, _context, _rect), fields(rect = ?_rect))
    )]
    fn render(&self, _context: &mut AppContext, _rect: Rectangle) {}

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }

    fn view_id(&self) -> Option<ViewId> {
        Some(self.view_id)
    }

    fn resize(
        &mut self,
        _rect: Rectangle,
        _hub: &Hub,
        _rq: &mut RenderQueue,
        _context: &mut AppContext,
    ) {
    }
}
