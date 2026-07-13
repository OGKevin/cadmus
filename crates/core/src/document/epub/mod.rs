use super::html::css::CssParser;
use super::html::dom::NodeRef;
use super::html::engine::{Engine, Page, ResourceFetcher};
use super::html::layout::TextAlign;
use super::html::layout::{DrawCommand, DrawState, ImageCommand, RootData, TextCommand};
use super::html::layout::{LoopContext, StyleData};
use super::html::style::StyleSheet;
use super::html::xml::parse_html5;
use super::pdf::PdfOpener;
use crate::document::{BoundedText, Document, Location, TextLocation, TocEntry, chapter_from_uri};
use crate::framebuffer::Pixmap;
use crate::geom::{Boundary, CycleDir};
use crate::helpers::{Normalize, decode_entities};
use crate::unit::pt_to_px;
use anyhow::{Error, format_err};
use fxhash::FxHashMap;
use opf::{ManifestEntry, OpfDocument, opf_path_from_container, parse_toc};
use percent_encoding::percent_decode_str;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

mod opf;

const VIEWER_STYLESHEET: &str = "css/epub.css";
const USER_STYLESHEET: &str = "css/epub-user.css";

type UriCache = FxHashMap<String, usize>;

impl<R: Read + Seek> ResourceFetcher for ZipArchive<R> {
    fn fetch(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        let mut file = self.by_name(name)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

/// Generic EPUB document that works with any Read + Seek source.
pub struct EpubDocument<R: Read + Seek> {
    archive: ZipArchive<R>,
    info: OpfDocument,
    parent: PathBuf,
    engine: Engine,
    spine: Vec<Chunk>,
    cache: FxHashMap<usize, Vec<Page>>,
    ignore_document_css: bool,
}

/// Type alias for file-based EPUB documents (backward compatibility).
pub type EpubDocumentFile = EpubDocument<File>;

/// Type alias for static EPUB documents (zero-copy for embedded assets).
pub type EpubDocumentStatic = EpubDocument<Cursor<&'static [u8]>>;

#[derive(Debug)]
struct Chunk {
    path: String,
    size: usize,
}

unsafe impl<R: Read + Seek> Send for EpubDocument<R> {}
unsafe impl<R: Read + Seek> Sync for EpubDocument<R> {}

/// Resolves spine `idref` values to [`Chunk`]s by probing the zip archive.
///
/// For each `idref` the function:
/// 1. Looks up the matching [`ManifestEntry`] to get the file's `href`.
/// 2. Decodes the `href` (HTML entities + percent-encoding) and resolves it
///    relative to `opf_parent` to obtain the archive-relative path.
/// 3. Opens the entry in the archive to read its **uncompressed byte size**.
///    The size is stored on [`Chunk`] and used as the chapter's contribution
///    to the global byte-offset coordinate system (reading positions, bookmarks).
///
/// Entries with no matching manifest item or with a non-UTF-8 path are silently
/// skipped — those indicate a structurally malformed EPUB. Entries whose file
/// is missing from the archive are logged at error level and skipped.
fn build_spine<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &[ManifestEntry],
    idrefs: &[String],
    opf_parent: &Path,
) -> Vec<Chunk> {
    idrefs
        .iter()
        .filter_map(|idref| {
            let entry = manifest.iter().find(|e| e.id == *idref)?;
            let href = decode_entities(&entry.href);
            let href = percent_decode_str(&href).decode_utf8_lossy();
            let path = opf_parent.join::<&str>(href.as_ref()).normalize();
            let path = path.to_str()?.to_string();

            let result = archive.by_name(&path).map(|zf| Chunk {
                path: path.clone(),
                size: zf.size() as usize,
            });

            match result {
                Ok(chunk) => Some(chunk),
                Err(e) => {
                    tracing::error!(
                        path,
                        error = %e,
                        "spine entry missing from archive"
                    );
                    None
                }
            }
        })
        .collect()
}

impl<R: Read + Seek> EpubDocument<R> {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn from_archive(mut archive: ZipArchive<R>, install_dir: &Path) -> Result<Self, Error> {
        let opf_path = {
            let mut zf = archive.by_name("META-INF/container.xml")?;
            let mut text = String::new();
            zf.read_to_string(&mut text)?;
            opf_path_from_container(&text)
        }
        .ok_or_else(|| format_err!("can't get the OPF path"))?;

        let parent = Path::new(&opf_path)
            .parent()
            .unwrap_or_else(|| Path::new(""));

        let opf_text = {
            let mut zf = archive.by_name(&opf_path)?;
            let mut text = String::new();
            zf.read_to_string(&mut text)?;
            text
        };

        let info = OpfDocument::parse(opf_text)
            .ok_or_else(|| format_err!("failed to parse OPF document"))?;

        let (idrefs, _) = info.spine_idrefs();
        let spine = build_spine(&mut archive, &info.manifest, idrefs, parent);

        if spine.is_empty() {
            return Err(format_err!("the spine is empty"));
        }

        Ok(EpubDocument {
            archive,
            info,
            parent: parent.to_path_buf(),
            engine: Engine::new(install_dir),
            spine,
            cache: FxHashMap::default(),
            ignore_document_css: false,
        })
    }

    fn offset(&self, index: usize) -> usize {
        self.spine.iter().take(index).map(|c| c.size).sum()
    }

    fn size(&self) -> usize {
        self.offset(self.spine.len())
    }

