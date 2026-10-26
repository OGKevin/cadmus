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
/// Proto names are `LOCALE_<CODE>` with hyphens turned into underscores.
/// This undoes that so `GetStatus.locale` and `site/<dir>/` stay the same
/// codes as `docs/po/*.po`. `LOCALE_UNSPECIFIED` maps to `en`.
pub fn locale_dir(locale: Locale) -> String {
    locale
        .proto_name()
        .strip_prefix("LOCALE_")
        .filter(|name| *name != "UNSPECIFIED")
        .unwrap_or("EN")
        .to_ascii_lowercase()
        .replace('_', "-")
}

/// [`Locale`] for a site directory or BCP-47 language subtag.
///
/// `fr` becomes `LOCALE_FR`. Unknown names, including device `en-GB` when
/// the site only has `en`, become `LOCALE_EN`.
pub fn locale_from_dir(dir: &str) -> Locale {
    let name = format!("LOCALE_{}", dir.to_ascii_uppercase().replace('-', "_"));
    Locale::from_proto_name(&name).unwrap_or(Locale::LOCALE_EN)
}
