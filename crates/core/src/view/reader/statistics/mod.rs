pub mod db;
pub mod models;

use crate::db::Database;
use crate::helpers::Fp;
use anyhow::Error;

use self::db::StatisticsDb;
use self::models::ReadingEventType;

pub(crate) fn should_record_page_turn(current_page: usize, target_page: usize) -> bool {
    current_page != target_page
}

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
    pub async fn record_event(&self, fp: Fp, event_type: ReadingEventType) -> Result<(), Error> {
        self.db.record_event(fp, event_type).await
    }

    /// Records an event from synchronous UI code without blocking the runtime.
    pub(crate) fn spawn_record_event(&self, fp: Fp, event_type: ReadingEventType) {
        let db = self.db.clone();
        crate::runtime::current_handle().spawn(async move {
            if let Err(e) = db.record_event(fp, event_type).await {
                tracing::error!(
                    error = %e,
                    fp = %fp,
                    event = %event_type,
                    "failed to log reading event"
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::db::types::UnixTimestamp;
    use crate::helpers::Fp;

    /// Helper to insert a minimal test book into the database.
    /// Required because reading_events has a foreign key constraint on book_fingerprint.
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

    #[test]
    fn should_record_page_turn_only_when_page_changes() {
        assert!(should_record_page_turn(0, 1));
        assert!(should_record_page_turn(4, 2));
        assert!(!should_record_page_turn(3, 3));
    }

    #[tokio::test]
    async fn test_record_event_delegates() {
        let context = create_test_context().await;
        let stats = Statistics::new(&context.database);
        let fp = Fp::from_u64(11);

        insert_test_book(context.database.pool(), fp).await;
        stats
            .record_event(fp, ReadingEventType::BookOpened)
            .await
            .unwrap();
        stats
            .record_event(fp, ReadingEventType::PageTurn)
            .await
            .unwrap();

        let events = stats.db.list_events(fp).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_typical_reader_session() {
        let context = create_test_context().await;
        let stats = Statistics::new(&context.database);
        let fp = Fp::from_u64(12);

        insert_test_book(context.database.pool(), fp).await;
        stats
            .record_event(fp, ReadingEventType::BookOpened)
            .await
            .unwrap();
        for _ in 0..10 {
            stats
                .record_event(fp, ReadingEventType::PageTurn)
                .await
                .unwrap();
        }
        stats
            .record_event(fp, ReadingEventType::BookClosed)
            .await
            .unwrap();

        let events = stats.db.list_events(fp).await.unwrap();
        assert_eq!(events.len(), 12);
        assert_eq!(events[0].event_type, ReadingEventType::BookOpened);
        assert_eq!(events[11].event_type, ReadingEventType::BookClosed);
    }
}