    fn vertebra_coordinates_with<F>(&self, test: F) -> Option<(usize, usize)>
    where
        F: Fn(usize, usize) -> bool,
    {
        let mut start_offset = 0;
        let mut end_offset = start_offset;
        let mut index = 0;

        while index < self.spine.len() {
            end_offset += self.spine[index].size;
            if test(index, end_offset) {
                return Some((index, start_offset));
            }
            start_offset = end_offset;
            index += 1;
        }

        None
    }

    fn vertebra_coordinates(&self, offset: usize) -> Option<(usize, usize)> {
        self.vertebra_coordinates_with(|_, end_offset| offset < end_offset)
    }

    fn vertebra_coordinates_from_name(&self, name: &str) -> Option<(usize, usize)> {
        self.vertebra_coordinates_with(|index, _| self.spine[index].path == name)
    }

    #[inline]
    fn page_index(&mut self, offset: usize, index: usize, start_offset: usize) -> Option<usize> {
        if !self.cache.contains_key(&index) {
            let display_list = self.build_display_list(index, start_offset);
            self.cache.insert(index, display_list);
        }
        self.cache.get(&index).map(|display_list| {
            if display_list.len() < 2
                || display_list[1].first().map(|dc| offset < dc.offset()) == Some(true)
            {
                return 0;
            } else if display_list[display_list.len() - 1]
                .first()
                .map(|dc| offset >= dc.offset())
                == Some(true)
            {
                return display_list.len() - 1;
            } else {
                for i in 1..display_list.len() - 1 {
                    if display_list[i].first().map(|dc| offset >= dc.offset()) == Some(true)
                        && display_list[i + 1].first().map(|dc| offset < dc.offset()) == Some(true)
                    {
                        return i;
                    }
                }
            }
            0
        })
    }

    fn resolve_link(&mut self, uri: &str, cache: &mut UriCache) -> Option<usize> {
        let frag_index_opt = uri.find('#');
        let name = &uri[..frag_index_opt.unwrap_or_else(|| uri.len())];

        let (index, start_offset) = self.vertebra_coordinates_from_name(name)?;

        if frag_index_opt.is_some() {
            let mut text = String::new();
            {
                let mut zf = self.archive.by_name(name).ok()?;
                zf.read_to_string(&mut text).ok()?;
            }
            let root = parse_html5(&text);
            self.cache_uris(root.root(), name, start_offset, cache);
            cache.get(uri).cloned()
        } else {
            let page_index = self.page_index(start_offset, index, start_offset)?;
            let offset = self
                .cache
                .get(&index)
                .and_then(|display_list| display_list[page_index].first())
                .map(DrawCommand::offset)?;
            cache.insert(uri.to_string(), offset);
            Some(offset)
        }
    }

    fn cache_uris(&mut self, node: NodeRef, name: &str, start_offset: usize, cache: &mut UriCache) {
        if let Some(id) = node.attribute("id") {
            let location = start_offset + node.offset();
            cache.insert(format!("{}#{}", name, id), location);
        }
        for child in node.children() {
            self.cache_uris(child, name, start_offset, cache);
        }
    }

