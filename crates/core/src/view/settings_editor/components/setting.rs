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
use crate::view::slint::generated::{SettingRow, SettingsCategory};
use crate::view::{Bus, EntryId, Event};

#[cfg(feature = "tracing")]
use crate::view::settings_editor::kinds::telemetry::OtlpEndpoint;
#[cfg(feature = "profiling")]
use crate::view::settings_editor::kinds::telemetry::PyroscopeEndpoint;
#[cfg(all(feature = "test", feature = "kobo"))]
use crate::view::settings_editor::kinds::telemetry::{EnableDbusLog, EnableKernLog};

/// Which row a tap named.
///
/// Static rows are a [`SettingRow`] from Slint. A library row is an index into
/// the library model, starting at 0.
#[derive(Clone, Copy, Debug)]
pub(super) enum PressedRow {
    Setting(SettingRow),
    /// Index into the library model, starting at 0.
    Library(i32),
}

/// Display string for `row`, or empty when this build has no such setting.
pub(super) fn text(row: SettingRow, context: &AppContext) -> slint::SharedString {
    kind(row, context)
        .map(|kind| display_of(kind.as_ref(), context))
        .unwrap_or_default()
        .into()
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

/// Static rows of `category`, in the same order as that page's `.slint` file.
///
/// Libraries and dictionaries are models, so they have no static rows.
#[cfg(test)]
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
    // Accepting the row closes the list. Redraw copies the new value onto the
    // settings row.
    Step::Redraw
}

/// Applies a submitted string to a settings row.
pub(super) fn apply_text(row: PressedRow, text: &str, context: &mut AppContext) {
    let Some(kind) = pressed_kind(row, context) else {
        return;
    };
    let Some(input) = kind.as_input_kind() else {
        return;
    };
    input.apply_text(text, &mut context.settings);
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
