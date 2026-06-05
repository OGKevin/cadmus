//! Trait and supporting types for defining individual settings.
//!
//! Each setting is a small struct that implements [`SettingKind`]. The trait
//! encapsulates everything a [`SettingRow`](crate::view::settings_editor::SettingRow)
//! needs to know: the row label, the current display value, which widget to
//! render (`ActionLabel`, `Toggle`, or `SubMenu`), and which event to fire on tap.
//!
//! [`SettingIdentity`] is the single, deduplicated identity enum used by
//! [`SettingsEvent::UpdateValue`](crate::view::settings_editor::SettingsEvent) to
//! target the correct [`SettingValue`](crate::view::settings_editor::SettingValue) view.

pub mod dictionary;
pub mod general;
pub mod identity;
pub mod import;
pub mod intermission;
pub mod library;
pub mod reader;
pub mod telemetry;

pub use identity::SettingIdentity;

use crate::geom::Rectangle;
use crate::settings::Settings;
use crate::view::{Bus, EntryId, EntryKind, Event, ViewId};

/// Identifies which boolean setting a toggle widget controls.
///
/// Used in [`ToggleEvent::Setting`](crate::view::ToggleEvent) so that
/// [`CategoryEditor`](crate::view::settings_editor::CategoryEditor) can dispatch
/// to the correct toggle handler without coupling to UI view IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToggleSettings {
    /// Sleep cover enable/disable setting
    SleepCover,
    /// Auto-share enable/disable setting
    AutoShare,
    /// Button scheme selection (natural or inverted)
    ButtonScheme,
    /// Logging enabled setting
    LoggingEnabled,
    /// Sync metadata enable/disable setting
    ImportSyncMetadata,
    /// Kernel logging enabled setting (test + kobo builds only)
    #[cfg(all(feature = "test", feature = "kobo"))]
    EnableKernLog,
    /// D-Bus logging enabled setting (test + kobo builds only)
    #[cfg(all(feature = "test", feature = "kobo"))]
    EnableDbusLog,
}

/// Describes how the value side of a setting row should be rendered.
///
/// Each variant is fully self-contained: it carries everything needed to build
/// the widget, including the tap event or sub-menu entries.
#[derive(Debug)]
pub enum WidgetKind {
    /// No interactive widget; the value is shown as static text only.
    None,
    /// A tappable label that opens a free-form editor (e.g. a text input dialog).
    ///
    /// The inner event is fired when the label is tapped.
    ActionLabel(Event),
    /// A two-state toggle switch.
    Toggle {
        /// Label shown on the left (the "on" side).
        left_label: String,
        /// Label shown on the right (the "off" side).
        right_label: String,
        /// Whether the toggle is currently in the left/enabled state.
        enabled: bool,
        /// Event fired when the toggle is tapped.
        tap_event: Event,
    },
    /// A tappable label that opens a sub-menu with the given entries.
    ///
    /// The entries (e.g. radio buttons) are stored here so that the widget is
    /// fully self-contained.
    SubMenu(Vec<EntryKind>),
}

/// All data needed to build and update the value side of a setting row.
pub struct SettingData {
    /// Text representation of the current value (shown in the widget).
    pub value: String,
    /// Which widget type to render, including all tap/event data for that widget.
    pub widget: WidgetKind,
}

/// A self-contained description of a single setting.
///
/// Implementing this trait is sufficient to add a new setting to the editor.
pub trait SettingKind {
    /// Unique identity used to route [`SettingsEvent::UpdateValue`](crate::view::settings_editor::SettingsEvent::UpdateValue) to the
    /// correct [`SettingValue`](crate::view::settings_editor::SettingValue) view.
    fn identity(&self) -> SettingIdentity;

    /// Human-readable label shown on the left side of the setting row.
    ///
    /// `settings` is provided for dynamic labels (e.g. library names).
    fn label(&self, settings: &Settings) -> String;

    /// Fetch the current display value and widget configuration from `settings`.
    fn fetch(&self, settings: &Settings) -> SettingData;

