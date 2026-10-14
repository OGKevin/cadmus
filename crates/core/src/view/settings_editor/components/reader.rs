//! Rust side of `ReaderPage`.
//!
//! Row values and the row callback live on [`ReaderPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{ReaderPageIo, SettingRow, SettingsWindow};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct ReaderPage {
    queue: CallbackQueue<SettingRow>,
}

impl ReaderPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for ReaderPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<ReaderPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<ReaderPageIo>();
        io.set_font_family(setting::text(SettingRow::FontFamily, context));
        io.set_finished_action(setting::text(SettingRow::FinishedAction, context));
        io.set_dithered_kinds(setting::text(SettingRow::DitheredKinds, context));
        io.set_refresh_rate(setting::text(SettingRow::RefreshRate, context));
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}
