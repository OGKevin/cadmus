//! Settings editor view and setting definitions.
//!
//! [`SettingsEditor`] coordinates the Slint components and applies their
//! actions to [`Settings`](crate::settings::Settings). Setting behavior is
//! defined in [`kinds`].

pub mod kinds;

mod actions;
mod category;
mod components;
mod view;

pub use self::kinds::{
    InputSettingKind, SettingIdentity, SettingKind, SettingsEvent, ToggleSettings,
};
pub use self::view::SettingsEditor;
pub use crate::view::slint::generated::SettingsCategory;
