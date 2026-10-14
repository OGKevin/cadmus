//! Rust side of `PowerPage`.
//!
//! Row values, the soft-suspend flag, and the row callback live on [`PowerPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::view::slint::generated::{PowerPageIo, SettingRow, SettingsWindow};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct PowerPage {
    queue: CallbackQueue<SettingRow>,
}

impl PowerPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for PowerPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<PowerPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<PowerPageIo>();
        io.set_show_soft_suspend(context.inhibitor.is_supported());
        io.set_autosleep_mode(setting::text(SettingRow::AutosleepMode, context));
        io.set_indicate_autosleep_led(setting::text(SettingRow::IndicateAutosleepLed, context));
        io.set_autosleep_grace(setting::text(SettingRow::AutosleepGrace, context));
        io.set_auto_suspend(setting::text(SettingRow::AutoSuspend, context));
        io.set_auto_power_off(setting::text(SettingRow::AutoPowerOff, context));
        io.set_sleep_cover(setting::text(SettingRow::SleepCover, context));
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}
