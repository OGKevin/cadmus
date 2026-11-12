//! Shared API proto and mapping generation for the `api-proto` and `cadmus-core`
//! build scripts.

use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct EnumVariant {
    pub name: String,
    pub shape: VariantShape,
    pub cfg: Option<String>,
}

#[derive(Clone, Debug)]
pub enum VariantShape {
    Unit,
    Index,
    Key,
}

pub fn pascal_to_screaming_snake(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_uppercase() && !out.is_empty() {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

pub fn rust_file_extensions(source: &str) -> Vec<String> {
    let Some(fn_body) = source.split("fn all()").nth(1) else {
        return Vec::new();
    };
    let Some(after_brace) = fn_body.split_once('{').map(|(_, rest)| rest) else {
        return Vec::new();
    };
    let Some(body_start) = after_brace.find('[') else {
        return Vec::new();
    };
    let rest = &after_brace[body_start..];
    let Some(body_end) = rest.find(']') else {
        return Vec::new();
    };
    rest[..body_end]
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("FileExtension::")
                .map(|name| name.trim_end_matches(',').to_string())
        })
        .filter(|name| !name.is_empty())
        .collect()
}

pub fn parse_rust_enum(source: &str, enum_name: &str) -> Vec<EnumVariant> {
    let needle = format!("pub enum {enum_name} {{");
    let Some(start) = source.find(&needle) else {
        return Vec::new();
    };
    let body = &source[start + needle.len()..];
    let Some(end) = body.find("\n}") else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    let mut pending_cfg: Option<String> = None;
    for line in body[..end].lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg") {
            pending_cfg = Some(trimmed.to_string());
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("///") || trimmed.starts_with("//") {
            continue;
        }
        let Some((name, rest)) = trimmed.split_once('(') else {
            let name = trimmed.trim_end_matches(',').trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                variants.push(EnumVariant {
                    name: name.to_string(),
                    shape: VariantShape::Unit,
                    cfg: pending_cfg.take(),
                });
            }
            continue;
        };
        let shape = if rest.starts_with("usize") {
            VariantShape::Index
        } else if rest.starts_with("String") {
            VariantShape::Key
        } else {
            continue;
        };
        variants.push(EnumVariant {
            name: name.to_string(),
            shape,
            cfg: pending_cfg.take(),
        });
    }
    variants
}

pub fn extension_proto_source(file_extension_rs: &str) -> String {
    let names = rust_file_extensions(file_extension_rs);
    assert!(
        !names.is_empty(),
        "FileExtension::all() variants missing in file_extension.rs"
    );
    let mut body = String::from(
        "syntax = \"proto3\";\n\n\
         package cadmus.api.shared.v1;\n\n\
         // Generated from FileExtension::all() in crates/core/src/document/file_extension.rs.\n\
         enum Extension {\n  EXTENSION_UNSPECIFIED = 0;\n",
    );
    for (index, name) in names.iter().enumerate() {
        let _ = writeln!(
            body,
            "  EXTENSION_{} = {};",
            name.to_ascii_uppercase(),
            index + 1
        );
    }
    body.push_str("}\n");
    body
}

pub fn category_id_proto_source(category_rs: &str) -> String {
    let variants = parse_rust_enum(category_rs, "Category");
    let mut body = String::from(
        "syntax = \"proto3\";\n\n\
         package cadmus.api.shared.v1;\n\n\
         // Generated from Category in crates/core/src/view/settings_editor/category.rs.\n\
         enum CategoryId {\n  CATEGORY_ID_UNSPECIFIED = 0;\n",
    );
    for (index, variant) in variants.iter().enumerate() {
        let snake = pascal_to_screaming_snake(&variant.name);
        let _ = writeln!(
            body,
            "  CATEGORY_ID_{} = {};",
            snake.to_ascii_uppercase(),
            index + 1
        );
    }
    body.push_str("}\n");
    body
}

pub fn text_mode_proto_source(mod_rs: &str) -> String {
    let variants = parse_rust_enum(mod_rs, "TextMode");
    let mut body = String::from(
        "syntax = \"proto3\";\n\n\
         package cadmus.api.shared.v1;\n\n\
         // Generated from TextMode in crates/core/src/view/settings_editor/kinds/mod.rs.\n\
         enum TextMode {\n",
    );
    for (index, variant) in variants.iter().enumerate() {
        let snake = pascal_to_screaming_snake(&variant.name);
        let _ = writeln!(
            body,
            "  TEXT_MODE_{} = {};",
            snake.to_ascii_uppercase(),
            index
        );
    }
    body.push_str("}\n");
    body
}

pub fn setting_identity_kind_proto_source(identity_rs: &str) -> String {
    let variants = parse_rust_enum(identity_rs, "SettingIdentity");
    let mut body = String::from(
        "syntax = \"proto3\";\n\n\
         package cadmus.api.shared.v1;\n\n\
         // Generated from SettingIdentity in crates/core/src/view/settings_editor/kinds/identity.rs.\n\
         enum SettingIdentityKind {\n  SETTING_IDENTITY_KIND_UNSPECIFIED = 0;\n",
    );
    for (index, variant) in variants.iter().enumerate() {
        let snake = pascal_to_screaming_snake(&variant.name);
        let _ = writeln!(
            body,
            "  SETTING_IDENTITY_KIND_{} = {};",
            snake.to_ascii_uppercase(),
            index + 1
        );
    }
    let next = variants.len() + 1;
    let _ = writeln!(
        body,
        "  // Not a SettingIdentity. Update with a LibraryValue appends a library row.\n  SETTING_IDENTITY_KIND_ADD_LIBRARY = {next};"
    );
    body.push_str("}\n");
    body
}

