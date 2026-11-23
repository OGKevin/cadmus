//! OpenAPI document generation shared by the `api-proto` build script and
//! `cadmuscli docs`.
//!
//! The generated enum protos and the `protoc-gen-connect-openapi` invocation
//! live here once, so the document embedded in the device and the one
//! published with the website cannot drift apart.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::api_codegen::{
    category_id_proto_source, extension_proto_source, locale_proto_source,
    setting_identity_kind_proto_source, text_mode_proto_source,
};
use crate::git::git_version;
use crate::utils::write_if_changed;

/// Package directory, relative to the output root, that holds the generated
/// protos.
const GENERATED_PACKAGE_DIR: &str = "cadmus/api/shared/v1";

/// Name of the generated OpenAPI document inside the output directory.
pub const OPENAPI_FILE: &str = "openapi.json";

/// Path prefix of operations served only by the loopback listener, which has
/// no authentication.
const INTERNAL_PATH_PREFIX: &str = "/cadmus.api.internal.";

/// Writes the protos generated from the enum manifest and `docs/po` into
/// `out`, and returns every proto the API consists of, sorted.
///
/// `root` is the repository root. Files whose content did not change are left
/// untouched so their modification times stay stable.
///
/// # Errors
///
/// Returns an error when the proto submodules cannot be initialised or a file
/// cannot be written.
pub fn write_generated_protos(root: &Path, out: &Path) -> Result<Vec<PathBuf>> {
    let proto = root.join("proto");
    if !proto.join("gnostic/openapiv3/annotations.proto").is_file() {
        crate::ensure_submodules(root).context("initialize proto submodules")?;
    }

    let package_dir = out.join(GENERATED_PACKAGE_DIR);
    fs::create_dir_all(&package_dir)
        .with_context(|| format!("create {}", package_dir.display()))?;

    let generated = [
        ("locale.proto", locale_proto_source(&root.join("docs/po"))),
        ("extension.proto", extension_proto_source()),
        ("category_id.proto", category_id_proto_source()),
        ("text_mode.proto", text_mode_proto_source()),
        (
            "setting_identity_kind.proto",
            setting_identity_kind_proto_source(),
        ),
    ];

    let mut files = Vec::new();
    collect_protos(&proto.join("cadmus"), &mut files)?;
    for (name, source) in generated {
        let path = package_dir.join(name);
        write_if_changed(&path, source.as_bytes())?;
        files.push(path);
    }
    files.sort();
    Ok(files)
}

/// Generates the OpenAPI document for every Cadmus proto into `out` and
/// returns its path.
///
/// # Errors
///
/// Returns an error when a generated proto cannot be written, `protoc` or
/// `protoc-gen-connect-openapi` is missing or fails, or the document cannot be
/// post-processed.
pub fn generate_openapi(root: &Path, out: &Path) -> Result<PathBuf> {
    let files = write_generated_protos(root, out)?;
    write_openapi(root, out, &files)
}

/// Runs `protoc` with `protoc-gen-connect-openapi` over `files`, then stamps
/// the version and documents authentication failures.
///
/// The document covers both surfaces: `cadmus.api.shared.v1` is served by both
/// listeners, and `cadmus.api.internal.v1` is documented for plugin authors.
/// The gnostic annotations are only visible to `protoc-gen-connect-openapi`,
/// so this runs `protoc` separately from the Rust codegen.
///
/// # Errors
///
/// Returns an error when the tools are missing, fail, or produce a document
/// that cannot be parsed.
pub fn write_openapi(root: &Path, out: &Path, files: &[PathBuf]) -> Result<PathBuf> {
    let proto = root.join("proto");
    let status = Command::new("protoc")
        .arg(format!("-I{}", proto.display()))
        .arg(format!("-I{}", proto.join("gnostic").display()))
        .arg(format!("-I{}", out.display()))
        .arg(format!("--connect-openapi_out={}", out.display()))
        .arg("--connect-openapi_opt=format=json")
        .arg(format!("--connect-openapi_opt=path={OPENAPI_FILE}"))
        .arg("--connect-openapi_opt=features=connectrpc;gnostic")
        .arg("--connect-openapi_opt=trim-unused-types")
        .args(files)
        .status()
        .context(
            "protoc or protoc-gen-connect-openapi is missing from PATH \
             (run the build inside `devenv shell`)",
        )?;
    if !status.success() {
        bail!("protoc-gen-connect-openapi failed: {status}");
    }

    let document = out.join(OPENAPI_FILE);
    finish_openapi(&document, &git_version(root))?;
    Ok(document)
}

