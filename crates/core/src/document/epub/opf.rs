//! xml5ever-backed parsers for EPUB structural XML files.
//!
//! Three kinds of XML are handled here, none of which need byte-offset tracking:
//!
//! - `META-INF/container.xml` — locates the OPF root file.
//! - The OPF (Open Packaging Format) file — manifest, spine, metadata.
//! - NCX or Navigation Document (NAV) — table of contents.
//!
//! [`OpfDocument`] parses the OPF source once at construction time and stores
//! all data as owned fields. Queries are plain field reads — no re-parsing.

use crate::document::Location;
use crate::document::TocEntry;
use crate::document::html::dom::NodeRef;
use crate::document::html::xml::parse_xml;
use crate::helpers::Normalize;
use percent_encoding::percent_decode_str;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// Extracts the OPF root-file path from a `META-INF/container.xml` string.
///
/// Returns `None` if `rootfile` or its `full-path` attribute is missing.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(text), fields(len = text.len())))]
pub fn opf_path_from_container(text: &str) -> Option<String> {
    parse_xml(text)
        .root()
        .find("rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(String::from)
}

/// Parsed OPF document with all data pre-extracted into owned fields.
///
/// Constructed once via [`OpfDocument::parse`]; all accessors are `O(1)`
/// field reads or cheap iterator passes over already-owned `Vec`s.
pub struct OpfDocument {
    /// Non-empty `<item>` entries from `<manifest>` that carry `id` and `href`.
    manifest: Vec<ManifestEntry>,
    /// `idref` values from `<spine><itemref>` in reading order.
    spine_idrefs: Vec<String>,
    /// The `toc` attribute of `<spine>` — the manifest id of the NCX file.
    spine_toc_id: Option<String>,
    /// Dublin Core metadata keyed by local name, e.g. `"creator"` → value.
    dc_metadata: HashMap<String, String>,
    /// Resolved href of the cover image.
    cover_href: Option<String>,
    /// Series title and the book's position within it.
    series: Option<(String, String)>,
    /// BISAC subject categories.
    categories: BTreeSet<String>,
}

impl OpfDocument {
    /// Parse an OPF source string, extracting all fields eagerly.
    ///
    /// Returns `None` when the tree has no `<package>` element.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(source), fields(len = source.len())))]
    pub fn parse(source: String) -> Option<Self> {
        let tree = parse_xml(&source);
        let root = tree.root();
        root.find("package")?;

        let manifest = extract_manifest(root);
        let (spine_idrefs, spine_toc_id) = extract_spine(root);
        let dc_metadata = extract_dc_metadata(root);
        let cover_href = extract_cover_href(root, &manifest);
        let series = extract_series(root);
        let categories = extract_categories(root);

        Some(OpfDocument {
            manifest,
            spine_idrefs,
            spine_toc_id,
            dc_metadata,
            cover_href,
            series,
            categories,
        })
    }

    /// Returns an empty `OpfDocument` with no manifest, spine, or metadata.
    ///
    /// Used in tests that construct a stub [`super::EpubDocument`] without a
    /// real OPF file.
    #[cfg(test)]
    pub fn empty() -> Self {
        OpfDocument {
            manifest: Vec::new(),
            spine_idrefs: Vec::new(),
            spine_toc_id: None,
            dc_metadata: HashMap::new(),
            cover_href: None,
            series: None,
            categories: BTreeSet::new(),
        }
    }

    /// Returns the manifest entries in document order.
    pub fn manifest(&self) -> &[ManifestEntry] {
        &self.manifest
    }

    /// Returns the `idref` values of all `<itemref>` children of `<spine>`,
    /// together with the `<spine toc="...">` attribute value if present.
    pub fn spine_idrefs(&self) -> (&[String], Option<&str>) {
        (&self.spine_idrefs, self.spine_toc_id.as_deref())
    }

    /// Returns the `<spine toc="...">` manifest id, naming the NCX file.
    pub fn spine_toc_id(&self) -> Option<&str> {
        self.spine_toc_id.as_deref()
    }

    /// Returns the value of a Dublin Core metadata element by local name,
    /// e.g. `"creator"`, `"title"`, `"language"`.
    ///
    /// Also accepts `"dc:local_name"` form for backward compatibility with the
    /// callers that used to pass the full qualified key.
    pub fn metadata_value(&self, key: &str) -> Option<String> {
        let local = key.strip_prefix("dc:").unwrap_or(key);
        self.dc_metadata.get(local).cloned()
    }

    /// Returns the cover image href.
    pub fn cover_image_href(&self) -> Option<String> {
        self.cover_href.clone()
    }

    /// Returns the Calibre / OPF3 series title and the book's position within
    /// that series, e.g. `("Discworld", "3")`.
    ///
    /// Only present when the OPF supplies both the title and the position.
    pub fn series(&self) -> Option<(String, String)> {
        self.series.clone()
    }

    /// Returns BISAC subject categories.
    pub fn categories(&self) -> BTreeSet<String> {
        self.categories.clone()
    }
}

/// A single `<item>` entry from the OPF `<manifest>`.
#[derive(Debug, Clone)]
pub struct ManifestEntry {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: String,
}

/// Reports whether `node` is a Dublin Core element, matched by its `dc:`
/// qualified name.
///
/// shortcut: matched by prefix, not by namespace URI; upgrade if an EPUB binds
/// Dublin Core to another prefix.
fn is_dc(node: NodeRef<'_>) -> bool {
    node.tag_qualified_name()
        .is_some_and(|name| name.starts_with("dc:"))
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(root)))]
fn extract_manifest(root: NodeRef<'_>) -> Vec<ManifestEntry> {
    let Some(manifest) = root.find("manifest") else {
        tracing::debug!("OPF has no manifest element");
        return Vec::new();
    };

    manifest
        .children()
        .filter(|n| n.tag_name() == Some("item"))
        .filter_map(|item| {
            let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) else {
                tracing::debug!(
                    id = ?item.attribute("id"),
                    href = ?item.attribute("href"),
                    "Skipping manifest item without id or href"
                );
                return None;
            };
            let media_type = item.attribute("media-type").unwrap_or("");
            if media_type.is_empty() {
                tracing::debug!(id, "Manifest item has no media-type");
            }
            Some(ManifestEntry {
                id: id.to_string(),
                href: href.to_string(),
                media_type: media_type.to_string(),
                properties: item.attribute("properties").unwrap_or("").to_string(),
            })
        })
        .collect()
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(root)))]
fn extract_spine(root: NodeRef<'_>) -> (Vec<String>, Option<String>) {
    let Some(spine) = root.find("spine") else {
        tracing::debug!("OPF has no spine element");
        return (Vec::new(), None);
    };

    let toc_id = spine.attribute("toc").map(String::from);
    let idrefs = spine
        .children()
        .filter(|n| n.tag_name() == Some("itemref"))
        .filter_map(|item| {
            let idref = item.attribute("idref");
            if idref.is_none() {
                tracing::debug!(
                    attributes = ?item.attributes(),
                    "Skipping spine itemref without idref"
                );
            }
            idref.map(String::from)
        })
        .collect();
    (idrefs, toc_id)
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(root)))]
fn extract_dc_metadata(root: NodeRef<'_>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(md) = root.find("metadata") else {
        return map;
    };

    for child in md.children().filter(|n| n.is_element()) {
        if is_dc(child) {
            let text = child.text();
            if text.is_empty() {
                tracing::debug!(
                    name = ?child.tag_name(),
                    "Ignoring Dublin Core element with empty text"
                );
            } else {
                map.entry(child.tag_name().unwrap_or("").to_string())
                    .or_insert(text);
            }
        }
    }

    map
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(root, manifest), fields(manifest_entries = manifest.len()))
)]
fn extract_cover_href(root: NodeRef<'_>, manifest: &[ManifestEntry]) -> Option<String> {
    let via_meta = root
        .find("metadata")
        .and_then(|md| {
            md.children()
                .find(|n| n.tag_name() == Some("meta") && n.attribute("name") == Some("cover"))
        })
        .and_then(|meta| meta.attribute("content").map(String::from))
        .and_then(
            |cover_id| match manifest.iter().find(|e| e.id == cover_id) {
                Some(entry) => Some(entry.href.clone()),
                None => {
                    tracing::debug!(cover_id, "Cover meta id has no matching manifest item");
                    None
                }
            },
        );

    via_meta.or_else(|| {
        manifest
            .iter()
            .find(|e| {
                (e.href.to_lowercase().contains("cover") || e.id.to_lowercase().contains("cover"))
                    && e.media_type.starts_with("image/")
            })
            .map(|e| e.href.clone())
    })
}

