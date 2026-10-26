//! Single, deduplicated identity enum for all settings.
//!
//! This replaces the two parallel `Kind` enums that previously existed in
//! `setting_row` and `setting_value`.  It is only used to route
//! [`SettingsEvent::UpdateValue`](crate::view::settings_editor::SettingsEvent)
//! events to the correct view — the view layer itself contains no per-setting
//! match arms.

include!(concat!(env!("OUT_DIR"), "/api_identity_enum.rs"));
