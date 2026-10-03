use crate::db::types::UnixTimestamp;
use crate::helpers::Fp;
use crate::view::reader::statistics::models::ReadingEventType;

#[cfg(test)]
use crate::library::BookId;
#[cfg(test)]
use crate::view::reader::statistics::models::ReadingEventRow;
use anyhow::Error;
use sqlx::sqlite::SqlitePool;

/// Database handle for statistics operations
#[derive(Clone)]
pub struct StatisticsDb {
    pool: SqlitePool,
}

impl StatisticsDb {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Record a reading event
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fp, event_type)))]
    pub async fn record_event(&self, fp: Fp, event_type: ReadingEventType) -> Result<(), Error> {
        tracing::debug!(fp = %fp, event_type = %event_type, "recording reading event");

        let now = UnixTimestamp::now();

        tracing::debug!(fp = %fp, event_type = %event_type, ts = %now, "inserting event");
        let result = sqlx::query!(
            r#"
            INSERT INTO reading_events (book_id, timestamp, event_type)
            SELECT book_id, ?, ? FROM book_keys WHERE fingerprint = ?
            "#,
            now,
            event_type,
            fp,
        )
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            anyhow::bail!("no book_keys row for fingerprint {fp}");
        }

        Ok(())
    }

    /// Get the last event for a book
    #[cfg(test)]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fp)))]
    pub async fn get_last_event(&self, fp: Fp) -> Result<Option<ReadingEventRow>, Error> {
        Ok(sqlx::query_as!(
            ReadingEventRow,
            r#"
            SELECT
                re.id,
                re.book_id       as "book_id: BookId",
                bk.fingerprint   as "book_fingerprint: Fp",
                re.timestamp     as "timestamp: UnixTimestamp",
                re.event_type    as "event_type: ReadingEventType"
            FROM reading_events re
            INNER JOIN book_keys bk ON re.book_id = bk.book_id
            WHERE bk.fingerprint = ?
            ORDER BY re.timestamp DESC, re.id DESC
            LIMIT 1
            "#,
            fp
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    #[cfg(test)]
    /// List all reading events for a book (for debugging/testing)
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fp)))]
    pub async fn list_events(&self, fp: Fp) -> Result<Vec<ReadingEventRow>, Error> {
        Ok(sqlx::query_as!(
            ReadingEventRow,
            r#"
            SELECT
                re.id,
                re.book_id       as "book_id: BookId",
                bk.fingerprint   as "book_fingerprint: Fp",
                re.timestamp     as "timestamp: UnixTimestamp",
                re.event_type    as "event_type: ReadingEventType"
            FROM reading_events re
            INNER JOIN book_keys bk ON re.book_id = bk.book_id
            WHERE bk.fingerprint = ?
            ORDER BY re.timestamp ASC, re.id ASC
            "#,
            fp
        )
        .fetch_all(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::helpers::Fp;

    /// Helper to insert a minimal test book into the database.
    /// Inserts into `books`; the `book_keys` trigger supplies the integer key for events.
    async fn insert_test_book(pool: &sqlx::sqlite::SqlitePool, fp: Fp) {
        let now = UnixTimestamp::now();
        let fp_str = fp.to_string();

        sqlx::query!(
            r#"
            INSERT INTO books (fingerprint, file_kind, file_size, added_at)
            VALUES (?, ?, ?, ?)
            "#,
            fp_str,
            "pdf",
            1024i64,
            now,
        )
        .execute(pool)
        .await
        .expect("failed to insert test book");
    }

    #[tokio::test]
    async fn test_record_single_event() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(1);

        insert_test_book(context.database.pool(), fp).await;
        let before = UnixTimestamp::now();
        db.record_event(fp, ReadingEventType::BookOpened)
            .await
            .expect("Failed to record event");

        let events = db.list_events(fp).await.expect("Failed to list events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].book_fingerprint, fp);
        assert_eq!(events[0].event_type, ReadingEventType::BookOpened);
        assert!(events[0].book_id.into_i64() > 0);
        assert!(events[0].timestamp >= before);
    }

    #[tokio::test]
    async fn test_record_multiple_event_types() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(2);

        insert_test_book(context.database.pool(), fp).await;
        db.record_event(fp, ReadingEventType::BookOpened)
            .await
            .unwrap();
        db.record_event(fp, ReadingEventType::PageTurn)
            .await
            .unwrap();
        db.record_event(fp, ReadingEventType::BookClosed)
            .await
            .unwrap();

        let events = db.list_events(fp).await.unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn test_record_multiple_books() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp1 = Fp::from_u64(100);
        let fp2 = Fp::from_u64(200);

        insert_test_book(context.database.pool(), fp1).await;
        insert_test_book(context.database.pool(), fp2).await;
        db.record_event(fp1, ReadingEventType::BookOpened)
            .await
            .unwrap();
        db.record_event(fp2, ReadingEventType::BookOpened)
            .await
            .unwrap();

        let events1 = db.list_events(fp1).await.unwrap();
        let events2 = db.list_events(fp2).await.unwrap();

        assert_eq!(events1.len(), 1);
        assert_eq!(events2.len(), 1);
        assert_eq!(events1[0].book_fingerprint, fp1);
        assert_eq!(events2[0].book_fingerprint, fp2);
    }

    #[tokio::test]
    async fn test_get_last_event_returns_latest() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(3);

        insert_test_book(context.database.pool(), fp).await;
        db.record_event(fp, ReadingEventType::BookOpened)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        db.record_event(fp, ReadingEventType::PageTurn)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        db.record_event(fp, ReadingEventType::BookClosed)
            .await
            .unwrap();

        let last = db.get_last_event(fp).await.unwrap();
        assert!(last.is_some());
        assert_eq!(last.unwrap().event_type, ReadingEventType::BookClosed);
    }

    #[tokio::test]
    async fn test_get_last_event_empty() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(4);

        let last = db.get_last_event(fp).await.unwrap();
        assert!(last.is_none());
    }

    #[tokio::test]
    async fn test_list_events_returns_all_for_book() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(6);

        insert_test_book(context.database.pool(), fp).await;
        for _ in 0..5 {
            db.record_event(fp, ReadingEventType::PageTurn)
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = db.list_events(fp).await.unwrap();
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_list_events_orders_by_timestamp_asc() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(8);

        insert_test_book(context.database.pool(), fp).await;
        db.record_event(fp, ReadingEventType::BookOpened)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        db.record_event(fp, ReadingEventType::PageTurn)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        db.record_event(fp, ReadingEventType::BookClosed)
            .await
            .unwrap();

        let events = db.list_events(fp).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, ReadingEventType::BookOpened);
        assert_eq!(events[1].event_type, ReadingEventType::PageTurn);
        assert_eq!(events[2].event_type, ReadingEventType::BookClosed);
    }

    #[tokio::test]
    async fn test_list_events_nonexistent_book() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(u64::MAX);

        let events = db.list_events(fp).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_get_last_event_nonexistent_book() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(u64::MAX);

        let last = db.get_last_event(fp).await.unwrap();
        assert!(last.is_none());
    }

    async fn book_key_id(pool: &sqlx::sqlite::SqlitePool, fp: Fp) -> BookId {
        sqlx::query_scalar!(
            r#"SELECT book_id as "book_id: BookId" FROM book_keys WHERE fingerprint = ?"#,
            fp,
        )
        .fetch_one(pool)
        .await
        .expect("book_keys row missing")
    }

    /// Exercises `book_keys_after_insert`: only `books` is inserted; the trigger must
    /// create the matching `book_keys` row (migration backfill does not apply to new books).
    #[tokio::test]
    async fn test_book_keys_after_insert_trigger() {
        let context = create_test_context().await;
        let pool = context.database.pool();
        let db = StatisticsDb::new(pool.clone());
        let fp = Fp::from_u64(50);

        let keys_before = sqlx::query_scalar!(
            "SELECT COUNT(*) AS \"count: i64\" FROM book_keys WHERE fingerprint = ?",
            fp,
        )
        .fetch_one(pool)
        .await
        .expect("count book_keys");
        assert_eq!(keys_before, 0);

        insert_test_book(pool, fp).await;

        let keys_after = sqlx::query_scalar!(
            "SELECT COUNT(*) AS \"count: i64\" FROM book_keys WHERE fingerprint = ?",
            fp,
        )
        .fetch_one(pool)
        .await
        .expect("count book_keys");
        assert_eq!(keys_after, 1);

        let book_id = book_key_id(pool, fp).await;
        assert!(book_id.into_i64() > 0);

        db.record_event(fp, ReadingEventType::BookOpened)
            .await
            .expect("record_event should succeed once trigger populated book_keys");
    }

    #[tokio::test]
    async fn test_record_event_missing_book() {
        let context = create_test_context().await;
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(51);

        let err = db
            .record_event(fp, ReadingEventType::BookOpened)
            .await
            .expect_err("expected error");
        assert!(err.to_string().contains("no book_keys row"));
    }

    #[tokio::test]
    async fn test_delete_book_cascades_reading_events() {
        let context = create_test_context().await;
        let pool = context.database.pool();
        let db = StatisticsDb::new(pool.clone());
        let fp = Fp::from_u64(52);

        insert_test_book(pool, fp).await;
        db.record_event(fp, ReadingEventType::BookOpened)
            .await
            .unwrap();

        sqlx::query!("DELETE FROM books WHERE fingerprint = ?", fp)
            .execute(pool)
            .await
            .unwrap();

        let events = db.list_events(fp).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_book_key_id_not_reused_after_delete() {
        let context = create_test_context().await;
        let pool = context.database.pool();
        let fp1 = Fp::from_u64(53);
        let fp2 = Fp::from_u64(54);

        insert_test_book(pool, fp1).await;
        let first_id = book_key_id(pool, fp1).await;

        sqlx::query!("DELETE FROM books WHERE fingerprint = ?", fp1)
            .execute(pool)
            .await
            .unwrap();

        insert_test_book(pool, fp2).await;
        let second_id = book_key_id(pool, fp2).await;

        assert!(second_id.into_i64() > first_id.into_i64());
    }
}