/// Series title and position, kept apart per metadata vocabulary so a title
/// is never paired with a position from the other source.
#[derive(Default)]
struct SeriesParts {
    calibre_title: Option<String>,
    calibre_index: Option<String>,
    epub3_title: Option<String>,
    epub3_index: Option<String>,
}

impl SeriesParts {
    /// Returns the Calibre pair, else the EPUB 3 pair, else `None`.
    ///
    /// Calibre wins because it is the vocabulary Calibre itself writes, and the
    /// two are usually equivalent when a book carries both.
    fn complete_pair(self) -> Option<(String, String)> {
        self.calibre_title
            .zip(self.calibre_index)
            .or_else(|| self.epub3_title.zip(self.epub3_index))
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(root)))]
fn extract_series(root: NodeRef<'_>) -> Option<(String, String)> {
    let md = root.find("metadata")?;
    let mut parts = SeriesParts::default();

    for child in md.children().filter(|n| n.tag_name() == Some("meta")) {
        match child.attribute("name") {
            Some("calibre:series") => {
                parts.calibre_title = child.attribute("content").map(String::from);
            }
            Some("calibre:series_index") => {
                parts.calibre_index = child.attribute("content").map(String::from);
            }
            Some(name) => tracing::debug!(
                name,
                content = child.attribute("content").unwrap_or_default(),
                "Ignoring unrecognized series meta name"
            ),
            None => match child.attribute("property") {
                Some("belongs-to-collection") => parts.epub3_title = Some(child.text()),
                Some("group-position") => parts.epub3_index = Some(child.text()),
                Some(property) => tracing::debug!(
                    property,
                    content = child.attribute("content").unwrap_or_default(),
                    "Ignoring series meta without a recognized property"
                ),
                None => tracing::debug!(
                    content = child.attribute("content").unwrap_or_default(),
                    "Series meta without a name or property attribute"
                ),
            },
        }
    }

    parts.complete_pair()
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(root)))]
fn extract_categories(root: NodeRef<'_>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let Some(md) = root.find("metadata") else {
        return result;
    };

    for child in md
        .children()
        .filter(|n| n.tag_name() == Some("subject") && is_dc(*n))
    {
        let subject = child.text();
        if subject.contains(" / ") {
            for categ in subject.split('|') {
                let start_index = categ.find(" - ").map(|i| i + 3).unwrap_or(0);
                result.insert(categ[start_index..].trim().replace(" / ", "."));
            }
        } else if subject.is_empty() {
            tracing::debug!("Ignoring Dublin Core subject with empty text");
        } else {
            result.insert(subject);
        }
    }

    result
}

