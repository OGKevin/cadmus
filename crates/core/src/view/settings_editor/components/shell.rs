//! Rust side of the settings shell.
//!
//! The window keeps the category, the page, the font, and the back callback.
//! Screen size lives on `InkScreen`. Category values do not live here.
//! The chrome stays white and black. A color panel paints the selected
//! category, the selection mark, the action button, and filled primary
//! buttons orange. Bar fills stay white, because a pale tint across a
//! Kaleido bar looks dirty.

use anyhow::Context as _;

use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{SettingsCategory, SettingsWindow};
use crate::view::slint::{CallbackQueue, Session, apply_ink, connect};

/// Opens the settings window on the device framebuffer, using the app font.
pub(in crate::view::settings_editor) fn open(
    context: &mut AppContext,
) -> anyhow::Result<Session<SettingsWindow>> {
    crate::view::slint::open(&context.device, |_family| {
        let ui = SettingsWindow::new().context("failed to create slint settings window")?;
        apply_ink(&ui, context);
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
        apply_ink(window, context);
        window.set_category(self.category);
    }

    async fn apply(&mut self, (): (), _env: &mut Env<'_>) -> Step {
        Step::Leave
    }
}

/// Opens a settings window on the install-font path used in production.
#[cfg(test)]
pub(super) async fn open_test_session() -> (AppContext, Session<SettingsWindow>) {
    use crate::context::test_helpers::create_test_context;
    use crate::view::slint::testing::install_ui_font;

    let mut context = create_test_context().await;
    install_ui_font(&context).await;
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
    use crate::view::slint::testing::install_ui_font;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn strokes_follow_scale_by_dpi() {
        for dpi in [167, 200, 212, 227, 265, 300] {
            let mut context =
                create_test_context_from_device(TestDevice::new().with_screen(600, 800, dpi)).await;
            install_ui_font(&context).await;
            let session = super::open(&mut context).expect("settings session");
            let metrics = session.component().global::<InkMetrics>();
            let stroke = |actual: f32, design: f32| {
                assert_eq!(
                    (actual * (f32::from(dpi) / BASE_DPI)) as i16,
                    scale_by_dpi(design, dpi) as i16,
                    "dpi {dpi} design {design}"
                );
            };
            stroke(metrics.get_hairline(), 2.0);
            stroke(metrics.get_frame(), 3.0);
            stroke(metrics.get_radius(), 9.0);
            stroke(metrics.get_mark(), 8.0);
            stroke(metrics.get_mark_radius(), 4.0);
        }
    }
}