/// Stamps `info.version` and documents the authentication failure response.
fn finish_openapi(document: &Path, version: &str) -> Result<()> {
    let text =
        fs::read_to_string(document).with_context(|| format!("read {}", document.display()))?;
    let mut spec: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", document.display()))?;
    spec["info"]["version"] = serde_json::Value::String(version.to_string());
    document_unauthenticated(&mut spec);
    let rendered = serde_json::to_string_pretty(&spec).context("render openapi document")?;
    write_if_changed(document, rendered.as_bytes())
}

/// Adds a `401` response to every operation that requires a bearer token.
///
/// Connect answers a missing or wrong token with HTTP 401 and the
/// `unauthenticated` error code. The generator only emits `200` and `default`,
/// so the shape a client sees when auth is enabled would otherwise be
/// undocumented. An operation requires a token unless its own `security`
/// lists an empty requirement, which opts it out of the document-wide one.
/// Operations under `cadmus.api.internal.` are served by the unauthenticated
/// loopback listener and never get a `401`.
fn document_unauthenticated(spec: &mut serde_json::Value) {
    let required = spec["security"]
        .as_array()
        .is_some_and(|requirements| !requirements.is_empty());
    let response = serde_json::json!({
        "description": "Missing or invalid bearer token.",
        "content": {
            "application/json": {
                "schema": { "$ref": "#/components/schemas/connect.error" },
                "example": { "code": "unauthenticated", "message": "unauthorized" }
            }
        }
    });
    let Some(paths) = spec["paths"].as_object_mut() else {
        return;
    };
    for (path, item) in paths.iter_mut() {
        if path.starts_with(INTERNAL_PATH_PREFIX) {
            continue;
        }
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        for operation in item.values_mut().filter(|value| value.is_object()) {
            let opts_out = operation["security"]
                .as_array()
                .is_some_and(|requirements| {
                    requirements.iter().any(|requirement| {
                        requirement
                            .as_object()
                            .is_some_and(|scheme| scheme.is_empty())
                    })
                });
            if required && !opts_out {
                operation["responses"]["401"] = response.clone();
            }
        }
    }
}

fn collect_protos(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry
            .with_context(|| format!("read entry in {}", dir.display()))?
            .path();
        if path.is_dir() {
            collect_protos(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::document_unauthenticated;
    use serde_json::{Value, json};

    fn spec() -> Value {
        json!({
            "security": [{ "bearer": [] }],
            "paths": {
                "/cadmus.api.shared.v1.Settings/List": {
                    "summary": "List settings",
                    "parameters": [],
                    "post": { "responses": { "200": {} } }
                },
                "/cadmus.api.shared.v1.Status/GetStatus": {
                    "post": { "security": [{}], "responses": { "200": {} } }
                },
                "/cadmus.api.internal.v1.Placeholder/Ping": {
                    "post": { "responses": { "200": {} } }
                }
            }
        })
    }

    #[test]
    fn internal_operations_get_no_unauthenticated_response() {
        let mut document = spec();
        document_unauthenticated(&mut document);
        let paths = &document["paths"];
        assert!(
            paths["/cadmus.api.shared.v1.Settings/List"]["post"]["responses"]
                .get("401")
                .is_some()
        );
        assert!(
            paths["/cadmus.api.shared.v1.Status/GetStatus"]["post"]["responses"]
                .get("401")
                .is_none()
        );
        assert!(
            paths["/cadmus.api.internal.v1.Placeholder/Ping"]["post"]["responses"]
                .get("401")
                .is_none()
        );
    }
}
