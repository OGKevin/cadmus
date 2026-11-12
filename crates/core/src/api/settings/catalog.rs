//! Settings catalog built from the on-device editor rows.

use super::ident::{category_id, identity_id, is_add_library};
use super::intermission::{
    apply_fill_color, custom_intermission_image, custom_intermission_image_path, fill_color_value,
    require_still_image, with_custom_image,
};
use super::libraries::{add_library, library_row, update_library};
use super::refresh::{apply_refresh_rate, refresh_rate_value};
use super::save;
use super::values::{checks_value, choice_value, text_value, toggle_value};
use crate::api::proto;
use crate::device::{AppContext, DevicePaths, DeviceRuntime};
use crate::settings::Settings;
use crate::view::settings_editor::Category;
use crate::view::settings_editor::kinds::{
    SettingIdentity, SettingKind, SettingsFetchData, TextMode, WidgetKind,
};
use crate::view::{Bus, EntryId, EntryKind, Event, Hub};
use connectrpc::ConnectError;
use std::collections::HashMap;
use std::path::Path;

/// Categories and rows from the on-device editor, minus Dictionaries and Force Full Import.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn list(context: &mut AppContext) -> proto::SettingsCatalog {
    let install = context.device.install_dir();
    let all = Category::all();
    let mut categories = Vec::with_capacity(all.len());
    for category in all {
        if category == Category::Dictionaries {
            continue;
        }
        let kinds = category.settings(context, None).await;
        let mut settings = Vec::with_capacity(kinds.len());
        for kind in &kinds {
            if matches!(kind.identity(), SettingIdentity::ForceFullImport) {
                continue;
            }
            settings.push(row(kind.as_ref(), &context.settings, &install));
        }
        categories.push(proto::SettingsCategory {
            id: category_id(category).into(),
            label: category.label(),
            settings,
            ..Default::default()
        });
    }
    proto::SettingsCatalog {
        categories,
        ..Default::default()
    }
}

/// Apply one catalog row on the main loop, then save.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn update(
    id: &proto::SettingId,
    value: &proto::SettingValue,
    hub: &Hub,
    bus: &mut Bus,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Result<proto::Setting, ConnectError> {
    if is_add_library(id) {
        return add_library(value, hub, context, runtime).await;
    }
    let kind = find_kind(context, id)
        .await
        .ok_or_else(|| ConnectError::not_found("unknown setting"))?;
    apply_update(kind, value, hub, bus, context, runtime).await
}

/// Dispatch by identity, then by value kind.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn apply_update(
    kind: Box<dyn SettingKind>,
    value: &proto::SettingValue,
    hub: &Hub,
    bus: &mut Bus,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Result<proto::Setting, ConnectError> {
    match kind.identity() {
        SettingIdentity::RefreshRate => {
            apply_refresh_rate(&mut context.settings, value)?;
            return commit(kind.as_ref(), hub, context, runtime).await;
        }
        SettingIdentity::IntermissionFillColor => {
            apply_fill_color(&mut context.settings, value)?;
            return commit(kind.as_ref(), hub, context, runtime).await;
        }
        SettingIdentity::LibraryInfo(index) => {
            return update_library(index, value, hub, context, runtime).await;
        }
        _ => {}
    }
    apply_value(kind, value, hub, bus, context, runtime).await
}

