//! Rust side of `TelemetryPage`.
//!
//! Row values, the feature flags, and the row callback live on [`TelemetryPageIo`].

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step};
use crate::device::AppContext;
use crate::view::slint::generated::{SettingRow, SettingsWindow, TelemetryPageIo};
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
        io.set_show_otlp(cfg!(feature = "tracing"));
        io.set_show_pyroscope(cfg!(feature = "profiling"));
        io.set_show_kern_log(cfg!(all(feature = "test", feature = "kobo")));
        io.set_show_dbus_log(cfg!(all(feature = "test", feature = "kobo")));
        io.set_logging_enabled(setting::text(SettingRow::LoggingEnabled, context));
        io.set_log_level(setting::text(SettingRow::LogLevel, context));
        io.set_otlp_endpoint(setting::text(SettingRow::OtlpEndpoint, context));
        io.set_pyroscope_endpoint(setting::text(SettingRow::PyroscopeEndpoint, context));
        io.set_enable_kern_log(setting::text(SettingRow::EnableKernLog, context));
        io.set_enable_dbus_log(setting::text(SettingRow::EnableDbusLog, context));
    }

    async fn apply(&mut self, row: SettingRow, env: &mut Env<'_>) -> Step {
        setting::activate(PressedRow::Setting(row), env)
    }
}
