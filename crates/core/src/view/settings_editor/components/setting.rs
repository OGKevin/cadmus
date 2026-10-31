//! Shared handling for a [`SettingRow`] tap.
//!
//! Each category component fires that row enum. This module maps it to a
//! [`SettingKind`] and returns the [`Step`] that opens the choice sheet, text
//! sheet, or file browser the row asks for, or applies a toggle in place.

use std::path::PathBuf;

use super::FilesMode;
use super::text_limits;
use super::{Editor, Env, Step, Waiting};
use crate::device::AppContext;
use crate::device::DevicePaths as _;
use crate::device::soft_suspend::SoftSuspendBackend as _;
use crate::fl;
use crate::view::settings_editor::kinds::general::{
    AutoFrontlight, AutoFrontlightBrightness, AutoFrontlightManualCoordinates, AutoShare, AutoTime,
    ButtonScheme, DbBackupRetentionSetting, KeyboardLayout, Locale, NtpServer, SettingsRetention,
    StartupModeSetting, WifiIdleTimeout,
};
use crate::view::settings_editor::kinds::import::{
    AllowedKindsSetting, ForceFullImport, ImportSyncMetadata,
};
use crate::view::settings_editor::kinds::intermission::{
    IntermissionFillColor, IntermissionPowerOff, IntermissionShare, IntermissionSuspend,
};
use crate::view::settings_editor::kinds::library::LibraryInfo;
use crate::view::settings_editor::kinds::power::{
    AutoPowerOff, AutoSuspend, AutosleepGrace, AutosleepModeSetting, IndicateAutosleepLed,
    SleepCover,
};
use crate::view::settings_editor::kinds::reader::{
    DitheredKindsSetting, FinishedActionSetting, FontFamily, RefreshRateInfo,
};
use crate::view::settings_editor::kinds::telemetry::{LogLevel, LoggingEnabled};
use crate::view::settings_editor::kinds::{SettingKind, SettingsFetchData, WidgetKind};
use crate::view::slint::ListRow;
use crate::view::slint::generated::{PagedLine, SettingsCategory};
use crate::view::slint::paged_line;
use crate::view::{Bus, EntryId, Event};

#[cfg(feature = "tracing")]
use crate::view::settings_editor::kinds::telemetry::OtlpEndpoint;
#[cfg(feature = "profiling")]
use crate::view::settings_editor::kinds::telemetry::PyroscopeEndpoint;
#[cfg(all(feature = "test", feature = "kobo"))]
use crate::view::settings_editor::kinds::telemetry::{EnableDbusLog, EnableKernLog};

/// One row of a settings page.
///
/// The row names the setting, so a tap does not depend on a list index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::view::settings_editor) enum SettingRow {
    Locale,
    AutoShare,
    AutoTime,
    NtpServer,
    AutoFrontlight,
    AutoFrontlightBrightness,
    AutoFrontlightManualCoordinates,
    WifiIdleTimeout,
    ButtonScheme,
    KeyboardLayout,
    SettingsRetention,
    DbBackupRetention,
    StartupMode,
    AutosleepMode,
    IndicateAutosleepLed,
    AutosleepGrace,
    AutoSuspend,
    AutoPowerOff,
    SleepCover,
    FontFamily,
    FinishedAction,
    DitheredKinds,
    RefreshRate,
    IntermissionSuspend,
    IntermissionPowerOff,
    IntermissionShare,
    IntermissionFillColor,
    ForceFullImport,
    ImportSyncMetadata,
    AllowedKinds,
    LoggingEnabled,
    LogLevel,
    OtlpEndpoint,
    PyroscopeEndpoint,
    EnableKernLog,
    EnableDbusLog,
}

/// Which row a tap named.
///
/// Static rows are a [`SettingRow`]. A library row is an index into the
/// library model, starting at 0.
#[derive(Clone, Copy, Debug)]
pub(super) enum PressedRow {
    Setting(SettingRow),
    /// Index into the library model, starting at 0.
    Library(i32),
}

