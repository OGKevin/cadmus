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

pub(crate) use component::{Component, Env};
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
use crate::framebuffer::Framebuffer as _;
use crate::input::DeviceEvent;
use crate::view::View;

/// Shared event forwarding and frame blitting for Slint-backed views.
#[async_trait::async_trait(?Send)]
pub(crate) trait SlintView: View {
    /// Generated component this screen draws.
    type Component: ComponentHandle;

    /// The open session, if the screen has one.
    fn session(&self) -> Option<&Session<Self::Component>>;

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
