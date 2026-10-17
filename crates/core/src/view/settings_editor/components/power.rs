//! Rust side of `PowerPage`.
//!
//! Row values and the row callback live on [`PowerPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{PowerPageIo, SettingRow, SettingsCategory, SettingsWindow};
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
        if let Some(rows) = setting::publish_rows(&io.get_rows(), SettingsCategory::Power, context)
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
    use super::super::{Sheet, Waiting};
    use super::{Component, PowerPage, Step};
    use crate::fl;
    use crate::view::slint::generated::{PowerPageIo, SettingRow};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_power_and_a_tap_applies_the_row() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let mut page = PowerPage::new();
        page.wire(window);
        context.settings.sleep_cover = true;
        page.sync(window, &context);

        let io = window.global::<PowerPageIo>();
        let rows = io.get_rows();
        assert_eq!(
            setting::row_ids(&rows),
            [
                SettingRow::AutoSuspend,
                SettingRow::AutoPowerOff,
                SettingRow::SleepCover,
            ]
        );
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

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let suspend = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
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
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(SettingRow::SleepCover, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(!context.settings.sleep_cover);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<PowerPageIo>().get_rows(),
                SettingRow::SleepCover,
            ),
            fl!("settings-general-toggle-off")
        );
    }
}
