use i18n_embed::{
    fluent::{fluent_language_loader, FluentLanguageLoader},
    LanguageLoader,
};
use rust_embed::RustEmbed;
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

include!(concat!(env!("OUT_DIR"), "/locales.rs"));

pub const DEFAULT_LOCALE: &str = "en-GB";

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

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