/// Parsed table-of-contents from either an NCX or Navigation Document.
pub struct Toc {
    entries: Vec<TocEntry>,
}

impl Toc {
    pub fn into_entries(self) -> Vec<TocEntry> {
        self.entries
    }
}

pub(super) fn epub_type_has_toc(value: &str) -> bool {
    value.split_whitespace().any(|token| token == "toc")
}

fn nav_is_toc(node: NodeRef<'_>) -> bool {
    if node.tag_name() != Some("nav") {
        return false;
    }
    if node.attribute("epub:type").is_some_and(epub_type_has_toc) {
        return true;
    }
    node.attributes().is_some_and(|attrs| {
        attrs
            .iter()
            .any(|(name, value)| name == "type" && epub_type_has_toc(value))
    })
}

/// Parses an NCX or EPUB3 Navigation Document and returns a [`Toc`].
///
/// `name` is the archive path of the TOC file (used to resolve relative links).
/// `is_ncx` selects the NCX `navMap` vs EPUB 3 `nav` document, matching the OPF
/// spine/manifest kind rather than the filename.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(text), fields(len = text.len(), name, is_ncx))
)]
pub fn parse_toc(text: &str, name: &str, is_ncx: bool) -> Option<Toc> {
    let tree = parse_xml(text);
    let toc_dir = Path::new(name).parent().unwrap_or_else(|| Path::new(""));
    let mut index = 0;

    let entries = if is_ncx {
        let Some(map) = tree.root().find("navMap") else {
            tracing::debug!(name, "NCX has no navMap element");
            return None;
        };
        walk_ncx(map, toc_dir, &mut index)
    } else {
        let Some(nav) = tree.root().descendants().find(|n| nav_is_toc(*n)) else {
            tracing::debug!(name, "Navigation document has no toc nav element");
            return None;
        };
        let Some(ol) = nav.children().find(|n| n.tag_name() == Some("ol")) else {
            tracing::debug!(name, "Navigation document toc nav has no ol element");
            return None;
        };
        walk_nav(ol, toc_dir, &mut index)
    };

    Some(Toc { entries })
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(node, toc_dir, index), fields(index = *index))
)]
fn walk_ncx(node: NodeRef<'_>, toc_dir: &Path, index: &mut usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for child in node.children().filter(|n| n.is_element()) {
        if child.tag_name() != Some("navPoint") {
            continue;
        }

        let title = child.find("text").map(|n| n.text()).unwrap_or_default();
        if title.is_empty() {
            tracing::debug!("NCX navPoint has no text title");
        }

        let rel_uri = child
            .find("content")
            .and_then(|n| n.attribute("src"))
            .map(percent_decode)
            .unwrap_or_default();
        if rel_uri.is_empty() {
            tracing::debug!(title, "NCX navPoint has no content src");
        }

        let loc = toc_dir
            .join(&rel_uri)
            .normalize()
            .to_str()
            .map(|uri| Location::Uri(uri.to_string()));

        let current_index = *index;
        *index += 1;

        let sub_entries = if child.children().filter(|n| n.is_element()).count() > 2 {
            walk_ncx(child, toc_dir, index)
        } else {
            Vec::new()
        };

        if let Some(location) = loc {
            entries.push(TocEntry {
                title,
                location,
                index: current_index,
                children: sub_entries,
            });
        } else {
            tracing::debug!(
                title,
                uri = rel_uri,
                "Skipping NCX navPoint with an unresolvable location"
            );
        }
    }

    entries
}

