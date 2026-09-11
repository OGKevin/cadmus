use crate::db::types::UnixTimestamp;
use crate::helpers::Fp;
use crate::runtime::RUNTIME;
use crate::view::reader::statistics::models::{ReadingEventRow, ReadingEventType};
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
    pub fn record_event(&self, fp: Fp, event_type: ReadingEventType) -> Result<(), Error> {
        tracing::debug!(fp = %fp, event_type = %event_type, "recording reading event");

        RUNTIME.block_on(async {
            let now = UnixTimestamp::now();

            tracing::debug!(fp = %fp, event_type = %event_type, ts = %now, "inserting event");
            sqlx::query!(
                "INSERT INTO reading_events (book_fingerprint, timestamp, event_type) VALUES (?, ?, ?)",
                fp,
                now,
                event_type,
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        })
    }

    /// Get the last event for a book
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fp)))]
    pub fn get_last_event(&self, fp: Fp) -> Result<Option<ReadingEventRow>, Error> {
        Ok(RUNTIME.block_on(async {
            sqlx::query_as!(
                ReadingEventRow,
                r#"
                SELECT
                    id,
                    book_fingerprint as "book_fingerprint: Fp",
                    timestamp        as "timestamp: UnixTimestamp",
                    event_type       as "event_type: ReadingEventType"
                FROM reading_events
                WHERE book_fingerprint = ?
                ORDER BY timestamp DESC, id DESC
                LIMIT 1
                "#,
                fp
            )
            .fetch_optional(&self.pool)
            .await
        })?)
    }

    #[cfg(test)]
    /// List all reading events for a book (for debugging/testing)
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fp)))]
    pub fn list_events(&self, fp: Fp) -> Result<Vec<ReadingEventRow>, Error> {
        Ok(RUNTIME.block_on(async {
            sqlx::query_as!(
                ReadingEventRow,
                r#"
                SELECT
                    id,
                    book_fingerprint as "book_fingerprint: Fp",
                    timestamp        as "timestamp: UnixTimestamp",
                    event_type       as "event_type: ReadingEventType"
                FROM reading_events
                WHERE book_fingerprint = ?
                ORDER BY timestamp ASC, id ASC
                "#,
                fp
            )
            .fetch_all(&self.pool)
            .await
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::helpers::Fp;

    /// Helper to insert a minimal test book into the database.
    /// Required because reading_events has a foreign key constraint on book_fingerprint.
    fn insert_test_book(pool: &sqlx::sqlite::SqlitePool, fp: Fp) {
        let now = UnixTimestamp::now();
        let fp_str = fp.to_string();

        RUNTIME.block_on(async {
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
        });
    }

    #[test]
    fn test_record_single_event() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(1);

        insert_test_book(context.database.pool(), fp);
        db.record_event(fp, ReadingEventType::BookOpened)
            .expect("Failed to record event");

        let events = db.list_events(fp).expect("Failed to list events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].book_fingerprint, fp);
        assert_eq!(events[0].event_type, ReadingEventType::BookOpened);
    }

    #[test]
    fn test_record_multiple_event_types() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(2);

        insert_test_book(context.database.pool(), fp);
        db.record_event(fp, ReadingEventType::BookOpened).unwrap();
        db.record_event(fp, ReadingEventType::PageTurn).unwrap();
        db.record_event(fp, ReadingEventType::BookClosed).unwrap();

        let events = db.list_events(fp).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_record_multiple_books() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp1 = Fp::from_u64(100);
        let fp2 = Fp::from_u64(200);

        insert_test_book(context.database.pool(), fp1);
        insert_test_book(context.database.pool(), fp2);
        db.record_event(fp1, ReadingEventType::BookOpened).unwrap();
        db.record_event(fp2, ReadingEventType::BookOpened).unwrap();

        let events1 = db.list_events(fp1).unwrap();
        let events2 = db.list_events(fp2).unwrap();

        assert_eq!(events1.len(), 1);
        assert_eq!(events2.len(), 1);
        assert_eq!(events1[0].book_fingerprint, fp1);
        assert_eq!(events2[0].book_fingerprint, fp2);
    }

    #[test]
    fn test_get_last_event_returns_latest() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(3);

        insert_test_book(context.database.pool(), fp);
        db.record_event(fp, ReadingEventType::BookOpened).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        db.record_event(fp, ReadingEventType::PageTurn).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        db.record_event(fp, ReadingEventType::BookClosed).unwrap();

        let last = db.get_last_event(fp).unwrap();
        assert!(last.is_some());
        assert_eq!(last.unwrap().event_type, ReadingEventType::BookClosed);
    }

    #[test]
    fn test_get_last_event_empty() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(4);

        let last = db.get_last_event(fp).unwrap();
        assert!(last.is_none());
    }

    #[test]
    fn test_list_events_returns_all_for_book() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(6);

        insert_test_book(context.database.pool(), fp);
        for _ in 0..5 {
            db.record_event(fp, ReadingEventType::PageTurn).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let events = db.list_events(fp).unwrap();
        assert_eq!(events.len(), 5);
    }

    #[test]
    fn test_list_events_orders_by_timestamp_asc() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(8);

        insert_test_book(context.database.pool(), fp);
        db.record_event(fp, ReadingEventType::BookOpened).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        db.record_event(fp, ReadingEventType::PageTurn).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        db.record_event(fp, ReadingEventType::BookClosed).unwrap();

        let events = db.list_events(fp).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_type, ReadingEventType::BookOpened);
        assert_eq!(events[1].event_type, ReadingEventType::PageTurn);
        assert_eq!(events[2].event_type, ReadingEventType::BookClosed);
    }

    #[test]
    fn test_list_events_nonexistent_book() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(u64::MAX);

        let events = db.list_events(fp).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_get_last_event_nonexistent_book() {
        let context = create_test_context();
        let db = StatisticsDb::new(context.database.pool().clone());
        let fp = Fp::from_u64(u64::MAX);

        let last = db.get_last_event(fp).unwrap();
        assert!(last.is_none());
    }
}
