use std::env::{self, VarError};
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

const BUNDLED_ASSET_DIRS: &[&str] = &[
    "bin",
    "css",
    "fonts",
    "hyphenation-patterns",
    "icons",
    "keyboard-layouts",
    "resources",
    "scripts",
];

fn main() {
    let target = env::var("TARGET").unwrap();

    println!("cargo:rerun-if-changed=.git/HEAD");
    let (git_version, pr_info) = get_version_info().expect("Failed to get version info");
    println!("cargo:rustc-env=GIT_VERSION={}", git_version);
    if let Some(pr) = pr_info {
        println!("cargo:rustc-env=PR_INFO={}", pr);
    }

    let build_uuid = Uuid::now_v7().to_string();
    println!("cargo:rustc-env=BUILD_UUID={}", build_uuid);

    // GitHub OAuth App client ID for device flow authentication.
    println!("cargo:rerun-if-env-changed=GH_OAUTH_CLIENT_ID");
    let client_id =
        env::var("GH_OAUTH_CLIENT_ID").unwrap_or_else(|_| "GH_OAUTH_CLIENT_ID_NOT_SET".to_string());
    println!("cargo:rustc-env=GH_OAUTH_CLIENT_ID={}", client_id);

    // Cross-compiling for Kobo.
    if target == "arm-unknown-linux-gnueabihf" {
        println!("cargo:rustc-env=PKG_CONFIG_ALLOW_CROSS=1");
        println!("cargo:rustc-link-search=target/mupdf_wrapper/Kobo");
        println!("cargo:rustc-link-search=libs");
        println!("cargo:rustc-link-lib=dylib=stdc++");
    // Handle the Linux and macOS platforms.
    } else {
        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
        match target_os.as_ref() {
            "linux" => {
                println!("cargo:rustc-link-search=target/mupdf_wrapper/Linux");
                println!("cargo:rustc-link-lib=dylib=stdc++");
            }
            "macos" => {
                println!("cargo:rustc-link-search=target/mupdf_wrapper/Darwin");
                println!("cargo:rustc-link-lib=dylib=c++");
            }
            _ => panic!("Unsupported platform: {}.", target_os),
        }

        println!("cargo:rustc-link-lib=mupdf-third");
    }

    println!("cargo:rustc-link-lib=z");
    println!("cargo:rustc-link-lib=bz2");
    println!("cargo:rustc-link-lib=jpeg");
    println!("cargo:rustc-link-lib=png16");
    println!("cargo:rustc-link-lib=gumbo");
    println!("cargo:rustc-link-lib=openjp2");
    println!("cargo:rustc-link-lib=jbig2dec");

    generate_locales();
    generate_bundled_assets();
}

fn generate_locales() {
    println!("cargo:rerun-if-changed=i18n/");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let mut locales: Vec<String> = std::fs::read_dir("i18n/")
        .expect("i18n/ directory not found")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if entry.file_type().ok()?.is_dir() {
                entry.file_name().into_string().ok()
            } else {
                None
            }
        })
        .collect();
    locales.sort();
    let entries: String = locales.iter().map(|l| format!("    \"{l}\",\n")).collect();
    let generated = format!("pub const AVAILABLE_LOCALES: &[&str] = &[\n{entries}];\n");
    std::fs::write(std::path::Path::new(&out_dir).join("locales.rs"), generated)
        .expect("failed to write locales.rs");
}

/// Generates the bundled asset manifest.
///
/// This is later used during OTA to delete them so that
/// the new update bundle can re-install bundled assets.
///
/// This ensures that e.g. there are no lingering scripts, fonts or libs etc
/// when they are removed.
fn generate_bundled_assets() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let workspace_root = workspace_root();
    let is_kobo = env::var("TARGET").unwrap_or_default() == "arm-unknown-linux-gnueabihf";
    let is_release = env::var("PROFILE").unwrap_or_default() == "release";
    let mut asset_files = Vec::new();

    for dir in BUNDLED_ASSET_DIRS {
        let asset_dir = workspace_root.join(dir);

        if is_kobo
            && is_release
            && matches!(*dir, "bin" | "resources" | "hyphenation-patterns")
            && !asset_dir.is_dir()
        {
            panic!(
                "required asset directory missing: {}. Run `cargo xtask download-assets` before build.",
                asset_dir.display()
            );
        }

        println!("cargo:rerun-if-changed={}", asset_dir.display());
        collect_asset_files(&workspace_root, &asset_dir, &mut asset_files);
    }

    asset_files.sort();

    let entries: String = asset_files
        .iter()
        .map(|path| format!("    {path:?},\n"))
        .collect();
    let generated = format!("const BUNDLED_ASSET_FILES: &[&str] = &[\n{entries}];\n");

    std::fs::write(Path::new(&out_dir).join("bundled_assets.rs"), generated)
        .expect("failed to write bundled_assets.rs");
}

fn workspace_root() -> PathBuf {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));

    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root not found")
        .to_path_buf()
}

fn collect_asset_files(workspace_root: &Path, dir: &Path, asset_files: &mut Vec<String>) {
    if !dir.exists() {
        return;
    }

    for entry in
        std::fs::read_dir(dir).unwrap_or_else(|_| panic!("failed to read {}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|_| panic!("failed to read entry in {}", dir.display()));
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|_| panic!("failed to read type for {}", path.display()));

        if file_type.is_dir() {
            collect_asset_files(workspace_root, &path, asset_files);
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let relative_path = path.strip_prefix(workspace_root).unwrap_or_else(|_| {
            panic!(
                "{} is not under {}",
                path.display(),
                workspace_root.display()
            )
        });

        asset_files.push(relative_path.to_string_lossy().replace('\\', "/"));
    }
}

fn get_version_info() -> Result<(String, Option<String>), VarError> {
    let git_version = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .and_then(|output| {
            output
                .status
                .success()
                .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let ci_var = env::var("CI").ok();
    match ci_var {
        Some(_) => {
            if !env::var("GITHUB_EVENT_NAME")
                .unwrap_or_default()
                .starts_with("pull_request")
            {
                return Ok((git_version, None));
            }

            let pr_number = env::var("PR_NUMBER").expect("PR_NUMBER not set in CI environment");
            let mut pr_head_sha =
                env::var("PR_HEAD_SHA").expect("PR_HEAD_SHA not set in CI environment");
            pr_head_sha = pr_head_sha.get(..7).unwrap_or(&pr_head_sha).to_string();

            Ok((
                git_version,
                Some(format!("PR #{} ({})", pr_number, pr_head_sha)),
            ))
        }
        _ => Ok((git_version, None)),
    }
}
