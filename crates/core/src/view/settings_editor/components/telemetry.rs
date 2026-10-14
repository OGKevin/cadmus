//! Rust side of `TelemetryPage`.
//!
//! Row values and the row callback live on [`TelemetryPageIo`]. Rust omits a
//! row whose setting this build does not include.

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{
    SettingRow, SettingsCategory, SettingsWindow, TelemetryPageIo,
};
use crate::view::slint::{CallbackQueue, connect};

pub(super) struct TelemetryPage {
    queue: CallbackQueue<SettingRow>,
}

impl TelemetryPage {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for TelemetryPage {
    type Window = SettingsWindow;
    type Action = SettingRow;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<TelemetryPageIo>();
        connect!(io, self.queue, on_row_pressed, (row) => row);
    }

    fn take(&self) -> Vec<SettingRow> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<TelemetryPageIo>();
        if let Some(rows) =
            setting::publish_rows(&io.get_rows(), SettingsCategory::Telemetry, context)
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
    use super::{Component, Step, TelemetryPage};
    use crate::fl;
    use crate::view::slint::generated::{SettingRow, TelemetryPageIo};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_publishes_telemetry_and_a_tap_applies_the_row() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let mut page = TelemetryPage::new();
        page.wire(window);
        context.settings.logging.enabled = true;
        page.sync(window, &context);

        let io = window.global::<TelemetryPageIo>();
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

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let level = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
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
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(SettingRow::LoggingEnabled, &mut env).await
        };
        assert!(matches!(toggled, Step::Redraw));
        assert!(!context.settings.logging.enabled);
        page.sync(window, &context);
        assert_eq!(
            setting::row_value(
                &window.global::<TelemetryPageIo>().get_rows(),
                SettingRow::LoggingEnabled,
            ),
            fl!("settings-general-toggle-off")
        );
    }
}
