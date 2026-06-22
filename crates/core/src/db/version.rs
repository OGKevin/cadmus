use crate::db::types::UnixTimestamp;
use crate::helpers::Fp;
use crate::version::GitVersion;
use anyhow::{Context, Error};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::str::FromStr;

include!(concat!(env!("OUT_DIR"), "/migration_hash.rs"));

/// BLAKE3 hash of all schema migration file paths and contents.
///
/// Backed by [`Fp`] so it shares the same hex encoding, parsing, and sqlx
/// serialisation behaviour as book content fingerprints.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MigrationHash(Fp);

impl MigrationHash {
    /// Returns the migration hash embedded in the running build.
    pub fn current() -> Self {
        MIGRATION_HASH
            .parse()
            .expect("generated migration hash should be valid")
    }
}

impl std::fmt::Display for MigrationHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for MigrationHash {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<Fp>().map(Self).map_err(Error::from)
    }
}

impl sqlx::Type<sqlx::Sqlite> for MigrationHash {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <Fp as sqlx::Type<sqlx::Sqlite>>::type_info()
    }

    fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
        <Fp as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for MigrationHash {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        self.0.encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for MigrationHash {
    fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        Fp::decode(value).map(Self)
    }
}

/// Returns the schema migration hash embedded in the running build.
pub fn current_migration_hash() -> MigrationHash {
    MigrationHash::current()
}

/// Version and schema migration state stored in `_cadmus_version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbVersionStamp {
    /// Cadmus version that last stamped the database.
    pub version: GitVersion,
    /// Migration hash that last stamped the database.
    pub migration_hash: MigrationHash,
}

/// Reads the Cadmus version stored in `_cadmus_version`.
///
/// Returns `None` if the table does not exist (database predates migration 012)
/// or if the row is missing.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn read_db_version(pool: &SqlitePool) -> Result<Option<GitVersion>, Error> {
    Ok(read_db_version_stamp(pool)
        .await?
        .map(|stamp| stamp.version))
}

/// Reads the Cadmus version stamp stored in `_cadmus_version`.
///
/// Returns `None` if the table does not exist or the singleton row is missing.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn read_db_version_stamp(pool: &SqlitePool) -> Result<Option<DbVersionStamp>, Error> {
    let result = sqlx::query!(
        r#"SELECT version AS "version: GitVersion", migration_hash AS "migration_hash: MigrationHash"
           FROM _cadmus_version
           WHERE id = 1"#,
    )
    .fetch_optional(pool)
    .await;

    match result {
        Ok(Some(row)) => Ok(Some(DbVersionStamp {
            version: row.version,
            migration_hash: row.migration_hash,
        })),
        Ok(None) => Ok(None),
        Err(sqlx::Error::Database(e)) if e.message().contains("no such table") => Ok(None),
        Err(e) => Err(Error::from(e).context("failed to read _cadmus_version")),
    }
}

/// Stamps the database with an explicit migration hash.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn stamp_db_version(
    pool: &SqlitePool,
    version: &GitVersion,
    migration_hash: &MigrationHash,
) -> Result<(), Error> {
    let migrated_at = UnixTimestamp::now();
    let version_str = version.to_string();
    let migration_hash_str = migration_hash.to_string();
    sqlx::query!(
        "INSERT INTO _cadmus_version (id, version, migration_hash, migrated_at)
         VALUES (1, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE
         SET version = excluded.version,
             migration_hash = excluded.migration_hash,
             migrated_at = excluded.migrated_at",
        version_str,
        migration_hash_str,
        migrated_at,
    )
    .execute(pool)
    .await
    .context("failed to stamp _cadmus_version")?;

    Ok(())
}

/// Compares the database version with the current application version.
///
/// A newer database is compatible with an older app only when both were built
/// from the same schema migration file set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionGateResult {
    /// The database was written by a newer Cadmus build; this is a downgrade.
    Downgrade,
    /// The database was written by an older Cadmus build; normal upgrade path.
    Upgrade,
    /// The database version matches the app version.
    Current,
    /// The database was written by a newer app with the same schema migrations.
    CompatibleDowngrade,
    /// No database version stamp exists (pre-012 database).
    Unknown,
}

