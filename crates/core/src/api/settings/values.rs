use crate::api::proto;
use crate::view::settings_editor::kinds::TextMode;

include!(concat!(env!("OUT_DIR"), "/api_text_mode.rs"));

/// On/off row.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(crate) fn toggle_value(on: bool) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::Toggle(on)),
        ..Default::default()
    }
}

/// Single-select row. Option id and label are the on-screen strings.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(crate) fn choice_value(selected: &str, options: &[(&str, &str)]) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::Choice(Box::new(choice(
            selected, options,
        )))),
        ..Default::default()
    }
}

#[cfg(test)]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
/// Custom intermission image path.
pub(crate) fn path_value(path: &str) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::Path(Box::new(
            proto::PathValue {
                path: path.to_string(),
                ..Default::default()
            },
        ))),
        ..Default::default()
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
/// Independent on/off items, used for file-type sets.
pub(crate) fn checks_value(items: &[(&str, &str, bool)]) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::Checks(Box::new(
            proto::ChecksValue {
                items: items
                    .iter()
                    .map(|(id, label, on)| proto::CheckOption {
                        id: (*id).to_string(),
                        label: (*label).to_string(),
                        on: *on,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
        ))),
        ..Default::default()
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
/// One library row, or the AddLibrary payload.
pub(crate) fn library_value(
    name: &str,
    path: &str,
    finished: proto::ChoiceValue,
    remove: bool,
) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::Library(Box::new(
            proto::LibraryValue {
                name: name.to_string(),
                path: proto::PathValue {
                    path: path.to_string(),
                    ..Default::default()
                }
                .into(),
                finished: finished.into(),
                remove,
                ..Default::default()
            },
        ))),
        ..Default::default()
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
/// ChoiceValue without the SettingValue wrapper.
pub(super) fn choice(selected: &str, options: &[(&str, &str)]) -> proto::ChoiceValue {
    proto::ChoiceValue {
        selected: selected.to_string(),
        options: options
            .iter()
            .map(|(id, label)| proto::ChoiceOption {
                id: (*id).to_string(),
                label: (*label).to_string(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
/// Free-text row plus editor mode.
pub(super) fn text_value(text: &str, mode: TextMode) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::Text(Box::new(
            proto::TextValue {
                text: text.to_string(),
                mode: proto_text_mode(mode).into(),
                ..Default::default()
            },
        ))),
        ..Default::default()
    }
}