/// Percent-decodes a link target that the XML parser already entity-decoded.
fn percent_decode(target: &str) -> String {
    percent_decode_str(target).decode_utf8_lossy().into_owned()
}

/// Returns the navigation entry title and link target for one `<li>`.
///
/// A group entry has no `<a>` of its own: its title is the direct `<span>`
/// label and it navigates to the first link inside it, so the heading keeps
/// both its name and a reachable target.
fn nav_link<'a>(li: NodeRef<'a>) -> Option<(String, &'a str)> {
    let direct_link = li.children().find(|n| n.tag_name() == Some("a"));
    let anchor = direct_link.or_else(|| li.find("a"))?;
    let href = anchor.attribute("href").filter(|href| !href.is_empty())?;
    let title = match direct_link {
        Some(link) => link.text(),
        None => li
            .children()
            .find(|n| n.tag_name() == Some("span"))
            .map(|span| span.text())
            .unwrap_or_default(),
    };

    Some((title, href))
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(node, toc_dir, index), fields(index = *index))
)]
fn walk_nav(node: NodeRef<'_>, toc_dir: &Path, index: &mut usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for child in node.children().filter(|n| n.tag_name() == Some("li")) {
        let current_index = *index;
        *index += 1;

        let sub_entries = child
            .children()
            .find(|n| n.tag_name() == Some("ol"))
            .map(|ol| walk_nav(ol, toc_dir, index))
            .unwrap_or_default();

        let Some((title, href)) = nav_link(child) else {
            tracing::debug!(
                index = current_index,
                text = %child.text(),
                "Skipping navigation list item without a link"
            );
            entries.extend(sub_entries);
            continue;
        };

        if title.is_empty() {
            tracing::debug!("Navigation entry has no link text");
        }

        let rel_uri = percent_decode(href);
        let loc = toc_dir
            .join(&rel_uri)
            .normalize()
            .to_str()
            .map(|uri| Location::Uri(uri.to_string()));

        if let Some(location) = loc {
            entries.push(TocEntry {
                title,
                location,
                index: current_index,
                children: sub_entries,
            });
        } else {
            tracing::debug!(
                title,
                uri = rel_uri,
                "Skipping navigation entry with an unresolvable location"
            );
        }
    }

    entries
}
#[cfg(test)]
mod tests {
    use super::*;