/// The [`SettingKind`] that reads and writes `row`.
///
/// The `.slint` file passes [`SettingRow`] in the callback, so this does not
/// depend on a list index. A row that the build does not include is `None`.
pub(in crate::view::settings_editor) fn kind(
    row: SettingRow,
    context: &AppContext,
) -> Option<Box<dyn SettingKind>> {
    match row {
        SettingRow::Locale => Some(Box::new(Locale)),
        SettingRow::AutoShare => Some(Box::new(AutoShare)),
        SettingRow::AutoTime => Some(Box::new(AutoTime)),
        SettingRow::NtpServer => Some(Box::new(NtpServer)),
        SettingRow::AutoFrontlight => Some(Box::new(AutoFrontlight)),
        SettingRow::AutoFrontlightBrightness => Some(Box::new(AutoFrontlightBrightness)),
        SettingRow::AutoFrontlightManualCoordinates => {
            Some(Box::new(AutoFrontlightManualCoordinates))
        }
        SettingRow::WifiIdleTimeout => Some(Box::new(WifiIdleTimeout)),
        SettingRow::ButtonScheme => Some(Box::new(ButtonScheme)),
        SettingRow::KeyboardLayout => Some(Box::new(KeyboardLayout)),
        SettingRow::SettingsRetention => Some(Box::new(SettingsRetention)),
        SettingRow::DbBackupRetention => Some(Box::new(DbBackupRetentionSetting)),
        SettingRow::StartupMode => Some(Box::new(StartupModeSetting)),
        SettingRow::AutosleepMode => Some(Box::new(AutosleepModeSetting::new(
            context.inhibitor.available_modes(),
        ))),
        SettingRow::IndicateAutosleepLed => Some(Box::new(IndicateAutosleepLed)),
        SettingRow::AutosleepGrace => Some(Box::new(AutosleepGrace)),
        SettingRow::AutoSuspend => Some(Box::new(AutoSuspend)),
        SettingRow::AutoPowerOff => Some(Box::new(AutoPowerOff)),
        SettingRow::SleepCover => Some(Box::new(SleepCover)),
        SettingRow::FontFamily => Some(Box::new(FontFamily)),
        SettingRow::FinishedAction => Some(Box::new(FinishedActionSetting)),
        SettingRow::DitheredKinds => Some(Box::new(DitheredKindsSetting)),
        SettingRow::RefreshRate => Some(Box::new(RefreshRateInfo)),
        SettingRow::IntermissionSuspend => Some(Box::new(IntermissionSuspend)),
        SettingRow::IntermissionPowerOff => Some(Box::new(IntermissionPowerOff)),
        SettingRow::IntermissionShare => Some(Box::new(IntermissionShare)),
        SettingRow::IntermissionFillColor => Some(Box::new(IntermissionFillColor)),
        SettingRow::ForceFullImport => Some(Box::new(ForceFullImport)),
        SettingRow::ImportSyncMetadata => Some(Box::new(ImportSyncMetadata)),
        SettingRow::AllowedKinds => Some(Box::new(AllowedKindsSetting)),
        SettingRow::LoggingEnabled => Some(Box::new(LoggingEnabled)),
        SettingRow::LogLevel => Some(Box::new(LogLevel)),
        #[cfg(feature = "tracing")]
        SettingRow::OtlpEndpoint => Some(Box::new(OtlpEndpoint)),
        #[cfg(not(feature = "tracing"))]
        SettingRow::OtlpEndpoint => None,
        #[cfg(feature = "profiling")]
        SettingRow::PyroscopeEndpoint => Some(Box::new(PyroscopeEndpoint)),
        #[cfg(not(feature = "profiling"))]
        SettingRow::PyroscopeEndpoint => None,
        #[cfg(all(feature = "test", feature = "kobo"))]
        SettingRow::EnableKernLog => Some(Box::new(EnableKernLog)),
        #[cfg(not(all(feature = "test", feature = "kobo")))]
        SettingRow::EnableKernLog => None,
        #[cfg(all(feature = "test", feature = "kobo"))]
        SettingRow::EnableDbusLog => Some(Box::new(EnableDbusLog)),
        #[cfg(not(all(feature = "test", feature = "kobo")))]
        SettingRow::EnableDbusLog => None,
    }
}

