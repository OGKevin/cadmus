//! Library rows and AddLibrary updates for the API settings catalog.

use super::ident::identity_id;
use super::save;
use super::values::{choice, library_value};
use crate::api::proto;
use crate::device::{AppContext, DeviceRuntime};
use crate::fl;
use crate::i18n::I18nDisplay;
use crate::settings::{FinishedAction, LibrarySettings};
use crate::view::settings_editor::kinds::SettingIdentity;
use crate::view::{Event, Hub};
use connectrpc::ConnectError;
use std::path::{Path, PathBuf};

/// One Libraries row. Empty Setting if the index is gone.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(index)))]
pub(super) fn library_row(index: usize, settings: &crate::settings::Settings) -> proto::Setting {
    let Some(library) = settings.libraries.get(index) else {
        return proto::Setting {
            id: identity_id(&SettingIdentity::LibraryInfo(index)).into(),
            ..Default::default()
        };
    };
    proto::Setting {
        id: identity_id(&SettingIdentity::LibraryInfo(index)).into(),
        label: library.name.clone(),
        value: library_value(
            &library.name,
            &absolute_path_text(&library.path),
            finished_choice(library.finished),
            false,
        )
        .into(),
        ..Default::default()
    }
}

/// Inherit plus Notify, Close, Go to next.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn finished_choice(current: Option<FinishedAction>) -> proto::ChoiceValue {
    let inherit = fl!("settings-library-inherit");
    let notify = FinishedAction::Notify.to_i18n_string();
    let close = FinishedAction::Close.to_i18n_string();
    let next = FinishedAction::GoToNext.to_i18n_string();
    let selected = match current {
        None => inherit.as_str(),
        Some(FinishedAction::Notify) => notify.as_str(),
        Some(FinishedAction::Close) => close.as_str(),
        Some(FinishedAction::GoToNext) => next.as_str(),
    };
    choice(
        selected,
        &[
            (inherit.as_str(), inherit.as_str()),
            (notify.as_str(), notify.as_str()),
            (close.as_str(), close.as_str()),
            (next.as_str(), next.as_str()),
        ],
    )
}

/// Empty or inherit is None. Unknown labels are invalid_argument.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn parse_finished(selected: &str) -> Result<Option<FinishedAction>, ConnectError> {
    if selected.is_empty() || selected == fl!("settings-library-inherit") {
        return Ok(None);
    }
    for action in [
        FinishedAction::Notify,
        FinishedAction::Close,
        FinishedAction::GoToNext,
    ] {
        if selected == action.to_i18n_string() {
            return Ok(Some(action));
        }
    }
    Err(ConnectError::invalid_argument("unknown choice"))
}

/// Require a LibraryValue payload.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn library_payload(value: &proto::SettingValue) -> Result<&proto::LibraryValue, ConnectError> {
    match value.kind.as_ref() {
        Some(proto::setting_value::Kind::Library(library)) => Ok(library.as_ref()),
        _ => Err(ConnectError::invalid_argument("setting is not a library")),
    }
}

/// Library path string from the proto.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn path_text(library: &proto::LibraryValue) -> &str {
    library.path.path.as_str()
}

/// Finished-action ChoiceValue.selected.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn finished_selected(library: &proto::LibraryValue) -> &str {
    library.finished.selected.as_str()
}

/// Path must exist and be a directory.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn require_directory(path: &Path) -> Result<(), ConnectError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| ConnectError::invalid_argument("path is not a directory"))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(ConnectError::invalid_argument("path is not a directory"))
    }
}

/// Absolute display path. `.` becomes the process current directory.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn absolute_path_text(path: &Path) -> String {
    std::path::absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
/// Append a library from an AddLibrary Update.
pub(super) async fn add_library(
    value: &proto::SettingValue,
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Result<proto::Setting, ConnectError> {
    let payload = library_payload(value)?;
    let name = {
        let name = payload.name.trim();
        if name.is_empty() {
            "untitled".to_string()
        } else {
            name.to_string()
        }
    };
    let path = {
        let text = path_text(payload).trim();
        if text.is_empty() {
            LibrarySettings::default().path
        } else {
            PathBuf::from(text)
        }
    };
    require_directory(&path).await?;
    let finished = parse_finished(finished_selected(payload))?;
    context.settings.libraries.push(LibrarySettings {
        name,
        path,
        finished,
        ..Default::default()
    });
    let index = context.settings.libraries.len() - 1;
    hub.send(
        Event::Settings(crate::view::settings_editor::SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryInfo(index),
            value: context.settings.libraries[index].path.display().to_string(),
        })
        .into(),
    )
    .ok();
    save(context, runtime).await;
    Ok(library_row(index, &context.settings))
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(index)))]
/// Edit or remove one library by index.
pub(super) async fn update_library(
    index: usize,
    value: &proto::SettingValue,
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Result<proto::Setting, ConnectError> {
    if index >= context.settings.libraries.len() {
        return Err(ConnectError::not_found("unknown setting"));
    }
    let payload = library_payload(value)?;
    if payload.remove {
        context.settings.libraries.remove(index);
        save(context, runtime).await;
        return Ok(proto::Setting {
            id: identity_id(&SettingIdentity::LibraryInfo(index)).into(),
            ..Default::default()
        });
    }
    let name = payload.name.trim();
    if name.is_empty() {
        return Err(ConnectError::invalid_argument("library name is empty"));
    }
    let path = PathBuf::from(path_text(payload).trim());
    require_directory(&path).await?;
    let selected = finished_selected(payload);
    let finished = if selected.is_empty() {
        context.settings.libraries[index].finished
    } else {
        parse_finished(selected)?
    };
    let library = &mut context.settings.libraries[index];
    library.name = name.to_string();
    library.path = path;
    library.finished = finished;
    hub.send(
        Event::Settings(crate::view::settings_editor::SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryInfo(index),
            value: library.path.display().to_string(),
        })
        .into(),
    )
    .ok();
    save(context, runtime).await;
    Ok(library_row(index, &context.settings))
}

#[cfg(test)]
mod tests {
    use super::absolute_path_text;
    use std::path::Path;

    #[test]
    fn library_list_paths_are_absolute() {
        assert!(Path::new(&absolute_path_text(Path::new("."))).is_absolute());
        assert_eq!(
            absolute_path_text(Path::new("/mnt/onboard")),
            "/mnt/onboard"
        );
    }
}