    const CONTAINER_XML: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf"
              media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    const OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf"
         xmlns:dc="http://purl.org/dc/elements/1.1/"
         xmlns:opf="http://www.idpf.org/2007/opf"
         version="2.0">
  <metadata>
    <dc:title>My Book</dc:title>
    <dc:creator opf:role="aut">Alice Author</dc:creator>
    <dc:language>en</dc:language>
    <dc:date>2024-01-15</dc:date>
    <dc:subject>Science / Physics</dc:subject>
    <meta name="cover" content="cover-img"/>
    <meta name="calibre:series" content="Great Series"/>
    <meta name="calibre:series_index" content="3"/>
  </metadata>
  <manifest>
    <item id="ch1"       href="chapter1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2"       href="chapter2.xhtml" media-type="application/xhtml+xml"/>
    <item id="ncx"       href="toc.ncx"        media-type="application/x-dtbncx+xml"/>
    <item id="cover-img" href="cover.jpg"       media-type="image/jpeg"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
</package>"#;

    const NCX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>
    <navPoint id="np1" playOrder="1">
      <navLabel><text>Chapter One</text></navLabel>
      <content src="chapter1.xhtml"/>
    </navPoint>
    <navPoint id="np2" playOrder="2">
      <navLabel><text>Chapter Two</text></navLabel>
      <content src="chapter2.xhtml"/>
      <navPoint id="np2a" playOrder="3">
        <navLabel><text>Section 2.1</text></navLabel>
        <content src="chapter2.xhtml#s1"/>
      </navPoint>
    </navPoint>
  </navMap>
</ncx>"#;

    const NAV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"
      xmlns:epub="http://www.idpf.org/2007/ops">
  <body>
    <nav epub:type="toc">
      <ol>
        <li><a href="chapter1.xhtml">Chapter One</a></li>
        <li>
          <a href="chapter2.xhtml">Chapter Two</a>
          <ol>
            <li><a href="chapter2.xhtml#s1">Section 2.1</a></li>
          </ol>
        </li>
      </ol>
    </nav>
  </body>
</html>"#;

    #[test]
    fn container_extracts_opf_path() {
        assert_eq!(
            opf_path_from_container(CONTAINER_XML),
            Some("OEBPS/content.opf".to_string())
        );
    }

    #[test]
    fn container_malformed_xml_returns_none() {
        assert_eq!(opf_path_from_container("<not valid xml>>>"), None);
    }

    #[test]
    fn container_missing_full_path_attr_returns_none() {
        let xml = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        assert_eq!(opf_path_from_container(xml), None);
    }

    fn doc() -> OpfDocument {
        OpfDocument::parse(OPF.to_string()).expect("OPF fixture should parse")
    }