/// Static rows of `category`, in page order.
///
/// Libraries and dictionaries are models, so they have no static rows.
/// Rows this device or build does not show are dropped before the page is drawn.
pub(in crate::view::settings_editor) fn static_rows(
    category: SettingsCategory,
) -> &'static [SettingRow] {
    match category {
        SettingsCategory::General => &[
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
        ],
        SettingsCategory::Power => &[
            SettingRow::AutosleepMode,
            SettingRow::IndicateAutosleepLed,
            SettingRow::AutosleepGrace,
            SettingRow::AutoSuspend,
            SettingRow::AutoPowerOff,
            SettingRow::SleepCover,
        ],
        SettingsCategory::Reader => &[
            SettingRow::FontFamily,
            SettingRow::FinishedAction,
            SettingRow::DitheredKinds,
            SettingRow::RefreshRate,
        ],
        SettingsCategory::Intermissions => &[
            SettingRow::IntermissionSuspend,
            SettingRow::IntermissionPowerOff,
            SettingRow::IntermissionShare,
            SettingRow::IntermissionFillColor,
        ],
        SettingsCategory::Import => &[
            SettingRow::ForceFullImport,
            SettingRow::ImportSyncMetadata,
            SettingRow::AllowedKinds,
        ],
        SettingsCategory::Telemetry => &[
            SettingRow::LoggingEnabled,
            SettingRow::LogLevel,
            SettingRow::OtlpEndpoint,
            SettingRow::PyroscopeEndpoint,
            SettingRow::EnableKernLog,
            SettingRow::EnableDbusLog,
        ],
        SettingsCategory::Libraries | SettingsCategory::Dictionaries => &[],
    }
}

struct ShownRow {
    row: SettingRow,
    kind: Box<dyn SettingKind>,
}

/// Rows `category` shows, each with the [`SettingKind`] that reads it.
///
/// Soft suspend is omitted when the device has no such modes. A telemetry
/// row whose [`kind`] is `None` is omitted, so the page does not keep an
/// empty slot for a feature this build lacks. The page and the category
/// tests both use this list.
fn shown(category: SettingsCategory, context: &AppContext) -> Vec<ShownRow> {
    static_rows(category)
        .iter()
        .copied()
        .filter_map(|row| {
            if soft_suspend_hidden(row, context) {
                return None;
            }
            kind(row, context).map(|kind| ShownRow { row, kind })
        })
        .collect()
}

fn soft_suspend_hidden(row: SettingRow, context: &AppContext) -> bool {
    matches!(
        row,
        SettingRow::AutosleepMode | SettingRow::IndicateAutosleepLed | SettingRow::AutosleepGrace
    ) && !context.inhibitor.is_supported()
}

/// The [`SettingKind`]s a static settings page shows.
///
/// Libraries and dictionaries are not in this list. Only the category tests
/// read this; the pages publish rows through [`publish_rows`].
#[cfg(test)]
pub(in crate::view::settings_editor) fn shown_kinds(
    category: SettingsCategory,
    context: &AppContext,
) -> Vec<Box<dyn SettingKind>> {
    shown(category, context)
        .into_iter()
        .map(|shown| shown.kind)
        .collect()
}

/// Rows `category` shows for this device and build.
#[cfg(test)]
fn visible_rows(category: SettingsCategory, context: &AppContext) -> Vec<SettingRow> {
    shown(category, context)
        .into_iter()
        .map(|shown| shown.row)
        .collect()
}

fn line_of(kind: &dyn SettingKind, context: &AppContext) -> PagedLine {
    paged_line(ListRow {
        label: kind.label(&context.settings),
        value: display_of(kind, context),
        ..Default::default()
    })
}

/// The rows `category` shows, in order, with the line each one draws.
pub(super) fn shown_rows(
    category: SettingsCategory,
    context: &AppContext,
) -> Vec<(SettingRow, PagedLine)> {
    shown(category, context)
        .iter()
        .map(|shown| {
            let line = line_of(shown.kind.as_ref(), context);
            (shown.row, line)
        })
        .collect()
}

/// Turns a row tap into the [`Step`] that shows or applies it.
pub(super) fn activate(row: PressedRow, env: &mut Env<'_>) -> Step {
    let Some(kind) = pressed_kind(row, env.context) else {
        return Step::Idle;
    };
    let install_dir = env.context.device.install_dir();
    let data = kind.fetch(SettingsFetchData {
        settings: &env.context.settings,
        install_dir: Some(install_dir.as_path()),
    });
    let label = kind.label(&env.context.settings);
    match data.widget {
        WidgetKind::None => Step::Idle,
        WidgetKind::Toggle { tap_event, .. } => {
            kind.handle(&tap_event, env.context, env.bus);
            Step::Redraw
        }
        WidgetKind::SubMenu(entries) => Step::choice(Waiting::SettingChoice(row), label, entries),
        WidgetKind::ActionLabel(event) => {
            if kind.as_input_kind().is_some() {
                begin_input(row, env)
            } else {
                dispatch_event(&event, env)
            }
        }
    }
}

