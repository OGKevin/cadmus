//! Build script for the `core` crate.
//!
//! All third-party dependency orchestration lives in the
//! `build_deps` crate. This script's responsibilities are limited to:
//!
//! 1. Emitting compile-time metadata (git version, PR info, build
//!    provenance).
//! 2. Asking `build_deps` to produce the MuPDF/libwebp artifacts
//!    needed for the active target.
//! 3. Translating those artifacts into `cargo:rustc-link-*`
//!    directives for the Rust compiler.
//! 4. Generating the locales and bundled asset manifests.
//!
//! The whole script returns `Result`; the single `panic!` lives in
//! [`main`] so failures surface as a coherent error chain with cargo's
//! expected non-zero exit.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use anyhow::{Context, Ok, Result, bail};
use build_deps::build::kobo;
use build_deps::build::{mupdf_wrapper, native};

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

/// Set this to any changing value to force build metadata to refresh.
const FORCE_REBUILD_ENV: &str = "FORCE_REBUILD";

/// When set, skip building and linking third-party native dependencies.
/// Used by `cargo doc` and other no-link workflows where only Rust source
/// analysis and metadata generation are needed.
const SKIP_THIRDPARTY_DEPS_ENV: &str = "CADMUS_SKIP_THIRDPARTY_DEPS";

fn main() {
    if let Err(err) = try_main() {
        panic!("cadmus build script failed: {err:#}");
    }
}

/// Returns `true` when `CADMUS_SKIP_THIRDPARTY_DEPS` is set,
/// indicating that native dependency building and linking should be skipped.
fn skip_thirdparty_deps() -> bool {
    env::var_os(SKIP_THIRDPARTY_DEPS_ENV).is_some()
}

fn try_main() -> Result<()> {
    let target = env::var("TARGET").context("TARGET not set")?;

    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-env-changed={FORCE_REBUILD_ENV}");
    println!("cargo:rerun-if-env-changed={SKIP_THIRDPARTY_DEPS_ENV}");
    println!("cargo:rerun-if-env-changed=TARGET");

    let (git_version, pr_info) = get_version_info()?;
    println!("cargo:rustc-env=GIT_VERSION={git_version}");
    if let Some(pr) = pr_info {
        println!("cargo:rustc-env=PR_INFO={pr}");
    }

    let build_uuid = Uuid::now_v7().to_string();
    println!("cargo:rustc-env=BUILD_UUID={build_uuid}");

    let build_attributes = get_build_attributes()?;
    println!("cargo:rustc-env=BUILD_USER={}", build_attributes.user);
    println!("cargo:rustc-env=BUILD_HOST={}", build_attributes.host);
    println!(
        "cargo:rustc-env=BUILD_TIMESTAMP={}",
        build_attributes.timestamp
    );

    println!("cargo:rerun-if-env-changed=GH_OAUTH_CLIENT_ID");
    let client_id =
        env::var("GH_OAUTH_CLIENT_ID").unwrap_or_else(|_| "GH_OAUTH_CLIENT_ID_NOT_SET".to_string());
    println!("cargo:rustc-env=GH_OAUTH_CLIENT_ID={client_id}");

    let root = workspace_root()?;

    generate_migration_hash()?;
    generate_locales()?;
    generate_bundled_assets()?;

    if skip_thirdparty_deps() {
        println!("Skipping thirdparty deps");

        return Ok(());
    }

    let host = env::var("HOST").context("HOST not set")?;
    if target == "arm-unknown-linux-gnueabihf" {
        emit_kobo_link_directives();
        kobo::ensure_kobo_artifacts(&root)?;
    } else {
        if host != target {
            bail!(
                "cross-compilation detected: HOST={host} != TARGET={target}. Run with CADMUS_SKIP_THIRDPARTY_DEPS=1 or set up cross-compilation support."
            );
        }

        let artifacts =
            native::ensure_native_artifacts(&root).context("failed to build native deps")?;
        mupdf_wrapper::build_native_if_needed(&root, &artifacts.include)
            .context("failed to build mupdf_wrapper")?;
        emit_native_link_directives(&root, &target)?;
    }

    emit_common_link_directives();

    Ok(())
}

