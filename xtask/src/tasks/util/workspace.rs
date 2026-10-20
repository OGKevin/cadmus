//! Workspace root resolution and metadata helpers.
//!
//! The xtask binary can be invoked from any directory inside the Cargo
//! workspace.  All tasks need the workspace root so they can locate source
//! files, thirdparty directories, and build outputs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Returns the absolute path to the Cargo workspace root.
///
/// Resolution order:
///
/// 1. `CADMUS_WORKSPACE_ROOT` — if set, it must be a Cadmus workspace or this
///    function returns an error (it does not fall through).
/// 2. The parent of `CARGO_MANIFEST_DIR` — used when that parent is a Cadmus
///    workspace. If `CARGO_MANIFEST_DIR` is set but the parent is not a Cadmus
///    workspace, resolution falls through.
/// 3. Walk up from the process current directory until a Cadmus workspace is
///    found.
///
/// A directory is a Cadmus workspace when its `Cargo.toml` parses as TOML, has
/// a `[workspace]` table, and `workspace.members` contains `"xtask"`.
///
/// # Errors
///
/// Returns an error if no Cadmus workspace is found. Set
/// `CADMUS_WORKSPACE_ROOT`, run `cargo xtask`, or invoke `cadmuscli` from
/// inside the repository.
///
/// # Examples
///
/// ```no_run
/// # use std::path::PathBuf;
/// // Note: requires running inside the Cadmus workspace.
/// let root = xtask_lib::tasks::util::workspace::root().unwrap();
/// assert!(root.join("Cargo.toml").exists());
/// ```
pub fn root() -> Result<PathBuf> {
    resolve_root(
        std::env::var_os("CADMUS_WORKSPACE_ROOT").map(PathBuf::from),
        std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from),
        std::env::current_dir().context("failed to get current directory")?,
    )
}

fn resolve_root(
    workspace_root_env: Option<PathBuf>,
    cargo_manifest_dir: Option<PathBuf>,
    cwd: PathBuf,
) -> Result<PathBuf> {
    if let Some(explicit) = workspace_root_env {
        return require_cadmus_workspace(explicit);
    }

    if let Some(root) = cargo_manifest_dir
        .as_deref()
        .and_then(workspace_parent_of_manifest_dir)
    {
        return Ok(root);
    }

    if let Some(root) = nearest_cadmus_workspace(&cwd) {
        return Ok(root);
    }

    anyhow::bail!(
        "could not locate the Cadmus workspace root from {}; set CADMUS_WORKSPACE_ROOT, run `cargo xtask`, or invoke cadmuscli from inside the repository",
        cwd.display()
    )
}

fn require_cadmus_workspace(dir: PathBuf) -> Result<PathBuf> {
    if is_cadmus_workspace(&dir) {
        return Ok(dir);
    }
    anyhow::bail!(
        "CADMUS_WORKSPACE_ROOT is not a Cadmus workspace (expected a Cargo.toml [workspace] whose members include \"xtask\"): {}",
        dir.display()
    )
}

fn workspace_parent_of_manifest_dir(manifest_dir: &Path) -> Option<PathBuf> {
    let parent = manifest_dir.parent()?;
    if !is_cadmus_workspace(parent) {
        return None;
    }
    Some(parent.to_path_buf())
}

fn nearest_cadmus_workspace(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if is_cadmus_workspace(&dir) {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn is_cadmus_workspace(dir: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
        return false;
    };
    let Ok(table) = content.parse::<toml::Table>() else {
        return false;
    };
    table
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .is_some_and(|members| {
            members
                .iter()
                .any(|member| member.as_str() == Some("xtask"))
        })
}

/// Returns the version of the `cadmus` binary crate from its `Cargo.toml`.
///
/// # Errors
///
/// Returns an error if the workspace root cannot be found, the `Cargo.toml`
/// cannot be read, or the version field is missing.
pub fn current_version() -> Result<String> {
    let workspace_root = root()?;
    let cargo_toml_path = workspace_root.join("crates/cadmus/Cargo.toml");

    let content = std::fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("failed to read {}", cargo_toml_path.display()))?;

    let doc: toml::Table = content
        .parse()
        .with_context(|| format!("failed to parse {}", cargo_toml_path.display()))?;

    doc.get("package")
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .context("version field not found in crates/cadmus/Cargo.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_cadmus_workspace(dir: &Path) {
        fs::write(
            dir.join("Cargo.toml"),
            "[workspace]\nmembers = [\"xtask\"]\n",
        )
        .unwrap();
    }

    fn cadmus_workspace() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        write_cadmus_workspace(tmp.path());
        tmp
    }

    #[test]
    fn explicit_workspace_root_wins_over_manifest_dir_and_cwd() {
        let explicit = cadmus_workspace();
        let other = cadmus_workspace();
        let nested = other.path().join("docs/src");
        fs::create_dir_all(&nested).unwrap();

        let root = resolve_root(
            Some(explicit.path().to_path_buf()),
            Some(other.path().join("xtask")),
            nested,
        )
        .unwrap();

        assert_eq!(root, explicit.path());
    }

    #[test]
    fn cargo_manifest_dir_parent_when_cadmus_workspace() {
        let workspace = cadmus_workspace();
        let cwd = tempfile::tempdir().unwrap();

        let root = resolve_root(
            None,
            Some(workspace.path().join("xtask")),
            cwd.path().to_path_buf(),
        )
        .unwrap();

        assert_eq!(root, workspace.path());
    }

    #[test]
    fn walk_up_from_nested_cwd() {
        let workspace = cadmus_workspace();
        let nested = workspace.path().join("docs/src");
        fs::create_dir_all(&nested).unwrap();

        let root = resolve_root(None, None, nested).unwrap();

        assert_eq!(root, workspace.path());
    }

    #[test]
    fn workspace_members_without_xtask_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"other\"]\n",
        )
        .unwrap();
        let good = cadmus_workspace();

        let walk = resolve_root(None, None, tmp.path().to_path_buf());
        assert!(walk.is_err());

        let explicit = resolve_root(
            Some(tmp.path().to_path_buf()),
            None,
            good.path().to_path_buf(),
        );
        assert!(explicit.is_err());
    }

    #[test]
    fn explicit_invalid_workspace_root_errors() {
        let empty = tempfile::tempdir().unwrap();
        let empty_err = resolve_root(
            Some(empty.path().to_path_buf()),
            None,
            empty.path().to_path_buf(),
        );
        assert!(empty_err.is_err());

        let missing = tempfile::tempdir().unwrap();
        fs::write(missing.path().join("README"), b"not a workspace").unwrap();
        let missing_err = resolve_root(
            Some(missing.path().to_path_buf()),
            None,
            missing.path().to_path_buf(),
        );
        assert!(missing_err.is_err());
    }

    #[test]
    fn cargo_manifest_dir_non_workspace_falls_through_to_cwd_walk() {
        let workspace = cadmus_workspace();
        let nested = workspace.path().join("docs/src");
        fs::create_dir_all(&nested).unwrap();
        let decoy = tempfile::tempdir().unwrap();

        let root = resolve_root(None, Some(decoy.path().join("xtask")), nested).unwrap();

        assert_eq!(root, workspace.path());
    }
}
