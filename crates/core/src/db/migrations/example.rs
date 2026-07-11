//! Example migration included only in test builds.
//!
//! Demonstrates the minimal migration shape.
crate::migration!(
    /// A minimal example migration that prints to stdout.
    ///
    /// In a real migration, you would:
    /// 1. Use `ctx.pool`, `ctx.device`, and `ctx.settings` as needed
    /// 2. Execute SQL queries using `sqlx::query!` or `sqlx::query_scalar!`
    /// 3. Return `Ok(())` on success or propagate errors with `?`
    ///
    /// # Example
    ///
    /// ```rust
    /// mod my_migrations {
    ///     use cadmus_core::db::migrations::MigrationContext;
    ///
    ///     cadmus_core::migration!(
    ///         /// Backfills metadata from legacy storage.
    ///         "v2_backfill_metadata",
    ///         async fn backfill_metadata(ctx: &mut MigrationContext<'_>) {
    ///             // sqlx::query!(...).execute(ctx.pool).await?;
    ///             Ok(())
    ///         }
    ///     );
    /// }
    /// ```
    "example_hello_world",
    async fn hello_world(_ctx: &mut crate::db::migrations::MigrationContext<'_>) {
        println!("hello world");
        Ok(())
    }
);