/// Checks, text, custom intermission image, or the on-device widget event.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn apply_value(
    kind: Box<dyn SettingKind>,
    value: &proto::SettingValue,
    hub: &Hub,
    bus: &mut Bus,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Result<proto::Setting, ConnectError> {
    if let Some(proto::setting_value::Kind::Checks(_)) = value.kind.as_ref() {
        let install = context.device.install_dir();
        let data = kind.fetch(SettingsFetchData {
            settings: &context.settings,
            install_dir: Some(&install),
        });
        apply_checks(kind.as_ref(), &data.widget, value, context, bus)?;
        return commit(kind.as_ref(), hub, context, runtime).await;
    }
    if let Some(proto::setting_value::Kind::Text(text)) = value.kind.as_ref() {
        let input = kind
            .as_input_kind()
            .ok_or_else(|| ConnectError::invalid_argument("setting is not text"))?;
        let mode = text
            .mode
            .as_known()
            .ok_or_else(|| ConnectError::invalid_argument("unknown text mode"))?;
        super::values::text_mode_from_proto(mode)
            .map_err(|_| ConnectError::invalid_argument("unknown text mode"))?;
        input
            .validate_text(&text.text)
            .map_err(ConnectError::invalid_argument)?;
        let display = input.apply_text(&text.text, &mut context.settings);
        notify(kind.as_ref(), display, hub);
        save(context, runtime).await;
        let install = context.device.install_dir();
        return Ok(row(kind.as_ref(), &context.settings, &install));
    }
    if let Some(path) = custom_intermission_image_path(kind.identity(), value) {
        require_still_image(&path).await?;
        let (display, _) = kind.handle(&Event::FileChooserClosed(Some(path)), context, bus);
        notify_optional(kind.as_ref(), display, hub);
        save(context, runtime).await;
        let install = context.device.install_dir();
        return Ok(row(kind.as_ref(), &context.settings, &install));
    }
    let install = context.device.install_dir();
    let data = kind.fetch(SettingsFetchData {
        settings: &context.settings,
        install_dir: Some(&install),
    });
    if let Some(event) = event_for(&data.widget, value)? {
        let (display, _) = kind.handle(&event, context, bus);
        notify_optional(kind.as_ref(), display, hub);
    }
    save(context, runtime).await;
    Ok(row(kind.as_ref(), &context.settings, &install))
}

/// Fetch the new display string, redraw an open editor, save, return the row.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn commit(
    kind: &dyn SettingKind,
    hub: &Hub,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> Result<proto::Setting, ConnectError> {
    let install = context.device.install_dir();
    let display = kind
        .fetch(SettingsFetchData {
            settings: &context.settings,
            install_dir: Some(&install),
        })
        .value;
    notify(kind, display, hub);
    save(context, runtime).await;
    Ok(row(kind, &context.settings, &install))
}

/// Redraw an open on-device settings row.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn notify(kind: &dyn SettingKind, display: String, hub: &Hub) {
    hub.send(
        Event::Settings(crate::view::settings_editor::SettingsEvent::UpdateValue {
            kind: kind.identity(),
            value: display,
        })
        .into(),
    )
    .ok();
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn notify_optional(kind: &dyn SettingKind, display: Option<String>, hub: &Hub) {
    if let Some(display) = display {
        notify(kind, display, hub);
    }
}

/// Look up a catalog row by SettingId. Skips Force Full Import.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn find_kind(
    context: &mut AppContext,
    id: &proto::SettingId,
) -> Option<Box<dyn SettingKind>> {
    for category in Category::all() {
        if category == Category::Dictionaries {
            continue;
        }
        let kinds = category.settings(context, None).await;
        for kind in kinds {
            if matches!(kind.identity(), SettingIdentity::ForceFullImport) {
                continue;
            }
            if identity_id(&kind.identity()) == *id {
                return Some(kind);
            }
        }
    }
    None
}

/// One companion row from an on-device SettingKind.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn row(kind: &dyn SettingKind, settings: &Settings, install: &Path) -> proto::Setting {
    if let SettingIdentity::LibraryInfo(index) = kind.identity() {
        return library_row(index, settings);
    }
    let data = kind.fetch(SettingsFetchData {
        settings,
        install_dir: Some(install),
    });
    let value = setting_value(kind, settings, &data.value, &data.widget);
    proto::Setting {
        id: identity_id(&kind.identity()).into(),
        label: kind.label(settings),
        value: value.into(),
        ..Default::default()
    }
}