    /// Handle an incoming event that may apply a change to this setting.
    ///
    /// Mutates `settings` if the event is relevant and returns:
    /// - `Some(display_string)` as the first element when the event changes this
    ///   setting's display value, or `None` if the event does not apply.
    /// - `true` as the second element when the event has been fully consumed and
    ///   should stop propagating; `false` to allow further handlers to see it.
    ///
    /// `bus` is available for settings that need to propagate side-effects.
    fn handle(
        &self,
        _evt: &Event,
        _settings: &mut Settings,
        _bus: &mut Bus,
    ) -> (Option<String>, bool) {
        (None, false)
    }

    /// Returns this setting as an [`InputSettingKind`] if it supports text input.
    ///
    /// [`InputSettingKind`] implementors override this to return `Some(self)`.
    /// All other settings inherit the default `None`.
    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        None
    }

    /// Returns the [`EntryId`] that triggers opening a file chooser for this setting.
    ///
    /// Default `None`. Implement on settings that offer a "Custom Image..." option
    /// (currently the three intermission kinds).
    fn file_chooser_entry_id(&self) -> Option<EntryId> {
        None
    }

    /// The event that should be emitted if the settings is held.
    fn hold_event(&self, _rect: Rectangle) -> Option<Event> {
        None
    }

    /// Whether a submenu should remain open after this setting handles a selection.
    fn keep_menu_open(&self) -> bool {
        false
    }
}

impl<T: SettingKind + ?Sized> SettingKind for &T {
    fn identity(&self) -> SettingIdentity {
        (**self).identity()
    }

    fn label(&self, settings: &Settings) -> String {
        (**self).label(settings)
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        (**self).fetch(settings)
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        bus: &mut Bus,
    ) -> (Option<String>, bool) {
        (**self).handle(evt, settings, bus)
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        (**self).as_input_kind()
    }

    fn file_chooser_entry_id(&self) -> Option<EntryId> {
        (**self).file_chooser_entry_id()
    }

    fn hold_event(&self, rect: Rectangle) -> Option<Event> {
        (**self).hold_event(rect)
    }

    fn keep_menu_open(&self) -> bool {
        (**self).keep_menu_open()
    }
}

impl<T: SettingKind + ?Sized> SettingKind for Box<T> {
    fn identity(&self) -> SettingIdentity {
        (**self).identity()
    }

    fn label(&self, settings: &Settings) -> String {
        (**self).label(settings)
    }

    fn fetch(&self, settings: &Settings) -> SettingData {
        (**self).fetch(settings)
    }

    fn handle(
        &self,
        evt: &Event,
        settings: &mut Settings,
        bus: &mut Bus,
    ) -> (Option<String>, bool) {
        (**self).handle(evt, settings, bus)
    }

    fn as_input_kind(&self) -> Option<&dyn InputSettingKind> {
        (**self).as_input_kind()
    }

    fn file_chooser_entry_id(&self) -> Option<EntryId> {
        (**self).file_chooser_entry_id()
    }

    fn hold_event(&self, rect: Rectangle) -> Option<Event> {
        (**self).hold_event(rect)
    }

    fn keep_menu_open(&self) -> bool {
        (**self).keep_menu_open()
    }
}

/// Extended trait for settings that accept free-form text input via a [`NamedInput`] overlay.
///
/// [`NamedInput`]: crate::view::named_input::NamedInput
pub trait InputSettingKind: SettingKind {
    /// The [`ViewId`] used by this setting's [`NamedInput`] and its submit event.
    ///
    /// [`NamedInput`]: crate::view::named_input::NamedInput
    fn submit_view_id(&self) -> ViewId;

    /// The [`EntryId`] event that opens this setting's input dialog when tapped.
    fn open_entry_id(&self) -> EntryId;

    /// Label shown inside the [`NamedInput`] dialog.
    ///
    /// [`NamedInput`]: crate::view::named_input::NamedInput
    fn input_label(&self) -> String;

    /// Maximum number of characters the input accepts.
    fn input_max_chars(&self) -> usize;

    /// The current value as a string to pre-populate the input field.
    fn current_text(&self, settings: &Settings) -> String;

    /// Parse `text` from the input, mutate `settings`, and return the display string.
    fn apply_text(&self, text: &str, settings: &mut Settings) -> String;
}
