//! Lightweight, roxmltree-backed parsers for EPUB structural XML files.
//!
//! Three kinds of XML are handled here — none of which are HTML and none of
//! which require byte-offset tracking:
//!
//! - `META-INF/container.xml` — locates the OPF root file.
//! - The OPF (Open Packaging Format) file — manifest, spine, metadata.
//! - NCX or Navigation Document (NAV) — table of contents.
//!
//! [`OpfDocument`] parses the OPF source once at construction time and stores
//! all data as owned fields. Queries are plain field reads — no re-parsing.

use crate::document::Location;
use crate::document::TocEntry;
use crate::helpers::{Normalize, decode_entities};
use percent_encoding::percent_decode_str;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// XML namespace URI for Dublin Core Elements 1.1.
///
/// EPUB OPF files use Dublin Core to store book metadata such as `<dc:title>`,
/// `<dc:creator>`, `<dc:language>`, etc. The `dc:` prefix in the source is a
/// local alias that can vary between EPUBs; roxmltree resolves it to this URI,
/// which is the stable identifier used for matching.
const DC_NS: &str = "http://purl.org/dc/elements/1.1/";

/// Extracts the OPF root-file path from a `META-INF/container.xml` string.
///
/// Returns `None` if the document is malformed or the `full-path` attribute
/// is missing.
pub fn opf_path_from_container(text: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(text).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(String::from)
}

/// Parsed OPF document with all data pre-extracted into owned fields.
///
/// Constructed once via [`OpfDocument::parse`]; all accessors are `O(1)`
/// field reads or cheap iterator passes over already-owned `Vec`s.
pub struct OpfDocument {
    pub manifest: Vec<ManifestEntry>,
    /// `idref` values from `<spine><itemref>` in reading order.
    pub spine_idrefs: Vec<String>,
    /// The `toc` attribute of `<spine>` — the manifest id of the NCX file.
    pub spine_toc_id: Option<String>,
    /// Dublin Core metadata keyed by local name, e.g. `"creator"` → value.
    dc_metadata: HashMap<String, String>,
    pub cover_href: Option<String>,
    pub series: Option<(String, String)>,
    pub categories: BTreeSet<String>,
}

