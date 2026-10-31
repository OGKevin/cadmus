//! Shared software-rendering host for Slint-backed views.
//!
//! Use [`open`] to create a [`Session`]. [`SlintHost`] drives one
//! [`SlintScreen`] and owns the event loop, the panel updates, and the
//! resize. Generated component types live in [`generated`]. Reusable overlay
//! state lives in [`components`]. [`Component`] is the callback loop a screen
//! implements for each of its pieces.

mod component;
pub(super) mod components;
pub(super) mod generated;
mod host;
mod metrics;
mod model;
mod overlay;
mod page;
mod queue;
mod renderer;
#[cfg(test)]
pub(crate) mod testing;

pub(in crate::view) use component::Component;
pub use component::Env;
pub use host::{FrameCause, RefreshPlan, SlintHost, SlintScreen};
#[cfg(test)]
pub(crate) use metrics::apply_ink;
pub(in crate::view) use metrics::pixel_scale;
pub(crate) use model::{ListRow, changed_model, paged_line};
pub(in crate::view) use overlay::{Asked, Overlay, OverlayStack};
pub(in crate::view) use page::PagedIo;
pub(in crate::view) use queue::{CallbackQueue, connect};
pub use renderer::Session;
#[cfg(test)]
pub(crate) use renderer::advance_test_clock;

/// What the view loop should do with the framebuffer after Slint callbacks.
///
/// A screen reduces its own effect to this. [`Flow::Idle`] leaves the captured
/// frame. [`Flow::Redraw`] captures the window and queues a panel update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
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

/// Routes a button event that a screen's own shortcuts did not take.
///
/// A screen that turns a press into its own action records that button in
/// `recorded`. The release and the repeats of that hold must not reach Slint,
/// or it sees a key that is never released. A fresh press of the same button
/// starts a new hold, so the record is dropped and the press is forwarded.
///
/// Returns `true` when the event was part of the recorded hold.
pub(in crate::view) fn swallow_stale_press(
    recorded: &mut Option<ButtonCode>,
    event: &DeviceEvent,
) -> bool {
    let DeviceEvent::Button { code, status, .. } = event else {
        return false;
    };
    if *recorded != Some(*code) {
        return false;
    }
    match status {
        ButtonStatus::Released => *recorded = None,
        ButtonStatus::Repeated => return true,
        ButtonStatus::Pressed => {
            *recorded = None;
            return false;
        }
    }
    true
}

use ::slint::ComponentHandle;
use anyhow::Result;

use crate::device::Device;
use crate::framebuffer::Framebuffer as _;
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};

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

#[cfg(test)]
mod tests {
    use super::*;

    fn button(code: ButtonCode, status: ButtonStatus) -> DeviceEvent {
        DeviceEvent::Button {
            time: 0.0,
            code,
            status,
        }
    }

    #[test]
    fn a_recorded_hold_is_swallowed_until_it_is_released() {
        let mut recorded = Some(ButtonCode::Forward);
        assert!(swallow_stale_press(
            &mut recorded,
            &button(ButtonCode::Forward, ButtonStatus::Repeated)
        ));
        assert!(swallow_stale_press(
            &mut recorded,
            &button(ButtonCode::Forward, ButtonStatus::Released)
        ));
        assert_eq!(recorded, None);
    }

    #[test]
    fn a_new_press_of_the_recorded_button_is_forwarded() {
        let mut recorded = Some(ButtonCode::Forward);
        assert!(!swallow_stale_press(
            &mut recorded,
            &button(ButtonCode::Forward, ButtonStatus::Pressed)
        ));
        assert_eq!(recorded, None);
    }

    #[test]
    fn other_buttons_and_gestures_are_forwarded() {
        let mut recorded = Some(ButtonCode::Forward);
        assert!(!swallow_stale_press(
            &mut recorded,
            &button(ButtonCode::Backward, ButtonStatus::Released)
        ));
        assert_eq!(recorded, Some(ButtonCode::Forward));
        assert!(!swallow_stale_press(&mut recorded, &DeviceEvent::CoverOn));
        assert_eq!(recorded, Some(ButtonCode::Forward));
    }
}