    fn build_display_list(&mut self, index: usize, start_offset: usize) -> Vec<Page> {
        let mut text = String::new();
        let mut spine_dir = PathBuf::default();

        {
            let path = &self.spine[index].path;
            if let Some(parent) = Path::new(path).parent() {
                spine_dir = parent.to_path_buf();
            }

            if let Ok(mut zf) = self.archive.by_name(path) {
                zf.read_to_string(&mut text).ok();
            }
        }

        let mut root = parse_html5(&text);
        root.wrap_lost_inlines();

        let mut stylesheet = StyleSheet::new();

        if let Ok(text) = fs::read_to_string(VIEWER_STYLESHEET) {
            let mut css = CssParser::new(&text).parse();
            stylesheet.append(&mut css, true);
        }

        if let Ok(text) = fs::read_to_string(USER_STYLESHEET) {
            let mut css = CssParser::new(&text).parse();
            stylesheet.append(&mut css, true);
        }

        if !self.ignore_document_css {
            let mut inner_css = StyleSheet::new();
            if let Some(head) = root.root().find("head") {
                for child in head.children() {
                    if child.tag_name() == Some("link")
                        && child.attribute("rel") == Some("stylesheet")
                    {
                        if let Some(href) = child.attribute("href") {
                            if let Some(name) = spine_dir.join(href).normalize().to_str() {
                                let mut text = String::new();
                                if let Ok(mut zf) = self.archive.by_name(name) {
                                    zf.read_to_string(&mut text).ok();
                                    let mut css = CssParser::new(&text).parse();
                                    inner_css.append(&mut css, false);
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

        let mut display_list = Vec::new();

        if let Some(body) = root.root().find("body") {
            let mut rect = self.engine.rect();
            rect.shrink(&self.engine.margin);

            let language = self.language().or_else(|| {
                root.root()
                    .find("html")
                    .and_then(|html| html.attribute("xml:lang"))
                    .map(String::from)
            });

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
                start_offset,
                spine_dir,
                rect,
            };

            display_list.push(Vec::new());

            self.engine.build_display_list(
                body,
                &style,
                &loop_context,
                &stylesheet,
                &root_data,
                &mut self.archive,
                &mut draw_state,
                &mut display_list,
            );

            display_list.retain(|page| !page.is_empty());

            if display_list.is_empty() {
                display_list.push(vec![DrawCommand::Marker(start_offset + body.offset())]);
            }
        } else {
            display_list.push(vec![DrawCommand::Marker(start_offset)]);
        }

        display_list
    }

    pub fn categories(&self) -> BTreeSet<String> {
        self.info.categories()
    }

    fn chapter_aux<'a>(
        &mut self,
        toc: &'a [TocEntry],
        offset: usize,
        next_offset: usize,
        path: &str,
        end_offset: &mut usize,
        chap_before: &mut Option<&'a TocEntry>,
        offset_before: &mut usize,
        chap_after: &mut Option<&'a TocEntry>,
        offset_after: &mut usize,
    ) {
        for entry in toc {
            if let Location::Uri(ref uri) = entry.location {
                if uri.starts_with(path) {
                    if let Some(entry_offset) = self.resolve_location(entry.location.clone()) {
                        if entry_offset < offset
                            && (chap_before.is_none() || entry_offset > *offset_before)
                        {
                            *chap_before = Some(entry);
                            *offset_before = entry_offset;
                        }
                        if entry_offset >= offset
                            && entry_offset < next_offset
                            && (chap_after.is_none() || entry_offset < *offset_after)
                        {
                            *chap_after = Some(entry);
                            *offset_after = entry_offset;
                        }
                        if entry_offset >= next_offset && entry_offset < *end_offset {
                            *end_offset = entry_offset;
                        }
                    }
                }
            }
            self.chapter_aux(
                &entry.children,
                offset,
                next_offset,
                path,
                end_offset,
                chap_before,
                offset_before,
                chap_after,
                offset_after,
            );
        }
    }

    fn previous_chapter<'a>(
        &mut self,
        chap: Option<&TocEntry>,
        start_offset: usize,
        end_offset: usize,
        toc: &'a [TocEntry],
    ) -> Option<&'a TocEntry> {
        for entry in toc.iter().rev() {
            let result = self.previous_chapter(chap, start_offset, end_offset, &entry.children);
            if result.is_some() {
                return result;
            }

            if let Some(chap) = chap {
                if entry.index < chap.index {
                    let entry_offset = self.resolve_location(entry.location.clone())?;
                    if entry_offset < start_offset || entry_offset >= end_offset {
                        return Some(entry);
                    }
                }
            } else {
                let entry_offset = self.resolve_location(entry.location.clone())?;
                if entry_offset < start_offset {
                    return Some(entry);
                }
            }
        }
        None
    }

    fn next_chapter<'a>(
        &mut self,
        chap: Option<&TocEntry>,
        start_offset: usize,
        end_offset: usize,
        toc: &'a [TocEntry],
    ) -> Option<&'a TocEntry> {
        for entry in toc {
            if let Some(chap) = chap {
                if entry.index > chap.index {
                    let entry_offset = self.resolve_location(entry.location.clone())?;
                    if entry_offset < start_offset || entry_offset >= end_offset {
                        return Some(entry);
                    }
                }
            } else {
                let entry_offset = self.resolve_location(entry.location.clone())?;
                if entry_offset >= end_offset {
                    return Some(entry);
                }
            }

            let result = self.next_chapter(chap, start_offset, end_offset, &entry.children);
            if result.is_some() {
                return result;
            }
        }
        None
    }

    pub fn series(&self) -> Option<(String, String)> {
        self.info.series()
    }

    pub fn cover_image(&self) -> Option<String> {
        self.info.cover_image_href()
    }

    pub fn description(&self) -> Option<String> {
        self.metadata("dc:description")
    }

    pub fn publisher(&self) -> Option<String> {
        self.metadata("dc:publisher")
    }

    pub fn language(&self) -> Option<String> {
        self.metadata("dc:language")
    }

    pub fn year(&self) -> Option<String> {
        self.metadata("dc:date")
            .map(|s| s.chars().take(4).collect())
    }
}

impl EpubDocumentFile {
    pub fn new<P: AsRef<Path>>(path: P, install_dir: &Path) -> Result<Self, Error> {
        let file = File::open(path)?;
        let archive = ZipArchive::new(file)?;
        Self::from_archive(archive, install_dir)
    }
}

impl EpubDocumentStatic {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn new_from_static(bytes: &'static [u8], install_dir: &Path) -> Result<Self, Error> {
        let cursor = Cursor::new(bytes);
        let archive = ZipArchive::new(cursor)?;
        Self::from_archive(archive, install_dir)
    }
}

impl<R: Read + Seek> Document for EpubDocument<R> {
    fn preview_pixmap(&mut self, width: f32, height: f32, samples: usize) -> Option<Pixmap> {
        let opener = PdfOpener::new()?;
        self.cover_image()
            .map(|path| self.parent.join(path).to_string_lossy().into_owned())
            .and_then(|path| {
                self.archive
                    .fetch(&path)
                    .ok()
                    .and_then(|buf| opener.open_memory(&path, &buf))
                    .and_then(|mut doc| {
                        doc.dims(0).and_then(|dims| {
                            let scale = (width / dims.0).min(height / dims.1);
                            doc.pixmap(Location::Exact(0), scale, samples)
                        })
                    })
            })
            .or_else(|| {
                self.dims(0).and_then(|dims| {
                    let scale = (width / dims.0).min(height / dims.1);
                    self.pixmap(Location::Exact(0), scale, samples)
                })
            })
            .map(|(pixmap, _)| pixmap)
    }

    #[inline]
    fn dims(&self, _index: usize) -> Option<(f32, f32)> {
        Some((self.engine.dims.0 as f32, self.engine.dims.1 as f32))
    }

    fn pages_count(&self) -> usize {
        self.spine.iter().map(|c| c.size).sum()
    }

    fn toc(&mut self) -> Option<Vec<TocEntry>> {
        let name = self.info.toc_href().map(|href| {
            self.parent
                .join(href)
                .normalize()
                .to_string_lossy()
                .into_owned()
        })?;

        let mut text = String::new();
        if let Ok(mut zf) = self.archive.by_name(&name) {
            zf.read_to_string(&mut text).ok()?;
        } else {
            return None;
        }

        parse_toc(&text, &name).map(|toc| toc.into_entries())
    }

