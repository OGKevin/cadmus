//! Device flow authentication view for GitHub OAuth.
//!
//! Displays the user code and verification URL, then polls GitHub in a
//! background thread until the user authorizes (or the code expires).
//!
//! On success, sends [`Event::Github`] with [`GithubEvent::DeviceAuthComplete`].
//! On expiry, sends [`Event::Github`] with [`GithubEvent::DeviceAuthExpired`].
//! On error, sends [`Event::Github`] with [`GithubEvent::DeviceAuthError`].
//! On cancel, the polling thread is stopped via a shared cancel flag.

use super::button::Button;
use super::filler::Filler;
use super::label::Label;
use super::{Align, Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View, ViewId};
use crate::color::WHITE;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{Fonts, NORMAL_STYLE, font_from_style};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::github::{GithubClient, GithubError, TokenPollResult};
use crate::unit::scale_by_dpi;
use crate::view::github::GithubEvent;
use crate::view::ota::OtaViewId;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// Displays the GitHub device auth flow user code and polls for authorization.
///
/// Shows two lines of text:
/// - The verification URL (`github.com/login/device`)
/// - The user code to enter (e.g. `WDJB-MJHT`)
///
/// A Cancel button stops the background polling thread and closes the view.
/// A background thread polls GitHub at the required interval. When the user
/// authorizes, [`Event::Github`] with [`GithubEvent::DeviceAuthComplete`] is sent through the hub.
pub struct DeviceAuthView {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    view_id: ViewId,
    /// Shared flag — set to `true` to stop the polling thread.
    cancelled: Arc<AtomicBool>,
}

impl DeviceAuthView {
    /// Creates a new device auth view and immediately starts polling.
    ///
    /// Initiates the GitHub device auth flow, builds the UI with the user code,
    /// and spawns a background thread to poll for authorization.
    ///
    /// # Arguments
    ///
    /// * `hub` - Event hub used to send auth result events
    /// * `context` - Application context for font metrics
    ///
    /// # Errors
    ///
    /// If the device flow initiation fails, sends [`Event::Github`] with [`GithubEvent::DeviceAuthError`]
    /// immediately and returns a view with an error message.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn new(hub: &Hub, context: &mut Context) -> Self {
        let id = ID_FEEDER.next();
        let view_id = ViewId::Ota(OtaViewId::DeviceAuth);
        let (width, height) = CURRENT_DEVICE.dims;
        let full_rect = rect![0, 0, width as i32, height as i32];
        let cancelled = Arc::new(AtomicBool::new(false));

        let mut children: Vec<Box<dyn View>> = Vec::new();
        children.push(Box::new(Filler::new(full_rect, WHITE)));

        let (url_text, code_text) = match Self::initiate_and_spawn(hub, Arc::clone(&cancelled)) {
            Ok((url, code)) => (format!("Go to: {}", url), format!("Enter code: {}", code)),
            Err(e) => {
                tracing::error!(error = %e, "Device flow initiation failed");
                hub.send(Event::Github(GithubEvent::DeviceAuthError(e.to_string())))
                    .ok();
                (
                    "GitHub auth failed".to_owned(),
                    "Check logs for details".to_owned(),
                )
            }
        };

        let dpi = CURRENT_DEVICE.dpi;
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
        children.push(Box::new(Label::new(url_rect, url_text, Align::Center)));

        let code_rect = rect![
            padding,
            center_y + padding / 2,
            width as i32 - padding,
            center_y + line_height + padding / 2
        ];
        children.push(Box::new(Label::new(code_rect, code_text, Align::Center)));

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

        Self {
            id,
            rect: full_rect,
            children,
            view_id,
            cancelled,
        }
    }

    /// Initiates the device flow and spawns the polling thread.
    ///
    /// Returns `(verification_uri, user_code)` on success so the caller can
    /// display them. The polling thread checks `cancelled` before each poll
    /// and exits cleanly when it is set.
    fn initiate_and_spawn(
        hub: &Hub,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(String, String), crate::github::GithubError> {
        let client = GithubClient::new(None)?;
        let device_code_response = client.initiate_device_flow().map_err(GithubError::Api)?;

        let verification_uri = device_code_response.verification_uri.clone();
        let user_code = device_code_response.user_code.clone();
        let device_code = device_code_response.device_code.clone();
        let interval_secs = device_code_response.interval;

        tracing::info!(
            user_code = %user_code,
            verification_uri = %verification_uri,
            "Device flow initiated"
        );

        let hub2 = hub.clone();
        let parent_span = tracing::Span::current();

        thread::spawn(move || {
            let _span = tracing::info_span!(parent: &parent_span, "device_flow_poll").entered();

            let poll_client = match GithubClient::new(None) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to create poll client");
                    hub2.send(Event::Github(GithubEvent::DeviceAuthError(e.to_string())))
                        .ok();
                    return;
                }
            };

            let mut interval = Duration::from_secs(interval_secs);

            loop {
                thread::sleep(interval);

                if cancelled.load(Ordering::Relaxed) {
                    tracing::info!("Device flow polling cancelled");
                    return;
                }

                match poll_client.poll_device_token(&device_code) {
                    Ok(TokenPollResult::Pending) => {
                        tracing::debug!("Authorization pending, continuing to poll");
                    }
                    Ok(TokenPollResult::SlowDown) => {
                        interval += Duration::from_secs(5);
                        tracing::debug!(interval_secs = interval.as_secs(), "Slowing down poll");
                    }
                    Ok(TokenPollResult::Complete(token)) => {
                        tracing::info!("Device flow authorization complete");
                        hub2.send(Event::Github(GithubEvent::DeviceAuthComplete(token)))
                            .ok();
                        return;
                    }
                    Ok(TokenPollResult::Expired) => {
                        tracing::warn!("Device flow code expired");
                        hub2.send(Event::Github(GithubEvent::DeviceAuthExpired))
                            .ok();
                        return;
                    }
                    Ok(TokenPollResult::Cancelled) => {
                        tracing::info!("Device flow cancelled by user on GitHub");
                        hub2.send(Event::Github(GithubEvent::DeviceAuthError(
                            "Authorization cancelled".to_owned(),
                        )))
                        .ok();
                        return;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Device flow poll error");
                        hub2.send(Event::Github(GithubEvent::DeviceAuthError(e)))
                            .ok();
                        return;
                    }
                }
            }
        });

        Ok((verification_uri, user_code))
    }

    /// Stops the background polling thread.
    fn cancel_polling(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl View for DeviceAuthView {
    /// Handles events for the device auth view.
    ///
    /// Captures all tap gestures within the view to prevent parent views from
    /// handling them (which would close the modal). The user must use the
    /// Cancel button to close this view and return to the parent.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, _hub, bus, _rq, _context),
            fields(event = ?evt),
            ret(level = tracing::Level::TRACE)
        )
    )]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::Close(id) if *id == self.view_id => {
                self.cancel_polling();
                bus.push_back(Event::Close(ViewId::Ota(OtaViewId::Main)));
                true
            }
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(*center) => true,
            _ => false,
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, _fb, _fonts, _rect), fields(rect = ?_rect))
    )]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

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
        _context: &mut Context,
    ) {
    }
}