    #[test]
    fn opf_parse_fails_on_malformed_xml() {
        assert!(OpfDocument::parse("<bad".to_string()).is_none());
    }

    #[test]
    fn opf_manifest_entries_extracted() {
        let d = doc();
        assert_eq!(d.manifest.len(), 4);

        let ch1 = d.manifest.iter().find(|e| e.id == "ch1").unwrap();
        assert_eq!(ch1.href, "chapter1.xhtml");
        assert_eq!(ch1.media_type, "application/xhtml+xml");

        let cover = d.manifest.iter().find(|e| e.id == "cover-img").unwrap();
        assert_eq!(cover.href, "cover.jpg");
        assert_eq!(cover.media_type, "image/jpeg");
    }

    #[test]
    fn opf_spine_idrefs_in_order_with_toc_id() {
        let d = doc();
        let (idrefs, toc_id) = d.spine_idrefs();
        assert_eq!(idrefs, &["ch1", "ch2"]);
        assert_eq!(toc_id, Some("ncx"));
    }

    #[test]
    fn opf_dc_metadata_extracted() {
        let d = doc();
        assert_eq!(d.metadata_value("title"), Some("My Book".to_string()));
        assert_eq!(
            d.metadata_value("creator"),
            Some("Alice Author".to_string())
        );
        assert_eq!(d.metadata_value("language"), Some("en".to_string()));
        assert_eq!(d.metadata_value("date"), Some("2024-01-15".to_string()));
    }

    #[test]
    fn opf_metadata_value_accepts_dc_prefix() {
        let d = doc();
        assert_eq!(d.metadata_value("dc:title"), Some("My Book".to_string()));
        assert_eq!(
            d.metadata_value("dc:creator"),
            Some("Alice Author".to_string())
        );
    }

    #[test]
    fn opf_metadata_value_missing_key_returns_none() {
        assert_eq!(doc().metadata_value("publisher"), None);
    }

    #[test]
    fn opf_cover_href_via_meta_name() {
        assert_eq!(doc().cover_href, Some("cover.jpg".to_string()));
    }

    #[test]
    fn opf_cover_href_fallback_by_id_contains_cover() {
        let xml = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata/>
  <manifest>
    <item id="cover-image" href="img/cover.jpg" media-type="image/jpeg"/>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="ch1"/></spine>
</package>"#;
        let d = OpfDocument::parse(xml.to_string()).unwrap();
        assert_eq!(d.cover_href, Some("img/cover.jpg".to_string()));
    }

    #[test]
    fn opf_cover_href_none_when_no_cover() {
        let xml = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata/>
  <manifest>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="ch1"/></spine>
</package>"#;
        let d = OpfDocument::parse(xml.to_string()).unwrap();
        assert_eq!(d.cover_href, None);
    }

    #[test]
    fn opf_series_calibre_meta() {
        assert_eq!(
            doc().series,
            Some(("Great Series".to_string(), "3".to_string()))
        );
    }

    #[test]
    fn opf_series_none_when_absent() {
        let xml = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata/>
  <manifest/>
  <spine/>
</package>"#;
        assert_eq!(OpfDocument::parse(xml.to_string()).unwrap().series, None);
    }

    #[test]
    fn opf_categories_bisac_splitting() {
        let d = doc();
        // "Science / Physics" contains " / " so it becomes "Science.Physics"
        // after the BISAC replace.
        assert!(
            d.categories.contains("Science.Physics"),
            "expected 'Science.Physics', got: {:?}",
            d.categories
        );
    }

    #[test]
    fn parse_toc_ncx_top_level_entries() {
        let toc = parse_toc(NCX, "OEBPS/toc.ncx", true).unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Chapter One");
        assert_eq!(entries[1].title, "Chapter Two");
    }

