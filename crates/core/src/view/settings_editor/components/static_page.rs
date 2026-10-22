//! Rust side of the static settings page.
//!
//! Row values and the row callback live on [`StaticPageIo`]. Rust publishes
//! the visible category's rows; libraries and dictionaries keep their own
//! models.

use slint::ComponentHandle;

use super::setting::{self, PressedRow, SettingRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{SettingsCategory, SettingsWindow, StaticPageIo};
use crate::view::slint::{CallbackQueue, changed_model, connect};

pub(super) struct StaticPage {
    queue: CallbackQueue<usize>,
    /// The rows the page shows, in order. A tap names the row by its
    /// place in this list.
    rows: Vec<SettingRow>,
}

impl StaticPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
            rows: Vec::new(),
        }
    }

    /// The rows the page shows, in order.
    #[cfg(test)]
    pub(super) fn rows(&self) -> &[SettingRow] {
        &self.rows
    }
}

#[async_trait::async_trait(?Send)]
impl Component for StaticPage {
    type Window = SettingsWindow;
    type Action = usize;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<StaticPageIo>();
        connect!(io, self.queue, on_row_pressed, (index) => index as usize);
    }

    fn take(&self) -> Vec<usize> {
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
        let rows = setting::shown_rows(category, context);
        self.rows = rows.iter().map(|(row, _)| *row).collect();
        let lines = rows.into_iter().map(|(_, line)| line).collect();
        let io = window.global::<StaticPageIo>();
        if let Some(model) = changed_model(&io.get_rows(), lines) {
            io.set_rows(model);
        }
    }

    async fn apply(&mut self, index: usize, env: &mut Env<'_>) -> Step {
        let Some(row) = self.rows.get(index).copied() else {
            return Step::Idle;
        };
        setting::activate(PressedRow::Setting(row), env)
    }
}

#[cfg(test)]
mod tests {
    use slint::ComponentHandle;

    use super::super::apply_env;
    use super::super::assert_labels;
    use super::super::setting::{self, PressedRow, SettingRow};
    use super::super::{Editor, Sheet, Waiting};
    use super::{Component, StaticPage, Step};
    use crate::fl;
    use crate::view::slint::generated::{SettingsCategory, StaticPageIo};

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

    /// The rows the page shows, in order.
    fn row_ids(page: &StaticPage) -> Vec<SettingRow> {
        page.rows().to_vec()
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
            row_ids(&page),
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
        let auto_share = setting::row_index(page.rows(), SettingRow::AutoShare);
        assert_eq!(
            setting::row_value(&rows, auto_share),
            fl!("settings-general-toggle-off")
        );

        io.invoke_row_pressed(auto_share as i32);
        assert!(matches!(page.take().as_slice(), [index] if *index == auto_share));

        let mut host = apply_env();
        let locale = {
            let mut env = host.env(&mut context);
            page.apply(0, &mut env).await
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
            page.apply(auto_share, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(context.settings.auto_share);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(&window.global::<StaticPageIo>().get_rows(), auto_share,),
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
            row_ids(&page),
            [
                SettingRow::AutoSuspend,
                SettingRow::AutoPowerOff,
                SettingRow::SleepCover,
            ]
        );
        assert_labels(&rows);
        let sleep_cover = setting::row_index(page.rows(), SettingRow::SleepCover);
        assert_eq!(
            setting::row_value(&rows, sleep_cover),
            fl!("settings-general-toggle-on")
        );

        io.invoke_row_pressed(sleep_cover as i32);
        assert!(matches!(page.take().as_slice(), [index] if *index == sleep_cover));

        let mut host = apply_env();
        let suspend = {
            let mut env = host.env(&mut context);
            page.apply(0, &mut env).await
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
            page.apply(sleep_cover, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(!context.settings.sleep_cover);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(&window.global::<StaticPageIo>().get_rows(), sleep_cover,),
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
            row_ids(&page),
            [
                SettingRow::FontFamily,
                SettingRow::FinishedAction,
                SettingRow::DitheredKinds,
                SettingRow::RefreshRate,
            ]
        );
        assert_labels(&rows);
        let global = &context.settings.reader.refresh_rate.global;
        let refresh_rate = setting::row_index(page.rows(), SettingRow::RefreshRate);
        let font_family = setting::row_index(page.rows(), SettingRow::FontFamily);
        assert_eq!(
            setting::row_value(&rows, refresh_rate),
            format!("{} / {}", global.regular, global.inverted)
        );
        assert_eq!(
            setting::row_value(&rows, font_family),
            context.settings.reader.font_family
        );

        context.settings.reader.refresh_rate.global.regular = 21;
        let inverted = context.settings.reader.refresh_rate.global.inverted;
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(&window.global::<StaticPageIo>().get_rows(), refresh_rate,),
            format!("21 / {inverted}")
        );

        io.invoke_row_pressed(font_family as i32);
        assert!(matches!(page.take().as_slice(), [index] if *index == font_family));

        let mut host = apply_env();
        let font = {
            let mut env = host.env(&mut context);
            page.apply(font_family, &mut env).await
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
            page.apply(refresh_rate, &mut env).await
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
        assert_eq!(row_ids(&page), expected);
        assert_labels(&rows);
        let logging = setting::row_index(page.rows(), SettingRow::LoggingEnabled);
        assert_eq!(
            setting::row_value(&rows, logging),
            fl!("settings-general-toggle-on")
        );

        io.invoke_row_pressed(logging as i32);
        assert!(matches!(page.take().as_slice(), [index] if *index == logging));

        let mut host = apply_env();
        let level = {
            let mut env = host.env(&mut context);
            page.apply(1, &mut env).await
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
            page.apply(logging, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(!context.settings.logging.enabled);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(&window.global::<StaticPageIo>().get_rows(), logging,),
            fl!("settings-general-toggle-off")
        );
    }
}
