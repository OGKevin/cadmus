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
    InkMetrics, MaterialPalette, SettingsCategory, SettingsChrome, SettingsWindow,
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
        align_strokes(&ui, context.device.dpi());
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

fn align_strokes(window: &SettingsWindow, dpi: u16) {
    let metrics = window.global::<InkMetrics>();
    metrics.set_hairline(device_px(2.0, dpi));
    metrics.set_frame(device_px(3.0, dpi));
    metrics.set_radius(device_px(9.0, dpi));
    metrics.set_mark(device_px(8.0, dpi));
    metrics.set_mark_radius(device_px(4.0, dpi));
}

/// Logical length for `design_px` at `dpi`.
///
/// `scale_by_dpi` is the physical size the rest of the UI uses. Dividing back
/// by the window scale (`dpi / BASE_DPI`) keeps that size once Slint draws it.
fn device_px(design_px: f32, dpi: u16) -> f32 {
    logical_px(crate::unit::scale_by_dpi(design_px, dpi), dpi)
}

fn logical_px(physical: f32, dpi: u16) -> f32 {
    physical * crate::unit::BASE_DPI / f32::from(dpi.max(1))
}

fn logical_size(context: &AppContext) -> (f32, f32) {
    let (width, height) = context.device.framebuffer().dims();
    let dpi = context.device.dpi();
    (
        logical_px(width as f32, dpi),
        logical_px(height as f32, dpi),
    )
}

/// Writes the UI font into the device install directory.
///
/// The install directory is one fixed path shared by every test process, so
/// the write is atomic: a concurrent `register_install_fonts` in another
/// process must never read a half-written font and fail to open its session.
#[cfg(test)]
pub(in crate::view::settings_editor) fn install_ui_font(context: &AppContext) {
    use std::path::Path;

    use crate::device::DevicePaths as _;

    let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts/NotoSans-Regular.ttf");
    let bytes =
        std::fs::read(&font).unwrap_or_else(|error| panic!("read {}: {error}", font.display()));
    let fonts_dir = context.device.install_dir().join("fonts");
    std::fs::create_dir_all(&fonts_dir).expect("create fonts dir");
    let tmp = tempfile::Builder::new()
        .prefix(".ui-font-")
        .tempfile_in(&fonts_dir)
        .expect("temp font file");
    std::fs::write(tmp.path(), bytes).expect("write font");
    tmp.persist(fonts_dir.join("NotoSans-Regular.ttf"))
        .expect("persist font");
}

/// Opens a settings window on the install-font path used in production.
#[cfg(test)]
pub(super) async fn open_test_session() -> (AppContext, Session<SettingsWindow>) {
    use crate::context::test_helpers::create_test_context;

    let mut context = create_test_context().await;
    install_ui_font(&context);
    let session = open(&mut context).expect("settings session");
    (context, session)
}

#[cfg(test)]
mod tests {
    use slint::ComponentHandle;

    use crate::context::test_helpers::create_test_context_from_device;
    use crate::device::test_device::TestDevice;
    use crate::unit::{BASE_DPI, scale_by_dpi};
    use crate::view::slint::generated::InkMetrics;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn strokes_follow_scale_by_dpi_at_212() {
        let mut context =
            create_test_context_from_device(TestDevice::new().with_screen(600, 800, 212)).await;
        super::install_ui_font(&context);
        let session = super::open(&mut context).expect("settings session");
        let metrics = session.component().global::<InkMetrics>();
        let logical = |design: f32| scale_by_dpi(design, 212) * BASE_DPI / 212.0;

        assert_eq!(metrics.get_hairline(), logical(2.0));
        assert_eq!(metrics.get_frame(), logical(3.0));
        assert_eq!(metrics.get_radius(), logical(9.0));
        assert_eq!(metrics.get_mark(), logical(8.0));
        assert_eq!(metrics.get_mark_radius(), logical(4.0));
    }
}
