use crate::db::types::UnixTimestamp;
use crate::version::GitVersion;
use anyhow::{Context, Error};
use sqlx::SqlitePool;

/// Reads the Cadmus version stored in `_cadmus_version`.
///
/// Returns `None` if the table does not exist (database predates migration 012)
/// or if the row is missing.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn read_db_version(pool: &SqlitePool) -> Result<Option<GitVersion>, Error> {
    let result = sqlx::query_scalar!("SELECT version FROM _cadmus_version WHERE id = 1")
        .fetch_optional(pool)
        .await;

    let version_str: Option<String> = match result {
        Ok(v) => v,
        Err(sqlx::Error::Database(e)) if e.message().contains("no such table") => return Ok(None),
        Err(e) => return Err(Error::from(e).context("failed to read _cadmus_version")),
    };

    match version_str {
        Some(s) => Ok(Some(
            s.parse::<GitVersion>()
                .context("failed to parse stored _cadmus_version.version")?,
        )),
        None => Ok(None),
    }
}

/// Stamps the database with the current Cadmus version and migration timestamp.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn stamp_db_version(pool: &SqlitePool, version: &GitVersion) -> Result<(), Error> {
    let migrated_at = UnixTimestamp::now();
    let version_str = version.to_string();
    sqlx::query!(
        "INSERT INTO _cadmus_version (id, version, migrated_at)
         VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE
         SET version = excluded.version, migrated_at = excluded.migrated_at",
        version_str,
        migrated_at,
    )
    .execute(pool)
    .await
    .context("failed to stamp _cadmus_version")?;

    Ok(())
}

/// Compares the database version with the current application version.
///
/// Returns `None` when the database version is missing or equal to the app version.
/// Returns `Some(Comparison)` when they differ, indicating the relationship of the
/// app version to the database version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionGateResult {
    /// The database was written by a newer Cadmus build; this is a downgrade.
    Downgrade,
    /// The database was written by an older Cadmus build; normal upgrade path.
    Upgrade,
    /// The database version matches the app version.
    Current,
    /// No database version stamp exists (pre-012 database).
    Unknown,
}

/// Checks whether the database version is compatible with the running app.
///
/// A `Downgrade` result means the database was touched by a newer Cadmus version
/// and a backup from the current app version should be restored.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn check_version_gate(
    pool: &SqlitePool,
    app_version: &GitVersion,
) -> Result<VersionGateResult, Error> {
    match read_db_version(pool).await? {
        None => Ok(VersionGateResult::Unknown),
        Some(db_version) => match db_version.cmp(app_version) {
            std::cmp::Ordering::Greater => Ok(VersionGateResult::Downgrade),
            std::cmp::Ordering::Less => Ok(VersionGateResult::Upgrade),
            std::cmp::Ordering::Equal => Ok(VersionGateResult::Current),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::runtime::RUNTIME;
    use crate::version::get_current_version;
    use std::str::FromStr;

    fn setup_db() -> Database {
        let mut db = Database::new(":memory:").expect("failed to create in-memory database");
        db.init(0).expect("failed to run migrations");
        db
    }

    #[test]
    fn read_db_version_returns_none_before_table_exists() {
        // Database::new creates the pool but does not run migrations, so the
        // _cadmus_version table does not exist yet.
        let db = Database::new(":memory:").expect("failed to create in-memory database");
        let version = RUNTIME.block_on(async { read_db_version(db.pool()).await.unwrap() });
        assert!(version.is_none());
    }

    #[test]
    fn stamp_and_read_db_version_roundtrip() {
        let db = setup_db();
        let version = GitVersion::from_str("v0.10.0").unwrap();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &version).await.unwrap();
            let read = read_db_version(db.pool()).await.unwrap();
            assert_eq!(read, Some(version));
        });
    }

    #[test]
    fn check_version_gate_detects_upgrade() {
        let db = setup_db();
        let older = GitVersion::from_str("v0.9.0").unwrap();
        let newer = GitVersion::from_str("v0.10.0").unwrap();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &older).await.unwrap();
            let gate = check_version_gate(db.pool(), &newer).await.unwrap();
            assert_eq!(gate, VersionGateResult::Upgrade);
        });
    }

    #[test]
    fn check_version_gate_detects_downgrade() {
        let db = setup_db();
        let older = GitVersion::from_str("v0.9.0").unwrap();
        let newer = GitVersion::from_str("v0.10.0").unwrap();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &newer).await.unwrap();
            let gate = check_version_gate(db.pool(), &older).await.unwrap();
            assert_eq!(gate, VersionGateResult::Downgrade);
        });
    }

    #[test]
    fn check_version_gate_detects_current() {
        let db = setup_db();
        let version = GitVersion::from_str("v0.10.0").unwrap();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &version).await.unwrap();
            let gate = check_version_gate(db.pool(), &version).await.unwrap();
            assert_eq!(gate, VersionGateResult::Current);
        });
    }

    #[test]
    fn check_version_gate_unknown_when_table_is_empty() {
        let db = setup_db();

        RUNTIME.block_on(async {
            sqlx::query("DELETE FROM _cadmus_version")
                .execute(db.pool())
                .await
                .unwrap();
            let gate = check_version_gate(db.pool(), &get_current_version())
                .await
                .unwrap();
            assert_eq!(gate, VersionGateResult::Unknown);
        });
    }
}
