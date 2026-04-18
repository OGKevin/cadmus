use crate::document::asciify;
use crate::document::djvu::DjvuOpener;
use crate::document::epub::EpubDocument;
use crate::document::html::HtmlDocument;
use crate::document::pdf::PdfOpener;
use crate::document::{Document, SimpleTocEntry, TextLocation};
use crate::geom::Point;
use crate::helpers::datetime_format;
use crate::helpers::Fp;
use chrono::{Local, NaiveDateTime};
use fxhash::FxHashMap;
use lazy_static::lazy_static;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use titlecase::titlecase;
use tracing::{error, warn};

pub const DEFAULT_CONTRAST_EXPONENT: f32 = 1.0;
pub const DEFAULT_CONTRAST_GRAY: f32 = 224.0;

pub type Metadata = Vec<Info>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Info {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub subtitle: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub year: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub publisher: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub series: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub edition: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub volume: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub number: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub identifier: String,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub categories: BTreeSet<String>,
    pub file: FileInfo,
    #[serde(skip_serializing)]
    pub reader: Option<ReaderInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toc: Option<Vec<SimpleTocEntry>>,
    #[serde(with = "datetime_format")]
    pub added: NaiveDateTime,
    #[serde(skip)]
    pub fp: Option<Fp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileInfo {
    pub path: PathBuf,
    #[serde(skip)]
    pub absolute_path: PathBuf,
    pub kind: String,
    pub size: u64,
}

impl Default for FileInfo {
    fn default() -> Self {
        FileInfo {
            path: PathBuf::default(),
            absolute_path: PathBuf::default(),
            kind: String::default(),
            size: u64::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Annotation {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub selection: [TextLocation; 2],
    #[serde(with = "datetime_format")]
    pub modified: NaiveDateTime,
}

impl Default for Annotation {
    fn default() -> Self {
        Annotation {
            note: String::new(),
            text: String::new(),
            selection: [TextLocation::Dynamic(0), TextLocation::Dynamic(1)],
            modified: Local::now().naive_local(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Margin {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Margin {
    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Margin {
        Margin {
            top,
            right,
            bottom,
            left,
        }
    }
}

impl Default for Margin {
    fn default() -> Margin {
        Margin::new(0.0, 0.0, 0.0, 0.0)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PageScheme {
    Any,
    EvenOdd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CroppingMargins {
    Any(Margin),
    EvenOdd([Margin; 2]),
}

impl CroppingMargins {
    pub fn margin(&self, index: usize) -> &Margin {
        match *self {
            CroppingMargins::Any(ref margin) => margin,
            CroppingMargins::EvenOdd(ref pair) => &pair[index % 2],
        }
    }

    pub fn margin_mut(&mut self, index: usize) -> &mut Margin {
        match *self {
            CroppingMargins::Any(ref mut margin) => margin,
            CroppingMargins::EvenOdd(ref mut pair) => &mut pair[index % 2],
        }
    }

    pub fn apply(&mut self, index: usize, scheme: PageScheme) {
        let margin = self.margin(index).clone();

        match scheme {
            PageScheme::Any => *self = CroppingMargins::Any(margin),
            PageScheme::EvenOdd => *self = CroppingMargins::EvenOdd([margin.clone(), margin]),
        }
    }

    pub fn is_split(&self) -> bool {
        !matches!(*self, CroppingMargins::Any(..))
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum TextAlign {
    Justify,
    Left,
    Right,
    Center,
}

impl TextAlign {
    pub fn icon_name(&self) -> &str {
        match self {
            TextAlign::Justify => "align-justify",
            TextAlign::Left => "align-left",
            TextAlign::Right => "align-right",
            TextAlign::Center => "align-center",
        }
    }
}

impl fmt::Display for TextAlign {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReaderInfo {
    #[serde(with = "datetime_format")]
    pub opened: NaiveDateTime,
    pub current_page: usize,
    pub pages_count: usize,
    pub finished: bool,
    pub dithered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zoom_mode: Option<ZoomMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scroll_mode: Option<ScrollMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_offset: Option<Point>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<i8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cropping_margins: Option<CroppingMargins>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_margin_width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<TextAlign>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast_exponent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contrast_gray: Option<f32>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub page_names: BTreeMap<usize, String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub bookmarks: BTreeSet<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
pub enum ZoomMode {
    FitToPage,
    FitToWidth,
    Custom(f32),
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
pub enum ScrollMode {
    Screen,
    Page,
}

impl PartialEq for ZoomMode {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ZoomMode::FitToPage, ZoomMode::FitToPage) => true,
            (ZoomMode::FitToWidth, ZoomMode::FitToWidth) => true,
            (ZoomMode::Custom(z1), ZoomMode::Custom(z2)) => (z1 - z2).abs() < f32::EPSILON,
            _ => false,
        }
    }
}

impl Eq for ZoomMode {}

impl ReaderInfo {
    pub fn progress(&self) -> f32 {
        (self.current_page / self.pages_count) as f32
    }
}

impl Default for ReaderInfo {
    fn default() -> Self {
        ReaderInfo {
            opened: Local::now().naive_local(),
            current_page: 0,
            pages_count: 1,
            finished: false,
            dithered: false,
            zoom_mode: None,
            scroll_mode: None,
            page_offset: None,
            rotation: None,
            cropping_margins: None,
            margin_width: None,
            screen_margin_width: None,
            font_family: None,
            font_size: None,
            text_align: None,
            line_height: None,
            contrast_exponent: None,
            contrast_gray: None,
            page_names: BTreeMap::new(),
            bookmarks: BTreeSet::new(),
            annotations: Vec::new(),
        }
    }
}

impl Default for Info {
    fn default() -> Self {
        Info {
            title: String::default(),
            subtitle: String::default(),
            author: String::default(),
            year: String::default(),
            language: String::default(),
            publisher: String::default(),
            series: String::default(),
            edition: String::default(),
            volume: String::default(),
            number: String::default(),
            identifier: String::default(),
            categories: BTreeSet::new(),
            file: FileInfo::default(),
            added: Local::now().naive_local(),
            reader: None,
            reader_info: None,
            toc: None,
            fp: None,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Status {
    New,
    Reading(f32),
    Finished,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SimpleStatus {
    New,
    Reading,
    Finished,
}

impl fmt::Display for SimpleStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl Info {
    pub fn status(&self) -> Status {
        if let Some(ref r) = self.reader {
            if r.finished {
                Status::Finished
            } else {
                Status::Reading(r.current_page as f32 / r.pages_count as f32)
            }
        } else {
            Status::New
        }
    }

    pub fn simple_status(&self) -> SimpleStatus {
        if let Some(ref r) = self.reader {
            if r.finished {
                SimpleStatus::Finished
            } else {
                SimpleStatus::Reading
            }
        } else {
            SimpleStatus::New
        }
    }

    pub fn file_stem(&self) -> String {
        self.file
            .path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    pub fn title(&self) -> String {
        if self.title.is_empty() {
            return self.file_stem();
        }

        let mut title = self.title.clone();

        if !self.number.is_empty() && self.series.is_empty() {
            title = format!("{} #{}", title, self.number);
        }

        if !self.volume.is_empty() {
            title = format!("{} — vol. {}", title, self.volume);
        }

        if !self.subtitle.is_empty() {
            title = if self.subtitle.chars().next().unwrap().is_alphanumeric()
                && title.chars().last().unwrap().is_alphanumeric()
            {
                format!("{}: {}", title, self.subtitle)
            } else {
                format!("{} {}", title, self.subtitle)
            };
        }

        if !self.series.is_empty() && !self.number.is_empty() {
            title = format!("{} ({} #{})", title, self.series, self.number);
        }

        title
    }

    // TODO: handle the following case: *Walter M. Miller Jr.*?
    // NOTE: e.g.: John Le Carré: the space between *Le* and *Carré*
    // is a non-breaking space
    pub fn alphabetic_author(&self) -> &str {
        alphabetic_author(&self.author)
    }

    pub fn alphabetic_title(&self) -> &str {
        alphabetic_title(&self.title, &self.language)
    }

    pub fn label(&self) -> String {
        if !self.author.is_empty() {
            format!("{} · {}", self.title(), &self.author)
        } else {
            self.title()
        }
    }
}

/// Returns the sort key for a title by stripping leading articles based on
/// the book's language, delegating to the [`TITLE_PREFIXES`] regex table.
///
/// Used by both [`Info::alphabetic_title`] and the DB sort-rank layer so that
/// the article-stripping logic lives in exactly one place.
pub(crate) fn alphabetic_title<'a>(title: &'a str, language: &str) -> &'a str {
    let lang = if language.is_empty() || language.starts_with("en") {
        "en"
    } else if language.starts_with("fr") {
        "fr"
    } else {
        language
    };

    if let Some(m) = TITLE_PREFIXES.get(lang).and_then(|re| re.find(title)) {
        return title[m.end()..].trim_start();
    }

    title
}

/// Returns the sort key for an author string by extracting the last word of
/// the first name in a comma-separated list.
///
/// Used by both [`Info::alphabetic_author`] and the DB sort-rank layer so that
/// the extraction logic lives in exactly one place.
pub(crate) fn alphabetic_author(author: &str) -> &str {
    author
        .split(',')
        .next()
        .and_then(|a| a.split(' ').next_back())
        .unwrap_or_default()
}

pub fn make_query(text: &str) -> Option<Regex> {
    let any = Regex::new(r"^(\.*|\s)$").unwrap();

    if any.is_match(text) {
        return None;
    }

    let text = text
        .replace('a', "[aáàâä]")
        .replace('e', "[eéèêë]")
        .replace('i', "[iíìîï]")
        .replace('o', "[oóòôö]")
        .replace('u', "[uúùûü]")
        .replace('c', "[cç]")
        .replace("ae", "(ae|æ)")
        .replace("oe", "(oe|œ)");
    Regex::new(&format!("(?i){}", text))
        .map_err(|e| error!("Can't create query: {:#}.", e))
        .ok()
}

#[derive(Debug, Clone, Default)]
pub struct BookQuery {
    pub free: Option<Regex>,
    pub title: Option<Regex>,
    pub subtitle: Option<Regex>,
    pub author: Option<Regex>,
    pub year: Option<Regex>,
    pub language: Option<Regex>,
    pub publisher: Option<Regex>,
    pub series: Option<Regex>,
    pub edition: Option<Regex>,
    pub volume: Option<Regex>,
    pub number: Option<Regex>,
    pub reading: Option<bool>,
    pub new: Option<bool>,
    pub finished: Option<bool>,
    pub annotations: Option<bool>,
    pub bookmarks: Option<bool>,
    pub opened_after: Option<(bool, NaiveDateTime)>,
    pub added_after: Option<(bool, NaiveDateTime)>,
}

impl BookQuery {
    pub fn new(text: &str) -> Option<BookQuery> {
        let mut buf = Vec::new();
        let mut query = BookQuery::default();
        for word in text.rsplit(' ') {
            let mut chars = word.chars().peekable();
            match chars.next() {
                Some('\'') => {
                    let mut invert = false;
                    if chars.peek() == Some(&'!') {
                        invert = true;
                        chars.next();
                    }
                    match chars.next() {
                        Some('t') => {
                            buf.reverse();
                            query.title = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('u') => {
                            buf.reverse();
                            query.subtitle = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('a') => {
                            buf.reverse();
                            query.author = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('y') => {
                            buf.reverse();
                            query.year = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('l') => {
                            buf.reverse();
                            query.language = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('p') => {
                            buf.reverse();
                            query.publisher = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('s') => {
                            buf.reverse();
                            query.series = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('e') => {
                            buf.reverse();
                            query.edition = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('v') => {
                            buf.reverse();
                            query.volume = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('n') => {
                            buf.reverse();
                            query.number = make_query(&buf.join(" "));
                            buf.clear();
                        }
                        Some('R') => query.reading = Some(!invert),
                        Some('N') => query.new = Some(!invert),
                        Some('F') => query.finished = Some(!invert),
                        Some('A') => query.annotations = Some(!invert),
                        Some('B') => query.bookmarks = Some(!invert),
                        Some('O') => {
                            buf.reverse();
                            query.opened_after = NaiveDateTime::parse_from_str(
                                &buf.join(" "),
                                datetime_format::FORMAT,
                            )
                            .ok()
                            .map(|opened| (!invert, opened));
                            buf.clear();
                        }
                        Some('D') => {
                            buf.reverse();
                            query.added_after = NaiveDateTime::parse_from_str(
                                &buf.join(" "),
                                datetime_format::FORMAT,
                            )
                            .ok()
                            .map(|added| (!invert, added));
                            buf.clear();
                        }
                        Some('\'') => buf.push(&word[1..]),
                        _ => (),
                    }
                }
                _ => buf.push(word),
            }
        }
        buf.reverse();
        query.free = make_query(&buf.join(" "));
        if query.free.is_none()
            && query.title.is_none()
            && query.subtitle.is_none()
            && query.author.is_none()
            && query.year.is_none()
            && query.language.is_none()
            && query.publisher.is_none()
            && query.series.is_none()
            && query.edition.is_none()
            && query.volume.is_none()
            && query.number.is_none()
            && query.reading.is_none()
            && query.new.is_none()
            && query.finished.is_none()
            && query.annotations.is_none()
            && query.bookmarks.is_none()
            && query.opened_after.is_none()
            && query.added_after.is_none()
        {
            None
        } else {
            Some(query)
        }
    }

    #[inline]
    pub fn is_match(&self, info: &Info) -> bool {
        self.free.as_ref().map(|re| {
            re.is_match(&info.title)
                || re.is_match(&info.subtitle)
                || re.is_match(&info.author)
                || re.is_match(&info.series)
                || info.file.path.to_str().map_or(false, |s| re.is_match(s))
        }) != Some(false)
            && self.title.as_ref().map(|re| re.is_match(&info.title)) != Some(false)
            && self.subtitle.as_ref().map(|re| re.is_match(&info.subtitle)) != Some(false)
            && self.author.as_ref().map(|re| re.is_match(&info.author)) != Some(false)
            && self.year.as_ref().map(|re| re.is_match(&info.year)) != Some(false)
            && self.language.as_ref().map(|re| re.is_match(&info.language)) != Some(false)
            && self
                .publisher
                .as_ref()
                .map(|re| re.is_match(&info.publisher))
                != Some(false)
            && self.series.as_ref().map(|re| re.is_match(&info.series)) != Some(false)
            && self.edition.as_ref().map(|re| re.is_match(&info.edition)) != Some(false)
            && self.volume.as_ref().map(|re| re.is_match(&info.volume)) != Some(false)
            && self.number.as_ref().map(|re| re.is_match(&info.number)) != Some(false)
            && self
                .reading
                .as_ref()
                .map(|eq| info.simple_status().eq(&SimpleStatus::Reading) == *eq)
                != Some(false)
            && self
                .new
                .as_ref()
                .map(|eq| info.simple_status().eq(&SimpleStatus::New) == *eq)
                != Some(false)
            && self
                .finished
                .as_ref()
                .map(|eq| info.simple_status().eq(&SimpleStatus::Finished) == *eq)
                != Some(false)
            && self.annotations.as_ref().map(|eq| {
                info.reader
                    .as_ref()
                    .map_or(false, |r| !r.annotations.is_empty())
                    == *eq
            }) != Some(false)
            && self.bookmarks.as_ref().map(|eq| {
                info.reader
                    .as_ref()
                    .map_or(false, |r| !r.bookmarks.is_empty())
                    == *eq
            }) != Some(false)
            && self.opened_after.as_ref().map(|(eq, opened)| {
                info.reader.as_ref().map_or(false, |r| r.opened.gt(opened)) == *eq
            }) != Some(false)
            && self
                .added_after
                .as_ref()
                .map(|(eq, added)| info.added.gt(added) == *eq)
                != Some(false)
    }

    #[inline]
    pub fn is_simple_match(&self, text: &str) -> bool {
        self.free.as_ref().map_or(true, |q| q.is_match(text))
    }
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SortMethod {
    Opened,
    Added,
    Status,
    Progress,
    Title,
    Year,
    Author,
    Series,
    Pages,
    Size,
    Kind,
    FileName,
    FilePath,
}

impl SortMethod {
    pub fn reverse_order(self) -> bool {
        !matches!(
            self,
            SortMethod::Author
                | SortMethod::Title
                | SortMethod::Series
                | SortMethod::Kind
                | SortMethod::FileName
                | SortMethod::FilePath
        )
    }

    pub fn is_status_related(self) -> bool {
        matches!(
            self,
            SortMethod::Opened | SortMethod::Status | SortMethod::Progress
        )
    }

    pub fn label(&self) -> &str {
        match *self {
            SortMethod::Opened => "Date Opened",
            SortMethod::Added => "Date Added",
            SortMethod::Status => "Status",
            SortMethod::Progress => "Progress",
            SortMethod::Author => "Author",
            SortMethod::Title => "Title",
            SortMethod::Year => "Year",
            SortMethod::Series => "Series",
            SortMethod::Size => "File Size",
            SortMethod::Kind => "File Type",
            SortMethod::Pages => "Pages Count",
            SortMethod::FileName => "File Name",
            SortMethod::FilePath => "File Path",
        }
    }

    pub fn title(self) -> String {
        format!("Sort by: {}", self.label())
    }
}

#[inline]
#[cfg_attr(feature = "otel", tracing::instrument)]
pub fn sorter(sort_method: SortMethod) -> fn(&Info, &Info) -> Ordering {
    match sort_method {
        SortMethod::Opened => sort_opened,
        SortMethod::Added => sort_added,
        SortMethod::Status => sort_status,
        SortMethod::Progress => sort_progress,
        SortMethod::Author => sort_author,
        SortMethod::Title => sort_title,
        SortMethod::Year => sort_year,
        SortMethod::Series => sort_series,
        SortMethod::Size => sort_size,
        SortMethod::Kind => sort_kind,
        SortMethod::Pages => sort_pages,
        SortMethod::FileName => sort_filename,
        SortMethod::FilePath => sort_filepath,
    }
}

pub fn sort_opened(i1: &Info, i2: &Info) -> Ordering {
    i1.reader
        .as_ref()
        .map(|r1| r1.opened)
        .cmp(&i2.reader.as_ref().map(|r2| r2.opened))
}

pub fn sort_added(i1: &Info, i2: &Info) -> Ordering {
    i1.added.cmp(&i2.added)
}

pub fn sort_pages(i1: &Info, i2: &Info) -> Ordering {
    i1.reader
        .as_ref()
        .map(|r1| r1.pages_count)
        .cmp(&i2.reader.as_ref().map(|r2| r2.pages_count))
}

// FIXME: 'Z'.cmp('É') equals Ordering::Less
pub fn sort_author(i1: &Info, i2: &Info) -> Ordering {
    i1.alphabetic_author().cmp(i2.alphabetic_author())
}

pub fn sort_title(i1: &Info, i2: &Info) -> Ordering {
    let mut i1_title = i1.alphabetic_title().to_string();
    let mut i2_title = i2.alphabetic_title().to_string();

    if i1_title.is_empty() {
        i1_title = i1.file_stem()
    }

    if i2_title.is_empty() {
        i2_title = i2.file_stem()
    }

    natural_cmp(i1_title.as_str(), i2_title.as_str())
}

pub fn sort_status(i1: &Info, i2: &Info) -> Ordering {
    match (i1.simple_status(), i2.simple_status()) {
        (SimpleStatus::Reading, SimpleStatus::Reading)
        | (SimpleStatus::Finished, SimpleStatus::Finished) => sort_opened(i1, i2),
        (SimpleStatus::New, SimpleStatus::New) => sort_added(i1, i2),
        (SimpleStatus::New, SimpleStatus::Finished) => Ordering::Greater,
        (SimpleStatus::Finished, SimpleStatus::New) => Ordering::Less,
        (SimpleStatus::New, SimpleStatus::Reading) => Ordering::Less,
        (SimpleStatus::Reading, SimpleStatus::New) => Ordering::Greater,
        (SimpleStatus::Finished, SimpleStatus::Reading) => Ordering::Less,
        (SimpleStatus::Reading, SimpleStatus::Finished) => Ordering::Greater,
    }
}

// Ordering: Finished < New < Reading.
pub fn sort_progress(i1: &Info, i2: &Info) -> Ordering {
    match (i1.status(), i2.status()) {
        (Status::Finished, Status::Finished) => Ordering::Equal,
        (Status::New, Status::New) => Ordering::Equal,
        (Status::New, Status::Finished) => Ordering::Greater,
        (Status::Finished, Status::New) => Ordering::Less,
        (Status::New, Status::Reading(..)) => Ordering::Less,
        (Status::Reading(..), Status::New) => Ordering::Greater,
        (Status::Finished, Status::Reading(..)) => Ordering::Less,
        (Status::Reading(..), Status::Finished) => Ordering::Greater,
        (Status::Reading(p1), Status::Reading(p2)) => {
            p1.partial_cmp(&p2).unwrap_or(Ordering::Equal)
        }
    }
}

pub fn sort_size(i1: &Info, i2: &Info) -> Ordering {
    i1.file.size.cmp(&i2.file.size)
}

pub fn sort_kind(i1: &Info, i2: &Info) -> Ordering {
    i1.file.kind.cmp(&i2.file.kind)
}

pub fn sort_year(i1: &Info, i2: &Info) -> Ordering {
    i1.year.cmp(&i2.year)
}

pub fn sort_series(i1: &Info, i2: &Info) -> Ordering {
    i1.series.cmp(&i2.series).then_with(|| {
        i1.number
            .parse::<usize>()
            .ok()
            .zip(i2.number.parse::<usize>().ok())
            .map_or_else(|| i1.number.cmp(&i2.number), |(a, b)| a.cmp(&b))
    })
}

pub fn sort_filename(i1: &Info, i2: &Info) -> Ordering {
    let n1 = i1.file.path.file_name().map(OsStr::to_string_lossy);
    let n2 = i2.file.path.file_name().map(OsStr::to_string_lossy);
    match (n1, n2) {
        (Some(a), Some(b)) => natural_cmp(&a, &b),
        (a, b) => a.map(|s| s.into_owned()).cmp(&b.map(|s| s.into_owned())),
    }
}

pub fn sort_filepath(i1: &Info, i2: &Info) -> Ordering {
    natural_cmp(
        &i1.file.path.to_string_lossy(),
        &i2.file.path.to_string_lossy(),
    )
}

/// Compares two strings using natural sort order so that embedded numbers sort
/// by value rather than by their string representation ("9" < "10" < "100").
///
/// Each string is split into alternating runs of numeric and non-numeric
/// characters. A numeric token is an integer optionally followed by a decimal
/// part (`.<digits>`), so `"4.5"` is treated as a single number rather than
/// the three segments `4`, `.`, `5`. Numeric tokens are compared as `f64`
/// values, which means leading zeros are ignored ("01" == "1") and fractional
/// parts are respected ("4" < "4.5" < "5"). Non-numeric runs are compared
/// lexicographically. When one run is numeric and the other is text, the
/// numeric segment sorts first.
///
/// # Examples
///
/// ```ignore
/// // This example uses private API; doc tests cannot access non-public items.
/// use std::cmp::Ordering;
///
/// // Numeric runs compare by value, not string length.
/// assert_eq!(natural_cmp("9", "10"), Ordering::Less);
/// assert_eq!(natural_cmp("100", "99"), Ordering::Greater);
///
/// // Fractional numbers are supported.
/// assert_eq!(natural_cmp("Vol 4", "Vol 4.5"), Ordering::Less);
/// assert_eq!(natural_cmp("Vol 4.5", "Vol 5"), Ordering::Less);
///
/// // Leading zeros are ignored: "01" and "1" are numerically equal.
/// assert_eq!(natural_cmp("01", "1"), Ordering::Equal);
///
/// // Mixed strings compare segment by segment.
/// assert_eq!(natural_cmp("Chapter 9", "Chapter 10"), Ordering::Less);
/// ```
#[cfg_attr(feature = "otel", tracing::instrument(ret(level=tracing::Level::TRACE)))]
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a_rest = a;
    let mut b_rest = b;

    loop {
        if a_rest.is_empty() && b_rest.is_empty() {
            return Ordering::Equal;
        }
        if a_rest.is_empty() {
            return Ordering::Less;
        }
        if b_rest.is_empty() {
            return Ordering::Greater;
        }

        let a_digit = a_rest.starts_with(|c: char| c.is_ascii_digit());
        let b_digit = b_rest.starts_with(|c: char| c.is_ascii_digit());

        match (a_digit, b_digit) {
            (true, true) => {
                let a_num_len = numeric_token_len(a_rest);
                let b_num_len = numeric_token_len(b_rest);
                let a_num: f64 = a_rest[..a_num_len].parse().unwrap_or(f64::MAX);
                let b_num: f64 = b_rest[..b_num_len].parse().unwrap_or(f64::MAX);
                let ord = a_num.partial_cmp(&b_num).unwrap_or(Ordering::Equal);
                if ord != Ordering::Equal {
                    return ord;
                }
                a_rest = &a_rest[a_num_len..];
                b_rest = &b_rest[b_num_len..];
            }
            (false, false) => {
                let a_text_len = a_rest
                    .find(|c: char| c.is_ascii_digit())
                    .unwrap_or(a_rest.len());
                let b_text_len = b_rest
                    .find(|c: char| c.is_ascii_digit())
                    .unwrap_or(b_rest.len());
                let ord = a_rest[..a_text_len].cmp(&b_rest[..b_text_len]);
                if ord != Ordering::Equal {
                    return ord;
                }
                a_rest = &a_rest[a_text_len..];
                b_rest = &b_rest[b_text_len..];
            }
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
        }
    }
}

/// Returns the byte length of a numeric token starting at the beginning of
/// `s`. A numeric token is one or more ASCII digits optionally followed by a
/// single `'.'` and one or more additional ASCII digits (e.g. `"4"`, `"4.5"`).
/// A trailing dot with no digits after it (e.g. `"4."`) is not included.
fn numeric_token_len(s: &str) -> usize {
    let int_len = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());

    let rest = &s[int_len..];
    if let Some(after_dot) = rest.strip_prefix('.') {
        let frac_len = after_dot
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after_dot.len());
        if frac_len > 0 {
            return int_len + 1 + frac_len;
        }
    }

    int_len
}

lazy_static! {
    pub static ref TITLE_PREFIXES: FxHashMap<&'static str, Regex> = {
        let mut p = FxHashMap::default();
        p.insert("en", Regex::new(r"^(The|An?)\s").unwrap());
        p.insert(
            "fr",
            Regex::new(r"^(Les?\s|La\s|L’|Une?\s|Des?\s|Du\s)").unwrap(),
        );
        p
    };
}

#[inline]
#[cfg_attr(feature = "otel", tracing::instrument(skip(info)))]
pub fn extract_metadata_from_document(prefix: &Path, info: &mut Info) {
    let path = prefix.join(&info.file.path);

    match info.file.kind.as_ref() {
        "epub" => match EpubDocument::new(&path) {
            Ok(doc) => {
                info.title = doc.title().unwrap_or_default();
                info.author = doc.author().unwrap_or_default();
                info.year = doc.year().unwrap_or_default();
                info.publisher = doc.publisher().unwrap_or_default();
                if let Some((title, index)) = doc.series() {
                    info.series = title;
                    info.number = index;
                }
                info.language = doc.language().unwrap_or_default();
                info.categories.append(&mut doc.categories());
            }
            Err(e) => error!("Can't open {}: {:#}.", info.file.path.display(), e),
        },
        "html" | "htm" => match HtmlDocument::new(&path) {
            Ok(doc) => {
                info.title = doc.title().unwrap_or_default();
                info.author = doc.author().unwrap_or_default();
                info.language = doc.language().unwrap_or_default();
            }
            Err(e) => error!("Can't open {}: {:#}.", info.file.path.display(), e),
        },
        "pdf" => match PdfOpener::new().and_then(|o| o.open(path).ok()) {
            Some(doc) => {
                info.title = doc.title().unwrap_or_default();
                info.author = doc.author().unwrap_or_default();
            }
            None => error!("Can't open {}.", info.file.path.display()),
        },
        "djvu" | "djv" => match DjvuOpener::new().and_then(|o| o.open(path)) {
            Some(doc) => {
                info.title = doc.title().unwrap_or_default();
                info.author = doc.author().unwrap_or_default();
                info.year = doc.year().unwrap_or_default();
                info.series = doc.series().unwrap_or_default();
                info.publisher = doc.publisher().unwrap_or_default();
            }
            None => error!("Can't open {}.", info.file.path.display()),
        },
        _ => {
            warn!(
                "Don't know how to extract metadata from {}.",
                &info.file.kind
            );
        }
    }
}

pub fn extract_metadata_from_filename(_prefix: &Path, info: &mut Info) {
    if let Some(filename) = info.file.path.file_name().and_then(OsStr::to_str) {
        let mut start_index = 0;

        if filename.starts_with('(') {
            start_index += 1;
            if let Some(index) = filename[start_index..].find(')') {
                info.series = filename[start_index..start_index + index]
                    .trim_end()
                    .to_string();
                start_index += index + 1;
            }
        }

        if let Some(index) = filename[start_index..].find("- ") {
            info.author = filename[start_index..start_index + index]
                .trim()
                .to_string();
            start_index += index + 1;
        }

        let title_start = start_index;

        if let Some(index) = filename[start_index..].find('_') {
            info.title = filename[start_index..start_index + index]
                .trim_start()
                .to_string();
            start_index += index + 1;
        }

        if let Some(index) = filename[start_index..].find('-') {
            if title_start == start_index {
                info.title = filename[start_index..start_index + index]
                    .trim_start()
                    .to_string();
            } else {
                info.subtitle = filename[start_index..start_index + index]
                    .trim_start()
                    .to_string();
            }
            start_index += index + 1;
        }

        if let Some(index) = filename[start_index..].find('(') {
            info.publisher = filename[start_index..start_index + index]
                .trim_end()
                .to_string();
            start_index += index + 1;
        }

        if let Some(index) = filename[start_index..].find(')') {
            info.year = filename[start_index..start_index + index].to_string();
        }
    }
}

pub fn consolidate(_prefix: &Path, info: &mut Info) {
    if info.subtitle.is_empty() {
        if let Some(index) = info.title.find(':') {
            let cur_title = info.title.clone();
            let (title, subtitle) = cur_title.split_at(index);
            info.title = title.trim_end().to_string();
            info.subtitle = subtitle[1..].trim_start().to_string();
        }
    }

    if info.language.is_empty() || info.language.starts_with("en") {
        info.title = titlecase(&info.title);
        info.subtitle = titlecase(&info.subtitle);
    }

    info.title = info.title.replace('\'', "’");
    info.subtitle = info.subtitle.replace('\'', "’");
    info.author = info.author.replace('\'', "’");
    if info.year.len() > 4 {
        info.year = info.year[..4].to_string();
    }
    info.series = info.series.replace('\'', "’");
    info.publisher = info.publisher.replace('\'', "’");
}

pub fn rename_from_info(prefix: &Path, info: &mut Info) {
    let new_file_name = file_name_from_info(info);
    if !new_file_name.is_empty() {
        let old_path = prefix.join(&info.file.path);
        let new_path = old_path.with_file_name(&new_file_name);
        if old_path != new_path {
            match fs::rename(&old_path, &new_path) {
                Err(e) => error!(
                    "Can't rename {} to {}: {:#}.",
                    old_path.display(),
                    new_path.display(),
                    e
                ),
                Ok(..) => {
                    let relat = new_path.strip_prefix(prefix).unwrap_or(&new_path);
                    info.file.path = relat.to_path_buf();
                }
            }
        }
    }
}

pub fn file_name_from_info(info: &Info) -> String {
    if info.title.is_empty() {
        return "".to_string();
    }
    let mut base = asciify(&info.title);
    if !info.subtitle.is_empty() {
        base = format!("{} - {}", base, asciify(&info.subtitle));
    }
    if !info.volume.is_empty() {
        base = format!("{} - {}", base, info.volume);
    }
    if !info.number.is_empty() && info.series.is_empty() {
        base = format!("{} - {}", base, info.number);
    }
    if !info.author.is_empty() {
        base = format!("{} - {}", base, asciify(&info.author));
    }
    base = format!("{}.{}", base, info.file.kind);
    base.replace("..", ".")
        .replace('/', " ")
        .replace('?', "")
        .replace('!', "")
        .replace(':', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_info(title: &str, filename: &str) -> Info {
        Info {
            title: title.to_string(),
            file: FileInfo {
                path: PathBuf::from(filename),
                absolute_path: PathBuf::new(),
                kind: "epub".to_string(),
                size: 0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn natural_cmp_pure_numbers() {
        assert_eq!(natural_cmp("9", "10"), Ordering::Less);
        assert_eq!(natural_cmp("10", "9"), Ordering::Greater);
        assert_eq!(natural_cmp("100", "99"), Ordering::Greater);
        assert_eq!(natural_cmp("10", "10"), Ordering::Equal);
    }

    #[test]
    fn natural_cmp_leading_zeros_are_numerically_equal() {
        assert_eq!(natural_cmp("01", "1"), Ordering::Equal);
        assert_eq!(natural_cmp("001", "1"), Ordering::Equal);
        assert_eq!(natural_cmp("007", "7"), Ordering::Equal);
    }

    #[test]
    fn natural_cmp_mixed_strings() {
        assert_eq!(natural_cmp("Chapter 9", "Chapter 10"), Ordering::Less);
        assert_eq!(natural_cmp("Chapter 10", "Chapter 9"), Ordering::Greater);
        assert_eq!(
            natural_cmp("Vol 2 Chapter 9", "Vol 2 Chapter 10"),
            Ordering::Less
        );
    }

    #[test]
    fn natural_cmp_pure_text() {
        assert_eq!(natural_cmp("abc", "abd"), Ordering::Less);
        assert_eq!(natural_cmp("abd", "abc"), Ordering::Greater);
        assert_eq!(natural_cmp("abc", "abc"), Ordering::Equal);
    }

    #[test]
    fn natural_cmp_empty_strings() {
        assert_eq!(natural_cmp("", ""), Ordering::Equal);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a", ""), Ordering::Greater);
    }

    #[test]
    fn natural_cmp_fractional_numbers() {
        assert_eq!(natural_cmp("Vol 4", "Vol 4.5"), Ordering::Less);
        assert_eq!(natural_cmp("Vol 4.5", "Vol 5"), Ordering::Less);
        assert_eq!(natural_cmp("Vol 4.5", "Vol 4.5"), Ordering::Equal);
        // 4.10 parses as 4.1 which is less than 4.9
        assert_eq!(natural_cmp("Vol 4.10", "Vol 4.9"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_decimal_between_integers() {
        assert_eq!(natural_cmp("1", "10.5"), Ordering::Less);
        assert_eq!(natural_cmp("10", "10.5"), Ordering::Less);
        assert_eq!(natural_cmp("10.5", "11"), Ordering::Less);

        let mut items = ["11", "1", "10.5", "10", "2"];
        items.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(items, ["1", "2", "10", "10.5", "11"]);
    }

    #[test]
    fn sort_title_decimal_volume_between_integers() {
        let v1 = make_info("Series Vol. 1", "a.epub");
        let v2 = make_info("Series Vol. 2", "b.epub");
        let v10 = make_info("Series Vol. 10", "c.epub");
        let v10_5 = make_info("Series Vol. 10.5", "d.epub");
        let v11 = make_info("Series Vol. 11", "e.epub");

        assert_eq!(sort_title(&v1, &v10_5), Ordering::Less);
        assert_eq!(sort_title(&v10, &v10_5), Ordering::Less);
        assert_eq!(sort_title(&v10_5, &v11), Ordering::Less);

        let mut books = [&v11, &v1, &v10_5, &v10, &v2];
        books.sort_by(|a, b| sort_title(a, b));
        let titles: Vec<_> = books.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Series Vol. 1",
                "Series Vol. 2",
                "Series Vol. 10",
                "Series Vol. 10.5",
                "Series Vol. 11",
            ]
        );
    }

    #[test]
    fn natural_cmp_trailing_dot_not_included_in_number() {
        // "4." — the dot has no digits after it, so it is not part of the number token.
        // "4." therefore sorts the same as "4" for the numeric part, then "." > "" text-wise.
        assert_eq!(natural_cmp("4.", "4"), Ordering::Greater);
    }

    #[test]
    fn natural_cmp_digit_before_text_segment() {
        assert_eq!(natural_cmp("1abc", "abc"), Ordering::Less);
        assert_eq!(natural_cmp("abc", "1abc"), Ordering::Greater);
    }

    #[test]
    fn natural_cmp_three_digit_before_two_digit() {
        let mut items = ["100", "9", "10", "1", "99", "01"];
        items.sort_by(|a, b| natural_cmp(a, b));
        // "01" and "1" are numerically equal so their relative order is
        // unspecified; assert only that the numeric ordering is correct.
        let without_leading_zero: Vec<_> = items.iter().filter(|&&s| s != "01").copied().collect();
        assert_eq!(without_leading_zero, vec!["1", "9", "10", "99", "100"]);
        assert!(items.contains(&"01"));
    }

    #[test]
    fn sort_filename_numerical_order() {
        let i9 = make_info("", "9 - Title.epub");
        let i10 = make_info("", "10 - Title.epub");
        let i100 = make_info("", "100 - Title.epub");
        assert_eq!(sort_filename(&i9, &i10), Ordering::Less);
        assert_eq!(sort_filename(&i100, &i10), Ordering::Greater);
        assert_eq!(sort_filename(&i10, &i10), Ordering::Equal);
    }

    #[test]
    fn sort_filename_mixed_names() {
        let i1 = make_info("", "Chapter 9.epub");
        let i2 = make_info("", "Chapter 10.epub");
        assert_eq!(sort_filename(&i1, &i2), Ordering::Less);
    }

    #[test]
    fn sort_filepath_numerical_order() {
        let i1 = make_info("", "Library/Vol 2/Chapter 9.epub");
        let i2 = make_info("", "Library/Vol 2/Chapter 10.epub");
        assert_eq!(sort_filepath(&i1, &i2), Ordering::Less);
    }

    #[test]
    fn sort_filepath_directory_numerical_order() {
        let i1 = make_info("", "Vol 9/book.epub");
        let i2 = make_info("", "Vol 10/book.epub");
        assert_eq!(sort_filepath(&i1, &i2), Ordering::Less);
    }

    #[test]
    fn sort_title_strips_articles_then_natural_sorts() {
        let i1 = make_info("The 9th Chapter", "a.epub");
        let i2 = make_info("The 10th Chapter", "b.epub");
        // After stripping "The ", compares "9th Chapter" vs "10th Chapter" — 9 < 10
        assert_eq!(sort_title(&i1, &i2), Ordering::Less);
    }

    #[test]
    fn sort_title_numbered_titles() {
        let i1984 = make_info("1984", "a.epub");
        let i2001 = make_info("2001: A Space Odyssey", "b.epub");
        assert_eq!(sort_title(&i1984, &i2001), Ordering::Less);
    }

    #[test]
    fn sort_title_plain_text_unchanged() {
        let ia = make_info("Apple", "a.epub");
        let ib = make_info("Banana", "b.epub");
        assert_eq!(sort_title(&ia, &ib), Ordering::Less);
        assert_eq!(sort_title(&ib, &ia), Ordering::Greater);
    }
}
