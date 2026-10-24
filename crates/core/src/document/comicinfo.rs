//! ComicInfo.xml metadata for CBZ and CBR archives.
//!
//! Reads a `ComicInfo.xml` member from a comic archive during library import and
//! maps legacy ComicInfo fields into [`Info`]. CBZ uses
//! async zip I/O; CBR uses the [`rars`] crate on the blocking pool to decode only
//! the matching archive member into memory (no full extract to disk).
//!
//! Reading direction (`Manga`) and other fields not mapped to [`Info`] are ignored.

use super::file_extension::FileExtension;
use super::html::dom::NodeRef;
use super::html::xml::parse_xml;
use crate::helpers::decode_entities;
use crate::metadata::Info;
use crate::runtime;
use async_zip::base::read1::seek::ZipArchiveReader;
use async_zip::spec::constructs::CDR;
use rars::Archive;
use rars::ArchiveReadOptions;
use rars::ArchiveReader;
use rars::Error as RarError;
use std::collections::BTreeSet;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::debug;

/// Maximum inflated ComicInfo payload accepted from an archive member.
const MAX_COMICINFO_UNCOMPRESSED_BYTES: u64 = 1024 * 1024;

/// Parsed ComicInfo.xml values before they are merged into [`Info`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ComicInfoFields {
    title: String,
    series: String,
    number: String,
    volume: String,
    year: String,
    language: String,
    publisher: String,
    author: String,
    identifier: String,
    categories: BTreeSet<String>,
}

/// Outcome of reading ComicInfo from an archive during import or refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComicInfoReadResult {
    /// Archive I/O or decode failed; stored metadata is left unchanged.
    KeepStored,
    /// The archive has no ComicInfo member; ComicInfo-derived fields are cleared.
    ClearDerived,
    /// ComicInfo was read and parsed successfully.
    Fields(Box<ComicInfoFields>),
}

enum ComicInfoXmlLoad {
    MemberMissing,
    ReadFailed,
    Xml(String),
}

/// Loads and parses ComicInfo metadata from `path` when `kind` is [`FileExtension::Cbz`] or
/// [`FileExtension::Cbr`].
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(path), fields(path = %path.display(), kind = ?kind))
)]
pub(crate) async fn read_comicinfo_from_path(
    path: &Path,
    kind: FileExtension,
) -> ComicInfoReadResult {
    let xml = match kind {
        FileExtension::Cbz => read_comicinfo_xml_from_cbz(path).await,
        FileExtension::Cbr => read_comicinfo_xml_from_cbr(path).await,
        _ => return ComicInfoReadResult::KeepStored,
    };
    match xml {
        ComicInfoXmlLoad::MemberMissing => ComicInfoReadResult::ClearDerived,
        ComicInfoXmlLoad::ReadFailed => ComicInfoReadResult::KeepStored,
        ComicInfoXmlLoad::Xml(xml) => match parse_comicinfo_xml(&xml) {
            Some(fields) => {
                debug!(
                    title = %fields.title,
                    series = %fields.series,
                    number = %fields.number,
                    author = %fields.author,
                    category_count = fields.categories.len(),
                    xml_bytes = xml.len(),
                    "parsed ComicInfo metadata"
                );
                ComicInfoReadResult::Fields(Box::new(fields))
            }
            None => {
                debug!(xml_bytes = xml.len(), "ComicInfo XML could not be parsed");
                ComicInfoReadResult::KeepStored
            }
        },
    }
}

/// Replaces ComicInfo-derived [`Info`] fields from `fields`.
///
/// Empty strings in `fields` clear the corresponding [`Info`] field. Genres replace
/// [`Info::categories`] entirely.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(fields, info)))]
pub(crate) fn apply_comicinfo_to_info(fields: &ComicInfoFields, info: &mut Info) {
    info.title = fields.title.clone();
    info.series = fields.series.clone();
    info.number = fields.number.clone();
    info.volume = fields.volume.clone();
    info.year = fields.year.clone();
    info.language = fields.language.clone();
    info.publisher = fields.publisher.clone();
    info.author = fields.author.clone();
    info.identifier = fields.identifier.clone();
    info.categories = fields.categories.clone();
}

