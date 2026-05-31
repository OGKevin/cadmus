use super::models::{BookRow, ReadingStateRow, TocEntryRow};
use crate::document::{SimpleTocEntry, TocLocation};
use crate::helpers::Fp;
use crate::metadata::{Info, ReaderInfo};
use anyhow::{Context as AnyhowContext, Error};

/// Convert Info struct to BookRow for database insertion.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(fp, info), fields(fingerprint = %fp), ret(level = tracing::Level::TRACE)))]
pub fn info_to_book_row(fp: Fp, info: &Info) -> BookRow {
    BookRow {
        fingerprint: fp.to_string(),
        title: info.title.clone(),
        subtitle: info.subtitle.clone(),
        year: info.year.clone(),
        language: info.language.clone(),
        publisher: info.publisher.clone(),
        series: info.series.clone(),
        edition: info.edition.clone(),
        volume: info.volume.clone(),
        number: info.number.clone(),
        identifier: info.identifier.clone(),
        file_path: info.file.path.display().to_string(),
        absolute_path: info.file.absolute_path.display().to_string(),
        file_kind: info.file.kind.clone(),
        file_size: info.file.size as i64,
        added_at: info.added.into(),
    }
}

/// Extract authors from Info.author (comma-separated string)
#[cfg_attr(feature = "tracing", tracing::instrument(skip(author_str), ret(level = tracing::Level::TRACE)))]
pub fn extract_authors(author_str: &str) -> Vec<String> {
    author_str
        .split(", ")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Convert ReaderInfo to ReadingStateRow for database insertion.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(fp, reader_info), fields(fingerprint = %fp), ret(level = tracing::Level::TRACE)))]
pub fn reader_info_to_reading_state_row(fp: Fp, reader_info: &ReaderInfo) -> ReadingStateRow {
    let (page_offset_x, page_offset_y) = if let Some(offset) = reader_info.page_offset {
        (Some(offset.x as i64), Some(offset.y as i64))
    } else {
        (None, None)
    };

    let cropping_margins_json = reader_info
        .cropping_margins
        .as_ref()
        .and_then(|cm| serde_json::to_string(cm).ok());

    let zoom_mode = reader_info
        .zoom_mode
        .as_ref()
        .and_then(|zm| serde_json::to_string(zm).ok());

    let scroll_mode = reader_info
        .scroll_mode
        .as_ref()
        .and_then(|sm| serde_json::to_string(sm).ok());

    let text_align = reader_info
        .text_align
        .as_ref()
        .and_then(|ta| serde_json::to_string(ta).ok());

    let page_names_json = if !reader_info.page_names.is_empty() {
        serde_json::to_string(&reader_info.page_names).ok()
    } else {
        None
    };

    let bookmarks_json = if !reader_info.bookmarks.is_empty() {
        serde_json::to_string(&reader_info.bookmarks).ok()
    } else {
        None
    };

    let annotations_json = if !reader_info.annotations.is_empty() {
        serde_json::to_string(&reader_info.annotations).ok()
    } else {
        None
    };

    ReadingStateRow {
        fingerprint: fp.to_string(),
        opened: reader_info.opened.into(),
        current_page: reader_info.current_page as i64,
        pages_count: reader_info.pages_count as i64,
        finished: if reader_info.finished { 1 } else { 0 },
        dithered: if reader_info.dithered { 1 } else { 0 },
        zoom_mode,
        scroll_mode,
        page_offset_x,
        page_offset_y,
        rotation: reader_info.rotation.map(|r| r as i64),
        cropping_margins_json,
        margin_width: reader_info.margin_width.map(|mw| mw as i64),
        screen_margin_width: reader_info.screen_margin_width.map(|smw| smw as i64),
        font_family: reader_info.font_family.clone(),
        font_size: reader_info.font_size.map(|fs| fs as f64),
        text_align,
        line_height: reader_info.line_height.map(|lh| lh as f64),
        contrast_exponent: reader_info.contrast_exponent.map(|ce| ce as f64),
        contrast_gray: reader_info.contrast_gray.map(|cg| cg as f64),
        page_names_json,
        bookmarks_json,
        annotations_json,
    }
}

/// Encode a `TocLocation` into the `(location_kind, location_exact, location_uri)` column triple.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(loc), ret(level = tracing::Level::TRACE)))]
pub fn encode_location(loc: &TocLocation) -> (&'static str, Option<i64>, Option<String>) {
    match loc {
        TocLocation::Exact(n) => ("exact", Some(*n as i64), None),
        TocLocation::Uri(uri) => ("uri", None, Some(uri.clone())),
    }
}

