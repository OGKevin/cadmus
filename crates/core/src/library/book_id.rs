use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};

/// Stable integer key for a book row in `book_keys` (`INTEGER PRIMARY KEY AUTOINCREMENT`).
///
/// Distinct from `books.fingerprint` (`Fp`): append-heavy tables such as `reading_events`
/// reference this narrow key instead of the 64-character hex primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BookId(i64);

impl BookId {
    pub fn into_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for BookId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<BookId> for i64 {
    fn from(value: BookId) -> Self {
        value.0
    }
}

impl sqlx::Type<Sqlite> for BookId {
    fn type_info() -> SqliteTypeInfo {
        <i64 as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <i64 as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, Sqlite> for BookId {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        self.0.encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for BookId {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let id = <i64 as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(Self(id))
    }
}