/// Clears every [`Info`] field populated from ComicInfo.xml.
///
/// Fields not sourced from ComicInfo (for example [`Info::subtitle`]) are left unchanged.
pub(crate) fn clear_comicinfo_derived_fields(info: &mut Info) {
    info.title.clear();
    info.series.clear();
    info.number.clear();
    info.volume.clear();
    info.year.clear();
    info.language.clear();
    info.publisher.clear();
    info.author.clear();
    info.identifier.clear();
    info.categories.clear();
}

/// Applies a [`ComicInfoReadResult`] to `info`.
pub(crate) fn apply_comicinfo_read(result: ComicInfoReadResult, info: &mut Info) {
    match result {
        ComicInfoReadResult::KeepStored => {}
        ComicInfoReadResult::ClearDerived => clear_comicinfo_derived_fields(info),
        ComicInfoReadResult::Fields(fields) => {
            clear_comicinfo_derived_fields(info);
            apply_comicinfo_to_info(&fields, info);
        }
    }
}

/// Inflates the first zip entry whose basename is `comicinfo.xml` into a UTF-8 string.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(path), fields(path = %path.display()))
)]
async fn read_comicinfo_xml_from_cbz(path: &Path) -> ComicInfoXmlLoad {
    let file = match File::open(path).await {
        Ok(file) => file,
        Err(error) => {
            debug!(path = %path.display(), error = %error, "failed to open CBZ for ComicInfo");
            return ComicInfoXmlLoad::ReadFailed;
        }
    };
    let mut reader = match ZipArchiveReader::open(BufReader::new(file).compat()).await {
        Ok(reader) => reader,
        Err(error) => {
            debug!(path = %path.display(), error = %error, "failed to open CBZ archive");
            return ComicInfoXmlLoad::ReadFailed;
        }
    };
    let index = match comicinfo_zip_cdr_index(reader.cdrs()) {
        Some(index) => index,
        None => return ComicInfoXmlLoad::MemberMissing,
    };
    let uncompressed = match reader.cdrs()[index].uncompressed_size() {
        Ok(size) => size,
        Err(error) => {
            debug!(
                path = %path.display(),
                zip_entry_index = index,
                error = %error,
                "ComicInfo zip entry has invalid uncompressed size"
            );
            return ComicInfoXmlLoad::ReadFailed;
        }
    };
    if !comicinfo_uncompressed_within_limit(uncompressed) {
        debug!(
            path = %path.display(),
            zip_entry_index = index,
            uncompressed,
            limit = MAX_COMICINFO_UNCOMPRESSED_BYTES,
            "ComicInfo zip entry exceeds size limit"
        );
        return ComicInfoXmlLoad::ReadFailed;
    }
    let entry = match reader.file(index).await {
        Ok(entry) => entry,
        Err(error) => {
            debug!(
                path = %path.display(),
                zip_entry_index = index,
                error = %error,
                "failed to open ComicInfo zip entry"
            );
            return ComicInfoXmlLoad::ReadFailed;
        }
    };
    let xml = match read_utf8_with_max_bytes(
        &mut entry.compat(),
        MAX_COMICINFO_UNCOMPRESSED_BYTES as usize,
    )
    .await
    {
        Some(xml) => xml,
        None => return ComicInfoXmlLoad::ReadFailed,
    };
    debug!(
        path = %path.display(),
        xml_bytes = xml.len(),
        zip_entry_index = index,
        "read ComicInfo XML from CBZ"
    );
    ComicInfoXmlLoad::Xml(xml)
}

/// Returns the central-directory index of the ComicInfo zip entry, if present.
fn comicinfo_zip_cdr_index(cdrs: &[CDR]) -> Option<usize> {
    cdrs.iter().enumerate().find_map(|(index, cdr)| {
        cdr.insecure_file_name
            .as_str()
            .filter(|name| is_comicinfo_archive_path(name))
            .map(|_| index)
    })
}

/// Decodes ComicInfo bytes from a RAR archive on the blocking pool.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(path), fields(path = %path.display()))
)]
async fn read_comicinfo_xml_from_cbr(path: &Path) -> ComicInfoXmlLoad {
    let path = path.to_path_buf();
    let decode_path = path.clone();
    let bytes = match runtime::spawn_blocking(move || decode_comicinfo_from_rar(&decode_path)).await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            debug!(path = %path.display(), error = %error, "ComicInfo RAR decode task failed");
            return ComicInfoXmlLoad::ReadFailed;
        }
    };
    match bytes {
        RarComicInfoBytes::MemberMissing => ComicInfoXmlLoad::MemberMissing,
        RarComicInfoBytes::ReadFailed => ComicInfoXmlLoad::ReadFailed,
        RarComicInfoBytes::Payload(bytes) => match String::from_utf8(bytes) {
            Ok(xml) => {
                debug!(
                    path = %path.display(),
                    xml_bytes = xml.len(),
                    "read ComicInfo XML from CBR"
                );
                ComicInfoXmlLoad::Xml(xml)
            }
            Err(error) => {
                debug!(
                    path = %path.display(),
                    byte_count = error.as_bytes().len(),
                    "ComicInfo XML is not valid UTF-8"
                );
                ComicInfoXmlLoad::ReadFailed
            }
        },
    }
}