/// Applies the entry `id` chosen from the settings choice sheet.
///
/// Accepting the row closes the list. [`Step::Redraw`] copies the new value
/// onto the settings row. A choice that keeps the menu open refreshes the
/// list instead of closing it.
pub(super) fn apply_setting_choice(row: PressedRow, id: EntryId, env: &mut Env<'_>) -> Step {
    let Some(kind) = pressed_kind(row, env.context) else {
        return Step::Idle;
    };
    if kind.file_chooser_entry_id().as_ref() == Some(&id) {
        if let PressedRow::Setting(setting) = row {
            let start = env.context.device.install_dir();
            return Step::file(Waiting::SettingFile(setting), start, FilesMode::File);
        }
        return Step::Idle;
    }
    let keep = kind.keep_menu_open();
    kind.handle(&Event::Select(id), env.context, env.bus);
    if keep {
        return refresh_choice(row, env);
    }
    Step::Redraw
}

/// Applies a submitted string to a settings row.
///
/// The kind handles `Event::Submit` first, so alarms and other live state
/// stay in step with the saved value. A kind that ignores the submit still
/// persists the text.
pub(super) fn apply_text(row: PressedRow, text: &str, env: &mut Env<'_>) {
    let Some(kind) = pressed_kind(row, env.context) else {
        return;
    };
    let Some(input) = kind.as_input_kind() else {
        return;
    };
    let event = Event::Submit(input.submit_view_id(), text.to_string());
    let (_display, consumed) = kind.handle(&event, env.context, env.bus);
    if !consumed {
        input.apply_text(text, &mut env.context.settings);
    }
}

/// Applies a file chosen for a row that used the file chooser.
pub(super) fn apply_file_choice(row: SettingRow, path: PathBuf, context: &mut AppContext) {
    if let Some(kind) = kind(row, context) {
        let mut ignored = Bus::new();
        kind.handle(&Event::FileChooserClosed(Some(path)), context, &mut ignored);
    }
}

fn begin_input(row: PressedRow, env: &mut Env<'_>) -> Step {
    let Some(kind) = pressed_kind(row, env.context) else {
        return Step::Idle;
    };
    let Some(input) = kind.as_input_kind() else {
        return Step::Idle;
    };
    Step::text(
        Waiting::SettingText(row),
        input.input_label(),
        input.current_text(&env.context.settings),
        text_limits::for_pressed(row, env.context),
    )
}

/// The same list with its rows read again, so a toggle shows its new state.
fn refresh_choice(row: PressedRow, env: &mut Env<'_>) -> Step {
    let Some(kind) = pressed_kind(row, env.context) else {
        return Step::Idle;
    };
    let install_dir = env.context.device.install_dir();
    let data = kind.fetch(SettingsFetchData {
        settings: &env.context.settings,
        install_dir: Some(install_dir.as_path()),
    });
    match data.widget {
        WidgetKind::SubMenu(entries) => Step::Rows(entries),
        _ => Step::Idle,
    }
}

fn dispatch_event(event: &Event, _env: &mut Env<'_>) -> Step {
    match event {
        Event::OpenRefreshRateEditor => Step::Edit(Editor::RefreshRates),
        Event::EditLibrary(index) => Step::Edit(Editor::Library(Some(*index))),
        Event::Select(EntryId::RequestForceImport) => Step::confirm(
            Waiting::ForceImport,
            fl!("settings-import-force-full-import-confirm"),
            fl!("settings-import-force-full-import-confirm-button"),
        ),
        _ => Step::Idle,
    }
}

fn pressed_kind(row: PressedRow, context: &AppContext) -> Option<Box<dyn SettingKind>> {
    match row {
        PressedRow::Setting(row) => kind(row, context),
        PressedRow::Library(index) if index >= 0 => {
            let index = index as usize;
            (index < context.settings.libraries.len())
                .then(|| Box::new(LibraryInfo(index)) as Box<dyn SettingKind>)
        }
        PressedRow::Library(_) => None,
    }
}