    fn chapter<'a>(&mut self, offset: usize, toc: &'a [TocEntry]) -> Option<(&'a TocEntry, f32)> {
        let next_offset = self
            .resolve_location(Location::Next(offset))
            .unwrap_or(usize::MAX);
        let (index, start_offset) = self.vertebra_coordinates(offset)?;
        let path = self.spine[index].path.clone();
        let mut end_offset = start_offset + self.spine[index].size;
        let mut chap_before = None;
        let mut chap_after = None;
        let mut offset_before = 0;
        let mut offset_after = usize::MAX;

        self.chapter_aux(
            toc,
            offset,
            next_offset,
            &path,
            &mut end_offset,
            &mut chap_before,
            &mut offset_before,
            &mut chap_after,
            &mut offset_after,
        );

        if chap_after.is_none() && chap_before.is_none() {
            for i in (0..index).rev() {
                let chap = chapter_from_uri(&self.spine[i].path, toc);
                if chap.is_some() {
                    end_offset = if let Some(j) = (index + 1..self.spine.len())
                        .find(|&j| chapter_from_uri(&self.spine[j].path, toc).is_some())
                    {
                        self.offset(j)
                    } else {
                        self.size()
                    };
                    let chap_offset = self.offset(i);
                    let progress =
                        (offset - chap_offset) as f32 / (end_offset - chap_offset) as f32;
                    return chap.zip(Some(progress));
                }
            }
            None
        } else {
            match (chap_after, chap_before) {
                (Some(..), _) => chap_after.zip(Some(0.0)),
                (None, Some(..)) => chap_before.zip(Some(
                    (offset - offset_before) as f32 / (end_offset - offset_before) as f32,
                )),
                _ => None,
            }
        }
    }

    fn chapter_relative<'a>(
        &mut self,
        offset: usize,
        dir: CycleDir,
        toc: &'a [TocEntry],
    ) -> Option<&'a TocEntry> {
        let next_offset = self
            .resolve_location(Location::Next(offset))
            .unwrap_or(usize::MAX);
        let chap = self.chapter(offset, toc).map(|(c, _)| c);

