//! Known document file extensions used across import, settings, and the library.

use fxhash::FxHashSet;
use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;

/// A known file extension for documents Cadmus can import and open.
///
/// The serialized string (e.g. `"epub"`, `"cbz"`) is used as the TOML key in
/// refresh-rate-by-kind maps and as values in allowed/metadata/dithered kind sets.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileExtension {
    /// Comic book RAR archive.
    Cbr,
    /// Comic book ZIP archive.
    Cbz,
    /// DjVu document.
    #[serde(alias = "djv")]
    Djvu,
    /// EPUB ebook.
    Epub,
    /// FictionBook document.
    Fb2,
    /// HTML document.
    #[serde(alias = "htm")]
    Html,
    /// JPEG image using the long extension.
    Jpeg,
    /// JPEG image using the short extension.
    Jpg,
    /// Mobipocket ebook.
    Mobi,
    /// OpenXPS document.
    Oxps,
    /// PDF document.
    Pdf,
    /// PNG image.
    Png,
    /// SVG image.
    Svg,
    /// Plain text document.
    Txt,
    /// WebP image.
    Webp,
    /// XPS document.
    Xps,
}

impl FileExtension {
    /// Returns all known file extensions.
    pub fn all() -> &'static [FileExtension] {
        &[
            FileExtension::Cbr,
            FileExtension::Cbz,
            FileExtension::Djvu,
            FileExtension::Epub,
            FileExtension::Fb2,
            FileExtension::Html,
            FileExtension::Jpeg,
            FileExtension::Jpg,
            FileExtension::Mobi,
            FileExtension::Oxps,
            FileExtension::Pdf,
            FileExtension::Png,
            FileExtension::Svg,
            FileExtension::Txt,
            FileExtension::Webp,
            FileExtension::Xps,
        ]
    }

    /// Returns the lowercase canonical string used for storage and TOML keys.
    pub fn as_str(self) -> &'static str {
        match self {
            FileExtension::Cbr => "cbr",
            FileExtension::Cbz => "cbz",
            FileExtension::Djvu => "djvu",
            FileExtension::Epub => "epub",
            FileExtension::Fb2 => "fb2",
            FileExtension::Html => "html",
            FileExtension::Jpeg => "jpeg",
            FileExtension::Jpg => "jpg",
            FileExtension::Mobi => "mobi",
            FileExtension::Oxps => "oxps",
            FileExtension::Pdf => "pdf",
            FileExtension::Png => "png",
            FileExtension::Svg => "svg",
            FileExtension::Txt => "txt",
            FileExtension::Webp => "webp",
            FileExtension::Xps => "xps",
        }
    }
}

/// Error returned when a string does not match any known file extension.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown file extension: {0}")]
pub struct UnknownFileExtension(
    /// Extension string that could not be parsed.
    pub String,
);

impl std::str::FromStr for FileExtension {
    type Err = UnknownFileExtension;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cbr" => Ok(FileExtension::Cbr),
            "cbz" => Ok(FileExtension::Cbz),
            "djvu" | "djv" => Ok(FileExtension::Djvu),
            "epub" => Ok(FileExtension::Epub),
            "fb2" => Ok(FileExtension::Fb2),
            "html" | "htm" => Ok(FileExtension::Html),
            "jpeg" => Ok(FileExtension::Jpeg),
            "jpg" => Ok(FileExtension::Jpg),
            "mobi" => Ok(FileExtension::Mobi),
            "oxps" => Ok(FileExtension::Oxps),
            "pdf" => Ok(FileExtension::Pdf),
            "png" => Ok(FileExtension::Png),
            "svg" => Ok(FileExtension::Svg),
            "txt" => Ok(FileExtension::Txt),
            "webp" => Ok(FileExtension::Webp),
            "xps" => Ok(FileExtension::Xps),
            _ => Err(UnknownFileExtension(s.to_owned())),
        }
    }
}

