//! Rust side of `SettingsFooter`.
//!
//! The plus button is the footer's only callback. It is shown for libraries
//! and opens the new-library editor.

use super::{Component, Editor, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::SettingsWindow;
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct Footer {
    queue: CallbackQueue<()>,
}

impl Footer {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for Footer {
    type Window = SettingsWindow;
    type Action = ();
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        connect!(window, self.queue, on_action_pressed, () => ());
    }

    fn take(&self) -> Vec<()> {
        self.queue.drain()
    }

    fn sync(&mut self, _window: &SettingsWindow, _context: &AppContext) {}

    async fn apply(&mut self, (): (), _env: &mut Env<'_>) -> Step {
        Step::Edit(Editor::Library(None))
    }
}
