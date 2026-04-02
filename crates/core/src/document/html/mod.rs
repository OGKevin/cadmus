//! HTML document rendering for Cadmus.
//!
//! This module provides two concrete document types that share a common
//! rendering pipeline through [`HtmlBase`]:
//!
//! - [`HtmlDocument`] — backed by the hand-rolled [`XmlParser`]. Node offsets
//!   are exact byte positions in the source string, which is required when
//!   reading positions, bookmarks, and annotations are persisted to disk.
//!   Used for standalone HTML files and EPUB spine chapters.
//!
//! - [`Html5Document`] — backed by html5ever. Node offsets are synthetic.
//!   Used for ephemeral rendering (e.g. the dictionary view) where HTML5
//!   conformance matters more than offset precision.
//!
//! The shared [`HtmlBase`] struct holds the parsed [`XmlTree`], the layout
//! [`Engine`], the page cache, and stylesheet paths. Both document types
//! compose it and delegate all rendering operations to it.

pub mod css;
pub mod dom;
pub mod engine;
pub mod html5;
pub mod layout;
pub mod parse;
pub mod style;
pub mod xml;

pub use html5::Html5Document;

use self::css::CssParser;
use self::dom::{NodeRef, XmlTree};
use self::engine::{Engine, Page, ResourceFetcher};
use self::layout::{DrawCommand, ImageCommand, TextAlign, TextCommand};
use self::layout::{DrawState, LoopContext, RootData, StyleData};
use self::style::StyleSheet;
use self::xml::XmlParser;
use crate::document::{BoundedText, Document, Location, TextLocation, TocEntry};
use crate::framebuffer::Pixmap;
use crate::geom::{Boundary, CycleDir, Edge};
use crate::helpers::{decode_entities, Normalize};
use crate::unit::pt_to_px;
use anyhow::Error;
use fxhash::FxHashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Path to the viewer stylesheet applied to all HTML documents.
const VIEWER_STYLESHEET: &str = "css/html.css";
/// Path to the user-editable stylesheet overlaid on top of the viewer styles.
const USER_STYLESHEET: &str = "css/html-user.css";

/// Map from URI fragment strings (e.g. `"chapter.html#section-2"`) to the
/// document offset of the element with the matching `id` attribute.
type UriCache = FxHashMap<String, usize>;

/// Parser-independent rendering state shared by [`HtmlDocument`] and
/// [`Html5Document`].
///
/// Owns the parsed [`XmlTree`], the layout [`Engine`], a lazily-built page
/// cache, and the stylesheet configuration. Neither parser-specific logic nor
/// the raw source text belongs here — those live in the concrete document types
/// that compose this struct.
pub(crate) struct HtmlBase {
    /// The parsed document tree.
    pub(crate) content: XmlTree,
    /// Layout engine responsible for building draw commands and rendering pages.
    pub(crate) engine: Engine,
    /// Lazily built list of pages. Cleared whenever layout parameters change.
    pub(crate) pages: Vec<Page>,
    /// Directory used to resolve relative resource paths (images, stylesheets).
    pub(crate) parent: PathBuf,
    /// Byte size of the source content, used as a proxy for `pages_count`.
    pub(crate) size: usize,
    /// Path to the viewer stylesheet (typically `css/html.css`).
    pub(crate) viewer_stylesheet: PathBuf,
    /// Path to the user stylesheet (typically `css/html-user.css`).
    pub(crate) user_stylesheet: PathBuf,
    /// When `true`, `<style>` and `<link rel=stylesheet>` tags in the document
    /// are ignored during page building.
    pub(crate) ignore_document_css: bool,
}

impl HtmlBase {
    /// Creates a new `HtmlBase` from an already-parsed tree and configuration.
    pub(crate) fn new(
        content: XmlTree,
        size: usize,
        parent: PathBuf,
        viewer_stylesheet: PathBuf,
        user_stylesheet: PathBuf,
    ) -> Self {
        HtmlBase {
            content,
            engine: Engine::new(),
            pages: Vec::new(),
            parent,
            size,
            viewer_stylesheet,
            user_stylesheet,
            ignore_document_css: false,
        }
    }

