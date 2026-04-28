use anyhow::{Context, Error};
use entities::ENTITIES;
use fxhash::FxHashMap;
use lazy_static::lazy_static;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::char;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use walkdir::DirEntry;

lazy_static! {
    pub static ref CHARACTER_ENTITIES: FxHashMap<&'static str, &'static str> = {
        let mut m = FxHashMap::default();
        for e in ENTITIES.iter() {
            m.insert(e.entity, e.characters);
        }
        m
    };
}

pub fn decode_entities(text: &str) -> Cow<'_, str> {
    if text.find('&').is_none() {
        return Cow::Borrowed(text);
    }

    let mut cursor = text;
    let mut buf = String::with_capacity(text.len());

    while let Some(start_index) = cursor.find('&') {
        buf.push_str(&cursor[..start_index]);
        cursor = &cursor[start_index..];
        if let Some(end_index) = cursor.find(';') {
            if let Some(repl) = CHARACTER_ENTITIES.get(&cursor[..=end_index]) {
                buf.push_str(repl);
            } else if cursor[1..].starts_with('#') {
                let radix = if cursor[2..].starts_with('x') { 16 } else { 10 };
                let drift_index = 2 + radix as usize / 16;
                if let Some(ch) = u32::from_str_radix(&cursor[drift_index..end_index], radix)
                    .ok()
                    .and_then(char::from_u32)
                {
                    buf.push(ch);
                } else {
                    buf.push_str(&cursor[..=end_index]);
                }
            } else {
                buf.push_str(&cursor[..=end_index]);
            }
            cursor = &cursor[end_index + 1..];
        } else {
            break;
        }
    }

    buf.push_str(cursor);
    Cow::Owned(buf)
}

pub fn load_json<T, P: AsRef<Path>>(path: P) -> Result<T, Error>
where
    for<'a> T: Deserialize<'a>,
{
    let file = File::open(path.as_ref())
        .with_context(|| format!("can't open file {}", path.as_ref().display()))?;
    let reader = BufReader::new(file);
    serde_json::from_reader(reader)
        .with_context(|| format!("can't parse JSON from {}", path.as_ref().display()))
        .map_err(Into::into)
}

pub fn save_json<T, P: AsRef<Path>>(data: &T, path: P) -> Result<(), Error>
where
    T: Serialize,
{
    let file = File::create(path.as_ref())
        .with_context(|| format!("can't create file {}", path.as_ref().display()))?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, data)
        .with_context(|| format!("can't serialize to JSON file {}", path.as_ref().display()))
        .map_err(Into::into)
}

pub fn load_toml<T, P: AsRef<Path>>(path: P) -> Result<T, Error>
where
    for<'a> T: Deserialize<'a>,
{
    let s = fs::read_to_string(path.as_ref())
        .with_context(|| format!("can't read file {}", path.as_ref().display()))?;
    toml::from_str(&s)
        .with_context(|| format!("can't parse TOML content from {}", path.as_ref().display()))
        .map_err(Into::into)
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(data, path), fields(file_path = %path.as_ref().display())))]
pub fn save_toml<T, P: AsRef<Path>>(data: &T, path: P) -> Result<(), Error>
where
    T: Serialize,
{
    let path_ref = path.as_ref();
    tracing::debug!(file_path = %path_ref.display(), "serializing data to TOML");
    let s = toml::to_string(data).context("can't convert to TOML format")?;

    tracing::debug!(
        file_path = %path_ref.display(),
        toml_size = s.len(),
        "writing TOML to file"
    );

    match fs::write(path_ref, &s) {
        Ok(()) => {
            let file_size = path_ref.metadata().ok().map(|m| m.len());

            tracing::debug!(
                file_path = %path_ref.display(),
                file_size = ?file_size,
                "successfully wrote TOML file"
            );

            Ok(())
        }
        Err(e) => {
            tracing::error!(
                file_path = %path_ref.display(),
                error = %e,
                "failed to write TOML file"
            );
            Err(anyhow::Error::new(e))
                .context(format!("can't write to file {}", path_ref.display()))
        }
    }
}

/// Computes a content-based fingerprint for a file.
///
/// Implemented on [`Path`] to hash the full file contents using BLAKE3,
/// producing a stable 32-byte digest that is independent of filesystem
/// metadata such as modification time or file size.
///
/// # Hashing strategy
///
/// The implementation selects between two BLAKE3 strategies based on file size:
///
/// - **< 10 MiB** — [`update_reader`](blake3::Hasher::update_reader): plain
///   buffered sequential read. Avoids mmap syscall overhead for small files.
///   On slow storage, a single sequential `read()` into a buffer is faster
///   than taking page faults through a memory mapping. The typical e-book
///   (100 KiB–500 KiB) falls into this range.
///
/// - **≥ 10 MiB** — [`update_mmap`](blake3::Hasher::update_mmap):
///   single-threaded memory-mapped hashing. Avoids buffered-read overhead for
///   large files while keeping CPU usage on a single core.
pub trait Fingerprint {
    fn fingerprint(&self) -> io::Result<Fp>;
}

const MMAP_THRESHOLD: u64 = 10 * 1024 * 1024;

impl Fingerprint for Path {
    #[cfg_attr(feature = "tracing", tracing::instrument(ret(level=tracing::Level::TRACE)))]
    fn fingerprint(&self) -> io::Result<Fp> {
        let mut hasher = blake3::Hasher::new();
        if std::fs::metadata(self)?.len() >= MMAP_THRESHOLD {
            hasher.update_mmap(self)?;
        } else {
            let file = std::fs::File::open(self)?;
            hasher.update_reader(file)?;
        }
        Ok(Fp(*hasher.finalize().as_bytes()))
    }
}