pub(super) fn display_of(kind: &dyn SettingKind, context: &AppContext) -> String {
    let install_dir = context.device.install_dir();
    let data = kind.fetch(SettingsFetchData {
        settings: &context.settings,
        install_dir: Some(install_dir.as_path()),
    });
    display_value(&data.value, &data.widget)
}

fn display_value(value: &str, widget: &WidgetKind) -> String {
    match widget {
        WidgetKind::Toggle {
            enabled,
            left_label,
            right_label,
            ..
        } => {
            if *enabled {
                left_label.clone()
            } else {
                right_label.clone()
            }
        }
        _ => value.to_string(),
    }
}

pub(super) fn entry_id(entry: &crate::view::EntryKind) -> Option<EntryId> {
    match entry {
        crate::view::EntryKind::Command(_, id)
        | crate::view::EntryKind::CheckBox(_, id, _)
        | crate::view::EntryKind::RadioButton(_, id, _) => Some(id.clone()),
        _ => None,
    }
}

/// The index of `row` among the rows a page shows.
#[cfg(test)]
pub(in crate::view::settings_editor) fn row_index(rows: &[SettingRow], row: SettingRow) -> usize {
    rows.iter()
        .position(|candidate| *candidate == row)
        .unwrap_or_else(|| panic!("missing {row:?}"))
}

/// The value the row at `index` shows.
#[cfg(test)]
pub(in crate::view::settings_editor) fn row_value(
    rows: &slint::ModelRc<PagedLine>,
    index: usize,
) -> String {
    use slint::Model as _;

    rows.row_data(index)
        .unwrap_or_else(|| panic!("missing row {index}"))
        .value
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::test_helpers::create_test_context;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn visible_rows_omit_unsupported_soft_suspend() {
        let context = create_test_context().await;
        assert!(!context.inhibitor.is_supported());
        let rows = visible_rows(SettingsCategory::Power, &context);
        for hidden in [
            SettingRow::AutosleepMode,
            SettingRow::IndicateAutosleepLed,
            SettingRow::AutosleepGrace,
        ] {
            assert!(!rows.contains(&hidden), "{hidden:?} was shown");
        }
        assert!(rows.contains(&SettingRow::AutoSuspend));
        assert_eq!(rows.first().copied(), Some(SettingRow::AutoSuspend));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_row_label_is_the_fluent_string() {
        let context = create_test_context().await;

        let rows = shown_rows(SettingsCategory::General, &context);
        let language = rows
            .iter()
            .find(|(row, _)| *row == SettingRow::Locale)
            .expect("language row");
        assert_eq!(language.1.label.as_str(), fl!("settings-general-language"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn apply_text_zero_cancels_auto_suspend_alarm() {
        use crate::AlarmType;

        let mut context = create_test_context().await;
        context.settings.auto_suspend = 30.0;
        crate::device::reschedule_auto_suspend_alarm(&mut context);
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        {
            let mut env = super::super::settings_env(&hub, &mut bus, &mut context);
            apply_text(PressedRow::Setting(SettingRow::AutoSuspend), "0", &mut env);
        }

        assert_eq!(context.settings.auto_suspend, 0.0);
        assert!(
            !context
                .alarm_manager
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .has_alarm(AlarmType::AutoSuspend)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn apply_text_auto_frontlight_brightness_pushes_config_changed() {
        let mut context = create_test_context().await;
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        {
            let mut env = super::super::settings_env(&hub, &mut bus, &mut context);
            apply_text(
                PressedRow::Setting(SettingRow::AutoFrontlightBrightness),
                "25",
                &mut env,
            );
        }

        assert_eq!(
            context.settings.auto_frontlight_night_brightness,
            Some(25.0.into())
        );
        assert!(
            bus.iter()
                .any(|event| matches!(event, Event::AutoFrontlightConfigChanged))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn apply_text_wifi_idle_timeout_updates_when_handle_ignores_submit() {
        let mut context = create_test_context().await;
        context.settings.wifi_idle_timeout = 10.0;
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        {
            let mut env = super::super::settings_env(&hub, &mut bus, &mut context);
            apply_text(
                PressedRow::Setting(SettingRow::WifiIdleTimeout),
                "0",
                &mut env,
            );
        }

        assert_eq!(context.settings.wifi_idle_timeout, 0.0);
        assert!(bus.is_empty());
    }
}
