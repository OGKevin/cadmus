use i18n_embed::{
    LanguageLoader,
    fluent::{FluentLanguageLoader, fluent_language_loader},
};
use rust_embed::RustEmbed;
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

pub const DEFAULT_LOCALE: &str = "en-GB";

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// Trait for types that can provide localized string representations
pub trait I18nDisplay {
    /// Returns a localized string representation
    fn to_i18n_string(&self) -> String;
}

/// Returns the global [`FluentLanguageLoader`], initialising it on first call.
///
/// The fallback language ([DEFAULT_LOCALE]) is loaded automatically, so the loader is
/// always usable even before [`init`] is called.
pub fn language_loader() -> &'static FluentLanguageLoader {
    static LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();

    LOADER.get_or_init(|| {
        let loader = fluent_language_loader!();
        loader
            .load_fallback_language(&Localizations)
            .expect("fallback language (en-GB) FTL assets must be present at compile time");
        loader
    })
}

/// Selects the active UI language from the [`LanguageIdentifier`] stored in [`Settings`].
///
/// Call once at startup, passing `settings.locale.as_ref()`. Passing `None`
/// keeps the English fallback active.
///
/// [`Settings`]: crate::settings::Settings
#[cfg_attr(feature = "tracing", tracing::instrument(skip(locale), level = tracing::Level::TRACE))]
pub fn init(locale: Option<&LanguageIdentifier>) {
    let requested: Vec<LanguageIdentifier> = locale.cloned().into_iter().collect();

    i18n_embed::select(language_loader(), &Localizations, &requested)
        .expect("failed to select i18n language");
}

/// Looks up a Fluent message by ID using the active language loader.
///
/// # Usage
///
/// ```ignore
/// // This example uses the crate-internal fl! macro.
/// let label = crate::fl!("startup-loading");
/// ```
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::language_loader(), $message_id)
    }};
    ($message_id:literal, $($key:ident = $value:expr),* $(,)?) => {{
        i18n_embed_fl::fl!($crate::i18n::language_loader(), $message_id, $($key = $value),*)
    }};
}

/// Looks up a Fluent message by a runtime ID, returning `fallback` when the ID has
/// no translation in the active language or in [`DEFAULT_LOCALE`].
///
/// Unlike [`fl!`], the message ID is not checked at compile time. Fluent
/// placeholders use the same `name = value` syntax as [`fl!`]:
///
/// The two-argument form calls [`FluentLanguageLoader::get`] and does not supply
/// Fluent variables. Messages with `{ $var }` placeholders need the `name = value`
/// arm (same as [`fl!`]).
///
/// ```
/// # use cadmus_core::fl_or;
/// # let dynamic_id = "startup-loading";
/// fl_or!(dynamic_id, "Loading…");
/// fl_or!(dynamic_id, "Features: …", features = "kobo");
/// ```
#[macro_export]
macro_rules! fl_or {
    ($message_id:expr, $fallback:expr $(,)?) => {{
        let loader = $crate::i18n::language_loader();
        let message_id: &str = &$message_id;
        if loader.has(message_id) {
            loader.get(message_id)
        } else {
            ::tracing::warn!(message_id, "missing translation, using fallback");
            $fallback.to_string()
        }
    }};
    ($message_id:expr, $fallback:expr, $($key:ident = $value:expr),* $(,)?) => {{
        let loader = $crate::i18n::language_loader();
        let message_id: &str = &$message_id;
        if loader.has(message_id) {
            let args = {
                let mut map = ::std::collections::HashMap::new();
                $( map.insert(stringify!($key), ::std::convert::Into::into($value)); )*
                map
            };
            loader.get_args_concrete(message_id, args)
        } else {
            ::tracing::warn!(message_id, "missing translation, using fallback");
            $fallback.to_string()
        }
    }};
}

#[cfg(test)]
mod tests {
    use crate::i18n::language_loader;

    #[test]
    fn fl_or_known_id_ignores_fallback() {
        assert_eq!(fl_or!("startup-loading", "unused"), fl!("startup-loading"));
    }

    #[test]
    fn fl_or_unknown_id_returns_fallback() {
        assert_eq!(fl_or!("definitely-not-a-message", "English"), "English");
    }

    #[test]
    fn fl_or_accepts_runtime_string_id() {
        let id = String::from("startup-loading");
        assert_eq!(fl_or!(id, "unused"), fl!("startup-loading"));
        let id = "startup-loading";
        assert_eq!(fl_or!(id, "unused"), fl!("startup-loading"));
    }

    #[test]
    fn fl_or_with_fluent_args_matches_fl() {
        assert_eq!(
            fl_or!("build-features", "unused", features = "kobo"),
            fl!("build-features", features = "kobo"),
        );
    }

    #[test]
    fn fl_or_unknown_id_with_fluent_args_returns_fallback() {
        assert_eq!(
            fl_or!("definitely-not-a-message", "English", features = "kobo"),
            "English",
        );
    }

    #[test]
    fn fl_or_accepts_mixed_fluent_arg_types() {
        assert_eq!(
            fl_or!(
                "notification-downloading-dictionary-progress",
                "unused",
                lang = "fr",
                downloaded = 3usize,
                total = 10u32,
            ),
            fl!(
                "notification-downloading-dictionary-progress",
                lang = "fr",
                downloaded = 3usize,
                total = 10u32,
            ),
        );
    }

    #[test]
    fn fl_or_two_arg_form_does_not_pass_fluent_variables() {
        assert_eq!(
            fl_or!("build-features", "unused"),
            language_loader().get("build-features"),
        );
        assert_ne!(
            fl_or!("build-features", "unused"),
            fl!("build-features", features = "kobo"),
        );
    }
}
