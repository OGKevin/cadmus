//! Generated ConnectRPC types for the Cadmus shared APIs.
//!
//! Built from every `proto/cadmus/api/**/*.proto`. The internal package in
//! `proto/cadmus/api/internal/v1/` is reserved for internal-only RPCs and holds
//! a placeholder until a real one exists.

connectrpc::include_generated!();

/// OpenAPI document generated at build time from the proto annotations.
///
/// `info.version` is stamped by `build.rs` with the same git-describe string
/// `cadmus-core` reports from `GetStatus`.
pub const OPENAPI_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/openapi.json"));

use buffa::Enumeration;
use cadmus::api::shared::v1::Locale;

/// Site directory for a [`Locale`] (`en`, `fr`, `pt-BR`).
///
/// The mapping is generated at build time from the `docs/po/*.po` stems, the
/// same names the site directories use, so a region or script subtag keeps its
/// exact case. `LOCALE_UNSPECIFIED` maps to `en`.
pub fn locale_dir(locale: Locale) -> String {
    locale.site_dir().unwrap_or("en").to_string()
}

/// [`Locale`] for a site directory or BCP-47 language tag, or `None`.
///
/// `fr` becomes `LOCALE_FR` and `pt-BR` becomes `LOCALE_PT_BR`. Unknown names,
/// including device `en-GB` when the site only has `en`, return `None`.
pub fn locale_from_dir(dir: &str) -> Option<Locale> {
    locale_from_site_dir(dir)
}

include!(concat!(env!("OUT_DIR"), "/locale_impls.rs"));

#[cfg(test)]
mod tests {
    use super::{Locale, locale_dir, locale_from_dir};

    #[test]
    fn locale_dir_keeps_the_region_case() {
        assert_eq!(locale_dir(Locale::LOCALE_EN), "en");
        assert_eq!(locale_dir(Locale::LOCALE_FR), "fr");
        assert_eq!(locale_dir(Locale::LOCALE_UNSPECIFIED), "en");
    }

    #[test]
    fn locale_round_trips_exact_site_directories() {
        for locale in [Locale::LOCALE_EN, Locale::LOCALE_FR] {
            let dir = locale_dir(locale);
            assert_eq!(locale_from_dir(&dir), Some(locale), "{dir}");
        }
        assert_eq!(locale_from_dir("en-GB"), None);
        assert_eq!(locale_from_dir("pt-BR"), None);
    }
}