    /// Returns the zero-based index of the page that contains `offset`, or
    /// `None` if no page contains it.
    ///
    /// Triggers a full `build_pages` pass the first time it is called after
    /// the page cache has been cleared.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(offset)))]
    pub(crate) fn page_index(&mut self, offset: usize) -> Option<usize> {
        if self.pages.is_empty() {
            self.pages = self.build_pages();
        }
        if self.pages.len() < 2
            || self.pages[1].first().map(|dc| offset < dc.offset()) == Some(true)
        {
            return Some(0);
        } else if self.pages[self.pages.len() - 1]
            .first()
            .map(|dc| offset >= dc.offset())
            == Some(true)
        {
            return Some(self.pages.len() - 1);
        } else {
            for i in 1..self.pages.len() - 1 {
                if self.pages[i].first().map(|dc| offset >= dc.offset()) == Some(true)
                    && self.pages[i + 1].first().map(|dc| offset < dc.offset()) == Some(true)
                {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Resolves a URI containing a fragment (e.g. `chapter.html#intro`) to the
    /// document offset of the element with the matching `id` attribute.
    ///
    /// Returns `None` if the URI has no `#` or no element with the given `id`
    /// is found. Results are written into `cache` so repeated lookups for the
    /// same URI are free.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, cache), fields(uri)))]
    fn resolve_link(&mut self, uri: &str, cache: &mut UriCache) -> Option<usize> {
        let frag_index = uri.find('#')?;
        let name = &uri[..frag_index];
        let content = self.content.clone();
        self.cache_uris(content.root(), name, cache);
        cache.get(uri).cloned()
    }

    /// Recursively walks the tree rooted at `node` and inserts an entry into
    /// `cache` for every element that carries an `id` attribute, mapping
    /// `"name#id"` to the element's offset.
    fn cache_uris(&mut self, node: NodeRef, name: &str, cache: &mut UriCache) {
        if let Some(id) = node.attribute("id") {
            cache.insert(format!("{}#{}", name, id), node.offset());
        }
        for child in node.children() {
            self.cache_uris(child, name, cache);
        }
    }

    /// Builds the complete list of pages from the current document tree and
    /// engine settings.
    ///
    /// Stylesheets are loaded in priority order:
    /// 1. The default viewer stylesheet (`css/html.css`).
    /// 2. A custom viewer stylesheet if one has been set and differs from the
    ///    default.
    /// 3. The user stylesheet.
    /// 4. Inline `<style>` elements and `<link rel=stylesheet>` references
    ///    found in the document `<head>`, unless `ignore_document_css` is set.
    ///
    /// Returns a non-empty list; if the engine produces no draw commands a
    /// single fallback page anchored at offset 0 is returned.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self)))]
    pub(crate) fn build_pages(&mut self) -> Vec<Page> {
        let mut stylesheet = StyleSheet::new();
        let spine_dir = PathBuf::default();

        if let Ok(text) = fs::read_to_string(VIEWER_STYLESHEET) {
            let mut css = CssParser::new(&text).parse();
            stylesheet.append(&mut css, true);
        }

        if self.viewer_stylesheet != Path::new(VIEWER_STYLESHEET) {
            if let Ok(text) = fs::read_to_string(&self.viewer_stylesheet) {
                let mut css = CssParser::new(&text).parse();
                stylesheet.append(&mut css, true);
            }
        }

        if let Ok(text) = fs::read_to_string(&self.user_stylesheet) {
            let mut css = CssParser::new(&text).parse();
            stylesheet.append(&mut css, true);
        }

        if !self.ignore_document_css {
            let mut inner_css = StyleSheet::new();

            if let Some(head) = self.content.root().find("head") {
                for child in head.children() {
                    if child.tag_name() == Some("link")
                        && child.attribute("rel") == Some("stylesheet")
                    {
                        if let Some(href) = child.attribute("href") {
                            if let Some(name) = spine_dir.join(href).normalize().to_str() {
                                if let Ok(buf) = self.parent.fetch(name) {
                                    if let Ok(text) = String::from_utf8(buf) {
                                        let mut css = CssParser::new(&text).parse();
                                        inner_css.append(&mut css, false);
                                    }
                                }
                            }
                        }
                    } else if child.tag_name() == Some("style")
                        && child.attribute("type") == Some("text/css")
                    {
                        let mut css = CssParser::new(&child.text()).parse();
                        inner_css.append(&mut css, false);
                    }
                }
            }

            stylesheet.append(&mut inner_css, true);
        }

        let mut pages = Vec::new();

        let mut rect = self.engine.rect();
        rect.shrink(&self.engine.margin);

        let language = self
            .content
            .root()
            .find("html")
            .and_then(|html| html.attribute("xml:lang"))
            .map(String::from);

        let style = StyleData {
            language,
            font_size: self.engine.font_size,
            line_height: pt_to_px(
                self.engine.line_height * self.engine.font_size,
                self.engine.dpi,
            )
            .round() as i32,
            text_align: self.engine.text_align,
            start_x: rect.min.x,
            end_x: rect.max.x,
            width: rect.max.x - rect.min.x,
            ..Default::default()
        };

        let loop_context = LoopContext::default();
        let mut draw_state = DrawState {
            position: rect.min,
            ..Default::default()
        };

        let root_data = RootData {
            start_offset: 0,
            spine_dir,
            rect,
        };

        pages.push(Vec::new());

        self.engine.build_display_list(
            self.content.root(),
            &style,
            &loop_context,
            &stylesheet,
            &root_data,
            &mut self.parent,
            &mut draw_state,
            &mut pages,
        );

        pages.retain(|page| !page.is_empty());

        if pages.is_empty() {
            pages.push(vec![DrawCommand::Marker(self.content.root().offset())]);
        }

        pages
    }

    /// Resolves a [`Location`] to a concrete document offset, triggering font
    /// loading and page building as needed.
    ///
    /// - `Exact(offset)` — snaps to the first draw command on the page
    ///   containing `offset`.
    /// - `Previous(offset)` / `Next(offset)` — steps one page back or forward.
    /// - `LocalUri` / `Uri` — resolves a URI fragment anchor.
    ///
    /// Returns `None` when the location cannot be resolved (e.g. already on
    /// the first page for `Previous`, or no element with the given `id`).
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    pub(crate) fn resolve_location(&mut self, loc: Location) -> Option<usize> {
        self.engine.load_fonts();

        match loc {
            Location::Exact(offset) => {
                let page_index = self.page_index(offset)?;
                self.pages[page_index].first().map(DrawCommand::offset)
            }
            Location::Previous(offset) => {
                let page_index = self.page_index(offset)?;
                if page_index > 0 {
                    self.pages[page_index - 1].first().map(DrawCommand::offset)
                } else {
                    None
                }
            }
            Location::Next(offset) => {
                let page_index = self.page_index(offset)?;
                if page_index < self.pages.len() - 1 {
                    self.pages[page_index + 1].first().map(DrawCommand::offset)
                } else {
                    None
                }
            }
            Location::LocalUri(_, ref uri) | Location::Uri(ref uri) => {
                let mut cache = FxHashMap::default();
                self.resolve_link(uri, &mut cache)
            }
        }
    }

    /// Returns all text spans on the page identified by `loc`, together with
    /// the resolved page offset.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    pub(crate) fn words(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        let offset = self.resolve_location(loc)?;
        let page_index = self.page_index(offset)?;

        Some((
            self.pages[page_index]
                .iter()
                .filter_map(|dc| match dc {
                    DrawCommand::Text(TextCommand {
                        text, rect, offset, ..
                    }) => Some(BoundedText {
                        text: text.clone(),
                        rect: (*rect).into(),
                        location: TextLocation::Dynamic(*offset),
                    }),
                    _ => None,
                })
                .collect(),
            offset,
        ))
    }

    /// Returns all image bounding rectangles on the page identified by `loc`,
    /// together with the resolved page offset.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    pub(crate) fn images(&mut self, loc: Location) -> Option<(Vec<Boundary>, usize)> {
        let offset = self.resolve_location(loc)?;
        let page_index = self.page_index(offset)?;

        Some((
            self.pages[page_index]
                .iter()
                .filter_map(|dc| match dc {
                    DrawCommand::Image(ImageCommand { rect, .. }) => Some((*rect).into()),
                    _ => None,
                })
                .collect(),
            offset,
        ))
    }

    /// Returns all tappable link spans on the page identified by `loc`,
    /// together with the resolved page offset.
    ///
    /// Both text and image draw commands are included when they carry a URI.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    pub(crate) fn links(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        let offset = self.resolve_location(loc)?;
        let page_index = self.page_index(offset)?;

        Some((
            self.pages[page_index]
                .iter()
                .filter_map(|dc| match dc {
                    DrawCommand::Text(TextCommand {
                        uri, rect, offset, ..
                    })
                    | DrawCommand::Image(ImageCommand {
                        uri, rect, offset, ..
                    }) if uri.is_some() => Some(BoundedText {
                        text: uri.clone().unwrap(),
                        rect: (*rect).into(),
                        location: TextLocation::Dynamic(*offset),
                    }),
                    _ => None,
                })
                .collect(),
            offset,
        ))
    }

    /// Renders the page identified by `loc` to a [`Pixmap`] at the given
    /// `scale` factor and returns it together with the resolved page offset.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc, scale, samples)))]
    pub(crate) fn pixmap(
        &mut self,
        loc: Location,
        scale: f32,
        samples: usize,
    ) -> Option<(Pixmap, usize)> {
        let offset = self.resolve_location(loc)?;
        let page_index = self.page_index(offset)?;
        let page = self.pages[page_index].clone();
        let pixmap = self
            .engine
            .render_page(&page, scale, samples, &mut self.parent)?;

        Some((pixmap, offset))
    }

    /// Reads the `content` attribute of the first `<meta name="key">` element
    /// found in the document `<head>`, decoding any HTML entities.
    pub(crate) fn metadata(&self, key: &str) -> Option<String> {
        self.content
            .root()
            .find("head")
            .and_then(|head| {
                head.children().find(|child| {
                    child.tag_name() == Some("meta") && child.attribute("name") == Some(key)
                })
            })
            .and_then(|child| {
                child
                    .attribute("content")
                    .map(|s| decode_entities(s).into_owned())
            })
    }
}

/// Resolves relative resource paths against the directory that contains the
/// HTML file being rendered.
impl ResourceFetcher for PathBuf {
    fn fetch(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        let mut file = File::open(self.join(name))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

/// HTML document backed by the hand-rolled [`XmlParser`].
///
/// Node offsets are exact byte positions in the source string, making this
/// suitable for EPUB spine chapters and standalone HTML files where reading
/// positions are persisted to disk as absolute byte offsets.
pub struct HtmlDocument {
    /// The raw source text, retained so that [`Document::save`] can write it
    /// back to disk unchanged.
    text: String,
    /// Shared rendering state.
    base: HtmlBase,
}

unsafe impl Send for HtmlDocument {}
unsafe impl Sync for HtmlDocument {}

impl HtmlDocument {
    /// Opens the file at `path`, parses it with [`XmlParser`], and returns a
    /// ready-to-render document.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(path), fields(path = %path.as_ref().display())))]
    pub fn new<P: AsRef<Path>>(path: P) -> Result<HtmlDocument, Error> {
        let mut file = File::open(&path)?;
        let size = file.metadata()?.len() as usize;
        let mut text = String::new();
        file.read_to_string(&mut text)?;
        let mut content = XmlParser::new(&text).parse();
        content.wrap_lost_inlines();
        let parent = path.as_ref().parent().unwrap_or_else(|| Path::new(""));

        Ok(HtmlDocument {
            text,
            base: HtmlBase::new(
                content,
                size,
                parent.to_path_buf(),
                PathBuf::from(VIEWER_STYLESHEET),
                PathBuf::from(USER_STYLESHEET),
            ),
        })
    }

    /// Parses an in-memory HTML string and returns a ready-to-render document.
    ///
    /// The document has no parent directory, so relative resource references
    /// (images, linked stylesheets) will not be resolved.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(text), fields(len = text.len())))]
    pub fn new_from_memory(text: &str) -> HtmlDocument {
        let size = text.len();
        let mut content = XmlParser::new(text).parse();
        content.wrap_lost_inlines();

        HtmlDocument {
            text: text.to_string(),
            base: HtmlBase::new(
                content,
                size,
                PathBuf::default(),
                PathBuf::from(VIEWER_STYLESHEET),
                PathBuf::from(USER_STYLESHEET),
            ),
        }
    }

    /// Replaces the document content with a freshly parsed version of `text`
    /// and clears the page cache.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, text), fields(len = text.len())))]
    pub fn update(&mut self, text: &str) {
        self.base.size = text.len();
        self.base.content = XmlParser::new(text).parse();
        self.base.content.wrap_lost_inlines();
        self.text = text.to_string();
        self.base.pages.clear();
    }

    /// Overrides the page margin. Clears the page cache.
    pub fn set_margin(&mut self, margin: &Edge) {
        self.base.engine.set_margin(margin);
        self.base.pages.clear();
    }

    /// Overrides the base font size in points. Clears the page cache.
    pub fn set_font_size(&mut self, font_size: f32) {
        self.base.engine.set_font_size(font_size);
        self.base.pages.clear();
    }

    /// Overrides the viewer stylesheet path. Clears the page cache.
    pub fn set_viewer_stylesheet<P: AsRef<Path>>(&mut self, path: P) {
        self.base.viewer_stylesheet = path.as_ref().to_path_buf();
        self.base.pages.clear();
    }

    /// Overrides the user stylesheet path. Clears the page cache.
    pub fn set_user_stylesheet<P: AsRef<Path>>(&mut self, path: P) {
        self.base.user_stylesheet = path.as_ref().to_path_buf();
        self.base.pages.clear();
    }

    /// Always returns `None`; category metadata is not embedded in HTML files.
    pub fn categories(&self) -> Option<String> {
        None
    }

    /// Returns the `content` of `<meta name="description">` if present.
    pub fn description(&self) -> Option<String> {
        self.base.metadata("description")
    }

    /// Returns the `xml:lang` attribute of the root `<html>` element if
    /// present.
    pub fn language(&self) -> Option<String> {
        self.base
            .content
            .root()
            .find("html")
            .and_then(|html| html.attribute("xml:lang"))
            .map(String::from)
    }

    /// Returns the first four characters of the `content` of
    /// `<meta name="date">` (i.e. the year) if present.
    pub fn year(&self) -> Option<String> {
        self.base
            .metadata("date")
            .map(|s| s.chars().take(4).collect())
    }
}

impl Document for HtmlDocument {
    #[inline]
    fn dims(&self, _index: usize) -> Option<(f32, f32)> {
        Some((
            self.base.engine.dims.0 as f32,
            self.base.engine.dims.1 as f32,
        ))
    }

    fn pages_count(&self) -> usize {
        self.base.size
    }

    fn toc(&mut self) -> Option<Vec<TocEntry>> {
        None
    }

    fn chapter<'a>(&mut self, _offset: usize, _toc: &'a [TocEntry]) -> Option<(&'a TocEntry, f32)> {
        None
    }

    fn chapter_relative<'a>(
        &mut self,
        _offset: usize,
        _dir: CycleDir,
        _toc: &'a [TocEntry],
    ) -> Option<&'a TocEntry> {
        None
    }

