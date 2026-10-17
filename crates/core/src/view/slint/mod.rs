//! Shared software-rendering host for Slint-backed views.
//!
//! Use [`open`] to create a [`Session`], and [`SlintView`] for common event
//! forwarding and frame blitting. Generated component types live in
//! [`generated`]. Reusable overlay state lives in [`components`]. [`Component`]
//! is the callback loop a screen implements for each of its pieces.

mod component;
pub(super) mod components;
pub(super) mod generated;
mod model;
mod queue;
mod renderer;

pub(in crate::view) use component::{Component, Env};
pub(crate) use model::changed_model;
pub(crate) use queue::{CallbackQueue, connect};
pub(crate) use renderer::Session;

/// What the view loop should do with the framebuffer after Slint callbacks.
///
/// A screen reduces its own effect to this. [`Flow::Idle`] leaves the captured
/// frame. [`Flow::Redraw`] captures the window and queues a panel update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    /// The captured frame is still current.
    Idle,
    /// The window changed. Capture it and redraw.
    Redraw,
}

impl Flow {
    /// A redraw wins over an idle result from another component in the same drain.
    pub(crate) fn join(self, later: Self) -> Self {
        match (self, later) {
            (Self::Redraw, _) | (_, Self::Redraw) => Self::Redraw,
            _ => Self::Idle,
        }
    }
}

use ::slint::ComponentHandle;
use anyhow::Result;

use crate::device::{AppContext, Device};
use crate::framebuffer::{Framebuffer as _, UpdateMode};
use crate::input::{DeviceEvent, FingerStatus};
use crate::view::{Bus, Hub, RenderData, RenderQueue, View};

/// What one forwarded event did to the window, before callbacks are applied.
///
/// Crate-visible because [`SlintView::settle`] takes it. The fields stay
/// private to this module.
pub(crate) struct AfterInput {
    /// Slint captured a frame for this event.
    repainted: bool,
    /// Leave that frame off the panel. A press preview waits for the release.
    skip: bool,
    /// The finger is down, so a keyboard key can flash.
    finger_down: bool,
}

/// Shared event forwarding and frame blitting for Slint-backed views.
///
/// [`drive`](SlintView::drive) forwards one event, applies queued callbacks,
/// and queues the dirty rectangles. [`on_timer`](SlintView::on_timer) does the
/// same for a timer wake. A screen that handles a button itself, such as the
/// settings category keys, calls these for every other event.
#[async_trait::async_trait(?Send)]
pub(crate) trait SlintView: View {
    /// Generated component this screen draws.
    type Component: ComponentHandle;

    /// The open session, if the screen has one.
    fn session(&self) -> Option<&Session<Self::Component>>;

    /// Applies callbacks the window queued during the last dispatch.
    ///
    /// The default does nothing. A screen with a callback queue overrides this.
    async fn apply_callbacks(
        &mut self,
        _hub: &Hub,
        _bus: &mut Bus,
        _context: &mut AppContext,
    ) -> Flow {
        Flow::Idle
    }

    /// Copies the screen's state into the window and captures a frame.
    fn sync_window(&mut self, _context: &AppContext) {}

    /// Whether the frame Slint just captured should stay off the panel.
    fn skip_preview(&self, _finger_down: bool, _finger_preview: bool) -> bool {
        false
    }

    /// Waveform for the rectangles [`queue_frame`](Self::queue_frame) is about
    /// to add. `(mode, wait)`.
    fn panel_refresh(&self, _finger_down: bool) -> (UpdateMode, bool) {
        (UpdateMode::Gui, true)
    }