pub fn extension_impls_rust(file_extension_rs: &str) -> String {
    let names = rust_file_extensions(file_extension_rs);
    let mut out = String::from(
        "// Generated by build.rs from FileExtension::all().\n\
         impl TryFrom<Extension> for FileExtension {\n\
             type Error = UnknownFileExtension;\n\n\
             fn try_from(value: Extension) -> Result<Self, Self::Error> {\n\
                 Ok(match value {\n\
                     Extension::EXTENSION_UNSPECIFIED => {\n\
                         return Err(UnknownFileExtension(\"unspecified\".into()));\n\
                     }\n",
    );
    for name in &names {
        let ext = name.to_ascii_uppercase();
        let variant = format!("FileExtension::{name}");
        let _ = writeln!(
            out,
            "                     Extension::EXTENSION_{ext} => {variant},"
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
    for name in &names {
        let ext = name.to_ascii_uppercase();
        let variant = format!("FileExtension::{name}");
        let _ = writeln!(
            out,
            "                     {variant} => Extension::EXTENSION_{ext},"
        );
    }
    out.push_str(
        "                 }\n\
             }\n\
         }\n",
    );
    out
}

pub fn category_id_impl_rust(category_rs: &str) -> String {
    let variants = parse_rust_enum(category_rs, "Category");
    let mut out = String::from(
        "// Generated by build.rs from Category.\n\
         #[cfg_attr(feature = \"tracing\", tracing::instrument(skip_all, fields(category = ?category)))]\n\
         pub(super) fn category_id(category: Category) -> proto::CategoryId {\n\
             match category {\n",
    );
    for variant in &variants {
        let snake = pascal_to_screaming_snake(&variant.name);
        let _ = writeln!(
            out,
            "                 Category::{name} => proto::CategoryId::CATEGORY_ID_{snake},",
            name = variant.name,
            snake = snake.to_ascii_uppercase()
        );
    }
    out.push_str("             }\n         }\n");
    out
}

pub fn text_mode_impl_rust(mod_rs: &str) -> String {
    let variants = parse_rust_enum(mod_rs, "TextMode");
    let mut out = String::from(
        "// Generated by build.rs from TextMode.\n\
         #[derive(Debug)]\n\
         pub(super) struct UnknownTextMode;\n\n\
         pub(super) fn proto_text_mode(mode: TextMode) -> proto::TextMode {\n\
             match mode {\n",
    );
    for variant in &variants {
        let snake = pascal_to_screaming_snake(&variant.name);
        let _ = writeln!(
            out,
            "                 TextMode::{name} => proto::TextMode::TEXT_MODE_{snake},",
            name = variant.name,
            snake = snake.to_ascii_uppercase()
        );
    }
    out.push_str(
        "             }\n\
         }\n\n\
         pub(super) fn text_mode_from_proto(mode: proto::TextMode) -> Result<TextMode, UnknownTextMode> {\n\
             Ok(match mode {\n",
    );
    for variant in &variants {
        let snake = pascal_to_screaming_snake(&variant.name);
        let _ = writeln!(
            out,
            "                 proto::TextMode::TEXT_MODE_{snake} => TextMode::{name},",
            name = variant.name,
            snake = snake.to_ascii_uppercase()
        );
    }
    out.push_str(
        "             })\n\
         }\n",
    );
    out
}

pub fn identity_id_impl_rust(identity_rs: &str) -> String {
    let variants = parse_rust_enum(identity_rs, "SettingIdentity");
    let mut out = String::from(
        "// Generated by build.rs from SettingIdentity.\n\
         #[cfg_attr(feature = \"tracing\", tracing::instrument(skip_all))]\n\
         pub(super) fn identity_id(id: &SettingIdentity) -> proto::SettingId {\n\
             match id {\n",
    );
    for variant in &variants {
        let snake = pascal_to_screaming_snake(&variant.name);
        let proto_kind = format!(
            "proto::SettingIdentityKind::SETTING_IDENTITY_KIND_{}",
            snake.to_ascii_uppercase()
        );
        let arm = match variant.shape {
            VariantShape::Unit => format!(
                "SettingIdentity::{name} => unit({proto_kind}),",
                name = variant.name,
                proto_kind = proto_kind
            ),
            VariantShape::Index => format!(
                "SettingIdentity::{name}(index) => indexed({proto_kind}, *index),",
                name = variant.name,
                proto_kind = proto_kind
            ),
            VariantShape::Key => format!(
                "SettingIdentity::{name}(key) => keyed({proto_kind}, key),",
                name = variant.name,
                proto_kind = proto_kind
            ),
        };
        if let Some(cfg) = &variant.cfg {
            let _ = writeln!(out, "                 {cfg}");
        }
        let _ = writeln!(out, "                 {arm}");
    }
    out.push_str("             }\n         }\n");
    out
}