/// Decode the `(location_kind, location_exact, location_uri)` column triple back to a `TocLocation`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(kind, exact, uri), ret(level = tracing::Level::TRACE)))]
pub fn decode_location(
    kind: &str,
    exact: Option<i64>,
    uri: Option<&str>,
) -> Result<TocLocation, Error> {
    match kind {
        "exact" => {
            let n = exact.with_context(|| "location_exact is NULL for kind='exact'")?;
            Ok(TocLocation::Exact(n as usize))
        }
        "uri" => {
            let s = uri
                .with_context(|| "location_uri is NULL for kind='uri'")?
                .to_string();
            Ok(TocLocation::Uri(s))
        }
        other => anyhow::bail!("unknown location_kind: {}", other),
    }
}

/// Reconstruct a `Vec<SimpleTocEntry>` tree from a flat list of rows ordered by `id`.
///
/// Rows must be ordered such that every parent appears before its children (pre-order),
/// which is guaranteed by inserting parents first and ordering by `id ASC`.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(rows), fields(entry_count = rows.len()), ret(level = tracing::Level::TRACE)))]
pub fn rows_to_toc_entries(rows: &[TocEntryRow]) -> Result<Vec<SimpleTocEntry>, Error> {
    // Build a map from row id → (entry, parent_id, position) so we can reconstruct
    // the tree in a single pass.
    use crate::db::types::Uuid7;
    use std::collections::HashMap;

    struct Node {
        entry: SimpleTocEntry,
        parent_id: Option<Uuid7>,
        position: i64,
    }

    let mut nodes: Vec<(Uuid7, Node)> = rows
        .iter()
        .map(|row| {
            let location = decode_location(
                &row.location_kind,
                row.location_exact,
                row.location_uri.as_deref(),
            )?;
            let entry = SimpleTocEntry::Leaf(row.title.clone(), location);
            Ok((
                row.id.clone(),
                Node {
                    entry,
                    parent_id: row.parent_id.0.clone(),
                    position: row.position,
                },
            ))
        })
        .collect::<Result<_, Error>>()?;

    // Sort children into their parents. We process in reverse so we can pop from the
    // end while building child lists.
    let mut id_to_children: HashMap<Uuid7, Vec<(i64, SimpleTocEntry)>> = HashMap::new();
    let mut roots: Vec<(i64, SimpleTocEntry)> = Vec::new();

    // Process in reverse pre-order: children come after parents in the flat list,
    // so we attach in reverse to preserve position ordering after the sort below.
    for (id, node) in nodes.drain(..).rev() {
        let children = id_to_children.remove(&id).unwrap_or_default();

        // Promote Leaf to Container if it has children.
        let entry = if children.is_empty() {
            node.entry
        } else {
            let mut sorted = children;
            sorted.sort_by_key(|(pos, _)| *pos);
            let child_entries = sorted.into_iter().map(|(_, e)| e).collect();
            match node.entry {
                SimpleTocEntry::Leaf(title, loc) => {
                    SimpleTocEntry::Container(title, loc, child_entries)
                }
                SimpleTocEntry::Container(title, loc, _) => {
                    SimpleTocEntry::Container(title, loc, child_entries)
                }
            }
        };

        match node.parent_id {
            Some(pid) => {
                id_to_children
                    .entry(pid)
                    .or_default()
                    .push((node.position, entry));
            }
            None => roots.push((node.position, entry)),
        }
    }

    roots.sort_by_key(|(pos, _)| *pos);
    Ok(roots.into_iter().map(|(_, e)| e).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::types::{OptionalUuid7, Uuid7};
    use crate::document::TocLocation;
    use crate::metadata::FileInfo;
    use std::path::PathBuf;
    use std::str::FromStr;

    #[test]
    fn test_extract_authors() {
        assert_eq!(
            extract_authors("John Doe, Jane Smith"),
            vec!["John Doe", "Jane Smith"]
        );
        assert_eq!(extract_authors("Single Author"), vec!["Single Author"]);
        assert_eq!(extract_authors(""), Vec::<String>::new());
    }

    #[test]
    fn test_info_to_book_row_roundtrip() {
        let fp = Fp::from_u64(1);
        let info = Info {
            title: "Test Book".to_string(),
            author: "Test Author".to_string(),
            file: FileInfo {
                path: PathBuf::from("/tmp/test.pdf"),
                absolute_path: PathBuf::from("/mnt/onboard/tmp/test.pdf"),
                kind: "pdf".to_string(),
                size: 1024,
                mtime: None,
            },
            ..Default::default()
        };

        let row = info_to_book_row(fp, &info);

        assert_eq!(row.fingerprint, fp.to_string());
        assert_eq!(row.title, "Test Book");
        assert_eq!(row.file_path, "/tmp/test.pdf");
        assert_eq!(row.absolute_path, "/mnt/onboard/tmp/test.pdf");
        assert_eq!(row.file_size, 1024);
    }

    #[test]
    fn test_encode_decode_exact_location() {
        let loc = TocLocation::Exact(42);
        let (kind, exact, uri) = encode_location(&loc);
        assert_eq!(kind, "exact");
        assert_eq!(exact, Some(42));
        assert!(uri.is_none());

        let decoded = decode_location(kind, exact, uri.as_deref()).unwrap();
        assert!(matches!(decoded, TocLocation::Exact(42)));
    }

    #[test]
    fn test_encode_decode_uri_location() {
        let loc = TocLocation::Uri("chapter1.xhtml".to_string());
        let (kind, exact, uri) = encode_location(&loc);
        assert_eq!(kind, "uri");
        assert!(exact.is_none());
        assert_eq!(uri.as_deref(), Some("chapter1.xhtml"));

        let decoded = decode_location(kind, exact, uri.as_deref()).unwrap();
        assert!(matches!(decoded, TocLocation::Uri(ref s) if s == "chapter1.xhtml"));
    }

    #[test]
    fn test_rows_to_toc_entries_flat() {
        let rows = vec![
            TocEntryRow {
                book_fingerprint: "fp1".to_string(),
                id: Uuid7::from_str("00000000-0000-7000-8000-000000000001").unwrap(),
                parent_id: OptionalUuid7(None),
                position: 0,
                title: "Chapter 1".to_string(),
                location_kind: "exact".to_string(),
                location_exact: Some(0),
                location_uri: None,
            },
            TocEntryRow {
                book_fingerprint: "fp1".to_string(),
                id: Uuid7::from_str("00000000-0000-7000-8000-000000000002").unwrap(),
                parent_id: OptionalUuid7(None),
                position: 1,
                title: "Chapter 2".to_string(),
                location_kind: "uri".to_string(),
                location_exact: None,
                location_uri: Some("ch2.xhtml".to_string()),
            },
        ];

        let entries = rows_to_toc_entries(&rows).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], SimpleTocEntry::Leaf(t, _) if t == "Chapter 1"));
        assert!(matches!(&entries[1], SimpleTocEntry::Leaf(t, _) if t == "Chapter 2"));
    }

    #[test]
    fn test_rows_to_toc_entries_nested() {
        // Parent at id=1, two children at id=2 and id=3
        let rows = vec![
            TocEntryRow {
                book_fingerprint: "fp1".to_string(),
                id: Uuid7::from_str("00000000-0000-7000-8000-000000000001").unwrap(),
                parent_id: OptionalUuid7(None),
                position: 0,
                title: "Part 1".to_string(),
                location_kind: "exact".to_string(),
                location_exact: Some(0),
                location_uri: None,
            },
            TocEntryRow {
                book_fingerprint: "fp1".to_string(),
                id: Uuid7::from_str("00000000-0000-7000-8000-000000000002").unwrap(),
                parent_id: OptionalUuid7(Some(
                    Uuid7::from_str("00000000-0000-7000-8000-000000000001").unwrap(),
                )),
                position: 0,
                title: "Chapter 1".to_string(),
                location_kind: "exact".to_string(),
                location_exact: Some(1),
                location_uri: None,
            },
            TocEntryRow {
                book_fingerprint: "fp1".to_string(),
                id: Uuid7::from_str("00000000-0000-7000-8000-000000000003").unwrap(),
                parent_id: OptionalUuid7(Some(
                    Uuid7::from_str("00000000-0000-7000-8000-000000000001").unwrap(),
                )),
                position: 1,
                title: "Chapter 2".to_string(),
                location_kind: "exact".to_string(),
                location_exact: Some(2),
                location_uri: None,
            },
        ];

        let entries = rows_to_toc_entries(&rows).unwrap();
        assert_eq!(entries.len(), 1);

        match &entries[0] {
            SimpleTocEntry::Container(title, _, children) => {
                assert_eq!(title, "Part 1");
                assert_eq!(children.len(), 2);
                assert!(matches!(&children[0], SimpleTocEntry::Leaf(t, _) if t == "Chapter 1"));
                assert!(matches!(&children[1], SimpleTocEntry::Leaf(t, _) if t == "Chapter 2"));
            }
            _ => panic!("expected Container"),
        }
    }
}