/// Proto value for a row: refresh, fill color, text, toggle, checks, or choice.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn setting_value(
    kind: &dyn SettingKind,
    settings: &Settings,
    display: &str,
    widget: &WidgetKind,
) -> proto::SettingValue {
    if matches!(kind.identity(), SettingIdentity::RefreshRate) {
        return refresh_rate_value(settings);
    }
    if matches!(kind.identity(), SettingIdentity::IntermissionFillColor) {
        return fill_color_value(settings.intermissions.fill_color());
    }
    if let Some(input) = kind.as_input_kind() {
        return text_value(&input.current_text(settings), input.text_mode());
    }
    match widget {
        WidgetKind::Toggle { enabled, .. } => toggle_value(*enabled),
        WidgetKind::SubMenu(entries) => {
            if let Some(path) = custom_intermission_image(kind.identity(), settings) {
                return with_custom_image(submenu_value(entries, ""), path);
            }
            submenu_value(entries, display)
        }
        WidgetKind::ActionLabel(_) | WidgetKind::None => text_value(display, TextMode::Text),
    }
}

/// ChecksValue when the submenu is checkboxes, otherwise ChoiceValue.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn submenu_value(entries: &[EntryKind], display: &str) -> proto::SettingValue {
    let mut checks = Vec::new();
    for entry in entries {
        if let EntryKind::CheckBox(label, id, on) = entry {
            checks.push((check_option_id(id, label), label.clone(), *on));
        }
    }
    if !checks.is_empty() {
        let items: Vec<(&str, &str, bool)> = checks
            .iter()
            .map(|(id, label, on)| (id.as_str(), label.as_str(), *on))
            .collect();
        return checks_value(&items);
    }

    let mut selected = String::new();
    let mut options = Vec::new();
    for entry in entries {
        if let EntryKind::RadioButton(label, _, is_selected) = entry {
            if *is_selected {
                selected.clone_from(label);
            }
            options.push((label.clone(), label.clone()));
        }
    }
    if selected.is_empty() && !display.is_empty() {
        selected = display.to_string();
        if !options.iter().any(|(id, _)| id == display) {
            options.push((display.to_string(), display.to_string()));
        }
    }
    let pairs: Vec<(&str, &str)> = options
        .iter()
        .map(|(id, label)| (id.as_str(), label.as_str()))
        .collect();
    choice_value(&selected, &pairs)
}

/// FileExtension::as_str() for kind toggles, otherwise the on-screen label.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn check_option_id(id: &EntryId, label: &str) -> String {
    match id {
        EntryId::ToggleDitheredKind(kind) | EntryId::ToggleAllowedKind(kind) => {
            kind.as_str().to_string()
        }
        _ => label.to_string(),
    }
}

/// Toggle each checkbox whose on-state differs from the request.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn apply_checks(
    kind: &dyn SettingKind,
    widget: &WidgetKind,
    value: &proto::SettingValue,
    context: &mut AppContext,
    bus: &mut Bus,
) -> Result<(), ConnectError> {
    let proto::setting_value::Kind::Checks(checks) = value
        .kind
        .as_ref()
        .ok_or_else(|| ConnectError::invalid_argument("missing value"))?
    else {
        return Err(ConnectError::invalid_argument("setting is not checks"));
    };
    let WidgetKind::SubMenu(entries) = widget else {
        return Err(ConnectError::invalid_argument("setting is not checks"));
    };
    let mut current = HashMap::new();
    for entry in entries {
        if let EntryKind::CheckBox(label, id, on) = entry {
            current.insert(check_option_id(id, label), (id.clone(), *on));
        }
    }
    let mut requested = HashMap::new();
    for item in &checks.items {
        if !current.contains_key(&item.id) {
            return Err(ConnectError::invalid_argument("unknown file type"));
        }
        requested.insert(item.id.clone(), item.on);
    }
    for (id, on) in requested {
        let Some((entry_id, current_on)) = current.get(&id) else {
            continue;
        };
        if *current_on == on {
            continue;
        }
        kind.handle(&Event::Select(entry_id.clone()), context, bus);
    }
    Ok(())
}