    fn resolve_location(&mut self, loc: Location) -> Option<usize> {
        self.base.resolve_location(loc)
    }

    fn words(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        self.base.words(loc)
    }

    fn lines(&mut self, _loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        None
    }

    fn images(&mut self, loc: Location) -> Option<(Vec<Boundary>, usize)> {
        self.base.images(loc)
    }

    fn links(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        self.base.links(loc)
    }

    fn pixmap(&mut self, loc: Location, scale: f32, samples: usize) -> Option<(Pixmap, usize)> {
        self.base.pixmap(loc, scale, samples)
    }

    fn layout(&mut self, width: u32, height: u32, font_size: f32, dpi: u16) {
        self.base.engine.layout(width, height, font_size, dpi);
        self.base.pages.clear();
    }

    fn set_text_align(&mut self, text_align: TextAlign) {
        self.base.engine.set_text_align(text_align);
        self.base.pages.clear();
    }

    fn set_font_family(&mut self, family_name: &str, search_path: &str) {
        self.base.engine.set_font_family(family_name, search_path);
        self.base.pages.clear();
    }

    fn set_margin_width(&mut self, width: i32) {
        self.base.engine.set_margin_width(width);
        self.base.pages.clear();
    }

    fn set_line_height(&mut self, line_height: f32) {
        self.base.engine.set_line_height(line_height);
        self.base.pages.clear();
    }

