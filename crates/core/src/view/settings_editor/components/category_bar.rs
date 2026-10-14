//! Category tabs across the top of the settings screen.

use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{SettingsCategory, SettingsWindow};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct CategoryBar {
    queue: CallbackQueue<SettingsCategory>,
}

impl CategoryBar {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for CategoryBar {
    type Window = SettingsWindow;
    type Action = SettingsCategory;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        connect!(window, self.queue, on_category_pressed, (category) => category);
    }

    fn take(&self) -> Vec<SettingsCategory> {
        self.queue.drain()
    }

    fn sync(&mut self, _window: &SettingsWindow, _context: &AppContext) {}

    async fn apply(&mut self, category: SettingsCategory, _env: &mut Env<'_>) -> Step {
        Step::Category(category)
    }
}