    #[test]
    fn parse_toc_ncx_resolves_paths_relative_to_toc() {
        let toc = parse_toc(NCX, "OEBPS/toc.ncx", true).unwrap();
        let entries = toc.into_entries();
        assert!(
            matches!(&entries[0].location, Location::Uri(u) if u == "OEBPS/chapter1.xhtml"),
            "expected OEBPS/chapter1.xhtml, got {:?}",
            entries[0].location
        );
    }

    #[test]
    fn parse_toc_ncx_nested_entries() {
        let toc = parse_toc(NCX, "OEBPS/toc.ncx", true).unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries[1].children.len(), 1);
        assert_eq!(entries[1].children[0].title, "Section 2.1");
    }

    #[test]
    fn parse_toc_nav_top_level_entries() {
        let toc = parse_toc(NAV, "OEBPS/nav.xhtml", false).unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Chapter One");
        assert_eq!(entries[1].title, "Chapter Two");
    }

    #[test]
    fn parse_toc_nav_nested_entries() {
        let toc = parse_toc(NAV, "OEBPS/nav.xhtml", false).unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries[1].children.len(), 1);
        assert_eq!(entries[1].children[0].title, "Section 2.1");
    }

    #[test]
    fn parse_toc_malformed_xml_returns_none() {
        assert!(parse_toc("<bad", "toc.ncx", true).is_none());
    }

    const OPF_SERIES_MIXED_SOURCES: &str = r##"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata>
    <meta name="calibre:series" content="Series A"/>
    <meta name="calibre:series_index" content="2"/>
    <meta property="belongs-to-collection" id="c01">Collection B</meta>
    <meta property="group-position" refines="#c01">9</meta>
  </metadata>
  <manifest/>
  <spine/>
</package>"##;

    const OPF_SERIES_EPUB3_ONLY: &str = r##"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata>
    <meta property="belongs-to-collection" id="c01">Collection B</meta>
    <meta property="group-position" refines="#c01">9</meta>
  </metadata>
  <manifest/>
  <spine/>
</package>"##;

    const OPF_SERIES_PARTIAL: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata>
    <meta name="calibre:series" content="Series A"/>
    <meta property="group-position">9</meta>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;

    #[test]
    fn opf_series_prefers_calibre_pair_over_epub3_pair() {
        let d = OpfDocument::parse(OPF_SERIES_MIXED_SOURCES.to_string()).unwrap();
        assert_eq!(
            d.series,
            Some(("Series A".to_string(), "2".to_string())),
            "title and position must come from the same source"
        );
    }

    #[test]
    fn opf_series_uses_epub3_pair_when_calibre_pair_absent() {
        let d = OpfDocument::parse(OPF_SERIES_EPUB3_ONLY.to_string()).unwrap();
        assert_eq!(
            d.series,
            Some(("Collection B".to_string(), "9".to_string()))
        );
    }

    #[test]
    fn opf_series_none_when_no_pair_is_complete() {
        let d = OpfDocument::parse(OPF_SERIES_PARTIAL.to_string()).unwrap();
        assert_eq!(d.series, None, "a lone part of either pair is not a series");
    }

    const NAV_GROUP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"
      xmlns:epub="http://www.idpf.org/2007/ops">
  <body>
    <nav epub:type="toc">
      <ol>
        <li>
          <span>Part One</span>
          <ol>
            <li><a href="chapter1.xhtml">Chapter One</a></li>
            <li><a href="chapter2.xhtml">Chapter Two</a></li>
          </ol>
        </li>
        <li><a href="chapter3.xhtml">Chapter Three</a></li>
      </ol>
    </nav>
  </body>
</html>"#;

    const NAV_UNREACHABLE_GROUPS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"
      xmlns:epub="http://www.idpf.org/2007/ops">
  <body>
    <nav epub:type="toc">
      <ol>
        <li><span>Dedication</span></li>
        <li>
          <a>Broken Group</a>
          <ol>
            <li><a href="chapter1.xhtml">Chapter One</a></li>
          </ol>
        </li>
      </ol>
    </nav>
  </body>
