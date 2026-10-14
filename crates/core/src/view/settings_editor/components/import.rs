//! Rust side of `ImportPage`.
//!
//! Row values and the row callback live on [`ImportPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{ImportPageIo, SettingRow, SettingsWindow};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct ImportPage {
    queue: CallbackQueue<SettingRow>,
}

impl ImportPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for ImportPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<ImportPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<ImportPageIo>();
        io.set_force_full_import(setting::text(SettingRow::ForceFullImport, context));
        io.set_sync_metadata(setting::text(SettingRow::ImportSyncMetadata, context));
        io.set_allowed_kinds(setting::text(SettingRow::AllowedKinds, context));
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}
