//! Rust side of `ReaderPage`.
//!
//! Row values and the row callback live on [`ReaderPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{ReaderPageIo, SettingRow, SettingsCategory, SettingsWindow};
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
        if let Some(rows) = setting::publish_rows(&io.get_rows(), SettingsCategory::Reader, context)
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

    use super::super::setting::{self, PressedRow};
    use super::super::settings_env;
    use super::super::{Editor, Sheet, Waiting};
    use super::{Component, ReaderPage, Step};
    use crate::view::slint::generated::{ReaderPageIo, SettingRow};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_reader_and_a_tap_opens_the_row() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let mut page = ReaderPage::new();
        page.wire(window);
        page.sync(window, &context);

        let io = window.global::<ReaderPageIo>();
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
                &window.global::<ReaderPageIo>().get_rows(),
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

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let font = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
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
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(SettingRow::RefreshRate, &mut env).await
        };
        assert!(matches!(refresh, Step::Edit(Editor::RefreshRates)));
    }
}
