use crate::api::proto;
use crate::view::settings_editor::Category;
use crate::view::settings_editor::kinds::SettingIdentity;

/// SettingId with no index or key.
fn unit(kind: proto::SettingIdentityKind) -> proto::SettingId {
    proto::SettingId {
        kind: kind.into(),
        ..Default::default()
    }
}

/// LibraryInfo and related rows.
fn indexed(kind: proto::SettingIdentityKind, index: usize) -> proto::SettingId {
    proto::SettingId {
        kind: kind.into(),
        index: Some(u32::try_from(index).unwrap_or(u32::MAX)),
        ..Default::default()
    }
}

/// DictionaryInfo language or RefreshRateByKind extension.
fn keyed(kind: proto::SettingIdentityKind, key: &str) -> proto::SettingId {
    proto::SettingId {
        kind: kind.into(),
        key: Some(key.to_string()),
        ..Default::default()
    }
}

include!(concat!(env!("OUT_DIR"), "/api_identity_id.rs"));
include!(concat!(env!("OUT_DIR"), "/api_category_id.rs"));

/// True when Update should append a library instead of editing a row.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn is_add_library(id: &proto::SettingId) -> bool {
    id.kind.as_known() == Some(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_ADD_LIBRARY)
}
