//! `cargo xtask sqlx-prepare` — rebuild `.sqlx/` against one temporary database.
//!
//! Every `query!` in a crate build reads one database URL. Library queries and
//! dictionary index queries both live in `cadmus-core`, so prepare cannot
//! target the two runtime files separately. See
//! <https://github.com/transact-rs/sqlx/issues/916>.
//!
//! The temporary file receives [`LIBRARY_MIGRATIONS`] through `sqlx migrate`,
//! then every `.sql` file under [`DICTIONARY_MIGRATIONS`] through `sqlite3`.
//! The dictionary migrator is not used on that file: both migrators store
//! version 1 in `_sqlx_migrations`. Library migration `018` drops the index
//! tables, so the dictionary SQL is applied after the library migrator.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::util::{cmd, workspace};

const LIBRARY_MIGRATIONS: &str = "crates/core/migrations";
const DICTIONARY_MIGRATIONS: &str = "crates/core/dictionary/migrations";

/// Builds a temporary schema and runs `cargo sqlx prepare` against it.
///
/// # Errors
///
/// Returns an error if the temporary directory cannot be created, either
/// migration set cannot be applied, or `cargo sqlx prepare` fails.
pub fn run() -> Result<()> {
    let root = workspace::root()?;
    let scratch = TempDir::create()?;
    let db_path = scratch.path.join("prepare.sqlite");
    let url = sqlite_url(&db_path);

    let source = root.join(LIBRARY_MIGRATIONS);
    let source = source
        .to_str()
        .context("library migrations path is not UTF-8")?;
    cmd::run(
        "sqlx",
        &["database", "create", "--no-dotenv", "--database-url", &url],
        &root,
        &[("SQLX_OFFLINE", "false")],
    )?;
    cmd::run(
        "sqlx",
        &[
            "migrate",
            "run",
            "--source",
            source,
            "--no-dotenv",
            "--database-url",
            &url,
            "--sqlite-create-db-wal",
            "false",
        ],
        &root,
        &[("SQLX_OFFLINE", "false")],
    )?;

    for sql in dictionary_schema_files(&root)? {
        apply_sql(&db_path, &sql)?;
    }

    cmd::run(
        "cargo",
        &[
            "sqlx",
            "prepare",
            "--all",
            "--workspace",
            "--no-dotenv",
            "--database-url",
            &url,
            "--",
            "--tests",
        ],
        &root,
        &[("DATABASE_URL", url.as_str()), ("SQLX_OFFLINE", "false")],
    )
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite:{}", path.display())
}

fn dictionary_schema_files(root: &Path) -> Result<Vec<PathBuf>> {
    let dir = root.join(DICTIONARY_MIGRATIONS);
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "sql") {
            files.push(path);
        }
    }
    files.sort();
    if files.is_empty() {
        bail!("no dictionary migrations in {}", dir.display());
    }
    Ok(files)
}

fn apply_sql(db: &Path, sql: &Path) -> Result<()> {
    println!("$ sqlite3 {} < {}", db.display(), sql.display());
    let file = File::open(sql).with_context(|| format!("open {}", sql.display()))?;
    let status = Command::new("sqlite3")
        .arg(db)
        .stdin(Stdio::from(file))
        .status()
        .context("failed to spawn `sqlite3`")?;
    if status.success() {
        return Ok(());
    }
    match status.code() {
        Some(code) => bail!("`sqlite3` exited with status {code}"),
        None => bail!("`sqlite3` was terminated by a signal"),
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("cadmus-sqlx-prepare-{}", std::process::id()));
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", path.display()));
            }
        }
        std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_url_is_absolute() {
        assert_eq!(
            sqlite_url(Path::new("/tmp/prepare.sqlite")),
            "sqlite:/tmp/prepare.sqlite"
        );
    }

    #[test]
    fn dictionary_schema_files_lists_the_index_migration() {
        let root = workspace::root().expect("workspace root");
        let files = dictionary_schema_files(&root).expect("dictionary migrations");
        assert!(
            files
                .iter()
                .any(|path| path.ends_with("001_dictionary_index.sql"))
        );
    }

    #[test]
    fn temp_dir_replaces_leftover_contents() {
        let path = std::env::temp_dir().join(format!("cadmus-sqlx-prepare-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("leftover dir");
        std::fs::write(path.join("stale"), b"x").expect("leftover file");
        let scratch = TempDir::create().expect("scratch");
        assert!(scratch.path.is_dir());
        assert!(!scratch.path.join("stale").exists());
    }
}
