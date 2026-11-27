use build_deps::api_codegen::locale_impls_rust;
use build_deps::utils::write_if_changed;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../..");
    let proto = root.join("proto");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    let files =
        build_deps::openapi::write_generated_protos(&root, &out).expect("write generated protos");
    write_if_changed(
        &out.join("locale_impls.rs"),
        locale_impls_rust(&root.join("docs/po")).as_bytes(),
    )
    .expect("write locale_impls.rs");

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

    build_deps::openapi::write_openapi(&root, &out, &files).expect("generate openapi document");

    println!("cargo:rerun-if-changed=../../docs/po");
    println!("cargo:rerun-if-changed=../../proto");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../devenv.nix");
    for path in build_deps::git::version_inputs(&root) {
        println!("cargo:rerun-if-changed={}", path.display());
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
