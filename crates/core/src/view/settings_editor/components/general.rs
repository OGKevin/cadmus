//! Rust side of `GeneralPage`.
//!
//! Row values and the row callback live on [`GeneralPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{GeneralPageIo, SettingRow, SettingsCategory, SettingsWindow};
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
        if let Some(rows) =
            setting::publish_rows(&io.get_rows(), SettingsCategory::General, context)
        {
            io.set_rows(rows);
        }
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}

#[cfg(test)]
mod tests {
    use slint::ComponentHandle;
    use slint::Model;

    use super::super::setting::{self, PressedRow};
    use super::super::settings_env;
    use super::super::{Sheet, Waiting};
    use super::{Component, GeneralPage, Step};
    use crate::fl;
    use crate::view::slint::generated::{GeneralPageIo, SettingRow};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_general_and_a_tap_applies_the_row() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let mut page = GeneralPage::new();
        page.wire(window);
        context.settings.auto_share = false;
        page.sync(window, &context);

        let io = window.global::<GeneralPageIo>();
        let rows = io.get_rows();
        assert_eq!(
            setting::row_ids(&rows),
            [
                SettingRow::Locale,
                SettingRow::AutoShare,
                SettingRow::AutoTime,
                SettingRow::NtpServer,
                SettingRow::AutoFrontlight,
                SettingRow::AutoFrontlightBrightness,
                SettingRow::AutoFrontlightManualCoordinates,
                SettingRow::WifiIdleTimeout,
                SettingRow::ButtonScheme,
                SettingRow::KeyboardLayout,
                SettingRow::SettingsRetention,
                SettingRow::DbBackupRetention,
                SettingRow::StartupMode,
            ]
        );
        assert!(rows.iter().all(|line| !line.label.is_empty()));
        assert_eq!(
            setting::row_value(&rows, SettingRow::AutoShare),
            fl!("settings-general-toggle-off")
        );

        io.invoke_row_pressed(SettingRow::Locale);
        io.invoke_row_pressed(SettingRow::AutoShare);
        assert!(matches!(
            page.take().as_slice(),
            [SettingRow::Locale, SettingRow::AutoShare]
        ));

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let locale = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(SettingRow::Locale, &mut env).await
        };
        let Step::Ask(
            Waiting::SettingChoice(PressedRow::Setting(SettingRow::Locale)),
            Sheet::Choice { entries, .. },
        ) = locale
        else {
            panic!("language did not open a choice");
        };
        assert!(!entries.is_empty());

        let toggled = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(SettingRow::AutoShare, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(context.settings.auto_share);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<GeneralPageIo>().get_rows(),
                SettingRow::AutoShare,
            ),
            fl!("settings-general-toggle-on")
        );
    }
}
