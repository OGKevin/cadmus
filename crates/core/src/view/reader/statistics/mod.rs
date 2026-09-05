pub mod db;
pub mod models;

use crate::db::Database;
use crate::helpers::Fp;
use anyhow::Error;

use self::db::StatisticsDb;
use self::models::ReadingEventType;

pub struct Statistics {
    db: StatisticsDb,
}

impl Statistics {
    pub fn new(database: &Database) -> Self {
        Self {
            db: StatisticsDb::new(database.pool().clone()),
        }
    }

    /// Record a reading event for a book.
    ///
    /// This records discrete events (BookOpened, BookClosed, PageTurn) to the library database.
    ///
    /// # Arguments
    /// * `fp` - Fingerprint of the book
    /// * `event_type` - Type of reading event
    ///
    /// # Returns
    /// * `Ok(())` - Event recorded successfully
    /// * `Err(Error)` - Failed to record event
    pub fn record_event(&self, fp: Fp, event_type: ReadingEventType) -> Result<(), Error> {
        self.db.record_event(fp, event_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::db::types::UnixTimestamp;
    use crate::helpers::Fp;
    use crate::runtime::RUNTIME;

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
    fn test_record_event_delegates() {
        let context = create_test_context();
        let stats = Statistics::new(&context.database);
        let fp = Fp::from_u64(11);

        insert_test_book(context.database.pool(), fp);
        stats
            .record_event(fp, ReadingEventType::BookOpened)
            .unwrap();
        stats.record_event(fp, ReadingEventType::PageTurn).unwrap();

        let events = stats.db.list_events(fp).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_typical_reader_session() {
        let context = create_test_context();
        let stats = Statistics::new(&context.database);
        let fp = Fp::from_u64(12);

        insert_test_book(context.database.pool(), fp);
        stats
            .record_event(fp, ReadingEventType::BookOpened)
            .unwrap();
        for _ in 0..10 {
            stats.record_event(fp, ReadingEventType::PageTurn).unwrap();
        }
        stats
            .record_event(fp, ReadingEventType::BookClosed)
            .unwrap();

        let events = stats.db.list_events(fp).unwrap();
        assert_eq!(events.len(), 12);
        assert_eq!(events[0].event_type, ReadingEventType::BookOpened);
        assert_eq!(events[11].event_type, ReadingEventType::BookClosed);
    }
}
