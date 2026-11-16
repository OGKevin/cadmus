//! Shared API proto, Rust enum, and mapping generation for the `api-proto` and
//! `cadmus-core` build scripts.
//!
//! [`api_enums.json`](api_enums.json) is the single definition of every API
//! enum that has a Rust side, validated by
//! [`api_enums.schema.json`](api_enums.schema.json). `Locale` is generated from
//! `docs/po/*.po` instead, because the translations on disk are its source.
//! Each entry lists the
//! Rust shape (derives, attributes, variant payloads, `cfg`) and the proto
//! number of every variant. The generators emit the Rust enums, the `.proto`
//! enums, and the mapping code from that one file, so a variant is written
//! once. The manifest is append-only: a new variant takes the next free
//! number, and the unit tests fail when a list is not strictly increasing, a
//! name repeats, a reserved number or name is reused, or the locale list falls
//! behind `docs/po`.

use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

/// One enum variant as the manifest and both generated sides describe it.
struct EnumVariant {
    rust: String,
    number: i32,
    doc: Option<String>,
    value_type: Option<String>,
    cfg: Option<String>,
    attrs: Vec<String>,
}

/// One enum from [`api_enums.json`](api_enums.json).
struct ProtoEnum {
    name: String,
    rust_name: String,
    doc: Option<String>,
    derives: Vec<String>,
    attrs: Vec<String>,
    all_doc: Option<String>,
    variants: Vec<EnumVariant>,
    reserved_numbers: Vec<i32>,
    reserved_names: Vec<String>,
}

/// The checked-in enum manifest both build scripts generate from.
struct ApiEnums {
    enums: Vec<ProtoEnum>,
}

impl ApiEnums {
    /// Enum called `name`.
    fn get(&self, name: &str) -> &ProtoEnum {
        self.enums
            .iter()
            .find(|entry| entry.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from api_enums.json"))
    }
}

/// Reads the manifest shipped with this crate.
fn api_enums() -> ApiEnums {
    let raw: Value =
        serde_json::from_str(include_str!("api_enums.json")).expect("api_enums.json is valid JSON");
    let enums = raw["enums"]
        .as_array()
        .expect("api_enums.json enums")
        .iter()
        .map(|entry| ProtoEnum {
            name: string_field(entry, "name"),
            rust_name: string_field(entry, "rust_name"),
            doc: optional_string(entry, "doc"),
            derives: string_list(entry, "derives"),
            attrs: string_list(entry, "attrs"),
            all_doc: optional_string(entry, "all_doc"),
            variants: entry["variants"]
                .as_array()
                .expect("enum variants")
                .iter()
                .map(|variant| EnumVariant {
                    rust: string_field(variant, "rust"),
                    number: i32::try_from(variant["number"].as_i64().expect("variant number"))
                        .expect("variant number fits i32"),
                    doc: optional_string(variant, "doc"),
                    value_type: optional_string(variant, "type"),
                    cfg: optional_string(variant, "cfg"),
                    attrs: string_list(variant, "attrs"),
                })
                .collect(),
            reserved_numbers: integer_list(entry, "reserved_numbers"),
            reserved_names: string_list(entry, "reserved_names"),
        })
        .collect();
    ApiEnums { enums }
}

fn string_field(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("api_enums.json field {key}"))
        .to_string()
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_string)
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().expect("string list item").to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn integer_list(value: &Value, key: &str) -> Vec<i32> {
    value[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    i32::try_from(item.as_i64().expect("integer list item"))
                        .expect("integer list item fits i32")
                })
                .collect()
        })
        .unwrap_or_default()
}

