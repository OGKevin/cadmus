use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use uuid::Uuid;

/// A Unix epoch timestamp (seconds since 1970-01-01 00:00:00 UTC) as stored in
/// SQLite `INTEGER` columns.
///
/// Implementing `sqlx::Type`, `sqlx::Encode`, and `sqlx::Decode` lets SQLx
/// macros treat this type as an `INTEGER` column, so `query_as!` / `query!`
/// can decode rows directly into `UnixTimestamp` fields without manual `i64`
/// intermediaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnixTimestamp(i64);

impl Serialize for UnixTimestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(self.0)
    }
}

impl<'de> Deserialize<'de> for UnixTimestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        i64::deserialize(deserializer).map(Self)
    }
}

impl UnixTimestamp {
    /// Returns the current UTC time as a `UnixTimestamp`.
    pub fn now() -> Self {
        Self(Utc::now().timestamp())
    }
}

impl std::fmt::Display for UnixTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dt = NaiveDateTime::from(*self);

        write!(f, "{} UTC ({})", dt, self.0)
    }
}

impl From<i64> for UnixTimestamp {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<UnixTimestamp> for i64 {
    fn from(value: UnixTimestamp) -> Self {
        value.0
    }
}

impl From<NaiveDate> for UnixTimestamp {
    fn from(value: NaiveDate) -> Self {
        Self(
            value
                .and_hms_opt(0, 0, 0)
                .expect("midnight should always be valid")
                .and_utc()
                .timestamp(),
        )
    }
}

impl From<UnixTimestamp> for NaiveDate {
    fn from(value: UnixTimestamp) -> Self {
        NaiveDateTime::from(value).date()
    }
}

impl From<NaiveDateTime> for UnixTimestamp {
    fn from(dt: NaiveDateTime) -> Self {
        Self(dt.and_utc().timestamp())
    }
}

/// Converts to `NaiveDateTime`, falling back to the Unix epoch for timestamps
/// outside the valid range.
impl From<UnixTimestamp> for NaiveDateTime {
    fn from(ts: UnixTimestamp) -> Self {
        DateTime::<Utc>::from(ts).naive_utc()
    }
}

/// Converts to `DateTime<Utc>`, falling back to the Unix epoch for timestamps
/// outside the valid range.
impl From<UnixTimestamp> for DateTime<Utc> {
    fn from(ts: UnixTimestamp) -> Self {
        DateTime::from_timestamp(ts.0, 0).unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap())
    }
}

impl sqlx::Type<Sqlite> for UnixTimestamp {
    fn type_info() -> SqliteTypeInfo {
        <i64 as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <i64 as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for UnixTimestamp {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        self.0.encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for UnixTimestamp {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let ts = <i64 as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(Self(ts))
    }
}

/// A file size in bytes as stored in SQLite `INTEGER` columns.
///
/// Implementing `sqlx::Type`, `sqlx::Encode`, and `sqlx::Decode` lets SQLx
/// macros treat this type as an `INTEGER` column, so `query!` can decode rows
/// directly into `FileSize` fields without manual `i64` intermediaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileSize(i64);

impl From<FileSize> for i64 {
    fn from(value: FileSize) -> Self {
        value.0
    }
}

impl From<i64> for FileSize {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl sqlx::Type<Sqlite> for FileSize {
    fn type_info() -> SqliteTypeInfo {
        <i64 as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <i64 as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for FileSize {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        self.0.encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for FileSize {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let n = <i64 as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(Self(n))
    }
}

/// A UUID version 7 identifier as stored in SQLite `TEXT` columns.
///
/// UUID7 is time-ordered, so `ORDER BY id ASC` on a `TEXT` column that holds
/// hyphenated UUID7 strings preserves insertion order without a separate
/// sequence column.
///
/// Implementing `sqlx::Type`, `sqlx::Encode`, and `sqlx::Decode` lets SQLx
/// macros treat this type as a `TEXT` column, so `query_as!` / `query!`
/// can decode rows directly into `Uuid7` fields without manual `String`
/// intermediaries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Uuid7(Uuid);

impl Uuid7 {
    /// Generates a new UUID version 7 using the current system time.
    pub fn now() -> Self {
        use uuid::timestamp::{Timestamp, context::NoContext};
        Self(Uuid::new_v7(Timestamp::now(NoContext)))
    }
}

impl std::fmt::Display for Uuid7 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl std::str::FromStr for Uuid7 {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid7> for String {
    fn from(id: Uuid7) -> Self {
        id.to_string()
    }
}

/// Panics if `s` is not a valid UUID string. Values stored in the database
/// are always written by `Uuid7::now()`, so this is only reachable on
/// database corruption.
impl From<String> for Uuid7 {
    fn from(s: String) -> Self {
        s.parse().expect("invalid UUID stored in database")
    }
}

impl sqlx::Type<Sqlite> for Uuid7 {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for Uuid7 {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        self.to_string().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Uuid7 {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        s.parse().map_err(Into::into)
    }
}

/// A nullable UUID version 7 identifier as stored in SQLite `TEXT` columns.
///
/// `query_as!` requires `From<Option<String>>` for nullable `TEXT` fields, but
/// the orphan rule prevents implementing that directly on `Option<Uuid7>`.
/// This newtype wraps `Option<Uuid7>` and provides the required `From` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionalUuid7(pub Option<Uuid7>);

impl From<Option<String>> for OptionalUuid7 {
    fn from(s: Option<String>) -> Self {
        Self(s.map(|s| s.parse().expect("invalid UUID stored in database")))
    }
}

impl From<OptionalUuid7> for Option<Uuid7> {
    fn from(o: OptionalUuid7) -> Self {
        o.0
    }
}

impl sqlx::Type<Sqlite> for OptionalUuid7 {
    fn type_info() -> SqliteTypeInfo {
        <Option<String> as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <Option<String> as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for OptionalUuid7 {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        self.0.as_ref().map(|id| id.to_string()).encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for OptionalUuid7 {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <Option<String> as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(Self::from(s))
    }
}
