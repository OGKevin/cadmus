//! Rust side of `SettingsPage`.
//!
//! Row values and the row callback live on [`StaticPageIo`]. Rust publishes
//! the visible category's rows; libraries and dictionaries keep their own
//! models.

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{SettingRow, SettingsCategory, SettingsWindow, StaticPageIo};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct StaticPage {
    queue: CallbackQueue<SettingRow>,
}

impl StaticPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for StaticPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<StaticPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let category = window.get_category();
        if matches!(
            category,
            SettingsCategory::Libraries | SettingsCategory::Dictionaries
        ) {
            return;
        }
        let io = window.global::<StaticPageIo>();
        if let Some(rows) = setting::publish_rows(&io.get_rows(), category, context) {
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

    use super::super::apply_env;
    use super::super::assert_labels;
    use super::super::setting::{self, PressedRow};
    use super::super::{Editor, Sheet, Waiting};
    use super::{Component, StaticPage, Step};
    use crate::fl;
    use crate::view::slint::generated::{SettingRow, SettingsCategory, StaticPageIo};

    async fn wired_page(
        category: SettingsCategory,
    ) -> (
        crate::device::AppContext,
        crate::view::slint::Session<crate::view::slint::generated::SettingsWindow>,
        StaticPage,
    ) {
        let (context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        window.set_category(category);
        let page = StaticPage::new();
        page.wire(window);
        (context, session, page)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_general_and_a_tap_applies_the_row() {
        let (mut context, session, mut page) = wired_page(SettingsCategory::General).await;
        let window = session.component();
        context.settings.auto_share = false;
        page.sync(window, &context);

        let io = window.global::<StaticPageIo>();
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
        assert_labels(&rows);
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

        let mut host = apply_env();
        let locale = {
            let mut env = host.env(&mut context);
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
            let mut env = host.env(&mut context);
            page.apply(SettingRow::AutoShare, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(context.settings.auto_share);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<StaticPageIo>().get_rows(),
                SettingRow::AutoShare,
            ),
            fl!("settings-general-toggle-on")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_power_and_a_tap_applies_the_row() {
        let (mut context, session, mut page) = wired_page(SettingsCategory::Power).await;
        let window = session.component();
        context.settings.sleep_cover = true;
        page.sync(window, &context);

        let io = window.global::<StaticPageIo>();
        let rows = io.get_rows();
        assert_eq!(
            setting::row_ids(&rows),
            [
                SettingRow::AutoSuspend,
                SettingRow::AutoPowerOff,
                SettingRow::SleepCover,
            ]
        );
        assert_labels(&rows);
        assert_eq!(
            setting::row_value(&rows, SettingRow::SleepCover),
            fl!("settings-general-toggle-on")
        );

        io.invoke_row_pressed(SettingRow::AutoSuspend);
        io.invoke_row_pressed(SettingRow::SleepCover);
        assert!(matches!(
            page.take().as_slice(),
            [SettingRow::AutoSuspend, SettingRow::SleepCover]
        ));

        let mut host = apply_env();
        let suspend = {
            let mut env = host.env(&mut context);
            page.apply(SettingRow::AutoSuspend, &mut env).await
        };
        let Step::Ask(
            Waiting::SettingText(PressedRow::Setting(SettingRow::AutoSuspend)),
            Sheet::Text { limits, .. },
        ) = suspend
        else {
            panic!("auto suspend did not open a number field");
        };
        assert!(limits.numbers);

        let toggled = {
            let mut env = host.env(&mut context);
            page.apply(SettingRow::SleepCover, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(!context.settings.sleep_cover);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<StaticPageIo>().get_rows(),
                SettingRow::SleepCover,
            ),
            fl!("settings-general-toggle-off")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_reader_and_a_tap_opens_the_row() {
        let (mut context, session, mut page) = wired_page(SettingsCategory::Reader).await;
        let window = session.component();
        page.sync(window, &context);

        let io = window.global::<StaticPageIo>();
        let rows = io.get_rows();
        assert_eq!(
            setting::row_ids(&rows),
            [
                SettingRow::FontFamily,
                SettingRow::FinishedAction,
                SettingRow::DitheredKinds,
                SettingRow::RefreshRate,
            ]
        );
        assert_labels(&rows);
        let global = &context.settings.reader.refresh_rate.global;
        assert_eq!(
            setting::row_value(&rows, SettingRow::RefreshRate),
            format!("{} / {}", global.regular, global.inverted)
        );
        assert_eq!(
            setting::row_value(&rows, SettingRow::FontFamily),
            context.settings.reader.font_family
        );

        context.settings.reader.refresh_rate.global.regular = 21;
        let inverted = context.settings.reader.refresh_rate.global.inverted;
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<StaticPageIo>().get_rows(),
                SettingRow::RefreshRate,
            ),
            format!("21 / {inverted}")
        );

        io.invoke_row_pressed(SettingRow::FontFamily);
        io.invoke_row_pressed(SettingRow::RefreshRate);
        assert!(matches!(
            page.take().as_slice(),
            [SettingRow::FontFamily, SettingRow::RefreshRate]
        ));

        let mut host = apply_env();
        let font = {
            let mut env = host.env(&mut context);
            page.apply(SettingRow::FontFamily, &mut env).await
        };
        let Step::Ask(
            Waiting::SettingChoice(PressedRow::Setting(SettingRow::FontFamily)),
            Sheet::Choice { entries, .. },
        ) = font
        else {
            panic!("font family did not open a choice");
        };
        assert!(!entries.is_empty());

        let refresh = {
            let mut env = host.env(&mut context);
            page.apply(SettingRow::RefreshRate, &mut env).await
        };
        assert!(matches!(refresh, Step::Edit(Editor::RefreshRates)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_telemetry_and_a_tap_applies_the_row() {
        let (mut context, session, mut page) = wired_page(SettingsCategory::Telemetry).await;
        let window = session.component();
        context.settings.logging.enabled = true;
        page.sync(window, &context);

        let io = window.global::<StaticPageIo>();
        let rows = io.get_rows();
        let expected = [
            SettingRow::LoggingEnabled,
            SettingRow::LogLevel,
            #[cfg(feature = "tracing")]
            SettingRow::OtlpEndpoint,
            #[cfg(feature = "profiling")]
            SettingRow::PyroscopeEndpoint,
            #[cfg(all(feature = "test", feature = "kobo"))]
            SettingRow::EnableKernLog,
            #[cfg(all(feature = "test", feature = "kobo"))]
            SettingRow::EnableDbusLog,
        ];
        assert_eq!(setting::row_ids(&rows), expected);
        assert_labels(&rows);
        assert_eq!(
            setting::row_value(&rows, SettingRow::LoggingEnabled),
            fl!("settings-general-toggle-on")
        );

        io.invoke_row_pressed(SettingRow::LogLevel);
        io.invoke_row_pressed(SettingRow::LoggingEnabled);
        assert!(matches!(
            page.take().as_slice(),
            [SettingRow::LogLevel, SettingRow::LoggingEnabled]
        ));

        let mut host = apply_env();
        let level = {
            let mut env = host.env(&mut context);
            page.apply(SettingRow::LogLevel, &mut env).await
        };
        let Step::Ask(
            Waiting::SettingChoice(PressedRow::Setting(SettingRow::LogLevel)),
            Sheet::Choice { entries, .. },
        ) = level
        else {
            panic!("log level did not open a choice");
        };
        assert!(!entries.is_empty());

        let toggled = {
            let mut env = host.env(&mut context);
            page.apply(SettingRow::LoggingEnabled, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(!context.settings.logging.enabled);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<StaticPageIo>().get_rows(),
                SettingRow::LoggingEnabled,
            ),
            fl!("settings-general-toggle-off")
        );
    }
}