fn emit_native_link_directives(root: &Path, target: &str) -> Result<()> {
    let target_os = env::var("CARGO_CFG_TARGET_OS").context("CARGO_CFG_TARGET_OS not set")?;
    let build_deps = root.join("target/cadmus-build-deps").join(target);
    let libwebp = build_deps.join("libwebp");

    match target_os.as_ref() {
        "linux" => {
            println!("cargo:rustc-link-search=target/mupdf_wrapper/Linux");
            println!(
                "cargo:rustc-link-search=native={}/src/.libs",
                libwebp.display()
            );
            println!(
                "cargo:rustc-link-search=native={}/src/demux/.libs",
                libwebp.display()
            );
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
        "macos" => {
            println!("cargo:rustc-link-search=target/mupdf_wrapper/Darwin");
            println!(
                "cargo:rustc-link-search=native={}/src/.libs",
                libwebp.display()
            );
            println!(
                "cargo:rustc-link-search=native={}/src/demux/.libs",
                libwebp.display()
            );
            println!("cargo:rustc-link-lib=dylib=c++");
        }
        other => bail!("unsupported platform: {other}"),
    }

    println!("cargo:rustc-link-lib=mupdf-third");
    Ok(())
}

fn emit_kobo_link_directives() {
    println!("cargo:rustc-env=PKG_CONFIG_ALLOW_CROSS=1");
    println!("cargo:rustc-link-search=target/mupdf_wrapper/Kobo");
    println!("cargo:rustc-link-search=libs");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

fn emit_common_link_directives() {
    println!("cargo:rustc-link-lib=z");
    println!("cargo:rustc-link-lib=bz2");
    println!("cargo:rustc-link-lib=jpeg");
    println!("cargo:rustc-link-lib=png16");
    println!("cargo:rustc-link-lib=gumbo");
    println!("cargo:rustc-link-lib=openjp2");
    println!("cargo:rustc-link-lib=jbig2dec");
    println!("cargo:rustc-link-lib=webp");
    println!("cargo:rustc-link-lib=webpdemux");
}

fn generate_locales() -> Result<()> {
    println!("cargo:rerun-if-changed=i18n/");
    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let locales_dir = Path::new("i18n");
    let mut locales: Vec<String> = std::fs::read_dir(locales_dir)
        .with_context(|| format!("i18n/ directory not found: {}", locales_dir.display()))?
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
    std::fs::write(Path::new(&out_dir).join("locales.rs"), generated)
        .context("failed to write locales.rs")?;
    Ok(())
}

/// Generates the schema migration content hash.
///
/// The hash is embedded in database version stamps so a downgrade is allowed
/// when the running build and database were produced from the same migration
/// set, even if their application versions differ.
fn generate_migration_hash() -> Result<()> {
    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let migrations_dir = Path::new("migrations");
    let mut migration_files = Vec::new();

    println!("cargo:rerun-if-changed={}", migrations_dir.display());
    collect_migration_files(migrations_dir, migrations_dir, &mut migration_files)?;
    migration_files.sort_by(|a, b| a.0.cmp(&b.0));

    let doc_entries: String = migration_files
        .iter()
        .map(|(relative_path, _)| format!("/// - `{relative_path}`\n"))
        .collect();

    let mut hasher = blake3::Hasher::new();
    for (relative_path, path) in &migration_files {
        hasher.update(relative_path.as_bytes());
        hasher.update(&[0]);
        hasher.update(
            &fs::read(path)
                .with_context(|| format!("failed to read migration {}", path.display()))?,
        );
        hasher.update(&[0]);
    }

    let generated = format!(
        "/// BLAKE3 hash of schema migration files.\n///\n/// Files included in this hash:\n{doc_entries}pub const MIGRATION_HASH: &str = {:?};\n",
        hasher.finalize().to_hex().to_string()
    );

    fs::write(Path::new(&out_dir).join("migration_hash.rs"), generated)
        .context("failed to write migration_hash.rs")?;

    Ok(())
}

fn collect_migration_files(
    root: &Path,
    dir: &Path,
    migration_files: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read type for {}", path.display()))?;

        if file_type.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect_migration_files(root, &path, migration_files)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }

        println!("cargo:rerun-if-changed={}", path.display());
        let relative_path = path
            .strip_prefix(root)
            .with_context(|| format!("{} is not under {}", path.display(), root.display()))?;
        migration_files.push((relative_path.to_string_lossy().replace('\\', "/"), path));
    }

    Ok(())
}