impl sqlx::Type<Sqlite> for FileExtension {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl sqlx::Encode<'_, Sqlite> for FileExtension {
    fn encode_by_ref(&self, buf: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        self.as_str().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for FileExtension {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(s.parse()?)
    }
}

impl fmt::Display for FileExtension {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Soft-decoded `file_kind` from SQLite TEXT: empty or unknown → [`None`].
///
/// Used on read paths so stub/legacy rows with `file_kind = ''` do not fail
/// decoding. Writes continue to use [`FileExtension`] directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionalFileExtension(pub Option<FileExtension>);

impl From<OptionalFileExtension> for Option<FileExtension> {
    fn from(value: OptionalFileExtension) -> Self {
        value.0
    }
}

/// Parses wire text into an optional extension (empty/unknown → [`None`]).
fn parse_optional(s: &str) -> Option<FileExtension> {
    if s.is_empty() {
        return None;
    }
    match s.parse() {
        Ok(ext) => Some(ext),
        Err(e) => {
            tracing::warn!(extension = %s, error = %e, "unknown file extension in database");
            None
        }
    }
}

impl sqlx::Type<Sqlite> for OptionalFileExtension {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    fn compatible(ty: &SqliteTypeInfo) -> bool {
        <String as sqlx::Type<Sqlite>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for OptionalFileExtension {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        Ok(Self(parse_optional(&s)))
    }
}

/// Soft-drop deserializer for `FxHashSet<FileExtension>` TOML sequences.
pub fn deserialize_file_extension_set<'de, D>(
    deserializer: D,
) -> Result<FxHashSet<FileExtension>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FileExtensionSetVisitor;

    impl<'de> serde::de::Visitor<'de> for FileExtensionSetVisitor {
        type Value = FxHashSet<FileExtension>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a sequence of file extension strings")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut set = FxHashSet::default();

            while let Some(s) = seq.next_element::<String>()? {
                match s.parse::<FileExtension>() {
                    Ok(ext) => {
                        set.insert(ext);
                    }
                    Err(e) => {
                        tracing::warn!(extension = %s, error = %e, "failed to load extension");
                    }
                }
            }

            Ok(set)
        }
    }

    deserializer.deserialize_seq(FileExtensionSetVisitor)
}

/// Soft-drop deserializer for maps keyed by file extension strings.
pub fn deserialize_file_extension_map<'de, D, V>(
    deserializer: D,
) -> Result<HashMap<FileExtension, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct FileExtensionMapVisitor<V> {
        marker: PhantomData<fn() -> V>,
    }

    impl<'de, V> serde::de::Visitor<'de> for FileExtensionMapVisitor<V>
    where
        V: Deserialize<'de>,
    {
        type Value = HashMap<FileExtension, V>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of file extension strings to values")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut out = HashMap::new();

            while let Some(key) = map.next_key::<String>()? {
                let value = map.next_value::<V>()?;
                match key.parse::<FileExtension>() {
                    Ok(ext) => {
                        out.insert(ext, value);
                    }
                    Err(e) => {
                        tracing::warn!(
                            extension = %key,
                            error = %e,
                            "failed to load extension map key"
                        );
                    }
                }
            }

            Ok(out)
        }
    }

    deserializer.deserialize_map(FileExtensionMapVisitor {
        marker: PhantomData,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_extension_round_trip_via_from_str() {
        for ext in FileExtension::all() {
            let parsed = ext.as_str().parse::<FileExtension>().ok();
            assert_eq!(parsed, Some(*ext), "round trip failed for {:?}", ext);
        }
    }

    #[test]
    fn test_htm_extension_parses_as_html() {
        assert_eq!("htm".parse(), Ok(FileExtension::Html));
        assert_eq!("html".parse(), Ok(FileExtension::Html));
        assert_eq!(FileExtension::Html.as_str(), "html");
        assert_eq!(
            serde_json::from_str::<FileExtension>(r#""htm""#).unwrap(),
            FileExtension::Html
        );
        assert_eq!(
            serde_json::to_string(&FileExtension::Html).unwrap(),
            r#""html""#
        );
    }

    #[test]
    fn test_djv_extension_parses_as_djvu() {
        assert_eq!("djv".parse(), Ok(FileExtension::Djvu));
        assert_eq!("djvu".parse(), Ok(FileExtension::Djvu));
        assert_eq!(FileExtension::Djvu.as_str(), "djvu");
        assert_eq!(
            serde_json::from_str::<FileExtension>(r#""djv""#).unwrap(),
            FileExtension::Djvu
        );
        assert_eq!(
            serde_json::to_string(&FileExtension::Djvu).unwrap(),
            r#""djvu""#
        );
    }

    #[test]
    fn test_jpg_and_jpeg_remain_distinct() {
        let jpg: FileExtension = "jpg".parse().unwrap();
        let jpeg: FileExtension = "jpeg".parse().unwrap();
        assert_eq!(jpg, FileExtension::Jpg);
        assert_eq!(jpeg, FileExtension::Jpeg);
        assert_ne!(jpg, jpeg);
    }

    #[test]
    fn test_parse_optional_empty_and_unknown() {
        assert_eq!(parse_optional(""), None);
        assert_eq!(parse_optional("epub"), Some(FileExtension::Epub));
        assert_eq!(parse_optional("unknown-format"), None);
    }
}