enum RarComicInfoBytes {
    MemberMissing,
    ReadFailed,
    Payload(Vec<u8>),
}

/// Opens `path` with [`ArchiveReader`] and returns decoded ComicInfo member bytes.
///
/// Solid predecessors required by the format may be decoded and discarded; page images are not
/// written to disk.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(path), fields(path = %path.display()))
)]
fn decode_comicinfo_from_rar(path: &Path) -> RarComicInfoBytes {
    let archive = match ArchiveReader::read_path(path) {
        Ok(archive) => archive,
        Err(error) => {
            debug!(path = %path.display(), error = %error, "failed to open RAR archive");
            return RarComicInfoBytes::ReadFailed;
        }
    };
    let (index, unpacked_size) = match comicinfo_rar_member_index(&archive) {
        Some(index_and_size) => index_and_size,
        None => {
            debug!(path = %path.display(), "no ComicInfo member in RAR archive");
            return RarComicInfoBytes::MemberMissing;
        }
    };
    if !comicinfo_uncompressed_within_limit(unpacked_size) {
        debug!(
            path = %path.display(),
            member_index = index,
            unpacked_size,
            limit = MAX_COMICINFO_UNCOMPRESSED_BYTES,
            "ComicInfo RAR member exceeds size limit"
        );
        return RarComicInfoBytes::ReadFailed;
    }
    match archive.read_member_at_with_options(index, comicinfo_rar_read_options()) {
        Ok(Some(bytes)) => {
            debug!(
                path = %path.display(),
                byte_count = bytes.len(),
                member_index = index,
                "decoded ComicInfo member from RAR"
            );
            RarComicInfoBytes::Payload(bytes)
        }
        Ok(None) => {
            debug!(
                path = %path.display(),
                member_index = index,
                "ComicInfo RAR member has no payload"
            );
            RarComicInfoBytes::ReadFailed
        }
        Err(error) => {
            if matches!(
                error.root_cause(),
                RarError::MemberOutputLimitExceeded { .. }
            ) {
                debug!(
                    path = %path.display(),
                    member_index = index,
                    limit = MAX_COMICINFO_UNCOMPRESSED_BYTES,
                    error = %error,
                    "ComicInfo RAR payload exceeds size limit during decode"
                );
            } else {
                debug!(path = %path.display(), error = %error, "failed to read ComicInfo from RAR");
            }
            RarComicInfoBytes::ReadFailed
        }
    }
}

fn comicinfo_rar_read_options() -> ArchiveReadOptions<'static> {
    ArchiveReadOptions::new().with_max_member_output_bytes(MAX_COMICINFO_UNCOMPRESSED_BYTES)
}

/// Archive-order index and declared unpacked size of the ComicInfo file member, if any.
fn comicinfo_rar_member_index(archive: &Archive) -> Option<(usize, u64)> {
    archive.members().enumerate().find_map(|(index, member)| {
        let meta = &member.meta;
        if meta.is_directory || meta.is_redirection {
            return None;
        }
        is_comicinfo_archive_path(&meta.name_lossy()).then_some((index, meta.unpacked_size))
    })
}

fn comicinfo_uncompressed_within_limit(uncompressed: u64) -> bool {
    uncompressed <= MAX_COMICINFO_UNCOMPRESSED_BYTES
}

async fn read_utf8_with_max_bytes(
    reader: &mut (impl AsyncReadExt + Unpin),
    max_bytes: usize,
) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let n = reader.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > max_bytes {
            debug!(
                byte_count = buf.len(),
                max_bytes, "ComicInfo payload exceeds size limit while reading"
            );
            return None;
        }
    }
    String::from_utf8(buf).ok()
}

/// Whether `path` names a ComicInfo file (basename `comicinfo.xml`, any directory prefix).
fn is_comicinfo_archive_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("comicinfo.xml"))
}