/// Generates the bundled asset manifest.
///
/// This is later used during OTA to delete them so that
/// the new update bundle can re-install bundled assets.
///
/// This ensures that e.g. there are no lingering scripts, fonts or libs etc
/// when they are removed.
fn generate_bundled_assets() -> Result<()> {
    let out_dir = env::var("OUT_DIR").context("OUT_DIR not set")?;
    let workspace_root = workspace_root()?;
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
            bail!(
                "required asset directory missing: {}. Run `cargo xtask download-assets` before build.",
                asset_dir.display()
            );
        }

        println!("cargo:rerun-if-changed={}", asset_dir.display());
        collect_asset_files(&workspace_root, &asset_dir, &mut asset_files)
            .with_context(|| format!("failed to collect asset files under {}", dir))?;
    }

    asset_files.sort();

    let entries: String = asset_files
        .iter()
        .map(|path| format!("    {path:?},\n"))
        .collect();
    let generated = format!("const BUNDLED_ASSET_FILES: &[&str] = &[\n{entries}];\n");

    std::fs::write(Path::new(&out_dir).join("bundled_assets.rs"), generated)
        .context("failed to write bundled_assets.rs")?;
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?);
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .context("workspace root not found")
        .map(Path::to_path_buf)
}

fn collect_asset_files(
    workspace_root: &Path,
    dir: &Path,
    asset_files: &mut Vec<String>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to read type for {}", path.display()))?;

        if file_type.is_dir() {
            collect_asset_files(workspace_root, &path, asset_files)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let relative_path = path.strip_prefix(workspace_root).with_context(|| {
            format!(
                "{} is not under {}",
                path.display(),
                workspace_root.display()
            )
        })?;

        asset_files.push(relative_path.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

struct BuildAttributes {
    user: String,
    host: String,
    timestamp: String,
}

/// Captures build provenance values passed to rustc as compile-time env vars.
fn get_build_attributes() -> Result<BuildAttributes> {
    Ok(BuildAttributes {
        user: command_output("whoami")?,
        host: command_output("hostname")?,
        timestamp: build_timestamp()?,
    })
}

/// Runs a command and returns its trimmed stdout, or `unknown` if it fails.
fn command_output(command: &str) -> Result<String> {
    let output = Command::new(command)
        .output()
        .with_context(|| format!("failed to spawn `{command}`"))?;

    if !output.status.success() {
        return Ok("unknown".to_string());
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(if value.is_empty() {
        "unknown".to_string()
    } else {
        value
    })
}

/// Returns the current Unix epoch timestamp for this build script run.
fn build_timestamp() -> Result<String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .or_else(|_| Ok("unknown".to_string()))
}

/// Compute the Git version and PR info to embed in the build.
fn get_version_info() -> Result<(String, Option<String>)> {
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

    if env::var("CI").is_err() {
        return Ok((git_version, None));
    }

    if !env::var("GITHUB_EVENT_NAME")
        .unwrap_or_default()
        .starts_with("pull_request")
    {
        return Ok((git_version, None));
    }

    let pr_number = env::var("PR_NUMBER").context("PR_NUMBER not set in CI environment")?;
    let pr_head_sha = env::var("PR_HEAD_SHA").context("PR_HEAD_SHA not set in CI environment")?;
    let pr_head_short = pr_head_sha.get(..7).unwrap_or(&pr_head_sha).to_string();

    Ok((
        git_version,
        Some(format!("PR #{} ({})", pr_number, pr_head_short)),
    ))
}