    /// Copies `rect` of the captured Slint frame into the device framebuffer.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    fn blit_frame(&self, context: &mut AppContext, rect: &crate::geom::Rectangle) {
        let Some(session) = self.session() else {
            return;
        };
        let (framebuffer, _, _) = context.framebuffer_and_fonts();
        if let Err(error) = session.blit_rect(framebuffer, rect) {
            tracing::error!(
                error = %error,
                %rect,
                component = std::any::type_name::<Self::Component>(),
                "slint blit failed"
            );
        }
    }

    /// Forwards one device event into the Slint item tree.
    ///
    /// Returns whether Slint captured a new frame. The event is not queued.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all, fields(event = ?event), ret))]
    fn forward_device(&self, event: DeviceEvent) -> bool {
        self.session()
            .is_some_and(|session| session.handle_device(event))
    }

    /// Forwards `event`, applies what that queued, and queues the dirty rectangles.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(level = "trace", skip_all, fields(event = ?event))
    )]
    async fn drive(
        &mut self,
        event: DeviceEvent,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let finger_down = matches!(
            event,
            DeviceEvent::Finger {
                status: FingerStatus::Down,
                ..
            }
        );
        let finger_preview = matches!(
            event,
            DeviceEvent::Finger {
                status: FingerStatus::Down | FingerStatus::Motion,
                ..
            }
        );
        let after = AfterInput {
            repainted: self.forward_device(event),
            skip: self.skip_preview(finger_down, finger_preview),
            finger_down,
        };
        self.settle(after, hub, bus, rq, context).await;
    }

    /// Runs due Slint timers, then the same settle path as [`drive`](Self::drive).
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    async fn on_timer(
        &mut self,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let repainted = self.session().is_some_and(|session| session.tick());
        self.settle(
            AfterInput {
                repainted,
                skip: false,
                finger_down: false,
            },
            hub,
            bus,
            rq,
            context,
        )
        .await;
    }

    /// Applies queued callbacks, copies the result back, and queues a panel update.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    async fn settle(
        &mut self,
        after: AfterInput,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let acted = self.apply_callbacks(hub, bus, context).await == Flow::Redraw;
        if acted {
            self.sync_window(context);
            self.queue_frame(rq, after.finger_down);
        } else if after.repainted && !after.skip {
            self.queue_frame(rq, after.finger_down);
        }
        if let Some(session) = self.session() {
            session.wake_for_timers(hub);
        }
    }

    /// Queues one panel update for each rectangle Slint changed.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    fn queue_frame(&self, rq: &mut RenderQueue, finger_down: bool) {
        let Some(session) = self.session() else {
            return;
        };
        let rects = session.take_dirty();
        if rects.is_empty() {
            tracing::trace!(
                component = std::any::type_name::<Self::Component>(),
                "slint frame had nothing dirty"
            );
            return;
        }
        let (mode, wait) = self.panel_refresh(finger_down);
        tracing::trace!(
            component = std::any::type_name::<Self::Component>(),
            ?mode,
            wait,
            rects = rects.len(),
            "slint queued a frame"
        );
        let id = self.id();
        for rect in rects {
            tracing::trace!(%rect, "slint dirty area");
            if wait {
                rq.add(RenderData::new(id, rect, mode));
            } else {
                rq.add(RenderData::no_wait(id, rect, mode));
            }
        }
    }
}

/// Opens `build`'s component on the device framebuffer, using the bundled fonts.
pub(crate) fn open<C: ComponentHandle>(
    device: &impl Device,
    build: impl FnOnce(&str) -> Result<C>,
) -> Result<Session<C>> {
    let (width, height) = device.framebuffer().dims();
    Session::open_with_install_fonts(device.dpi(), width, height, &device.install_dir(), build)
}

/// Opens a component at an explicit size with the supplied font bytes.
#[cfg(test)]
pub(crate) fn open_with_font<C: ComponentHandle>(
    dpi: u16,
    width: u32,
    height: u32,
    font_bytes: &[u8],
    build: impl FnOnce(&str) -> Result<C>,
) -> Result<Session<C>> {
    Session::open_with_app_font(dpi, width, height, font_bytes, build)
}

/// Selects the bundled Slint translation matching Fluent's current language.
/// Call after creating a component so Slint has installed its translation
/// catalog. Unsupported languages use the source-language strings.
pub(crate) fn apply_language() {
    let tag = crate::i18n::language_loader()
        .current_languages()
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| crate::i18n::DEFAULT_LOCALE.to_string());
    let primary = tag
        .split(['-', '_'])
        .next()
        .unwrap_or(tag.as_str())
        .to_string();
    for candidate in [tag.as_str(), primary.as_str()] {
        match slint::select_bundled_translation(candidate) {
            Ok(()) => return,
            Err(slint::SelectBundledTranslationError::NoTranslationsBundled) => return,
            Err(slint::SelectBundledTranslationError::LanguageNotFound { .. }) => {}
        }
    }
    if let Err(error) = slint::select_bundled_translation("") {
        tracing::debug!(
            error = %error,
            language = %tag,
            "slint kept the source language"
        );
    }
}