/// A 32-byte BLAKE3 content fingerprint used as the primary key for books.
///
/// Serialized as a 64-character lowercase hex string (e.g.
/// `"af1349b9f5f9a1a6a0404dea36dcc949..."`).
#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub struct Fp([u8; 32]);

impl Fp {
    /// Constructs an `Fp` from a `u64` seed for use in tests.
    ///
    /// The seed is written into the last 8 bytes (big-endian); the remaining
    /// 24 bytes are zero. This guarantees uniqueness for distinct seeds while
    /// producing a valid 32-byte fingerprint.
    #[cfg(test)]
    pub fn from_u64(seed: u64) -> Self {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&seed.to_be_bytes());
        Fp(bytes)
    }

    /// Parses a legacy 16-character uppercase hex fingerprint (mtime + size
    /// metadata format) into an `Fp`.
    ///
    /// The `u64` value is stored in the last 8 bytes (big-endian); the
    /// remaining 24 bytes are zero. This matches the `from_u64` layout so
    /// that legacy entries round-trip consistently. V2 migration will
    /// re-key these to real BLAKE3 hashes once the files are found on disk.
    pub(crate) fn from_legacy_str(s: &str) -> Result<Self, FpParseError> {
        if s.len() != 16 {
            return Err(FpParseError);
        }
        let seed = u64::from_str_radix(s, 16).map_err(|_| FpParseError)?;
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&seed.to_be_bytes());
        Ok(Fp(bytes))
    }
}

impl Deref for Fp {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Error returned when a hex string cannot be decoded into an [`Fp`].
#[derive(Debug)]
pub struct FpParseError;

impl fmt::Display for FpParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "invalid fingerprint: expected 64 hex characters or 16 hex characters (legacy format)",
        )
    }
}

impl std::error::Error for FpParseError {}

impl FromStr for Fp {
    type Err = FpParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.is_ascii() {
            return Err(FpParseError);
        }

        if s.len() == 16 {
            return Self::from_legacy_str(s);
        }

        if s.len() != 64 {
            return Err(FpParseError);
        }

        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| FpParseError)?;
        }

        Ok(Fp(bytes))
    }
}

impl fmt::Display for Fp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl Serialize for Fp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

struct FpVisitor;

impl<'de> Visitor<'de> for FpVisitor {
    type Value = Fp;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a 64-character hex string or a 16-character legacy hex string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Fp::from_str(value)
            .or_else(|_| Fp::from_legacy_str(value))
            .map_err(|e| E::custom(format!("can't parse fingerprint: {}", e)))
    }
}

impl<'de> Deserialize<'de> for Fp {
    fn deserialize<D>(deserializer: D) -> Result<Fp, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(FpVisitor)
    }
}

pub trait Normalize: ToOwned {
    fn normalize(&self) -> Self::Owned;
}

impl Normalize for Path {
    fn normalize(&self) -> PathBuf {
        let mut result = PathBuf::default();

        for c in self.components() {
            match c {
                Component::ParentDir => {
                    result.pop();
                }
                Component::CurDir => (),
                _ => result.push(c),
            }
        }

        result
    }
}

pub trait AsciiExtension {
    fn to_alphabetic_digit(self) -> Option<u32>;
}

impl AsciiExtension for char {
    fn to_alphabetic_digit(self) -> Option<u32> {
        if self.is_ascii_uppercase() {
            Some(self as u32 - 65)
        } else {
            None
        }
    }
}

pub mod datetime_format {
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub const FORMAT: &str = "%Y-%m-%d %H:%M:%S";

    pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}", date.format(FORMAT));
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        NaiveDateTime::parse_from_str(&s, FORMAT).map_err(serde::de::Error::custom)
    }
}

pub trait IsHidden {
    fn is_hidden(&self) -> bool;
}

impl IsHidden for DirEntry {
    fn is_hidden(&self) -> bool {
        self.file_name()
            .to_str()
            .map_or(false, |s| s.starts_with('.'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_entities() {
        assert_eq!(decode_entities("a &amp b"), "a &amp b");
        assert_eq!(decode_entities("a &zZz; b"), "a &zZz; b");
        assert_eq!(decode_entities("a &amp; b"), "a & b");
        assert_eq!(decode_entities("a &#x003E; b"), "a > b");
        assert_eq!(decode_entities("a &#38; b"), "a & b");
        assert_eq!(decode_entities("a &lt; b &gt; c"), "a < b > c");
    }

    #[test]
    fn fp_from_str_rejects_non_ascii_input() {
        let invalid = format!("a€{}", "0".repeat(60));

        assert_eq!(invalid.len(), 64);
        assert!(Fp::from_str(&invalid).is_err());
    }

    #[test]
    fn fp_from_str_parses_valid_legacy_hex() {
        let input = "0123456789ABCDEF";
        let fp = Fp::from_str(input).expect("legacy fingerprint should parse");

        assert_eq!(
            fp.to_string(),
            "0000000000000000000000000000000000000000000000000123456789abcdef"
        );
    }

    #[test]
    fn fp_from_str_rejects_invalid_legacy_hex() {
        assert!(Fp::from_str("0123456789ABCDEG").is_err());
    }

    #[test]
    fn fp_from_str_parses_valid_hex() {
        let input = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let fp = Fp::from_str(input).expect("valid fingerprint should parse");

        assert_eq!(fp.to_string(), input);
    }

    #[test]
    fn fp_from_str_accepts_uppercase_hex() {
        let input = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";
        let fp = Fp::from_str(input).expect("uppercase fingerprint should parse");

        assert_eq!(
            fp.to_string(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }
}
