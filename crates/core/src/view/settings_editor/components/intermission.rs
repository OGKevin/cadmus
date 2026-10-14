//! Rust side of `IntermissionPage`.
//!
//! Row values and the row callback live on [`IntermissionPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{
    IntermissionPageIo, SettingRow, SettingsCategory, SettingsWindow,
};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct IntermissionPage {
    queue: CallbackQueue<SettingRow>,
}

impl IntermissionPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for IntermissionPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<IntermissionPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<IntermissionPageIo>();
        if let Some(rows) =
            setting::publish_rows(&io.get_rows(), SettingsCategory::Intermissions, context)
        {
            io.set_rows(rows);
        }
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}
