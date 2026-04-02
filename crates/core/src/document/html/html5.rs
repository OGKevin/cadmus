//! [`Html5Document`] — an HTML document backed by the html5ever parser.
//!
//! This module exposes a single public type, [`Html5Document`], which wraps
//! the shared [`HtmlBase`] rendering pipeline and uses [`parse_html5`] to
//! build the document tree.
//!
//! Use [`Html5Document`] when HTML5 conformance matters more than offset
//! precision — for example, the dictionary view, which renders content from
//! third-party dictionaries that may contain entities, void elements, and
//! implicitly-closed tags. Because node offsets are synthetic (not byte
//! positions), reading positions must **not** be persisted when using this
//! type.

use super::layout::TextAlign;
use super::xml::parse_html5;
use super::HtmlBase;
use crate::document::{BoundedText, Document, Location, TocEntry};
use crate::framebuffer::Pixmap;
use crate::geom::{Boundary, CycleDir};
use anyhow::Error;
use std::path::{Path, PathBuf};

/// Default viewer stylesheet for dictionary rendering.
const VIEWER_STYLESHEET: &str = "css/dictionary.css";
/// Default user-editable stylesheet for dictionary rendering.
const USER_STYLESHEET: &str = "css/dictionary-user.css";

/// HTML document backed by the html5ever spec-compliant parser.
///
/// Handles HTML entities, void elements (`<br>`, `<img>`), and implicitly-
/// closed block tags correctly per the HTML5 spec. Node offsets are **synthetic**
/// (not byte positions in the source), so this type is **not** suitable for
/// persisting reading positions to disk. Use it for ephemeral rendering such
/// as the dictionary view.
///
/// For documents where offset accuracy matters (EPUB spine chapters, standalone
/// HTML files) use [`HtmlDocument`](super::HtmlDocument) instead.
pub struct Html5Document {
    /// Shared rendering state (tree, engine, page cache, stylesheets).
    pub(super) base: HtmlBase,
}

unsafe impl Send for Html5Document {}
unsafe impl Sync for Html5Document {}

impl Html5Document {
    /// Parses an in-memory HTML string using html5ever and returns a
    /// ready-to-render document.
    ///
    /// Defaults to the dictionary viewer and user stylesheets; call
    /// [`set_viewer_stylesheet`](Self::set_viewer_stylesheet) and
    /// [`set_user_stylesheet`](Self::set_user_stylesheet) to override them.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(text), fields(len = text.len())))]
    pub fn new_from_memory(text: &str) -> Html5Document {
        let content = parse_html5(text);
        Html5Document {
            base: HtmlBase::new(
                content,
                text.len(),
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
        self.base.content = parse_html5(text);
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
}

impl Document for Html5Document {
    /// Returns the current page dimensions in pixels as `(width, height)`.
    #[inline]
    fn dims(&self, _index: usize) -> Option<(f32, f32)> {
        Some((
            self.base.engine.dims.0 as f32,
            self.base.engine.dims.1 as f32,
        ))
    }

    /// Returns the byte length of the source content as a proxy for page count.
    fn pages_count(&self) -> usize {
        self.base.size
    }

    /// Always returns `None`; the dictionary has no table of contents.
    fn toc(&mut self) -> Option<Vec<TocEntry>> {
        None
    }

    /// Always returns `None`; chapter metadata is not applicable.
    fn chapter<'a>(&mut self, _offset: usize, _toc: &'a [TocEntry]) -> Option<(&'a TocEntry, f32)> {
        None
    }

    /// Always returns `None`; chapter-relative navigation is not applicable.
    fn chapter_relative<'a>(
        &mut self,
        _offset: usize,
        _dir: CycleDir,
        _toc: &'a [TocEntry],
    ) -> Option<&'a TocEntry> {
        None
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    fn resolve_location(&mut self, loc: Location) -> Option<usize> {
        self.base.resolve_location(loc)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    fn words(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        self.base.words(loc)
    }

    /// Always returns `None`; line-level layout is not exposed.
    fn lines(&mut self, _loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        None
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    fn images(&mut self, loc: Location) -> Option<(Vec<Boundary>, usize)> {
        self.base.images(loc)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc)))]
    fn links(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        self.base.links(loc)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(loc = ?loc, scale, samples)))]
    fn pixmap(&mut self, loc: Location, scale: f32, samples: usize) -> Option<(Pixmap, usize)> {
        self.base.pixmap(loc, scale, samples)
    }

    #[cfg_attr(
        feature = "otel",
        tracing::instrument(skip(self), fields(width, height, font_size, dpi))
    )]
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

    /// Always returns `None`; the dictionary does not expose a title.
    fn title(&self) -> Option<String> {
        None
    }

    /// Always returns `None`; the dictionary does not expose an author.
    fn author(&self) -> Option<String> {
        None
    }

    fn metadata(&self, key: &str) -> Option<String> {
        self.base.metadata(key)
    }

    /// No-op; `Html5Document` is always constructed from memory and has no
    /// file path to save to.
    fn save(&self, _path: &str) -> Result<(), Error> {
        Ok(())
    }

    fn is_reflowable(&self) -> bool {
        true
    }

    fn has_synthetic_page_numbers(&self) -> bool {
        true
    }
}
