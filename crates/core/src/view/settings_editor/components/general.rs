//! Rust side of `GeneralPage`.
//!
//! Row values and the row callback live on [`GeneralPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{GeneralPageIo, SettingRow, SettingsWindow};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct GeneralPage {
    queue: CallbackQueue<SettingRow>,
}

impl GeneralPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for GeneralPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<GeneralPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<GeneralPageIo>();
        io.set_locale(setting::text(SettingRow::Locale, context));
        io.set_auto_share(setting::text(SettingRow::AutoShare, context));
        io.set_auto_time(setting::text(SettingRow::AutoTime, context));
        io.set_ntp_server(setting::text(SettingRow::NtpServer, context));
        io.set_auto_frontlight(setting::text(SettingRow::AutoFrontlight, context));
        io.set_auto_frontlight_brightness(setting::text(
            SettingRow::AutoFrontlightBrightness,
            context,
        ));
        io.set_auto_frontlight_manual_coordinates(setting::text(
            SettingRow::AutoFrontlightManualCoordinates,
            context,
        ));
        io.set_wifi_idle_timeout(setting::text(SettingRow::WifiIdleTimeout, context));
        io.set_button_scheme(setting::text(SettingRow::ButtonScheme, context));
        io.set_keyboard_layout(setting::text(SettingRow::KeyboardLayout, context));
        io.set_settings_retention(setting::text(SettingRow::SettingsRetention, context));
        io.set_db_backup_retention(setting::text(SettingRow::DbBackupRetention, context));
        io.set_startup_mode(setting::text(SettingRow::StartupMode, context));
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}
