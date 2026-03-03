//! Example migration included only in test builds.
//!
//! Demonstrates the minimal migration shape.
crate::migration!(
    /// A minimal example migration that prints to stdout.
    ///
    /// In a real migration, you would:
    /// 1. Use the `pool` parameter directly
    /// 2. Execute SQL queries using `sqlx::query!` or `sqlx::query_scalar!`
    /// 3. Return `Ok(())` on success or propagate errors with `?`
    ///
    /// # Example
    ///
    /// ```rust
    /// mod my_migrations {
    ///     use sqlx::SqlitePool;
    ///
    ///     cadmus_core::migration!(
    ///         /// Backfills metadata from legacy storage.
    ///         "v2_backfill_metadata",
    ///         async fn backfill_metadata(pool: &SqlitePool) {
    ///             // sqlx::query!(...).execute(pool).await?;
    ///             Ok(())
    ///         }
    ///     );
    /// }
    /// ```
    "example_hello_world",
    async fn hello_world(_pool: &sqlx::SqlitePool) {
        println!("hello world");
        Ok(())
    }
);
