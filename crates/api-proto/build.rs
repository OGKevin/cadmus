use build_deps::api_codegen::{
    category_id_proto_source, extension_proto_source, setting_identity_kind_proto_source,
    text_mode_proto_source,
};
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core = manifest.join("../core/src");
    let proto = manifest.join("../../proto");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let locale_dir = out.join("cadmus/api/shared/v1");
    fs::create_dir_all(&locale_dir).expect("locale proto dir");

    let file_extension_rs =
        fs::read_to_string(core.join("document/file_extension.rs")).expect("file_extension.rs");
    let identity_rs = fs::read_to_string(core.join("view/settings_editor/kinds/identity.rs"))
        .expect("identity.rs");
    let category_rs =
        fs::read_to_string(core.join("view/settings_editor/category.rs")).expect("category.rs");
    let kinds_mod_rs =
        fs::read_to_string(core.join("view/settings_editor/kinds/mod.rs")).expect("kinds mod.rs");

    let locale_proto = locale_dir.join("locale.proto");
    fs::write(
        &locale_proto,
        locale_proto_source(&manifest.join("../../docs/po")),
    )
    .expect("write locale.proto");

    let extension_proto = locale_dir.join("extension.proto");
    fs::write(&extension_proto, extension_proto_source(&file_extension_rs))
        .expect("write extension.proto");

    let category_proto = locale_dir.join("category_id.proto");
    fs::write(&category_proto, category_id_proto_source(&category_rs))
        .expect("write category_id.proto");

    let text_mode_proto = locale_dir.join("text_mode.proto");
    fs::write(&text_mode_proto, text_mode_proto_source(&kinds_mod_rs))
        .expect("write text_mode.proto");

    let setting_identity_proto = locale_dir.join("setting_identity_kind.proto");
    fs::write(
        &setting_identity_proto,
        setting_identity_kind_proto_source(&identity_rs),
    )
    .expect("write setting_identity_kind.proto");

    println!("cargo:rerun-if-changed=../../docs/po");
    println!("cargo:rerun-if-changed=../../proto");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../core/src/document/file_extension.rs");
    println!("cargo:rerun-if-changed=../core/src/view/settings_editor/kinds/identity.rs");
    println!("cargo:rerun-if-changed=../core/src/view/settings_editor/category.rs");
    println!("cargo:rerun-if-changed=../core/src/view/settings_editor/kinds/mod.rs");

    let mut files = cadmus_proto_files(&proto);
    files.push(locale_proto);
    files.push(extension_proto);
    files.push(category_proto);
    files.push(text_mode_proto);
    files.push(setting_identity_proto);
    files.sort();

    connectrpc_build::Config::new()
        .files(&files)
        .includes(&[
            &out,
            &proto,
            &proto.join("gnostic"),
            &buffa_types_include(&manifest),
        ])
        .include_file("_connectrpc.rs")
        .compile()
        .expect("compile api proto");

    let document = out.join("openapi.json");
    write_openapi(&proto, &out, &files);
    set_info_version(
        &document,
        &build_deps::git::git_version(&manifest.join("../..")),
    );

    println!("cargo:rerun-if-changed=../../proto/gnostic");
    println!("cargo:rerun-if-changed=../../devenv.nix");
}

/// Generate the OpenAPI document from every Cadmus proto, then stamp the
/// version.
///
/// The document covers both surfaces: `cadmus.api.shared.v1` is served by both
/// listeners, and `cadmus.api.internal.v1` is documented for plugin authors. The
/// gnostic annotations are only visible to `protoc-gen-connect-openapi`, so this
/// runs `protoc` a second time instead of reusing the Rust codegen.
fn write_openapi(proto: &Path, out: &Path, files: &[PathBuf]) {
    let status = std::process::Command::new("protoc")
        .arg(format!("-I{}", proto.display()))
        .arg(format!("-I{}", proto.join("gnostic").display()))
        .arg(format!("-I{}", out.display()))
        .arg(format!("--connect-openapi_out={}", out.display()))
        .arg("--connect-openapi_opt=format=json")
        .arg("--connect-openapi_opt=path=openapi.json")
        .arg("--connect-openapi_opt=features=connectrpc;gnostic")
        .arg("--connect-openapi_opt=trim-unused-types")
        .args(files)
        .status()
        .unwrap_or_else(|error| {
            panic!(
                "protoc or protoc-gen-connect-openapi is missing from PATH \
                 (run the build inside `devenv shell`): {error}"
            )
        });
    assert!(
        status.success(),
        "protoc-gen-connect-openapi failed: {status}"
    );
}

/// Replace `info.version` in the generated document.
fn set_info_version(document: &Path, version: &str) {
    let text = fs::read_to_string(document).expect("read openapi.json");
    let mut spec: serde_json::Value = serde_json::from_str(&text).expect("parse openapi.json");
    spec["info"]["version"] = serde_json::Value::String(version.to_string());
    let rendered = serde_json::to_string_pretty(&spec).expect("render openapi.json");
    fs::write(document, rendered).expect("write openapi.json");
}

fn cadmus_proto_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_protos(&root.join("cadmus"), &mut files);
    files
}

fn collect_protos(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("proto entry").path();
        if path.is_dir() {
            collect_protos(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            files.push(path);
        }
    }
}

fn buffa_types_include(manifest: &Path) -> PathBuf {
    let lock = fs::read_to_string(manifest.join("../../Cargo.lock")).expect("Cargo.lock");
    let version = lock_version(&lock, "buffa-types");
    let crate_dir = format!("buffa-types-{version}");
    let src = cargo_home().join("registry/src");
    let Ok(vendors) = fs::read_dir(&src) else {
        panic!("CARGO_HOME registry missing: {}", src.display());
    };
    for vendor in vendors {
        let include = vendor
            .expect("registry vendor")
            .path()
            .join(&crate_dir)
            .join("protos");
        if include.join("google/protobuf/timestamp.proto").is_file() {
            return include;
        }
    }
    panic!(
        "buffa-types {version} timestamp.proto not in {}",
        src.display()
    );
}

fn cargo_home() -> PathBuf {
    if let Ok(home) = std::env::var("CARGO_HOME") {
        return PathBuf::from(home);
    }
    let home = std::env::var("HOME").expect("HOME");
    PathBuf::from(home).join(".cargo")
}

fn lock_version(lock: &str, name: &str) -> String {
    let header = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() != header {
            continue;
        }
        for line in lines.by_ref() {
            let line = line.trim();
            if let Some(version) = line.strip_prefix("version = \"") {
                return version.trim_end_matches('"').to_string();
            }
            if line.is_empty() || line.starts_with('[') {
                break;
            }
        }
    }
    panic!("{name} missing from Cargo.lock");
}

fn locale_proto_source(po_dir: &Path) -> String {
    let mut codes = vec!["en".to_string()];
    if po_dir.is_dir() {
        for entry in fs::read_dir(po_dir).expect("docs/po") {
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
    let mut body = String::from(
        "syntax = \"proto3\";\n\n\
         package cadmus.api.shared.v1;\n\n\
         // Generated from docs/po/*.po plus en. Same source as website/scripts/generate-locales.mjs.\n\
         enum Locale {\n  LOCALE_UNSPECIFIED = 0;\n",
    );
    for (index, code) in codes.iter().enumerate() {
        let name = code.to_ascii_uppercase().replace('-', "_");
        body.push_str(&format!("  LOCALE_{name} = {};\n", index + 1));
    }
    body.push_str("}\n");
    body
}