        match dir {
            CycleDir::Previous => self.previous_chapter(chap, offset, next_offset, toc),
            CycleDir::Next => self.next_chapter(chap, offset, next_offset, toc),
        }
    }

    fn resolve_location(&mut self, loc: Location) -> Option<usize> {
        self.engine.load_fonts();

        match loc {
            Location::Exact(offset) => {
                let (index, start_offset) = self.vertebra_coordinates(offset)?;
                let page_index = self.page_index(offset, index, start_offset)?;
                self.cache
                    .get(&index)
                    .and_then(|display_list| display_list[page_index].first())
                    .map(DrawCommand::offset)
            }
            Location::Previous(offset) => {
                let (index, start_offset) = self.vertebra_coordinates(offset)?;
                let page_index = self.page_index(offset, index, start_offset)?;
                if page_index > 0 {
                    self.cache.get(&index).and_then(|display_list| {
                        display_list[page_index - 1]
                            .first()
                            .map(DrawCommand::offset)
                    })
                } else {
                    if index == 0 {
                        return None;
                    }
                    let (index, start_offset) =
                        (index - 1, start_offset - self.spine[index - 1].size);
                    if !self.cache.contains_key(&index) {
                        let display_list = self.build_display_list(index, start_offset);
                        self.cache.insert(index, display_list);
                    }
                    self.cache.get(&index).and_then(|display_list| {
                        display_list
                            .last()
                            .and_then(|page| page.first())
                            .map(DrawCommand::offset)
                    })
                }
            }
            Location::Next(offset) => {
                let (index, start_offset) = self.vertebra_coordinates(offset)?;
                let page_index = self.page_index(offset, index, start_offset)?;
                if page_index < self.cache.get(&index).map(Vec::len)? - 1 {
                    self.cache.get(&index).and_then(|display_list| {
                        display_list[page_index + 1]
                            .first()
                            .map(DrawCommand::offset)
                    })
                } else {
                    if index == self.spine.len() - 1 {
                        return None;
                    }
                    let (index, start_offset) = (index + 1, start_offset + self.spine[index].size);
                    if !self.cache.contains_key(&index) {
                        let display_list = self.build_display_list(index, start_offset);
                        self.cache.insert(index, display_list);
                    }
                    self.cache.get(&index).and_then(|display_list| {
                        display_list
                            .first()
                            .and_then(|page| page.first())
                            .map(|dc| dc.offset())
                    })
                }
            }
            Location::LocalUri(offset, ref uri) => {
                let mut cache = FxHashMap::default();
                let normalized_uri: String = {
                    let (index, _) = self.vertebra_coordinates(offset)?;
                    let path = &self.spine[index].path;
                    if uri.starts_with('#') {
                        format!("{}{}", path, uri)
                    } else {
                        let parent = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
                        parent.join(uri).normalize().to_string_lossy().into_owned()
                    }
                };
                self.resolve_link(&normalized_uri, &mut cache)
            }
            Location::Uri(ref uri) => {
                let mut cache = FxHashMap::default();
                self.resolve_link(uri, &mut cache)
            }
        }
    }

    fn words(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        if self.spine.is_empty() {
            return None;
        }

        let offset = self.resolve_location(loc)?;
        let (index, start_offset) = self.vertebra_coordinates(offset)?;
        let page_index = self.page_index(offset, index, start_offset)?;

        self.cache.get(&index).map(|display_list| {
            (
                display_list[page_index]
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
            )
        })
    }

    fn lines(&mut self, _loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        None
    }

    fn links(&mut self, loc: Location) -> Option<(Vec<BoundedText>, usize)> {
        if self.spine.is_empty() {
            return None;
        }

        let offset = self.resolve_location(loc)?;
        let (index, start_offset) = self.vertebra_coordinates(offset)?;
        let page_index = self.page_index(offset, index, start_offset)?;

        self.cache.get(&index).map(|display_list| {
            (
                display_list[page_index]
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
            )
        })
    }

    fn images(&mut self, loc: Location) -> Option<(Vec<Boundary>, usize)> {
        if self.spine.is_empty() {
            return None;
        }

        let offset = self.resolve_location(loc)?;
        let (index, start_offset) = self.vertebra_coordinates(offset)?;
        let page_index = self.page_index(offset, index, start_offset)?;

        self.cache.get(&index).map(|display_list| {
            (
                display_list[page_index]
                    .iter()
                    .filter_map(|dc| match dc {
                        DrawCommand::Image(ImageCommand { rect, .. }) => Some((*rect).into()),
                        _ => None,
                    })
                    .collect(),
                offset,
            )
        })
    }

    fn pixmap(&mut self, loc: Location, scale: f32, samples: usize) -> Option<(Pixmap, usize)> {
        if self.spine.is_empty() {
            return None;
        }

        let offset = self.resolve_location(loc)?;
        let (index, start_offset) = self.vertebra_coordinates(offset)?;

        let page_index = self.page_index(offset, index, start_offset)?;
        let page = self.cache.get(&index)?.get(page_index)?.clone();

        let pixmap = self
            .engine
            .render_page(&page, scale, samples, &mut self.archive)?;

        Some((pixmap, offset))
    }

    fn layout(&mut self, width: u32, height: u32, font_size: f32, dpi: u16) {
        self.engine.layout(width, height, font_size, dpi);
        self.cache.clear();
    }

    fn set_text_align(&mut self, text_align: TextAlign) {
        self.engine.set_text_align(text_align);
        self.cache.clear();
    }

    fn set_font_family(&mut self, family_name: &str, search_path: &str) {
        self.engine.set_font_family(family_name, search_path);
        self.cache.clear();
    }

    fn set_margin_width(&mut self, width: i32) {
        self.engine.set_margin_width(width);
        self.cache.clear();
    }

    fn set_line_height(&mut self, line_height: f32) {
        self.engine.set_line_height(line_height);
        self.cache.clear();
    }

    fn set_hyphen_penalty(&mut self, hyphen_penalty: i32) {
        self.engine.set_hyphen_penalty(hyphen_penalty);
        self.cache.clear();
    }

    fn set_stretch_tolerance(&mut self, stretch_tolerance: f32) {
        self.engine.set_stretch_tolerance(stretch_tolerance);
        self.cache.clear();
    }

    fn set_ignore_document_css(&mut self, ignore: bool) {
        self.ignore_document_css = ignore;
        self.cache.clear();
    }

    fn title(&self) -> Option<String> {
        self.metadata("dc:title")
    }

    fn author(&self) -> Option<String> {
        // TODO: Consider the opf:file-as attribute?
        self.metadata("dc:creator")
    }

    fn metadata(&self, key: &str) -> Option<String> {
        self.info.metadata_value(key)
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
    use crate::document::html::dom::XmlTree;
    use crate::document::html::layout::DrawCommand;
    use crate::document::html::xml::XmlParser;
    use opf::OpfDocument;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::write::SimpleFileOptions;

    /// Minimal EPUB chapter that resembles a real spine file: XML declaration,
    /// DOCTYPE, explicit html/head/body, paragraphs with `id` attributes
    /// (needed for `cache_uris` and `DrawCommand::Marker`), and a text span.
    const CHAPTER_HTML: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"\">\n",
        "<html xmlns=\"http://www.w3.org/1999/xhtml\">",
        "<head><title>Test</title></head>",
        "<body>",
        "<p id=\"s1\">First paragraph.</p>",
        "<p id=\"s2\">Second <em>emphasis</em> paragraph.</p>",
        "<p id=\"s3\">Third paragraph with <span>inline</span> content.</p>",
        "</body></html>",
    );

    /// Variant of `CHAPTER_HTML` containing only block-level structure with no
    /// inline text nodes.  Used by the display-list Marker test because the
    /// engine's inline-text layout path requires loaded fonts, whereas the
    /// block path that emits `DrawCommand::Marker` does not.
    const CHAPTER_HTML_BLOCK_ONLY: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"\">\n",
        "<html xmlns=\"http://www.w3.org/1999/xhtml\">",
        "<head></head>",
        "<body>",
        "<div id=\"s1\"><div id=\"s1a\"><div id=\"s1b\"></div></div></div>",
        "<div id=\"s2\"><div id=\"s2a\"></div></div>",
        "<div id=\"s3\"></div>",
        "</body></html>",
    );

    /// Collect `(tag_name, id_attr_value, byte_offset)` for every element that
    /// has an `id` attribute, in document order.  Used to compare bookmark /
    /// annotation anchor points between parsers.
    fn collect_id_offsets(tree: &XmlTree) -> Vec<(String, String, usize)> {
        tree.root()
            .descendants()
            .filter_map(|n| {
                let tag = n.tag_name()?;
                let id = n.attribute("id")?;
                Some((tag.to_string(), id.to_string(), n.offset()))
            })
            .collect()
    }

    /// Collect all `DrawCommand::Marker` offsets from a flat display list, in
    /// order.  Marker offsets are exactly what gets stored as reading positions
    /// and bookmark targets.
    fn collect_marker_offsets(pages: &[Page]) -> Vec<usize> {
        pages
            .iter()
            .flatten()
            .filter_map(|cmd| match cmd {
                DrawCommand::Marker(offset) => Some(*offset),
                _ => None,
            })
            .collect()
    }

    /// Build an in-memory EPUB zip containing a single spine chapter and
    /// return it as a `Vec<u8>` suitable for `EpubDocument::from_archive`.
    fn build_minimal_epub(chapter_html: &str) -> Vec<u8> {
        let buf = Vec::new();
        let cursor = std::io::Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = SimpleFileOptions::default();

        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();

        let chapter_bytes = chapter_html.as_bytes();
        zip.start_file("OEBPS/chapter.xhtml", opts).unwrap();
        zip.write_all(chapter_bytes).unwrap();

        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata/>
  <manifest>
    <item id="ch1" href="chapter.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
  </spine>
</package>"#;
        zip.start_file("OEBPS/content.opf", opts).unwrap();
        zip.write_all(opf.as_bytes()).unwrap();

        zip.finish().unwrap().into_inner()
    }

    /// Verify that `parse_html5` and `XmlParser` assign identical byte offsets
    /// to every element that carries an `id` attribute in a realistic EPUB
    /// chapter.  These offsets are what gets stored as reading positions,
    /// bookmark targets, and annotation anchors.
    #[test]
    fn epub_spine_chapter_id_offsets_match_between_parsers() {
        let xml_offsets = {
            let mut tree = XmlParser::new(CHAPTER_HTML).parse();
            tree.wrap_lost_inlines();
            collect_id_offsets(&tree)
        };

        let h5_offsets = {
            let mut tree = parse_html5(CHAPTER_HTML);
            tree.wrap_lost_inlines();
            collect_id_offsets(&tree)
        };

        assert_eq!(
            xml_offsets, h5_offsets,
            "id-attribute node offsets differ between XmlParser and parse_html5\n\
             XmlParser: {xml_offsets:?}\n\
             html5ever: {h5_offsets:?}"
        );
    }

    /// Verify that `cache_uris` (the `#anchor-id` → byte-offset map used for
    /// in-book link resolution) produces identical mappings from both parsers.
    #[test]
    fn epub_spine_chapter_cache_uris_match_between_parsers() {
        let root_dir = PathBuf::from(
            std::env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for epub tests"),
        );
        let name = "OEBPS/chapter.xhtml";
        let start_offset: usize = 0;

        let xml_cache = {
            let mut cache = UriCache::default();
            let tree = XmlParser::new(CHAPTER_HTML).parse();
            let mut dummy_doc: EpubDocument<std::io::Cursor<Vec<u8>>> = EpubDocument {
                archive: ZipArchive::new(std::io::Cursor::new(build_minimal_epub(CHAPTER_HTML)))
                    .unwrap(),
                info: OpfDocument::empty(),
                parent: PathBuf::default(),
                engine: Engine::new(&root_dir),
                spine: vec![Chunk {
                    path: name.to_string(),
                    size: CHAPTER_HTML.len(),
                }],
                cache: FxHashMap::default(),
                ignore_document_css: false,
            };
            dummy_doc.cache_uris(tree.root(), name, start_offset, &mut cache);
            cache
        };

        let h5_cache = {
            let mut cache = UriCache::default();
            let tree = parse_html5(CHAPTER_HTML);
            let mut dummy_doc: EpubDocument<std::io::Cursor<Vec<u8>>> = EpubDocument {
                archive: ZipArchive::new(std::io::Cursor::new(build_minimal_epub(CHAPTER_HTML)))
                    .unwrap(),
                info: OpfDocument::empty(),
                parent: PathBuf::default(),
                engine: Engine::new(&root_dir),
                spine: vec![Chunk {
                    path: name.to_string(),
                    size: CHAPTER_HTML.len(),
                }],
                cache: FxHashMap::default(),
                ignore_document_css: false,
            };
            dummy_doc.cache_uris(tree.root(), name, start_offset, &mut cache);
            cache
        };

        assert_eq!(
            xml_cache, h5_cache,
            "cache_uris maps differ between XmlParser and parse_html5\n\
             XmlParser: {xml_cache:?}\n\
             html5ever: {h5_cache:?}"
        );
    }

    /// Verify that `build_display_list` emits `DrawCommand::Marker` commands
    /// with identical offsets whether the spine chapter was parsed by
    /// `XmlParser` or `parse_html5`.  Marker offsets are stored as reading
    /// positions and bookmark byte offsets, so they must be parser-independent.
    ///
    /// Uses a block-only chapter variant (no inline text nodes) so the engine
    /// does not require loaded fonts — the Marker path is font-free.
    #[test]
    fn epub_spine_chapter_marker_offsets_match_between_parsers() {
        let start_offset: usize = 512;

        let xml_markers = {
            let mut tree = XmlParser::new(CHAPTER_HTML_BLOCK_ONLY).parse();
            tree.wrap_lost_inlines();
            marker_offsets_from_tree(tree, start_offset)
        };

        let h5_markers = {
            let mut tree = parse_html5(CHAPTER_HTML_BLOCK_ONLY);
            tree.wrap_lost_inlines();
            marker_offsets_from_tree(tree, start_offset)
        };

        assert!(
            !xml_markers.is_empty(),
            "no Marker commands produced — check id attributes"
        );
        assert_eq!(
            xml_markers, h5_markers,
            "Marker offsets differ between XmlParser and parse_html5\n\
             XmlParser: {xml_markers:?}\n\
             html5ever: {h5_markers:?}"
        );
    }

    /// Drive `Engine::build_display_list` directly for a pre-parsed tree and
    /// collect all `DrawCommand::Marker` offsets.  Uses a no-op resource
    /// fetcher since the test chapter has no external assets.
    fn marker_offsets_from_tree(tree: XmlTree, start_offset: usize) -> Vec<usize> {
        let root_dir = PathBuf::from(
            std::env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for epub tests"),
        );
        struct NoopFetcher;
        impl ResourceFetcher for NoopFetcher {
            fn fetch(&mut self, _name: &str) -> Result<Vec<u8>, Error> {
                Ok(Vec::new())
            }
        }

        let mut engine = Engine::new(root_dir);
        engine.layout(600, 800, 12.0, 265);

        let rect = engine.rect();
        let mut draw_state = DrawState {
            position: rect.min,
            ..Default::default()
        };
        let root_data = RootData {
            start_offset,
            spine_dir: PathBuf::default(),
            rect,
        };
        let stylesheet = StyleSheet::new();
        let style = StyleData {
            font_size: engine.font_size,
            line_height: crate::unit::pt_to_px(engine.line_height * engine.font_size, engine.dpi)
                .round() as i32,
            text_align: engine.text_align,
            start_x: rect.min.x,
            end_x: rect.max.x,
            width: rect.max.x - rect.min.x,
            ..Default::default()
        };
        let loop_context = LoopContext::default();
        let mut pages: Vec<Page> = vec![Vec::new()];

        if let Some(body) = tree.root().find("body") {
            engine.build_display_list(
                body,
                &style,
                &loop_context,
                &stylesheet,
                &root_data,
                &mut NoopFetcher,
                &mut draw_state,
                &mut pages,
            );
        }

        collect_marker_offsets(&pages)
    }
    fn setup_epub() -> EpubDocumentFile {
        let root_dir = PathBuf::from(
            std::env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for epub tests"),
        );
        let epub_path = root_dir.join("docs/book/epub/Cadmus Documentation.epub");
        let mut doc =
            EpubDocumentFile::new(&epub_path, &root_dir).expect("failed to open test epub");
        doc.engine.layout(600, 800, 12.0, 265);
        doc.engine.set_margin_width(3);
        doc.engine.load_fonts_from(root_dir);
        doc
    }

    #[test]
    fn next_location_advances_to_next_spine_chapter() {
        let mut doc = setup_epub();

        let first_offset = doc
            .resolve_location(Location::Exact(0))
            .expect("should resolve offset 0");

        let last_page_offset = doc
            .cache
            .get(&0)
            .and_then(|dl| dl.last())
            .and_then(|page| page.first())
            .map(|dc| dc.offset())
            .expect("spine[0] last page has offset");

        let next_offset = doc
            .resolve_location(Location::Next(last_page_offset))
            .expect("navigating past last page of spine[0] should return Some");

        let (next_index, _) = doc
            .vertebra_coordinates(next_offset)
            .expect("next offset maps to spine");

        assert_eq!(
            next_index, 1,
            "navigating next from last page of spine[0] (offset={}) should land on spine[1], got spine[{}] offset={}",
            first_offset, next_index, next_offset
        );
    }

    #[test]
    fn first_spine_chapter_produces_pages_with_text() {
        let mut doc = setup_epub();

        let display_list = doc.build_display_list(0, 0);

        assert!(
            display_list.len() > 1,
            "expected multiple pages, got {}",
            display_list.len()
        );

        let has_text = display_list.iter().any(|page| {
            page.iter()
                .any(|cmd| matches!(cmd, DrawCommand::Text(_) | DrawCommand::ExtraText(_)))
        });
        assert!(has_text, "no text draw commands found across all pages");
    }

    #[test]
    fn next_page_exists_from_start() {
        let mut doc = setup_epub();

        let display_list = doc.build_display_list(0, 0);
        doc.cache.insert(0, display_list);

        let page_count = doc.cache.get(&0).map(|dl| dl.len()).unwrap_or(0);

        assert!(
            page_count > 1,
            "expected more than one page so next-page navigation works"
        );
    }

    /// Reproduces https://github.com/baskerville/plato/issues/426:
    /// EPUB files from royallib.com wrap block elements inside inline `<span>`
    /// ancestors. The engine's `has_blocks` check only looks at direct children,
    /// so the outer `<span>` appears inline-only, `gather_inline_material`
    /// recurses into it, and the nested `<div>`/`<p>` bodies are silently
    /// dropped — only the chapter title renders.
    ///
    /// Spine index 39 is OPS/ch1-38.xhtml. Its body starts with:
    ///   <span><span><span id="id90">
    ///     <div class="title6"><p>"Коса" жизни</p></div>
    ///     <p>Георгий Гамов озаглавил …</p>
    ///   …
    /// The first body paragraph is the canary: if block-in-inline
    /// promotion is broken, gather_inline_material swallows the <p>
    /// nodes and this text never appears in any DrawCommand.
    #[test]
    fn royallib_block_in_inline_renders_body_paragraphs() {
        let root_dir = PathBuf::from(
            std::env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for epub tests"),
        );
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let epub_path =
            manifest_dir.join("src/document/tests/fixtures/royallib-block-in-inline.epub");

        let mut doc =
            EpubDocumentFile::new(&epub_path, &root_dir).expect("failed to open royallib epub");
        doc.engine.layout(600, 800, 12.0, 265);
        doc.engine.set_margin_width(3);
        doc.engine.load_fonts_from(root_dir);

        let display_list = doc.build_display_list(39, 0);

        let rendered_text: String = display_list
            .iter()
            .flat_map(|page| page.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(tc) | DrawCommand::ExtraText(tc) => Some(tc.text.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            rendered_text.contains("Георгий") && rendered_text.contains("Гамов"),
            "body paragraph text not rendered — block-in-inline content was silently dropped",
        );
    }

    #[test]
    fn all_spine_chapters_produce_content() {
        let mut doc = setup_epub();

        let spine_len = doc.spine.len();
        assert!(spine_len > 0, "spine is empty");

        let mut start_offset = 0;
        for i in 0..spine_len {
            let display_list = doc.build_display_list(i, start_offset);
            assert!(
                !display_list.is_empty(),
                "spine chapter {} produced zero pages",
                i
            );
            let has_content = display_list.iter().any(|page| !page.is_empty());
            assert!(has_content, "spine chapter {} has only empty pages", i);
            start_offset += doc.spine[i].size;
        }
    }

    /// Build a minimal in-memory zip with arbitrary named entries and return it
    /// as a `ZipArchive` ready for `build_spine`.
    fn zip_with_entries(entries: &[(&str, &[u8])]) -> ZipArchive<std::io::Cursor<Vec<u8>>> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let opts = SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        ZipArchive::new(zip.finish().unwrap()).unwrap()
    }

    fn manifest_entry(id: &str, href: &str) -> ManifestEntry {
        ManifestEntry {
            id: id.to_string(),
            href: href.to_string(),
            media_type: "application/xhtml+xml".to_string(),
            properties: String::new(),
        }
    }

    #[test]
    fn build_spine_resolves_all_entries_in_order() {
        let ch1 = b"chapter one content";
        let ch2 = b"chapter two content longer";
        let mut archive = zip_with_entries(&[("OEBPS/ch1.xhtml", ch1), ("OEBPS/ch2.xhtml", ch2)]);

        let manifest = vec![
            manifest_entry("id1", "ch1.xhtml"),
            manifest_entry("id2", "ch2.xhtml"),
        ];
        let idrefs = vec!["id1".to_string(), "id2".to_string()];
        let parent = Path::new("OEBPS");

        let spine = build_spine(&mut archive, &manifest, &idrefs, parent);

        assert_eq!(spine.len(), 2);
        assert_eq!(spine[0].path, "OEBPS/ch1.xhtml");
        assert_eq!(spine[0].size, ch1.len());
        assert_eq!(spine[1].path, "OEBPS/ch2.xhtml");
        assert_eq!(spine[1].size, ch2.len());
    }

    #[test]
    fn build_spine_preserves_spine_order_not_manifest_order() {
        let mut archive =
            zip_with_entries(&[("OEBPS/ch1.xhtml", b"a"), ("OEBPS/ch2.xhtml", b"bb")]);

        // Manifest lists ch2 before ch1, but spine references ch1 first.
        let manifest = vec![
            manifest_entry("id2", "ch2.xhtml"),
            manifest_entry("id1", "ch1.xhtml"),
        ];
        let idrefs = vec!["id1".to_string(), "id2".to_string()];
        let parent = Path::new("OEBPS");

        let spine = build_spine(&mut archive, &manifest, &idrefs, parent);

        assert_eq!(spine.len(), 2);
        assert_eq!(spine[0].path, "OEBPS/ch1.xhtml");
        assert_eq!(spine[1].path, "OEBPS/ch2.xhtml");
    }

    #[test]
    fn build_spine_skips_idref_with_no_manifest_entry() {
        let mut archive = zip_with_entries(&[("OEBPS/ch1.xhtml", b"content")]);

        let manifest = vec![manifest_entry("id1", "ch1.xhtml")];
        // "ghost" has no matching manifest entry.
        let idrefs = vec!["id1".to_string(), "ghost".to_string()];
        let parent = Path::new("OEBPS");

        let spine = build_spine(&mut archive, &manifest, &idrefs, parent);

        assert_eq!(spine.len(), 1);
        assert_eq!(spine[0].path, "OEBPS/ch1.xhtml");
    }

    #[test]
    fn build_spine_skips_entry_absent_from_archive() {
        // Manifest references ch2.xhtml but it is not in the zip.
        let mut archive = zip_with_entries(&[("OEBPS/ch1.xhtml", b"content")]);

        let manifest = vec![
            manifest_entry("id1", "ch1.xhtml"),
            manifest_entry("id2", "ch2.xhtml"),
        ];
        let idrefs = vec!["id1".to_string(), "id2".to_string()];
        let parent = Path::new("OEBPS");

        let spine = build_spine(&mut archive, &manifest, &idrefs, parent);

        assert_eq!(spine.len(), 1, "missing archive entry should be skipped");
        assert_eq!(spine[0].path, "OEBPS/ch1.xhtml");
    }

    #[test]
    fn build_spine_decodes_percent_encoded_href() {
        // Space encoded as %20 in the manifest href.
        let mut archive = zip_with_entries(&[("OEBPS/chapter one.xhtml", b"hello")]);

        let manifest = vec![manifest_entry("id1", "chapter%20one.xhtml")];
        let idrefs = vec!["id1".to_string()];
        let parent = Path::new("OEBPS");

        let spine = build_spine(&mut archive, &manifest, &idrefs, parent);

        assert_eq!(spine.len(), 1);
        assert_eq!(spine[0].path, "OEBPS/chapter one.xhtml");
    }
}