</html>"#;

    #[test]
    fn parse_toc_nav_group_heading_uses_span_title_and_first_link() {
        let entries = parse_toc(NAV_GROUP, "OEBPS/nav.xhtml", false)
            .unwrap()
            .into_entries();
        assert_eq!(entries.len(), 2, "group heading must be kept");
        assert_eq!(entries[0].title, "Part One");
        assert!(
            matches!(&entries[0].location, Location::Uri(u) if u == "OEBPS/chapter1.xhtml"),
            "group heading must point at its first chapter, got {:?}",
            entries[0].location
        );
        assert_eq!(entries[0].children.len(), 2);
        assert_eq!(entries[0].children[0].title, "Chapter One");
        assert_eq!(entries[1].title, "Chapter Three");
    }

    #[test]
    fn parse_toc_nav_entry_without_href_keeps_nested_entries() {
        let entries = parse_toc(NAV_UNREACHABLE_GROUPS, "OEBPS/nav.xhtml", false)
            .unwrap()
            .into_entries();
        assert_eq!(
            entries.len(),
            1,
            "entries without a usable link are skipped, but nested chapters survive: {entries:?}"
        );
        assert_eq!(entries[0].title, "Chapter One");
        assert!(
            matches!(&entries[0].location, Location::Uri(u) if u == "OEBPS/chapter1.xhtml"),
            "nested chapter must keep its own location, got {:?}",
            entries[0].location
        );
    }

    const OPF_ENTITIES: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf"
         xmlns:dc="http://purl.org/dc/elements/1.1/"
         version="2.0">
  <metadata>
    <dc:title>Learning &amp;lt;XML&amp;gt;</dc:title>
    <dc:subject>Saga &amp;#65;</dc:subject>
    <meta name="calibre:series" content="Series &amp;#65;"/>
    <meta name="calibre:series_index" content="1"/>
  </metadata>
  <manifest/>
  <spine/>
</package>"#;

    #[test]
    fn opf_entities_decoded_exactly_once() {
        let d = OpfDocument::parse(OPF_ENTITIES.to_string()).unwrap();
        assert_eq!(
            d.metadata_value("title"),
            Some("Learning &lt;XML&gt;".to_string()),
            "character references must be decoded once, by the XML parser"
        );
        assert!(
            d.categories.contains("Saga &#65;"),
            "subject must keep literal entity text, got {:?}",
            d.categories
        );
        assert_eq!(
            d.series,
            Some(("Series &#65;".to_string(), "1".to_string()))
        );
    }

    const NAV_ESCAPED_HREF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"
      xmlns:epub="http://www.idpf.org/2007/ops">
  <body>
    <nav epub:type="toc">
      <ol>
        <li><a href="chapter%20one&amp;amp;two.xhtml">One</a></li>
      </ol>
    </nav>
  </body>
</html>"#;

    const NCX_ESCAPED_SRC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>
    <navPoint id="np1" playOrder="1">
      <navLabel><text>One</text></navLabel>
      <content src="chapter%20one&amp;amp;two.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#;

    #[test]
    fn parse_toc_nav_href_percent_decoded_but_entities_not_decoded_twice() {
        let entries = parse_toc(NAV_ESCAPED_HREF, "OEBPS/nav.xhtml", false)
            .unwrap()
            .into_entries();
        assert!(
            matches!(&entries[0].location, Location::Uri(u) if u == "OEBPS/chapter one&amp;two.xhtml"),
            "percent escapes decode, the source &amp;amp; stays one level of &, got {:?}",
            entries[0].location
        );
    }

    #[test]
    fn parse_toc_ncx_src_percent_decoded_but_entities_not_decoded_twice() {
        let entries = parse_toc(NCX_ESCAPED_SRC, "OEBPS/toc.ncx", true)
            .unwrap()
            .into_entries();
        assert!(
            matches!(&entries[0].location, Location::Uri(u) if u == "OEBPS/chapter one&amp;two.xhtml"),
            "percent escapes decode, the source &amp;amp; stays one level of &, got {:?}",
            entries[0].location
        );
    }
}
