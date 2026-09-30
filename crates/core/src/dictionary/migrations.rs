//! One-time data migrations for the dictionary index tables.

use crate::helpers::{Fingerprint as _, Fp};

crate::migration!(
    /// Records the on-disk stamp of every already-indexed `.index` file.
    ///
    /// Schema migration `014_dictionary_index_stamp.sql` added `index_mtime` and
    /// `index_size` so a dictionary reload can skip hashing an index that has
    /// not changed. Existing rows have both columns NULL, and a stamp with no
    /// mtime never matches a current one, so without this backfill every reload
    /// would rehash every installed dictionary until the indexer happened to
    /// rewrite those rows.
    ///
    /// A stamp is only safe to record when the file still matches the fingerprint
    /// stored for it: otherwise a file swapped before the upgrade would keep a
    /// stale index stamped as current. The backfill therefore re-hashes each file
    /// once and leaves the stamp unset when the hash no longer matches.
    "v1_dictionary_index_stamp",
    async fn backfill_index_stamps(ctx: &mut MigrationContext<'_>) {
        let rows = sqlx::query!(
            "SELECT dict_id, dict_path, fingerprint AS \"fingerprint: Fp\" FROM dictionary_index_meta"
        )
        .fetch_all(ctx.pool)
        .await?;

        let mut stamped = 0u32;
        for row in rows {
            let dict_id = row.dict_id;
            let path = row.dict_path;
            let stored = row.fingerprint;

            let current = match std::path::Path::new(&path).fingerprint().await {
                Ok(fingerprint) => fingerprint,
                Err(_) => {
                    tracing::warn!(
                        path = %path,
                        "indexed dictionary is not on disk, leaving its stamp unset"
                    );
                    continue;
                }
            };

            if current != stored {
                tracing::warn!(
                    path = %path,
                    stored = %stored,
                    current = %current,
                    "dictionary file no longer matches its stored fingerprint, leaving its stamp unset"
                );
                continue;
            }

            let Ok(stamp) = std::path::Path::new(&path).stamp() else {
                tracing::warn!(
                    path = %path,
                    "could not stat dictionary index, leaving its stamp unset"
                );
                continue;
            };

            sqlx::query!(
                r#"UPDATE dictionary_index_meta
                   SET index_mtime = ?, index_size = ?
                   WHERE dict_id = ?"#,
                stamp.mtime.map(i64::from),
                i64::from(stamp.size),
                dict_id,
            )
            .execute(ctx.pool)
            .await?;

            stamped += 1;
        }

        tracing::info!(stamped, "recorded dictionary index stamps");
        Ok(())
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::migrations::{MigrationContext, MigrationDevice};
    use crate::settings::Settings;
    use std::path::PathBuf;

    #[tokio::test]
    async fn stamps_only_files_that_still_match_their_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let matching = dir.path().join("matching.index");
        let swapped = dir.path().join("swapped.index");
        tokio::fs::write(&matching, b"alpha\tA\tA\n")
            .await
            .expect("write matching");
        tokio::fs::write(&swapped, b"beta\tB\tB\n")
            .await
            .expect("write swapped");

        let matching_fp = matching.fingerprint().await.expect("fingerprint matching");
        let stale_fp = swapped.fingerprint().await.expect("fingerprint swapped");
        tokio::fs::write(&swapped, b"gamma\tC\tC\n")
            .await
            .expect("rewrite swapped after fingerprint");

        let mut db = Database::new(":memory:").await.expect("in-memory db");
        db.init_for_test(0).await.expect("run migrations");
        let pool = db.pool();

        let matching_id = sqlx::query_scalar!(
            "INSERT INTO dictionary_index_meta (fingerprint, dict_path, total_lines, indexed_lines, completed) VALUES (?, ?, 1, 1, 1) RETURNING dict_id",
            matching_fp.to_string(),
            matching.display().to_string(),
        )
        .fetch_one(pool)
        .await
        .expect("insert matching row");

        let swapped_id = sqlx::query_scalar!(
            "INSERT INTO dictionary_index_meta (fingerprint, dict_path, total_lines, indexed_lines, completed) VALUES (?, ?, 1, 1, 1) RETURNING dict_id",
            stale_fp.to_string(),
            swapped.display().to_string(),
        )
        .fetch_one(pool)
        .await
        .expect("insert swapped row");

        let mut settings = Settings::default();
        let mut ctx = MigrationContext {
            pool,
            device: MigrationDevice {
                install_dir: PathBuf::from("/tmp"),
                data_dir: PathBuf::from("/tmp"),
                dpi: 300,
            },
            settings: &mut settings,
        };
        backfill_index_stamps(&mut ctx).await.expect("backfill");

        let matching_row = sqlx::query!(
            "SELECT dict_path, index_mtime, index_size FROM dictionary_index_meta WHERE dict_id = ?",
            matching_id,
        )
        .fetch_one(pool)
        .await
        .expect("matching row");
        assert!(
            matching_row.index_size.is_some(),
            "a file matching its stored fingerprint must be stamped"
        );

        let swapped_row = sqlx::query!(
            "SELECT dict_path, index_mtime, index_size FROM dictionary_index_meta WHERE dict_id = ?",
            swapped_id,
        )
        .fetch_one(pool)
        .await
        .expect("swapped row");
        assert!(
            swapped_row.index_size.is_none(),
            "a swapped file must be left unstamped so the next reload rehashes"
        );
    }
}