/// Parses a ComicInfo document string into [`ComicInfoFields`].
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(xml), fields(xml_bytes = xml.len()))
)]
fn parse_comicinfo_xml(xml: &str) -> Option<ComicInfoFields> {
    if !ends_with_comicinfo_close_tag(xml) {
        return None;
    }
    let tree = parse_xml(xml);
    let comic_root = tree
        .root()
        .children()
        .find(|node| tag_eq_ignore_case(node.tag_name(), "ComicInfo"))?;
    Some(fields_from_comicinfo_node(comic_root))
}

/// Whether `xml` ends with `</ComicInfo>` after trimming trailing whitespace.
///
/// xml5ever recovers unclosed elements at EOF, so a truncated member would otherwise
/// parse as a complete document with empty fields.
fn ends_with_comicinfo_close_tag(xml: &str) -> bool {
    const CLOSE_TAG: &str = "</ComicInfo>";
    let trimmed = xml.trim_end();
    trimmed
        .len()
        .checked_sub(CLOSE_TAG.len())
        .and_then(|start| trimmed.get(start..))
        .is_some_and(|tail| tail.eq_ignore_ascii_case(CLOSE_TAG))
}

/// Maps child elements under a `ComicInfo` root node into [`ComicInfoFields`].
///
/// `Writer` is preferred for author; otherwise the first non-empty role among penciller, colorist,
/// letterer, and editor. `Web` wins over `ISBN` for identifier.
fn fields_from_comicinfo_node(comic_root: NodeRef<'_>) -> ComicInfoFields {
    let mut fields = ComicInfoFields::default();
    let mut writer = String::new();
    let mut penciller = String::new();
    let mut colorist = String::new();
    let mut letterer = String::new();
    let mut editor = String::new();
    let mut web = String::new();
    let mut isbn = String::new();

    for child in comic_root.children() {
        let tag = child.tag_name();
        let text = child.text();
        let trimmed = decode_entities(text.trim());
        if trimmed.is_empty() {
            continue;
        }
        if tag_eq_ignore_case(tag, "Title") {
            fields.title = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Series") {
            fields.series = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Number") {
            fields.number = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Volume") {
            fields.volume = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Year") {
            fields.year = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "LanguageISO") {
            fields.language = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Publisher") {
            fields.publisher = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Writer") {
            writer = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Penciller") {
            penciller = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Colorist") {
            colorist = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Letterer") {
            letterer = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Editor") {
            editor = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "Genre") {
            fields.categories.insert(trimmed.into_owned());
        } else if tag_eq_ignore_case(tag, "Web") {
            web = trimmed.into_owned();
        } else if tag_eq_ignore_case(tag, "ISBN") {
            isbn = trimmed.into_owned();
        }
    }

    fields.author = first_nonempty(&[&writer, &penciller, &colorist, &letterer, &editor]);
    fields.identifier = if !web.is_empty() { web } else { isbn };

    fields
}

