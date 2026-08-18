//! Lifecycle status for library books (`books.status`).

use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};
use std::fmt;

/// Lifecycle status for a row in `books`.
///
/// Distinct from file format ([`crate::settings::FileExtension`]): this tracks
/// whether a book is awaiting on-disk discovery or is ready for the shelf.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) enum BookStatus {
    /// Stub or incomplete row; import must discover/fill before shelf display.
    PendingDiscovery,
    /// Normal library book visible on the shelf.
    Active,
}

impl BookStatus {
    /// Canonical wire text stored in SQLite.
    pub fn as_str(self) -> &'static str {
        match self {
            BookStatus::PendingDiscovery => "pending_discovery",
            BookStatus::Active => "active",
        }
    }
}

/// Error when a status string is not a known [`BookStatus`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown book status: {0}")]
pub(crate) struct UnknownBookStatus(pub String);

impl std::str::FromStr for BookStatus {
    type Err = UnknownBookStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending_discovery" => Ok(BookStatus::PendingDiscovery),
            "active" => Ok(BookStatus::Active),
            _ => Err(UnknownBookStatus(s.to_owned())),
        }
    }
}

impl fmt::Display for BookStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl sqlx::Type<Sqlite> for BookStatus {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, Sqlite> for BookStatus {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        self.as_str().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for BookStatus {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        s.parse::<Self>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trip() {
        assert_eq!(BookStatus::PendingDiscovery.as_str(), "pending_discovery");
        assert_eq!(BookStatus::Active.as_str(), "active");
        assert_eq!(
            "pending_discovery".parse::<BookStatus>().unwrap(),
            BookStatus::PendingDiscovery
        );
        assert_eq!("active".parse::<BookStatus>().unwrap(), BookStatus::Active);
    }

    #[test]
    fn rejects_unknown_status() {
        assert!("retained".parse::<BookStatus>().is_err());
        assert!("".parse::<BookStatus>().is_err());
        assert!("Active".parse::<BookStatus>().is_err());
    }
}