/// On-device Event for a toggle or radio choice. Text, checks, and refresh are separate.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn event_for(
    widget: &WidgetKind,
    value: &proto::SettingValue,
) -> Result<Option<Event>, ConnectError> {
    let kind_value = value
        .kind
        .as_ref()
        .ok_or_else(|| ConnectError::invalid_argument("missing value"))?;
    match kind_value {
        proto::setting_value::Kind::Toggle(requested) => match widget {
            WidgetKind::Toggle {
                enabled, tap_event, ..
            } if enabled != requested => Ok(Some(tap_event.clone())),
            WidgetKind::Toggle { .. } => Ok(None),
            _ => Err(ConnectError::invalid_argument("setting is not a toggle")),
        },
        proto::setting_value::Kind::Text(_) => {
            Err(ConnectError::invalid_argument("text is applied separately"))
        }
        proto::setting_value::Kind::Choice(choice) => match widget {
            WidgetKind::SubMenu(entries) => {
                let event = entries.iter().find_map(|entry| match entry {
                    EntryKind::RadioButton(label, id, _) if label == &choice.selected => {
                        Some(Event::Select(id.clone()))
                    }
                    _ => None,
                });
                event
                    .map(Some)
                    .ok_or_else(|| ConnectError::invalid_argument("unknown choice"))
            }
            _ => Err(ConnectError::invalid_argument("setting is not a choice")),
        },
        proto::setting_value::Kind::RefreshRate(_) => Err(ConnectError::invalid_argument(
            "setting is not a refresh rate",
        )),
        proto::setting_value::Kind::Checks(_) => Err(ConnectError::invalid_argument(
            "checks are applied separately",
        )),
        proto::setting_value::Kind::Path(_) => {
            Err(ConnectError::invalid_argument("setting is not a path"))
        }
        proto::setting_value::Kind::Library(_) => {
            Err(ConnectError::invalid_argument("setting is not a library"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::list;
    use crate::api::proto;
    use crate::context::test_helpers::create_test_context;
    use crate::settings::{IntermKind, IntermissionDisplay};
    use std::path::PathBuf;

    fn listed(
        catalog: &proto::SettingsCatalog,
        kind: proto::SettingIdentityKind,
    ) -> &proto::Setting {
        catalog
            .categories
            .iter()
            .flat_map(|category| &category.settings)
            .find(|setting| setting.id.kind.as_known() == Some(kind))
            .unwrap_or_else(|| panic!("missing {kind:?}"))
    }

    fn listed_choice(
        catalog: &proto::SettingsCatalog,
        kind: proto::SettingIdentityKind,
    ) -> proto::ChoiceValue {
        match listed(catalog, kind)
            .value
            .clone()
            .into_option()
            .unwrap_or_default()
            .kind
        {
            Some(proto::setting_value::Kind::Choice(choice)) => *choice,
            other => panic!("{kind:?} listed as {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn intermission_screens_are_editable_choices() {
        let mut context = create_test_context().await;
        let catalog = list(&mut context).await;
        for kind in [
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_SUSPEND,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_POWER_OFF,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_SHARE,
        ] {
            assert!(listed_choice(&catalog, kind).options.len() >= 4, "{kind:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn custom_intermission_image_stays_selected() {
        let mut context = create_test_context().await;
        context.settings.intermissions[IntermKind::Suspend] =
            IntermissionDisplay::Image(PathBuf::from("/tmp/photo.png"));
        let catalog = list(&mut context).await;
        let choice = listed_choice(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_SUSPEND,
        );
        assert_eq!(choice.selected, "/tmp/photo.png");
        assert!(
            choice
                .options
                .iter()
                .any(|option| { option.id == "/tmp/photo.png" && option.label == "photo.png" }),
            "{choice:?}"
        );
    }
}