fn pascal_to_screaming_snake(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_uppercase() && !out.is_empty() {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Writes `doc` as rustdoc lines, one per source line.
fn write_doc(out: &mut String, doc: &Option<String>, indent: &str) {
    let Some(doc) = doc else {
        return;
    };
    for line in doc.lines() {
        if line.is_empty() {
            let _ = writeln!(out, "{indent}///");
        } else {
            let _ = writeln!(out, "{indent}/// {line}");
        }
    }
}

/// Proto constant suffix for `variant` of `enum_name`.
fn proto_variant_name(enum_name: &str, variant: &str) -> String {
    match enum_name {
        "Extension" | "Locale" => variant.to_ascii_uppercase().replace('-', "_"),
        _ => pascal_to_screaming_snake(variant).to_ascii_uppercase(),
    }
}

/// Proto prefix shared by every constant of `enum_name`.
fn proto_prefix(enum_name: &str) -> &'static str {
    match enum_name {
        "Extension" => "EXTENSION",
        "CategoryId" => "CATEGORY_ID",
        "TextMode" => "TEXT_MODE",
        "SettingIdentityKind" => "SETTING_IDENTITY_KIND",
        "Locale" => "LOCALE",
        other => panic!("unknown proto enum {other}"),
    }
}

/// Rust enum definition generated from the manifest.
pub fn enum_rust(enum_name: &str) -> String {
    let enums = api_enums();
    let item = enums.get(enum_name);
    let mut out = String::from("// Generated by build.rs from api_enums.json.\n");
    write_doc(&mut out, &item.doc, "");
    let _ = writeln!(out, "#[derive({})]", item.derives.join(", "));
    for attr in &item.attrs {
        let _ = writeln!(out, "#[{attr}]");
    }
    let _ = writeln!(out, "pub enum {} {{", item.rust_name);
    for variant in &item.variants {
        if let Some(cfg) = &variant.cfg {
            let _ = writeln!(out, "    #[cfg({cfg})]");
        }
        write_doc(&mut out, &variant.doc, "    ");
        for attr in &variant.attrs {
            let _ = writeln!(out, "    #[{attr}]");
        }
        match &variant.value_type {
            Some(value_type) => {
                let _ = writeln!(out, "    {}({value_type}),", variant.rust);
            }
            None => {
                let _ = writeln!(out, "    {},", variant.rust);
            }
        }
    }
    out.push_str("}\n");
    if let Some(all_doc) = &item.all_doc {
        let _ = writeln!(out, "\nimpl {} {{", item.rust_name);
        let _ = writeln!(out, "    /// {all_doc}");
        let _ = writeln!(out, "    pub fn all() -> &'static [{}] {{", item.rust_name);
        out.push_str("        &[\n");
        for variant in &item.variants {
            if let Some(cfg) = &variant.cfg {
                let _ = writeln!(out, "            #[cfg({cfg})]");
            }
            let _ = writeln!(out, "            {}::{},", item.rust_name, variant.rust);
        }
        out.push_str("        ]\n    }\n}\n");
    }
    out
}

fn proto_source(enum_name: &str) -> String {
    let enums = api_enums();
    proto_source_for(enums.get(enum_name))
}

fn proto_source_for(item: &ProtoEnum) -> String {
    let prefix = proto_prefix(&item.name);
    let mut body = format!(
        "syntax = \"proto3\";\n\npackage cadmus.api.shared.v1;\n\n// Generated from api_enums.json.\nenum {} {{\n  {prefix}_UNSPECIFIED = 0;\n",
        item.name
    );
    if !item.reserved_numbers.is_empty() {
        let numbers: Vec<String> = item.reserved_numbers.iter().map(i32::to_string).collect();
        let _ = writeln!(body, "  reserved {};", numbers.join(", "));
    }
    if !item.reserved_names.is_empty() {
        let names: Vec<String> = item
            .reserved_names
            .iter()
            .map(|name| format!("\"{prefix}_{}\"", proto_variant_name(&item.name, name)))
            .collect();
        let _ = writeln!(body, "  reserved {};", names.join(", "));
    }
    for variant in &item.variants {
        let name = proto_variant_name(&item.name, &variant.rust);
        let _ = writeln!(body, "  {prefix}_{name} = {};", variant.number);
    }
    body.push_str("}\n");
    body
}

/// `Extension` proto enum.
pub fn extension_proto_source() -> String {
    proto_source("Extension")
}

/// `CategoryId` proto enum.
pub fn category_id_proto_source() -> String {
    proto_source("CategoryId")
}

/// `TextMode` proto enum.
pub fn text_mode_proto_source() -> String {
    proto_source("TextMode")
}

/// `SettingIdentityKind` proto enum.
pub fn setting_identity_kind_proto_source() -> String {
    proto_source("SettingIdentityKind")
}

/// `Locale` proto enum from `docs/po/*.po` plus English.
///
/// Numbers follow the sorted code order. Locale is read by name (`GetStatus`
/// and the site path) and never stored as a number, so a new translation may
/// shift the codes that sort after it.
pub fn locale_proto_source(po_dir: &Path) -> String {
    let prefix = proto_prefix("Locale");
    let mut body = format!(
        "syntax = \"proto3\";\n\npackage cadmus.api.shared.v1;\n\n// Generated from docs/po/*.po plus en.\nenum Locale {{\n  {prefix}_UNSPECIFIED = 0;\n"
    );
    for (index, code) in locale_codes(po_dir).iter().enumerate() {
        let name = code.to_ascii_uppercase().replace('-', "_");
        let _ = writeln!(body, "  {prefix}_{name} = {};", index + 1);
    }
    body.push_str("}\n");
    body
}

/// `Extension` ↔ [`FileExtension`](enum_rust) conversions.
pub fn extension_impls_rust() -> String {
    let enums = api_enums();
    let item = enums.get("Extension");
    let mut out = String::from(
        "// Generated by build.rs from api_enums.json.\n\
         impl TryFrom<Extension> for FileExtension {\n\
             type Error = UnknownFileExtension;\n\n\
             fn try_from(value: Extension) -> Result<Self, Self::Error> {\n\
                 Ok(match value {\n\
                     Extension::EXTENSION_UNSPECIFIED => {\n\
                         return Err(UnknownFileExtension(\"unspecified\".into()));\n\
                     }\n",
    );
    for variant in &item.variants {
        let name = proto_variant_name("Extension", &variant.rust);
        let _ = writeln!(
            out,
            "                     Extension::EXTENSION_{name} => FileExtension::{},",
            variant.rust
        );
    }
    out.push_str(
        "                 })\n\
             }\n\
         }\n\n\
         impl From<FileExtension> for Extension {\n\
             fn from(value: FileExtension) -> Self {\n\
                 match value {\n",
    );
    for variant in &item.variants {
        let name = proto_variant_name("Extension", &variant.rust);
        let _ = writeln!(
            out,
            "                     FileExtension::{} => Extension::EXTENSION_{name},",
            variant.rust
        );
    }
    out.push_str(
        "                 }\n\
             }\n\
         }\n",
    );
    out
}

/// `Category` → `CategoryId`.
pub fn category_id_impl_rust() -> String {
    let enums = api_enums();
    let item = enums.get("CategoryId");
    let mut out = String::from(
        "// Generated by build.rs from api_enums.json.\n\
         #[cfg_attr(feature = \"tracing\", tracing::instrument(skip_all, fields(category = ?category)))]\n\
         pub(super) fn category_id(category: Category) -> proto::CategoryId {\n\
             match category {\n",
    );
    for variant in &item.variants {
        let name = proto_variant_name("CategoryId", &variant.rust);
        let _ = writeln!(
            out,
            "                 Category::{} => proto::CategoryId::CATEGORY_ID_{name},",
            variant.rust
        );
    }
    out.push_str("             }\n         }\n");
    out
}

/// `TextMode` ↔ `proto::TextMode`.
pub fn text_mode_impl_rust() -> String {
    let enums = api_enums();
    let item = enums.get("TextMode");
    let mut out = String::from(
        "// Generated by build.rs from api_enums.json.\n\
         pub(super) fn proto_text_mode(mode: TextMode) -> proto::TextMode {\n\
             match mode {\n",
    );
    for variant in &item.variants {
        let name = proto_variant_name("TextMode", &variant.rust);
        let _ = writeln!(
            out,
            "                 TextMode::{} => proto::TextMode::TEXT_MODE_{name},",
            variant.rust
        );
    }
    out.push_str(
        "             }\n\
         }\n\n\
         pub(super) fn text_mode_from_proto(mode: proto::TextMode) -> TextMode {\n\
             match mode {\n\
                 proto::TextMode::TEXT_MODE_UNSPECIFIED => TextMode::Text,\n",
    );
    for variant in &item.variants {
        let name = proto_variant_name("TextMode", &variant.rust);
        let _ = writeln!(
            out,
            "                 proto::TextMode::TEXT_MODE_{name} => TextMode::{},",
            variant.rust
        );
    }
    out.push_str(
        "             }\n\
         }\n",
    );
    out
}

/// `SettingIdentity` → `SettingId`.
pub fn identity_id_impl_rust() -> String {
    let enums = api_enums();
    let item = enums.get("SettingIdentityKind");
    let mut out = String::from(
        "// Generated by build.rs from api_enums.json.\n\
         #[cfg_attr(feature = \"tracing\", tracing::instrument(skip_all))]\n\
         pub(super) fn identity_id(id: &SettingIdentity) -> proto::SettingId {\n\
             match id {\n",
    );
    for variant in &item.variants {
        let name = proto_variant_name("SettingIdentityKind", &variant.rust);
        let proto_kind = format!("proto::SettingIdentityKind::SETTING_IDENTITY_KIND_{name}");
        let arm = match variant.value_type.as_deref() {
            None => format!("SettingIdentity::{} => unit({proto_kind}),", variant.rust),
            Some("usize") => format!(
                "SettingIdentity::{}(index) => indexed({proto_kind}, *index),",
                variant.rust
            ),
            Some("String") => format!(
                "SettingIdentity::{}(key) => keyed({proto_kind}, key),",
                variant.rust
            ),
            Some(other) => panic!("SettingIdentity payload {other} has no proto mapping"),
        };
        if let Some(cfg) = &variant.cfg {
            let _ = writeln!(out, "                 #[cfg({cfg})]");
        }
        let _ = writeln!(out, "                 {arm}");
    }
    out.push_str("             }\n         }\n");
    out
}

/// `Locale` ↔ exact site-directory mapping from `docs/po/*.po` plus English.
///
/// Site directories are the `.po` stems, so both directions map exactly
/// without guessing the case of region or script subtags.
pub fn locale_impls_rust(po_dir: &Path) -> String {
    let prefix = proto_prefix("Locale");
    let codes = locale_codes(po_dir);
    let mut out = String::from(
        "// Generated by build.rs from docs/po/*.po plus en.\n\
         impl Locale {\n\
             /// Exact site directory for this locale, or `None` when the site has none.\n\
             pub fn site_dir(self) -> Option<&'static str> {\n\
                 match self.proto_name() {\n",
    );
    for code in &codes {
        let name = code.to_ascii_uppercase().replace('-', "_");
        let _ = writeln!(
            out,
            "                    \"{prefix}_{name}\" => Some(\"{code}\"),"
        );
    }
    out.push_str(
        "                    _ => None,\n\
                 }\n\
             }\n\
         }\n\n\
         /// Locale whose exact site directory is `dir`, or `None`.\n\
         pub fn locale_from_site_dir(dir: &str) -> Option<Locale> {\n\
             match dir {\n",
    );
    for code in &codes {
        let name = code.to_ascii_uppercase().replace('-', "_");
        let _ = writeln!(
            out,
            "                \"{code}\" => Some(Locale::{prefix}_{name}),"
        );
    }
    out.push_str(
        "                _ => None,\n\
             }\n\
         }\n",
    );
    out
}

/// Locale codes from `docs/po/*.po`, always including English, sorted.
fn locale_codes(po_dir: &Path) -> Vec<String> {
    let mut codes = vec!["en".to_string()];
    if po_dir.is_dir() {
        for entry in std::fs::read_dir(po_dir).expect("docs/po") {
            let path = entry.expect("po entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("po") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !codes.iter().any(|code| code == stem) {
                codes.push(stem.to_string());
            }
        }
    }
    codes.sort();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_append_only(item: &ProtoEnum) {
        let first = item.variants.first().expect("enum without variants");
        assert!(
            first.number >= 1,
            "0 is reserved for the UNSPECIFIED value: {}",
            first.rust,
        );
        for pair in item.variants.windows(2) {
            assert!(
                pair[1].number > pair[0].number,
                "{} numbers must strictly increase: {} before {}",
                item.name,
                pair[0].rust,
                pair[1].rust,
            );
        }
        let mut names: Vec<&str> = item.variants.iter().map(|v| v.rust.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            item.variants.len(),
            "{} repeats a variant name",
            item.name,
        );
        for number in &item.reserved_numbers {
            assert!(
                !item
                    .variants
                    .iter()
                    .any(|variant| variant.number == *number),
                "{} reuses reserved number {number}",
                item.name,
            );
        }
        for name in &item.reserved_names {
            assert!(
                !item.variants.iter().any(|variant| variant.rust == *name),
                "{} reuses reserved name {name}",
                item.name,
            );
        }
    }

    #[test]
    fn manifest_matches_its_schema() {
        let manifest: Value = serde_json::from_str(include_str!("api_enums.json"))
            .expect("api_enums.json is valid JSON");
        let schema: Value = serde_json::from_str(include_str!("api_enums.schema.json"))
            .expect("api_enums.schema.json is valid JSON");
        jsonschema::validate(&schema, &manifest)
            .unwrap_or_else(|error| panic!("api_enums.json does not match its schema: {error}"));
    }

    #[test]
    fn manifest_covers_every_generated_enum() {
        let enums = api_enums();
        let names: Vec<&str> = enums.enums.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["Extension", "CategoryId", "TextMode", "SettingIdentityKind"],
        );
        let rust_names: Vec<&str> = enums.enums.iter().map(|e| e.rust_name.as_str()).collect();
        assert_eq!(
            rust_names,
            ["FileExtension", "Category", "TextMode", "SettingIdentity"],
        );
        for item in &enums.enums {
            assert_append_only(item);
        }
    }

    #[test]
    fn locale_generation_follows_po_files() {
        let po_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/po");
        let proto = locale_proto_source(&po_dir);
        for (index, code) in locale_codes(&po_dir).iter().enumerate() {
            let name = code.to_ascii_uppercase().replace('-', "_");
            assert!(
                proto.contains(&format!("LOCALE_{name} = {};", index + 1)),
                "LOCALE_{name} missing from the generated Locale enum",
            );
        }
        assert!(proto.contains("LOCALE_UNSPECIFIED = 0;"));
    }

    #[test]
    fn generated_rust_declares_every_variant() {
        let enums = api_enums();
        for item in &enums.enums {
            let source = enum_rust(&item.name);
            assert!(
                source.contains(&format!("pub enum {} {{", item.rust_name)),
                "{} missing from generated Rust",
                item.rust_name,
            );
            for variant in &item.variants {
                let declaration = match &variant.value_type {
                    Some(value_type) => format!("    {}({value_type}),", variant.rust),
                    None => format!("    {},", variant.rust),
                };
                assert!(
                    source.contains(&declaration),
                    "{}::{}{} missing from generated Rust",
                    item.rust_name,
                    variant.rust,
                    variant
                        .value_type
                        .as_ref()
                        .map(|t| format!("({t})"))
                        .unwrap_or_default(),
                );
            }
        }
    }

    #[test]
    fn generated_proto_pins_each_number() {
        let enums = api_enums();
        for item in &enums.enums {
            let proto = proto_source(&item.name);
            let prefix = proto_prefix(&item.name);
            for variant in &item.variants {
                let name = proto_variant_name(&item.name, &variant.rust);
                assert!(
                    proto.contains(&format!("{prefix}_{name} = {};", variant.number)),
                    "{}_{name} missing from generated proto",
                    prefix,
                );
            }
        }
    }

    fn synthetic_enum(reserved_numbers: Vec<i32>, reserved_names: Vec<String>) -> ProtoEnum {
        ProtoEnum {
            name: "Extension".to_string(),
            rust_name: "FileExtension".to_string(),
            doc: None,
            derives: Vec::new(),
            attrs: Vec::new(),
            all_doc: None,
            variants: vec![EnumVariant {
                rust: "Pdf".to_string(),
                number: 11,
                doc: None,
                value_type: None,
                cfg: None,
                attrs: Vec::new(),
            }],
            reserved_numbers,
            reserved_names,
        }
    }

    #[test]
    fn proto_source_emits_reserved_statements() {
        let item = synthetic_enum(vec![4, 7], vec!["Remove".to_string()]);
        let proto = proto_source_for(&item);
        assert!(proto.contains("reserved 4, 7;"), "{proto}");
        assert!(proto.contains("reserved \"EXTENSION_REMOVE\";"), "{proto}");
    }

    #[test]
    #[should_panic(expected = "reuses reserved number")]
    fn append_only_rejects_a_reserved_number() {
        assert_append_only(&synthetic_enum(vec![11], Vec::new()));
    }

    #[test]
    #[should_panic(expected = "reuses reserved name")]
    fn append_only_rejects_a_reserved_name() {
        assert_append_only(&synthetic_enum(Vec::new(), vec!["Pdf".to_string()]));
    }

    #[test]
    fn locale_mapping_round_trips_every_po_locale() {
        let po_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/po");
        let source = locale_impls_rust(&po_dir);
        for code in locale_codes(&po_dir) {
            let name = code.to_ascii_uppercase().replace('-', "_");
            assert!(
                source.contains(&format!("\"LOCALE_{name}\" => Some(\"{code}\")")),
                "LOCALE_{name} must map to {code}",
            );
            assert!(
                source.contains(&format!("\"{code}\" => Some(Locale::LOCALE_{name})")),
                "{code} must map back to LOCALE_{name}",
            );
        }
    }
}
