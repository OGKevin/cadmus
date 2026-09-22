use crate::db::types::UnixTimestamp;
use crate::helpers::Fp;
use serde::{Deserialize, Serialize};
use sqlx::{
    FromRow, Sqlite,
    encode::IsNull,
    error::BoxDynError,
    sqlite::{SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef},
};
use std::fmt;
use std::str::FromStr;

/// Event types for reading time tracking
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ReadingEventType {
    /// Event recorded when a book is opened
    BookOpened,
    /// Event recorded when a book is closed
    BookClosed,
    /// Event recorded when a page is turned
    PageTurn,
}

/// Error returned when a string does not match any known reading event type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown reading event type: {0}")]
pub struct UnknownReadingEventType(
    /// Event type string that could not be parsed.
    pub String,
);

impl ReadingEventType {
    /// Canonical wire text stored in SQLite.
    pub fn as_str(self) -> &'static str {
        match self {
            ReadingEventType::BookOpened => "BookOpened",
            ReadingEventType::BookClosed => "BookClosed",
            ReadingEventType::PageTurn => "PageTurn",
        }
    }

    /// Returns all known reading event types.
    pub fn all() -> &'static [ReadingEventType] {
        &[
            ReadingEventType::BookOpened,
            ReadingEventType::BookClosed,
            ReadingEventType::PageTurn,
        ]
    }
}

impl FromStr for ReadingEventType {
    type Err = UnknownReadingEventType;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BookOpened" => Ok(ReadingEventType::BookOpened),
            "BookClosed" => Ok(ReadingEventType::BookClosed),
            "PageTurn" => Ok(ReadingEventType::PageTurn),
            _ => Err(UnknownReadingEventType(s.to_owned())),
        }
    }
}

impl fmt::Display for ReadingEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl sqlx::Type<Sqlite> for ReadingEventType {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for ReadingEventType {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        self.as_str().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for ReadingEventType {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        s.parse::<Self>().map_err(Into::into)
    }
}

/// Database row for the reading_events table
#[derive(Debug, Clone, FromRow)]
pub struct ReadingEventRow {
    pub id: i64,
    pub book_fingerprint: Fp,
    pub timestamp: UnixTimestamp,
    pub event_type: ReadingEventType,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reading_event_type_round_trip_via_from_str() {
        for variant in ReadingEventType::all() {
            let displayed = format!("{}", variant);
            let parsed = displayed.parse::<ReadingEventType>().ok();
            assert_eq!(
                parsed,
                Some(*variant),
                "round trip failed for {:?}",
                variant
            );
        }
    }

    #[test]
    fn test_reading_event_type_serde_roundtrip() {
        use serde_json;

        for variant in ReadingEventType::all() {
            let serialized = serde_json::to_string(variant).unwrap();
            let deserialized: ReadingEventType = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, *variant);
            assert_eq!(serialized, format!("\"{}\"", variant));
        }
    }

    #[test]
    fn test_reading_event_type_from_str_invalid() {
        let result = "InvalidEventType".parse::<ReadingEventType>();
        assert!(result.is_err());
    }

    #[test]
    fn test_all_variants_are_distinct() {
        assert_ne!(ReadingEventType::BookOpened, ReadingEventType::BookClosed);
        assert_ne!(ReadingEventType::BookOpened, ReadingEventType::PageTurn);
        assert_ne!(ReadingEventType::BookClosed, ReadingEventType::PageTurn);
    }

    #[test]
    fn test_reading_event_row_structure() {
        let now = UnixTimestamp::now();
        let row = ReadingEventRow {
            id: 42,
            book_fingerprint: Fp::from_u64(0x12345678),
            timestamp: now,
            event_type: ReadingEventType::BookOpened,
        };

        assert_eq!(row.id, 42);
        assert_eq!(row.event_type, ReadingEventType::BookOpened);
    }
}