impl OpfDocument {
    /// Parse an OPF source string, extracting all fields eagerly.
    ///
    /// Returns `None` if the XML is malformed.
    pub fn parse(source: String) -> Option<Self> {
        let doc = roxmltree::Document::parse(&source).ok()?;

        let manifest = extract_manifest(&doc);
        let (spine_idrefs, spine_toc_id) = extract_spine(&doc);
        let dc_metadata = extract_dc_metadata(&doc);
        let cover_href = extract_cover_href(&doc, &manifest);
        let series = extract_series(&doc);
        let categories = extract_categories(&doc);

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

    /// Returns the `idref` values of all `<itemref>` children of `<spine>`,
    /// together with the `<spine toc="...">` attribute value if present.
    pub fn spine_idrefs(&self) -> (&[String], Option<&str>) {
        (&self.spine_idrefs, self.spine_toc_id.as_deref())
    }

    /// Returns the href of the TOC file: first tries `<spine toc="ncx-id">`,
    /// then looks for a manifest item with `properties="nav"`.
    pub fn toc_href(&self) -> Option<String> {
        let via_ncx = self
            .spine_toc_id
            .as_deref()
            .and_then(|ncx_id| self.manifest.iter().find(|e| e.id == ncx_id))
            .map(|e| e.href.clone());

        via_ncx.or_else(|| {
            self.manifest
                .iter()
                .find(|e| e.properties.split_whitespace().any(|p| p == "nav"))
                .map(|e| e.href.clone())
        })
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

    /// Returns the Calibre / OPF3 series title and position.
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

fn extract_manifest(doc: &roxmltree::Document<'_>) -> Vec<ManifestEntry> {
    doc.descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "manifest")
        .flat_map(|manifest| {
            manifest
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "item")
                .filter_map(|item| {
                    let id = item.attribute("id")?;
                    let href = item.attribute("href")?;
                    Some(ManifestEntry {
                        id: id.to_string(),
                        href: href.to_string(),
                        media_type: item.attribute("media-type").unwrap_or("").to_string(),
                        properties: item.attribute("properties").unwrap_or("").to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn extract_spine(doc: &roxmltree::Document<'_>) -> (Vec<String>, Option<String>) {
    let spine = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "spine");

    let toc_id = spine
        .as_ref()
        .and_then(|s| s.attribute("toc"))
        .map(String::from);

    let idrefs = spine
        .iter()
        .flat_map(|s| s.children())
        .filter(|n| n.is_element() && n.tag_name().name() == "itemref")
        .filter_map(|item| item.attribute("idref").map(String::from))
        .collect();

    (idrefs, toc_id)
}

fn extract_dc_metadata(doc: &roxmltree::Document<'_>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(md) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
    else {
        return map;
    };

    for child in md.children().filter(|n| n.is_element()) {
        if child.tag_name().namespace() == Some(DC_NS)
            && let Some(text) = child.text()
        {
            map.entry(child.tag_name().name().to_string())
                .or_insert_with(|| decode_entities(text).into_owned());
        }
    }

    map
}

fn extract_cover_href(doc: &roxmltree::Document<'_>, manifest: &[ManifestEntry]) -> Option<String> {
    let via_meta = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
        .and_then(|md| {
            md.children().find(|n| {
                n.is_element()
                    && n.tag_name().name() == "meta"
                    && n.attribute("name") == Some("cover")
            })
        })
        .and_then(|meta| meta.attribute("content").map(String::from))
        .and_then(|cover_id| {
            manifest
                .iter()
                .find(|e| e.id == cover_id)
                .map(|e| e.href.clone())
        });

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

fn extract_series(doc: &roxmltree::Document<'_>) -> Option<(String, String)> {
    let md = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")?;

    let mut title = None;
    let mut index = None;

    for child in md.children().filter(|n| n.is_element()) {
        if child.tag_name().name() == "meta" {
            match child.attribute("name") {
                Some("calibre:series") => {
                    title = child
                        .attribute("content")
                        .map(|s| decode_entities(s).into_owned());
                }
                Some("calibre:series_index") => {
                    index = child
                        .attribute("content")
                        .map(|s| decode_entities(s).into_owned());
                }
                _ => {}
            }
            if child.attribute("property") == Some("belongs-to-collection") {
                title = child.text().map(|t| decode_entities(t).into_owned());
            } else if child.attribute("property") == Some("group-position") {
                index = child.text().map(|t| decode_entities(t).into_owned());
            }
        }
    }

    title.zip(index)
}

fn extract_categories(doc: &roxmltree::Document<'_>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();

    let Some(md) = doc
        .descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "metadata")
    else {
        return result;
    };

    for child in md.children().filter(|n| n.is_element()) {
        if child.tag_name().name() == "subject"
            && child.tag_name().namespace() == Some(DC_NS)
            && let Some(text) = child.text()
        {
            let subject = decode_entities(text);
            if subject.contains(" / ") {
                for categ in subject.split('|') {
                    let start_index = categ.find(" - ").map(|i| i + 3).unwrap_or(0);
                    result.insert(categ[start_index..].trim().replace(" / ", "."));
                }
            } else {
                result.insert(subject.into_owned());
            }
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

/// Parses an NCX or EPUB3 Navigation Document and returns a [`Toc`].
///
/// `name` is the archive path of the TOC file (used to decide NCX vs NAV and
/// to resolve relative links).
pub fn parse_toc(text: &str, name: &str) -> Option<Toc> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let toc_dir = Path::new(name).parent().unwrap_or_else(|| Path::new(""));
    let mut index = 0;

    let entries = if name.ends_with(".ncx") {
        doc.descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "navMap")
            .map(|map| walk_ncx(map, toc_dir, &mut index))
            .unwrap_or_default()
    } else {
        doc.descendants()
            .find(|n| {
                n.is_element()
                    && n.tag_name().name() == "nav"
                    && n.attribute("epub:type") == Some("toc")
            })
            .or_else(|| {
                doc.descendants().find(|n| {
                    n.is_element()
                        && n.tag_name().name() == "nav"
                        && n.attributes()
                            .any(|a| a.name() == "type" && a.value() == "toc")
                })
            })
            .and_then(|nav| {
                nav.children()
                    .find(|n| n.is_element() && n.tag_name().name() == "ol")
            })
            .map(|ol| walk_nav(ol, toc_dir, &mut index))
            .unwrap_or_default()
    };

    Some(Toc { entries })
}

fn walk_ncx(node: roxmltree::Node<'_, '_>, toc_dir: &Path, index: &mut usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for child in node.children().filter(|n| n.is_element()) {
        if child.tag_name().name() != "navPoint" {
            continue;
        }

        let title = child
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "text")
            .and_then(|n| n.text())
            .map(|t| decode_entities(t).into_owned())
            .unwrap_or_default();

        let rel_uri = child
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "content")
            .and_then(|n| n.attribute("src"))
            .map(|src| {
                percent_decode_str(&decode_entities(src))
                    .decode_utf8_lossy()
                    .into_owned()
            })
            .unwrap_or_default();

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
        }
    }

    entries
}

fn walk_nav(node: roxmltree::Node<'_, '_>, toc_dir: &Path, index: &mut usize) -> Vec<TocEntry> {
    let mut entries = Vec::new();

    for child in node.children().filter(|n| n.is_element()) {
        if child.tag_name().name() != "li" {
            continue;
        }

        let link = child
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "a");

        let title = link
            .and_then(|a| a.text())
            .map(|t| decode_entities(t).into_owned())
            .unwrap_or_default();

        let rel_uri = link
            .and_then(|a| a.attribute("href"))
            .map(|href| {
                percent_decode_str(&decode_entities(href))
                    .decode_utf8_lossy()
                    .into_owned()
            })
            .unwrap_or_default();

        let loc = toc_dir
            .join(&rel_uri)
            .normalize()
            .to_str()
            .map(|uri| Location::Uri(uri.to_string()));

        let current_index = *index;
        *index += 1;

        let sub_entries = child
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "ol")
            .map(|ol| walk_nav(ol, toc_dir, index))
            .unwrap_or_default();

        if let Some(location) = loc {
            entries.push(TocEntry {
                title,
                location,
                index: current_index,
                children: sub_entries,
            });
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
    fn toc_href_via_spine_toc_attribute() {
        assert_eq!(doc().toc_href(), Some("toc.ncx".to_string()));
    }

    #[test]
    fn toc_href_via_nav_properties() {
        let xml = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata/>
  <manifest>
    <item id="nav" href="nav.xhtml"
          media-type="application/xhtml+xml" properties="nav"/>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="ch1"/></spine>
</package>"#;
        let d = OpfDocument::parse(xml.to_string()).unwrap();
        assert_eq!(d.toc_href(), Some("nav.xhtml".to_string()));
    }

    #[test]
    fn toc_href_none_when_no_toc() {
        let xml = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata/>
  <manifest>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="ch1"/></spine>
</package>"#;
        assert_eq!(
            OpfDocument::parse(xml.to_string()).unwrap().toc_href(),
            None
        );
    }

    #[test]
    fn parse_toc_ncx_top_level_entries() {
        let toc = parse_toc(NCX, "OEBPS/toc.ncx").unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Chapter One");
        assert_eq!(entries[1].title, "Chapter Two");
    }

    #[test]
    fn parse_toc_ncx_resolves_paths_relative_to_toc() {
        let toc = parse_toc(NCX, "OEBPS/toc.ncx").unwrap();
        let entries = toc.into_entries();
        assert!(
            matches!(&entries[0].location, Location::Uri(u) if u == "OEBPS/chapter1.xhtml"),
            "expected OEBPS/chapter1.xhtml, got {:?}",
            entries[0].location
        );
    }

    #[test]
    fn parse_toc_ncx_nested_entries() {
        let toc = parse_toc(NCX, "OEBPS/toc.ncx").unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries[1].children.len(), 1);
        assert_eq!(entries[1].children[0].title, "Section 2.1");
    }

    #[test]
    fn parse_toc_nav_top_level_entries() {
        let toc = parse_toc(NAV, "OEBPS/nav.xhtml").unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Chapter One");
        assert_eq!(entries[1].title, "Chapter Two");
    }

    #[test]
    fn parse_toc_nav_nested_entries() {
        let toc = parse_toc(NAV, "OEBPS/nav.xhtml").unwrap();
        let entries = toc.into_entries();
        assert_eq!(entries[1].children.len(), 1);
        assert_eq!(entries[1].children[0].title, "Section 2.1");
    }

    #[test]
    fn parse_toc_malformed_xml_returns_none() {
        assert!(parse_toc("<bad", "toc.ncx").is_none());
    }
}