/// Checks whether the database version is compatible with the running app.
///
/// A `Downgrade` result means the database was touched by a newer Cadmus
/// version with different schema migrations, so a backup from the current app
/// version should be restored.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(pool)))]
pub async fn check_version_gate(
    pool: &SqlitePool,
    app_version: &GitVersion,
) -> Result<VersionGateResult, Error> {
    match read_db_version_stamp(pool).await? {
        None => Ok(VersionGateResult::Unknown),
        Some(db_stamp) => match db_stamp.version.cmp(app_version) {
            std::cmp::Ordering::Greater => {
                if db_stamp.migration_hash == current_migration_hash() {
                    return Ok(VersionGateResult::CompatibleDowngrade);
                }

                Ok(VersionGateResult::Downgrade)
            }
            std::cmp::Ordering::Less => Ok(VersionGateResult::Upgrade),
            std::cmp::Ordering::Equal => {
                if db_stamp.migration_hash == current_migration_hash() {
                    Ok(VersionGateResult::Current)
                } else {
                    Ok(VersionGateResult::Downgrade)
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::runtime::RUNTIME;
    use crate::version::get_current_version;

    fn different_migration_hash() -> MigrationHash {
        blake3::hash(uuid::Uuid::now_v7().as_bytes())
            .to_hex()
            .to_string()
            .parse()
            .unwrap()
    }

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
        let migration_hash = current_migration_hash();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &version, &migration_hash)
                .await
                .unwrap();
            let read = read_db_version(db.pool()).await.unwrap();
            let stamp = read_db_version_stamp(db.pool()).await.unwrap().unwrap();
            assert_eq!(read, Some(version));
            assert_eq!(stamp.migration_hash, current_migration_hash());
        });
    }

    #[test]
    fn check_version_gate_detects_upgrade() {
        let db = setup_db();
        let older = GitVersion::from_str("v0.9.0").unwrap();
        let newer = GitVersion::from_str("v0.10.0").unwrap();
        let migration_hash = current_migration_hash();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &older, &migration_hash)
                .await
                .unwrap();
            let gate = check_version_gate(db.pool(), &newer).await.unwrap();
            assert_eq!(gate, VersionGateResult::Upgrade);
        });
    }

    #[test]
    fn check_version_gate_allows_compatible_downgrade() {
        let db = setup_db();
        let older = GitVersion::from_str("v0.9.0").unwrap();
        let newer = GitVersion::from_str("v0.10.0").unwrap();
        let migration_hash = current_migration_hash();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &newer, &migration_hash)
                .await
                .unwrap();
            let gate = check_version_gate(db.pool(), &older).await.unwrap();
            assert_eq!(gate, VersionGateResult::CompatibleDowngrade);
        });
    }

    #[test]
    fn check_version_gate_detects_incompatible_downgrade() {
        let db = setup_db();
        let older = GitVersion::from_str("v0.9.0").unwrap();
        let newer = GitVersion::from_str("v0.10.0").unwrap();
        let migration_hash = different_migration_hash();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &newer, &migration_hash)
                .await
                .unwrap();
            let gate = check_version_gate(db.pool(), &older).await.unwrap();
            assert_eq!(gate, VersionGateResult::Downgrade);
        });
    }

    #[test]
    fn check_version_gate_detects_current() {
        let db = setup_db();
        let version = GitVersion::from_str("v0.10.0").unwrap();
        let migration_hash = current_migration_hash();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &version, &migration_hash)
                .await
                .unwrap();
            let gate = check_version_gate(db.pool(), &version).await.unwrap();
            assert_eq!(gate, VersionGateResult::Current);
        });
    }

    #[test]
    fn check_version_gate_detects_downgrade_on_equal_version_different_hash() {
        let db = setup_db();
        let version = GitVersion::from_str("v0.10.0").unwrap();
        let migration_hash = different_migration_hash();

        RUNTIME.block_on(async {
            stamp_db_version(db.pool(), &version, &migration_hash)
                .await
                .unwrap();
            let gate = check_version_gate(db.pool(), &version).await.unwrap();
            assert_eq!(gate, VersionGateResult::Downgrade);
        });
    }

    #[test]
    fn check_version_gate_unknown_when_table_is_empty() {
        let db = setup_db();

        RUNTIME.block_on(async {
            sqlx::query!("DELETE FROM _cadmus_version")
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
