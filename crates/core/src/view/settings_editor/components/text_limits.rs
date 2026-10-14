//! How many characters a settings text field keeps, and which keyboard page
//! it opens on.
//!
//! Counts and timeouts start on digits. Names and addresses start on letters.
//! Refresh rates are three digits, matching the old input.

use super::setting::PressedRow;
use crate::device::AppContext;
use crate::view::slint::generated::SettingRow;

/// How a settings text field behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TextLimits {
    /// The keyboard opens on digits.
    pub(super) numbers: bool,
    /// How many characters the field keeps.
    pub(super) max_chars: usize,
}

/// Library name: letters, ten characters.
pub(super) const LIBRARY_NAME: TextLimits = TextLimits {
    numbers: false,
    max_chars: 10,
};

/// Refresh rate: digits, three characters.
pub(super) const REFRESH_RATE: TextLimits = TextLimits {
    numbers: true,
    max_chars: 3,
};

/// Limits for the settings row that opened the text dialog.
pub(super) fn for_pressed(row: PressedRow, context: &AppContext) -> TextLimits {
    match row {
        PressedRow::Setting(row) => for_setting(row, context),
        PressedRow::Library(_) => TextLimits {
            numbers: false,
            max_chars: 80,
        },
    }
}

fn for_setting(row: SettingRow, context: &AppContext) -> TextLimits {
    let max_chars = super::setting::kind(row, context)
        .and_then(|kind| kind.as_input_kind().map(|input| input.input_max_chars()))
        .unwrap_or(80);
    TextLimits {
        numbers: numeric(row),
        max_chars,
    }
}

fn numeric(row: SettingRow) -> bool {
    matches!(
        row,
        SettingRow::WifiIdleTimeout
            | SettingRow::AutoFrontlightBrightness
            | SettingRow::AutoFrontlightManualCoordinates
            | SettingRow::SettingsRetention
            | SettingRow::DbBackupRetention
            | SettingRow::AutosleepGrace
            | SettingRow::AutoSuspend
            | SettingRow::AutoPowerOff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limits_follow_the_field() {
        use crate::context::test_helpers::create_test_context;
        use crate::view::slint::generated::SettingRow;

        let context = create_test_context().await;
        let row = |row, numbers, max_chars| {
            assert_eq!(
                for_pressed(PressedRow::Setting(row), &context),
                TextLimits { numbers, max_chars },
                "{row:?}"
            );
        };
        row(SettingRow::WifiIdleTimeout, true, 10);
        row(SettingRow::AutoSuspend, true, 10);
        row(SettingRow::AutoPowerOff, true, 10);
        row(SettingRow::AutosleepGrace, true, 10);
        row(SettingRow::AutoFrontlightBrightness, true, 3);
        row(SettingRow::SettingsRetention, true, 3);
        row(SettingRow::DbBackupRetention, true, 3);
        row(SettingRow::AutoFrontlightManualCoordinates, true, 32);
        row(SettingRow::NtpServer, false, 80);
        assert_eq!(LIBRARY_NAME.max_chars, 10);
        assert!(std::hint::black_box(REFRESH_RATE).numbers);
    }
}
