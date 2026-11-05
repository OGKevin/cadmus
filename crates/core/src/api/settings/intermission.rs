//! Intermission image and fill-color rows for the API settings catalog.

use super::values::choice_value;
use crate::api::proto;
use crate::color::{BLACK, Color, WHITE};
use crate::document::file_extension::FileExtension;
use crate::fl;
use crate::settings::{IntermKind, IntermissionDisplay, Settings};
use crate::view::settings_editor::kinds::SettingIdentity;
use connectrpc::ConnectError;
use std::path::{Path, PathBuf};

/// Custom image path if that screen is Image, not a builtin label.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn custom_intermission_image(
    identity: SettingIdentity,
    settings: &Settings,
) -> Option<&Path> {
    let kind = match identity {
        SettingIdentity::IntermissionSuspend => IntermKind::Suspend,
        SettingIdentity::IntermissionPowerOff => IntermKind::PowerOff,
        SettingIdentity::IntermissionShare => IntermKind::Share,
        _ => return None,
    };
    match &settings.intermissions[kind] {
        IntermissionDisplay::Image(path) => Some(path),
        _ => None,
    }
}

/// Put the full path in ChoiceValue.selected and add a filename option.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn with_custom_image(
    mut value: proto::SettingValue,
    path: &Path,
) -> proto::SettingValue {
    let Some(proto::setting_value::Kind::Choice(choice)) = value.kind.as_mut() else {
        return value;
    };
    let full = path.to_string_lossy().into_owned();
    let label = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| full.clone());
    choice.selected.clone_from(&full);
    if !choice.options.iter().any(|option| option.id == full) {
        choice.options.push(proto::ChoiceOption {
            id: full,
            label,
            ..Default::default()
        });
    }
    value
}

/// Absolute path from PathValue or a ChoiceValue.selected that looks like a path.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn custom_intermission_image_path(
    identity: SettingIdentity,
    value: &proto::SettingValue,
) -> Option<PathBuf> {
    if !matches!(
        identity,
        SettingIdentity::IntermissionSuspend
            | SettingIdentity::IntermissionPowerOff
            | SettingIdentity::IntermissionShare
    ) {
        return None;
    }
    match value.kind.as_ref() {
        Some(proto::setting_value::Kind::Path(path)) => Some(PathBuf::from(path.path.trim())),
        Some(proto::setting_value::Kind::Choice(choice)) => {
            let selected = choice.selected.trim();
            if Path::new(selected).is_absolute() || selected.contains('/') {
                Some(PathBuf::from(selected))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// True for the still images the intermission screen can draw.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn is_still_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(|suffix| suffix.to_ascii_lowercase().parse::<FileExtension>().ok())
        .is_some_and(FileExtension::is_still_image)
}

/// Reject missing files and non-image suffixes.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn require_still_image(path: &Path) -> Result<(), ConnectError> {
    if !is_still_image_path(path) {
        return Err(ConnectError::invalid_argument("path is not an image"));
    }
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| ConnectError::invalid_argument("path is not an image"))?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(ConnectError::invalid_argument("path is not an image"))
    }
}

/// `#rrggbb` from an on-device Color.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn rgb_hex(color: Color) -> String {
    let [red, green, blue] = color.rgb();
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// White, Black, or a custom `#rrggbb` choice.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn fill_color_value(color: Color) -> proto::SettingValue {
    let white = fl!("settings-intermission-fill-color-white");
    let black = fl!("settings-intermission-fill-color-black");
    let hex = rgb_hex(color);
    let selected = if color == WHITE {
        "#ffffff"
    } else if color == BLACK {
        "#000000"
    } else {
        hex.as_str()
    };
    let mut options = vec![("#ffffff", white.as_str()), ("#000000", black.as_str())];
    if selected != "#ffffff" && selected != "#000000" {
        options.push((selected, selected));
    }
    choice_value(selected, &options)
}

/// Parse a fill-color ChoiceValue onto Settings.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn apply_fill_color(
    settings: &mut Settings,
    value: &proto::SettingValue,
) -> Result<(), ConnectError> {
    let selected = match value.kind.as_ref() {
        Some(proto::setting_value::Kind::Choice(choice)) => choice.selected.as_str(),
        _ => return Err(ConnectError::invalid_argument("setting is not a choice")),
    };
    settings
        .intermissions
        .set_fill_color(parse_fill_color(selected)?);
    Ok(())
}

/// Choice ids are always hex, so white and black round-trip through the palette.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn parse_fill_color(selected: &str) -> Result<Color, ConnectError> {
    Color::from_hex(selected).ok_or_else(|| ConnectError::invalid_argument("invalid RGB color"))
}

#[cfg(test)]
mod tests {
    use super::{fill_color_value, parse_fill_color};
    use crate::api::proto;
    use crate::color::{BLACK, Color, WHITE};

    #[test]
    fn parse_fill_color_accepts_hex_only() {
        assert_eq!(parse_fill_color("#ffffff").unwrap(), WHITE);
        assert_eq!(parse_fill_color("#000000").unwrap(), BLACK);
        assert_eq!(
            parse_fill_color("#804020").unwrap(),
            Color::Rgb(128, 64, 32)
        );
        assert!(parse_fill_color("nope").is_err());
    }

    #[test]
    fn fill_color_value_uses_hex_ids() {
        let white = fill_color_value(WHITE);
        let Some(proto::setting_value::Kind::Choice(choice)) = white.kind else {
            panic!("expected choice");
        };
        assert_eq!(choice.selected, "#ffffff");
        assert_eq!(choice.options[0].id, "#ffffff");
        assert_eq!(choice.options[1].id, "#000000");

        let custom = fill_color_value(Color::Rgb(128, 64, 32));
        let Some(proto::setting_value::Kind::Choice(choice)) = custom.kind else {
            panic!("expected choice");
        };
        assert_eq!(choice.selected, "#804020");
        assert!(choice.options.iter().any(|option| option.id == "#804020"));
    }
}
