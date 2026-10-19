//! One-time data migrations for the dictionary index tables.

crate::migration!(
    /// No-op. The id stays so a library database that already recorded success
    /// does not run this migration again.
    ///
    /// The word index now lives in `dictionary.sqlite` and is rebuilt from the
    /// dictionary files. `018_drop_dictionary_index.sql` drops the old tables
    /// before runtime migrations run, so there is nothing left to stamp.
    "v1_dictionary_index_stamp",
    async fn backfill_index_stamps(_ctx: &mut MigrationContext<'_>) {
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
    async fn returns_ok_on_initialized_library_database() {
        let mut db = Database::new(":memory:").await.expect("in-memory db");
        db.init_for_test(0).await.expect("run migrations");
        let mut settings = Settings::default();
        let mut ctx = MigrationContext {
            pool: db.pool(),
            device: MigrationDevice {
                install_dir: PathBuf::from("/tmp"),
                data_dir: PathBuf::from("/tmp"),
                dpi: 300,
            },
            settings: &mut settings,
        };
        backfill_index_stamps(&mut ctx)
            .await
            .expect("stamp migration");
    }
}
