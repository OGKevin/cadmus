use build_deps::api_codegen::{
    category_id_proto_source, extension_proto_source, locale_impls_rust, locale_proto_source,
    setting_identity_kind_proto_source, text_mode_proto_source,
};
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../..");
    let proto = root.join("proto");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let locale_dir = out.join("cadmus/api/shared/v1");
    fs::create_dir_all(&locale_dir).expect("locale proto dir");

    if !proto.join("gnostic/openapiv3/annotations.proto").is_file() {
        build_deps::ensure_submodules(&manifest.join("../.."))
            .expect("initialize proto submodules");
    }

    let locale_proto = locale_dir.join("locale.proto");
    fs::write(
        &locale_proto,
        locale_proto_source(&manifest.join("../../docs/po")),
    )
    .expect("write locale.proto");

    let locale_impls = out.join("locale_impls.rs");
    fs::write(
        &locale_impls,
        locale_impls_rust(&manifest.join("../../docs/po")),
    )
    .expect("write locale_impls.rs");

    let extension_proto = locale_dir.join("extension.proto");
    fs::write(&extension_proto, extension_proto_source()).expect("write extension.proto");

    let category_proto = locale_dir.join("category_id.proto");
    fs::write(&category_proto, category_id_proto_source()).expect("write category_id.proto");

    let text_mode_proto = locale_dir.join("text_mode.proto");
    fs::write(&text_mode_proto, text_mode_proto_source()).expect("write text_mode.proto");

    let setting_identity_proto = locale_dir.join("setting_identity_kind.proto");
    fs::write(
        &setting_identity_proto,
        setting_identity_kind_proto_source(),
    )
    .expect("write setting_identity_kind.proto");

    println!("cargo:rerun-if-changed=../../docs/po");
    println!("cargo:rerun-if-changed=../../proto");
    println!("cargo:rerun-if-changed=../../Cargo.lock");

    let git_head = manifest.join("../../.git/HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed={}", git_head.display());
    }

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
        .emit_rerun_directives(false)
        .compile()
        .expect("compile api proto");

    let document = out.join("openapi.json");
    write_openapi(&proto, &out, &files);
    set_info_version(
        &document,
        &build_deps::git::git_version(&manifest.join("../..")),
    );

    println!("cargo:rerun-if-changed=../../docs/po");
    println!("cargo:rerun-if-changed=../../proto");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
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

/// Locates the `buffa-types` `protos` directory via `cargo metadata`.
///
/// The crate's location varies between registry, git, and vendored
/// checkouts, so asking cargo beats guessing the registry layout.
fn buffa_types_include(manifest: &Path) -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = std::process::Command::new(cargo)
        .current_dir(manifest)
        .args(["metadata", "--format-version", "1"])
        .output()
        .expect("cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata json");
    let manifest_path = metadata["packages"]
        .as_array()
        .expect("cargo metadata packages")
        .iter()
        .find(|package| package["name"] == "buffa-types")
        .and_then(|package| package["manifest_path"].as_str())
        .unwrap_or_else(|| panic!("buffa-types missing from cargo metadata"));
    let include = PathBuf::from(manifest_path)
        .parent()
        .expect("buffa-types manifest without parent")
        .join("protos");
    assert!(
        include.join("google/protobuf/timestamp.proto").is_file(),
        "buffa-types protos missing in {}",
        include.display()
    );
    include
}