    fn set_hyphen_penalty(&mut self, hyphen_penalty: i32) {
        self.base.engine.set_hyphen_penalty(hyphen_penalty);
        self.base.pages.clear();
    }

    fn set_stretch_tolerance(&mut self, stretch_tolerance: f32) {
        self.base.engine.set_stretch_tolerance(stretch_tolerance);
        self.base.pages.clear();
    }

    fn set_ignore_document_css(&mut self, ignore: bool) {
        self.base.ignore_document_css = ignore;
        self.base.pages.clear();
    }

    fn title(&self) -> Option<String> {
        self.base
            .content
            .root()
            .find("head")
            .and_then(|head| {
                head.children()
                    .find(|child| child.tag_name() == Some("title"))
            })
            .map(|child| decode_entities(&child.text()).into_owned())
    }

    fn author(&self) -> Option<String> {
        self.base.metadata("author")
    }

    fn metadata(&self, key: &str) -> Option<String> {
        self.base.metadata(key)
    }

    fn save(&self, path: &str) -> Result<(), Error> {
        let mut file = File::create(path)?;
        file.write_all(self.text.as_bytes()).map_err(Into::into)
    }

    fn is_reflowable(&self) -> bool {
        true
    }

    fn has_synthetic_page_numbers(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::html::layout::DrawCommand;
    use std::path::PathBuf;

    fn setup_doc(html: &str) -> HtmlDocument {
        let root_dir = PathBuf::from(
            std::env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for html tests"),
        );
        let mut doc = HtmlDocument::new_from_memory(html);
        doc.base.engine.layout(600, 800, 12.0, 265);
        doc.base.engine.set_margin_width(3);
        doc.base.engine.load_fonts_from(root_dir);
        doc
    }

    #[test]
    fn nested_list_items_are_indented_further_than_outer_items() {
        let html = r#"<ol><li>Outer item</li><ol style="list-style-type:lower-alpha"><li>Inner item</li></ol></ol>"#;
        let mut doc = setup_doc(html);

        let pages = doc.base.build_pages();
        let all_commands: Vec<_> = pages.iter().flatten().collect();

        let text_x_positions: Vec<i32> = all_commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(tc) => Some(tc.position.x),
                DrawCommand::ExtraText(tc) => Some(tc.position.x),
                _ => None,
            })
            .collect();

        assert!(
            text_x_positions.len() >= 2,
            "expected at least two text items, got {}",
            text_x_positions.len()
        );

        let min_x = text_x_positions.iter().copied().min().unwrap();
        let max_x = text_x_positions.iter().copied().max().unwrap();

        assert!(
            max_x > min_x,
            "inner list item (x={}) should be indented further than outer item (x={})",
            max_x,
            min_x
        );
    }
}