fn first_nonempty(values: &[&String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_default()
}

fn tag_eq_ignore_case(tag: Option<&str>, expected: &str) -> bool {
    tag.is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_zip::tokio::write::ZipFileWriter;
    use async_zip::{Compression, ZipEntryBuilder};

    #[test]
    fn parse_comicinfo_maps_core_fields() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>Test Title</Title>
  <Series>Test Series</Series>
  <Number>3</Number>
  <Volume>1</Volume>
  <Year>2020</Year>
  <LanguageISO>en</LanguageISO>
  <Publisher>Pub</Publisher>
  <Writer>Alice Writer</Writer>
  <Genre>Action</Genre>
  <Genre>Sci-Fi</Genre>
  <Web>https://example.com/book</Web>
</ComicInfo>"#;
        let fields = parse_comicinfo_xml(xml).expect("parse");
        assert_eq!(fields.title, "Test Title");
        assert_eq!(fields.series, "Test Series");
        assert_eq!(fields.number, "3");
        assert_eq!(fields.volume, "1");
        assert_eq!(fields.year, "2020");
        assert_eq!(fields.language, "en");
        assert_eq!(fields.publisher, "Pub");
        assert_eq!(fields.author, "Alice Writer");
        assert_eq!(
            fields.categories,
            BTreeSet::from(["Action".to_string(), "Sci-Fi".to_string()])
        );
        assert_eq!(fields.identifier, "https://example.com/book");
    }

    #[test]
    fn parse_comicinfo_author_falls_back_to_penciller() {
        let xml = r#"<ComicInfo><Penciller>Bob</Penciller></ComicInfo>"#;
        let fields = parse_comicinfo_xml(xml).expect("parse");
        assert_eq!(fields.author, "Bob");
    }

    #[test]
    fn parse_comicinfo_rejects_truncated_document() {
        assert!(parse_comicinfo_xml("<ComicInfo>").is_none());
        assert!(parse_comicinfo_xml("<ComicInfo><Title>Cut off").is_none());
        assert!(parse_comicinfo_xml("<ComicInfo><Title>Done</Title></ComicInfo>\n").is_some());
    }

    #[test]
    fn parse_comicinfo_decodes_xml_entities_in_text() {
        let xml =
            r#"<ComicInfo><Title>A &amp; B</Title><Writer>Tom &lt;Jerry&gt;</Writer></ComicInfo>"#;
        let fields = parse_comicinfo_xml(xml).expect("parse");
        assert_eq!(fields.title, "A & B");
        assert_eq!(fields.author, "Tom <Jerry>");
    }

    #[test]
    fn parse_comicinfo_decodes_decimal_and_hex_numeric_entities() {
        let xml =
            r#"<ComicInfo><Title>&#65;&#x42;</Title><Series>&#201;&#xE9;</Series></ComicInfo>"#;
        let fields = parse_comicinfo_xml(xml).expect("parse");
        assert_eq!(fields.title, "AB");
        assert_eq!(fields.series, "Éé");
    }

    #[test]
    fn refresh_comicinfo_clears_stale_fields_when_xml_omits_them() {
        let mut info = Info {
            title: "Stale title".to_string(),
            author: "Stale author".to_string(),
            categories: BTreeSet::from(["Stale genre".to_string()]),
            subtitle: "User subtitle".to_string(),
            ..Default::default()
        };
        let fields =
            parse_comicinfo_xml(r#"<ComicInfo><Writer>Fresh</Writer></ComicInfo>"#).expect("parse");
        apply_comicinfo_read(ComicInfoReadResult::Fields(Box::new(fields)), &mut info);
        assert_eq!(info.title, "");
        assert_eq!(info.author, "Fresh");
        assert_eq!(info.subtitle, "User subtitle");
        assert!(info.categories.is_empty());
    }

    #[test]
    fn refresh_comicinfo_clears_derived_fields_when_archive_has_no_comicinfo() {
        let mut info = Info {
            title: "From old ComicInfo".to_string(),
            author: "Old writer".to_string(),
            ..Default::default()
        };
        apply_comicinfo_read(ComicInfoReadResult::ClearDerived, &mut info);
        assert_eq!(info.title, "");
        assert_eq!(info.author, "");
    }

    #[test]
    fn apply_comicinfo_read_keeps_stored_metadata_on_read_failure() {
        let mut info = Info {
            title: "Kept title".to_string(),
            author: "Kept author".to_string(),
            ..Default::default()
        };
        apply_comicinfo_read(ComicInfoReadResult::KeepStored, &mut info);
        assert_eq!(info.title, "Kept title");
        assert_eq!(info.author, "Kept author");
    }

    #[test]
    fn is_comicinfo_archive_path_matches_basename_only() {
        assert!(is_comicinfo_archive_path("ComicInfo.xml"));
        assert!(is_comicinfo_archive_path("subdir/comicinfo.xml"));
        assert!(!is_comicinfo_archive_path("not-comicinfo.xml"));
    }

    async fn write_test_cbz(label: &str, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let mut buffer = Vec::new();
        {
            let mut writer = ZipFileWriter::with_tokio(&mut buffer);
            for (name, data) in entries {
                let builder = ZipEntryBuilder::new((*name).into(), Compression::Stored);
                writer
                    .write_entry_whole(builder, data)
                    .await
                    .expect("write entry");
            }
            writer.close().await.expect("close zip");
        }
        let path = std::env::temp_dir().join(format!(
            "cadmus-comicinfo-{}-{}-{}.cbz",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        tokio::fs::write(&path, &buffer).await.expect("write cbz");
        path
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbz_extracts_xml() {
        let path = write_test_cbz(
            "ok",
            &[(
                "ComicInfo.xml",
                b"<ComicInfo><Title>From Zip</Title></ComicInfo>",
            )],
        )
        .await;
        let result = read_comicinfo_from_path(&path, FileExtension::Cbz).await;
        let fields = match result {
            ComicInfoReadResult::Fields(fields) => fields,
            other => panic!("expected Fields, got {other:?}"),
        };
        assert_eq!(fields.title, "From Zip");
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbz_without_member_clears_derived() {
        let path = write_test_cbz("no-xml", &[("pages/1.jpg", b"fake")]).await;
        let result = read_comicinfo_from_path(&path, FileExtension::Cbz).await;
        assert_eq!(result, ComicInfoReadResult::ClearDerived);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbz_corrupt_archive_keeps_stored() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-comicinfo-corrupt-{}-{}.cbz",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        tokio::fs::write(&path, b"not a zip file")
            .await
            .expect("write cbz");
        let result = read_comicinfo_from_path(&path, FileExtension::Cbz).await;
        assert_eq!(result, ComicInfoReadResult::KeepStored);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbz_parse_failure_keeps_stored() {
        let path = write_test_cbz(
            "no-comicinfo-root",
            &[(
                "ComicInfo.xml",
                b"<Metadata><Title>Not ComicInfo root</Title></Metadata>",
            )],
        )
        .await;
        let result = read_comicinfo_from_path(&path, FileExtension::Cbz).await;
        assert_eq!(result, ComicInfoReadResult::KeepStored);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbz_invalid_utf8_keeps_stored() {
        let path = write_test_cbz("utf8", &[("ComicInfo.xml", &[0xff, 0xfe, 0x00])]).await;
        let result = read_comicinfo_from_path(&path, FileExtension::Cbz).await;
        assert_eq!(result, ComicInfoReadResult::KeepStored);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbz_oversized_member_keeps_stored() {
        let payload_len = MAX_COMICINFO_UNCOMPRESSED_BYTES as usize + 1;
        let xml = format!(
            "<ComicInfo><Title>{}</Title></ComicInfo>",
            "x".repeat(payload_len)
        );
        let path = write_test_cbz("oversize", &[("ComicInfo.xml", xml.as_bytes())]).await;
        let result = read_comicinfo_from_path(&path, FileExtension::Cbz).await;
        assert_eq!(result, ComicInfoReadResult::KeepStored);
        let _ = tokio::fs::remove_file(path).await;
    }

    fn comicinfo_cbr_fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/document/tests/fixtures")
            .join(name)
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbr_extracts_xml() {
        let path = comicinfo_cbr_fixture("comicinfo-minimal.cbr");
        let result = read_comicinfo_from_path(&path, FileExtension::Cbr).await;
        let fields = match result {
            ComicInfoReadResult::Fields(fields) => fields,
            other => panic!("expected Fields, got {other:?}"),
        };
        assert_eq!(fields.title, "From Rar");
    }

    #[tokio::test]
    async fn read_comicinfo_from_cbr_without_member_clears_derived() {
        let path = comicinfo_cbr_fixture("comicinfo-absent.cbr");
        let result = read_comicinfo_from_path(&path, FileExtension::Cbr).await;
        assert_eq!(result, ComicInfoReadResult::ClearDerived);
    }

    #[test]
    fn comicinfo_uncompressed_within_limit_rejects_oversized_declared_length() {
        assert!(comicinfo_uncompressed_within_limit(
            MAX_COMICINFO_UNCOMPRESSED_BYTES
        ));
        assert!(!comicinfo_uncompressed_within_limit(
            MAX_COMICINFO_UNCOMPRESSED_BYTES + 1
        ));
    }

    #[test]
    fn comicinfo_rar_read_options_cap_decode_output() {
        let path = comicinfo_cbr_fixture("comicinfo-minimal.cbr");
        let archive = ArchiveReader::read_path(&path).expect("open archive");
        let (index, _) = comicinfo_rar_member_index(&archive).expect("comicinfo member");
        let bytes = archive
            .read_member_at_with_options(index, comicinfo_rar_read_options())
            .expect("decode within limit")
            .expect("payload");
        assert!(comicinfo_uncompressed_within_limit(bytes.len() as u64));

        let err = archive
            .read_member_at_with_options(
                index,
                ArchiveReadOptions::new().with_max_member_output_bytes(1),
            )
            .expect_err("decode must fail when output limit is 1 byte");
        assert!(matches!(
            err.root_cause(),
            RarError::MemberOutputLimitExceeded { .. }
        ));
    }
}
