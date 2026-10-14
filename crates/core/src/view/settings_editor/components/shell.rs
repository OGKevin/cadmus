//! Rust side of the settings shell.
//!
//! The window keeps the category, the page, the font, and the back callback.
//! Screen size lives on `SettingsChrome`. Category values do not live here.
//! The chrome stays white and black. A color panel paints the selection
//! mark, the action button, and filled primary buttons orange. Bar fills
//! stay white, because a pale tint across a Kaleido bar looks dirty.

use anyhow::Context as _;
use slint::ComponentHandle;

use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::device::{DeviceCapabilities as _, DeviceHardware as _, DeviceIdentity as _};
use crate::framebuffer::Framebuffer as _;
use crate::view::slint::generated::{
    MaterialPalette, SettingsCategory, SettingsChrome, SettingsWindow,
};
use crate::view::slint::{CallbackQueue, Session, connect};

/// Opens the settings window on the device framebuffer, using the app font.
pub(in crate::view::settings_editor) fn open(
    context: &mut AppContext,
) -> anyhow::Result<Session<SettingsWindow>> {
    crate::view::slint::open(&context.device, |family| {
        let ui = SettingsWindow::new().context("failed to create slint settings window")?;
        ui.set_font_family(family.into());
        ui.global::<MaterialPalette>()
            .set_chromatic(context.device.color_samples() > 1);
        Ok(ui)
    })
}

pub(super) struct Shell {
    category: SettingsCategory,
    queue: CallbackQueue<()>,
}

impl Shell {
    pub(super) fn new() -> Self {
        Self {
            category: SettingsCategory::General,
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn category(&self) -> SettingsCategory {
        self.category
    }

    pub(super) fn set_category(&mut self, category: SettingsCategory) {
        self.category = category;
    }
}

#[async_trait::async_trait(?Send)]
impl Component for Shell {
    type Window = SettingsWindow;
    type Action = ();
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        connect!(window, self.queue, on_back_pressed, () => ());
    }

    fn take(&self) -> Vec<()> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let (width, height) = logical_size(context);
        let chrome = window.global::<SettingsChrome>();
        chrome.set_window_width(width);
        chrome.set_window_height(height);
        window
            .global::<MaterialPalette>()
            .set_chromatic(context.device.color_samples() > 1);
        window.set_category(self.category);
    }

    async fn apply(&mut self, (): (), _env: &mut Env<'_>) -> Step {
        Step::Leave
    }
}

fn logical_size(context: &AppContext) -> (f32, f32) {
    let dpi = context.device.dpi().max(1) as f32;
    let (width, height) = context.device.framebuffer().dims();
    (
        width as f32 * crate::unit::BASE_DPI / dpi,
        height as f32 * crate::unit::BASE_DPI / dpi,
    )
}
