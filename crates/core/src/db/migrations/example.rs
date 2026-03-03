//! Example migration included only in test builds.
//!
//! Demonstrates the minimal migration shape.
crate::migration!(
    /// A minimal example migration that prints to stdout.
    ///
    /// In a real migration, you would:
    /// 1. Call [`pool_from_token`] to get the database connection pool
    /// 2. Execute SQL queries using `sqlx::query!` or `sqlx::query_scalar!`
    /// 3. Return `Ok(())` on success or propagate errors with `?`
    ///
    /// # Example
    ///
    /// ```rust
    /// mod my_migrations {
    ///     use cadmus_core::db::migrations::{MigrationToken, pool_from_token};
    ///
    ///     cadmus_core::migration!(
    ///         /// Backfills metadata from legacy storage.
    ///         "v2_backfill_metadata",
    ///         async fn backfill_metadata(token: &MigrationToken) {
    ///             let _pool = pool_from_token(token);
    ///             // sqlx::query!(...).execute(_pool).await?;
    ///             Ok(())
    ///         }
    ///     );
    /// }
    /// ```
    "example_hello_world",
    async fn hello_world(_token: &MigrationToken) {
        println!("hello world");
        Ok(())
    }
);
