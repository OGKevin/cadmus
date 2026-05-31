mod preset;
pub mod versioned;

use crate::color::{BLACK, Color};
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::frontlight::LightLevels;
use crate::i18n::I18nDisplay;
use crate::metadata::{SortMethod, TextAlign};
use crate::unit::mm_to_px;
use fxhash::FxHashSet;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef};
use unic_langid::LanguageIdentifier;

pub use self::preset::{LightPreset, guess_frontlight};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fmt::{self, Debug};
use std::ops::{Index, IndexMut};
use std::path::PathBuf;

pub const SETTINGS_PATH: &str = "Settings.toml";
pub const DEFAULT_FONT_PATH: &str = "/mnt/onboard/fonts";
pub const INTERNAL_CARD_ROOT: &str = "/mnt/onboard";
pub const EXTERNAL_CARD_ROOT: &str = "/mnt/sd";
const LOGO_SPECIAL_PATH: &str = "logo:";
const COVER_SPECIAL_PATH: &str = "cover:";
const CALENDAR_SPECIAL_PATH: &str = "calendar:";
const BLANK_SPECIAL_PATH: &str = "blank:";
const BLANK_INVERTED_SPECIAL_PATH: &str = "blank-inverted:";

/// How to display intermission screens.
/// Logo, Cover, Calendar, Blank and BlankInverted are special values that map
/// to built-in displays.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum IntermissionDisplay {
    /// Display the built-in logo image.
    Logo,
    /// Display the cover of the currently reading book.
    Cover,
    /// Display the built-in calendar view.
    Calendar,
    /// Display a blank white screen.
    Blank,
    /// Display a blank black screen.
    BlankInverted,
    /// Display a custom image from the given path.
    Image(PathBuf),
}

impl Serialize for IntermissionDisplay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            IntermissionDisplay::Logo => serializer.serialize_str(LOGO_SPECIAL_PATH),
            IntermissionDisplay::Cover => serializer.serialize_str(COVER_SPECIAL_PATH),
            IntermissionDisplay::Calendar => serializer.serialize_str(CALENDAR_SPECIAL_PATH),
            IntermissionDisplay::Blank => serializer.serialize_str(BLANK_SPECIAL_PATH),
            IntermissionDisplay::BlankInverted => {
                serializer.serialize_str(BLANK_INVERTED_SPECIAL_PATH)
            }
            IntermissionDisplay::Image(path) => {
                serializer.serialize_str(path.to_string_lossy().as_ref())
            }
        }
    }
}

impl<'de> Deserialize<'de> for IntermissionDisplay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            LOGO_SPECIAL_PATH => IntermissionDisplay::Logo,
            COVER_SPECIAL_PATH => IntermissionDisplay::Cover,
            CALENDAR_SPECIAL_PATH => IntermissionDisplay::Calendar,
            BLANK_SPECIAL_PATH => IntermissionDisplay::Blank,
            BLANK_INVERTED_SPECIAL_PATH => IntermissionDisplay::BlankInverted,
            _ => IntermissionDisplay::Image(PathBuf::from(s)),
        })
    }
}

impl fmt::Display for IntermissionDisplay {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            IntermissionDisplay::Logo => write!(f, "Logo"),
            IntermissionDisplay::Cover => write!(f, "Cover"),
            IntermissionDisplay::Calendar => write!(f, "Calendar"),
            IntermissionDisplay::Blank => write!(f, "Blank"),
            IntermissionDisplay::BlankInverted => write!(f, "Blank Inverted"),
            IntermissionDisplay::Image(_) => write!(f, "Custom"),
        }
    }
}

impl IntermissionDisplay {
    /// Returns whether this display mode is supported for the given intermission kind.
    pub fn is_supported_for(&self, kind: IntermKind) -> bool {
        if !matches!(self, IntermissionDisplay::Calendar) {
            return true;
        }

        kind.supports_calendar()
    }
}

// Default font size in points.
pub const DEFAULT_FONT_SIZE: f32 = 11.0;
// Default margin width in millimeters.
pub const DEFAULT_MARGIN_WIDTH: i32 = 8;
// Default line height in ems.
pub const DEFAULT_LINE_HEIGHT: f32 = 1.2;
// Default font family name.
pub const DEFAULT_FONT_FAMILY: &str = "Libertinus Serif";
// Default text alignment.
pub const DEFAULT_TEXT_ALIGN: TextAlign = TextAlign::Left;
pub const HYPHEN_PENALTY: i32 = 50;
pub const STRETCH_TOLERANCE: f32 = 1.26;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RotationLock {
    Landscape,
    Portrait,
    Current,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ButtonScheme {
    Natural,
    Inverted,
}

impl fmt::Display for ButtonScheme {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

impl I18nDisplay for ButtonScheme {
    fn to_i18n_string(&self) -> String {
        match self {
            ButtonScheme::Natural => fl!("settings-button-scheme-natural"),
            ButtonScheme::Inverted => fl!("settings-button-scheme-inverted"),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntermKind {
    Suspend,
    PowerOff,
    Share,
}

impl IntermKind {
    pub fn text(&self) -> &str {
        match self {
            IntermKind::Suspend => "Sleeping",
            IntermKind::PowerOff => "Powered off",
            IntermKind::Share => "Shared",
        }
    }

    pub fn supports_calendar(self) -> bool {
        matches!(self, IntermKind::Suspend)
    }
}

/// Configuration for intermission screen displays.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Intermissions {
    suspend: IntermissionDisplay,
    power_off: IntermissionDisplay,
    share: IntermissionDisplay,
}

impl Index<IntermKind> for Intermissions {
    type Output = IntermissionDisplay;

    fn index(&self, key: IntermKind) -> &Self::Output {
        match key {
            IntermKind::Suspend => &self.suspend,
            IntermKind::PowerOff => &self.power_off,
            IntermKind::Share => &self.share,
        }
    }
}

impl IndexMut<IntermKind> for Intermissions {
    fn index_mut(&mut self, key: IntermKind) -> &mut Self::Output {
        match key {
            IntermKind::Suspend => &mut self.suspend,
            IntermKind::PowerOff => &mut self.power_off,
            IntermKind::Share => &mut self.share,
        }
    }
}

impl Intermissions {
    /// Updates an intermission display when the selected mode is valid for the target kind.
    pub fn set_display(&mut self, kind: IntermKind, display: IntermissionDisplay) -> bool {
        if !display.is_supported_for(kind) {
            return false;
        }

        self[kind] = display;
        true
    }

    /// Replaces unsupported intermission modes with the default logo display.
    pub fn sanitize(&mut self) -> bool {
        let mut changed = false;

        changed |= self.sanitize_kind(IntermKind::Suspend);
        changed |= self.sanitize_kind(IntermKind::PowerOff);
        changed |= self.sanitize_kind(IntermKind::Share);

        if changed {
            eprintln!(
                "ignoring unsupported calendar intermissions for power-off/share; using logo instead"
            );
        }

        changed
    }

    fn sanitize_kind(&mut self, kind: IntermKind) -> bool {
        if self[kind].is_supported_for(kind) {
            return false;
        }

        self[kind] = IntermissionDisplay::Logo;
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    pub selected_library: usize,
    pub keyboard_layout: String,
    pub frontlight: bool,
    pub wifi: bool,
    pub inverted: bool,
    pub sleep_cover: bool,
    pub auto_share: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_lock: Option<RotationLock>,
    pub button_scheme: ButtonScheme,
    pub auto_suspend: f32,
    pub auto_power_off: f32,
    pub time_format: String,
    pub date_format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_urls_queue: Option<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<LibrarySettings>,
    pub intermissions: Intermissions,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub frontlight_presets: Vec<LightPreset>,
    pub home: HomeSettings,
    pub reader: ReaderSettings,
    pub import: ImportSettings,
    pub dictionary: DictionarySettings,
    pub sketch: SketchSettings,
    pub calculator: CalculatorSettings,
    pub battery: BatterySettings,
    pub frontlight_levels: LightLevels,
    pub ota: OtaSettings,
    pub logging: LoggingSettings,
    pub settings_retention: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<LanguageIdentifier>,
}

impl Settings {
    /// Normalizes unsupported settings values loaded from disk.
    pub fn sanitize(&mut self) -> bool {
        self.intermissions.sanitize()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LibrarySettings {
    pub name: String,
    pub path: PathBuf,
    pub sort_method: SortMethod,
    pub first_column: FirstColumn,
    pub second_column: SecondColumn,
    pub thumbnail_previews: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<Hook>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<FinishedAction>,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        LibrarySettings {
            name: "Unnamed".to_string(),
            path: env::current_dir()
                .ok()
                .unwrap_or_else(|| PathBuf::from("/")),
            sort_method: SortMethod::Opened,
            first_column: FirstColumn::TitleAndAuthor,
            second_column: SecondColumn::Progress,
            thumbnail_previews: true,
            hooks: Vec::new(),
            finished: None,
        }
    }
}

/// Settings controlling which files are imported into the library.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ImportSettings {
    pub sync_metadata: bool,
    pub metadata_kinds: FxHashSet<String>,
    #[serde(deserialize_with = "deserialize_file_extension_set")]
    pub allowed_kinds: FxHashSet<FileExtension>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct DictionarySettings {
    pub margin_width: i32,
    pub font_size: f32,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, Vec<String>>,
}

impl Default for DictionarySettings {
    fn default() -> Self {
        DictionarySettings {
            font_size: 11.0,
            margin_width: 4,
            languages: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SketchSettings {
    pub save_path: PathBuf,
    pub notify_success: bool,
    pub pen: Pen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct CalculatorSettings {
    pub font_size: f32,
    pub margin_width: i32,
    pub history_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Pen {
    pub size: i32,
    pub color: Color,
    pub dynamic: bool,
    pub amplitude: f32,
    pub min_speed: f32,
    pub max_speed: f32,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            size: 2,
            color: BLACK,
            dynamic: true,
            amplitude: 4.0,
            min_speed: 0.0,
            max_speed: mm_to_px(254.0, CURRENT_DEVICE.dpi),
        }
    }
}

impl Default for SketchSettings {
    fn default() -> Self {
        SketchSettings {
            save_path: PathBuf::from("Sketches"),
            notify_success: true,
            pen: Pen::default(),
        }
    }
}

impl Default for CalculatorSettings {
    fn default() -> Self {
        CalculatorSettings {
            font_size: 8.0,
            margin_width: 2,
            history_size: 4096,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Columns {
    first: FirstColumn,
    second: SecondColumn,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirstColumn {
    TitleAndAuthor,
    FileName,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondColumn {
    Progress,
    Year,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Hook {
    pub path: PathBuf,
    pub program: PathBuf,
    pub sort_method: Option<SortMethod>,
    pub first_column: Option<FirstColumn>,
    pub second_column: Option<SecondColumn>,
}

impl Default for Hook {
    fn default() -> Self {
        Hook {
            path: PathBuf::default(),
            program: PathBuf::default(),
            sort_method: None,
            first_column: None,
            second_column: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct HomeSettings {
    pub address_bar: bool,
    pub navigation_bar: bool,
    pub max_levels: usize,
    pub max_trash_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RefreshRateSettings {
    #[serde(flatten)]
    pub global: RefreshRatePair,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub by_kind: HashMap<String, RefreshRatePair>,
}

/// A known file extension for which per-kind refresh rates can be configured.
///
/// The serialized string (e.g. `"epub"`, `"cbz"`) is used as the key in
/// [`RefreshRateSettings::by_kind`] and as values in [`ImportSettings::allowed_kinds`].
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileExtension {
    /// Comic book RAR archive.
    Cbr,
    /// Comic book ZIP archive.
    Cbz,
    /// DjVu document.
    Djvu,
    /// EPUB ebook.
    Epub,
    /// FictionBook document.
    Fb2,
    /// HTML document.
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

    /// Returns the lowercase string representation used as the TOML key.
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
            "djvu" => Ok(FileExtension::Djvu),
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

impl<'q> sqlx::Encode<'q, Sqlite> for FileExtension {
    fn encode_by_ref(&self, buf: &mut Vec<SqliteArgumentValue<'q>>) -> Result<IsNull, BoxDynError> {
        self.as_str().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for FileExtension {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as sqlx::Decode<'r, Sqlite>>::decode(value)?;
        s.parse()
            .map_err(|UnknownFileExtension(ext)| format!("unknown file extension: {ext}").into())
    }
}

fn deserialize_file_extension_set<'de, D>(
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

impl fmt::Display for FileExtension {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RefreshRatePair {
    pub regular: u8,
    pub inverted: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ReaderSettings {
    pub finished: FinishedAction,
    pub south_east_corner: SouthEastCornerAction,
    pub bottom_right_gesture: BottomRightGestureAction,
    pub south_strip: SouthStripAction,
    pub west_strip: WestStripAction,
    pub east_strip: EastStripAction,
    pub strip_width: f32,
    pub corner_width: f32,
    pub font_path: String,
    pub font_family: String,
    pub font_size: f32,
    pub min_font_size: f32,
    pub max_font_size: f32,
    pub text_align: TextAlign,
    pub margin_width: i32,
    pub min_margin_width: i32,
    pub max_margin_width: i32,
    pub line_height: f32,
    pub continuous_fit_to_width: bool,
    pub ignore_document_css: bool,
    #[serde(deserialize_with = "deserialize_file_extension_set")]
    pub dithered_kinds: FxHashSet<FileExtension>,
    pub paragraph_breaker: ParagraphBreakerSettings,
    pub refresh_rate: RefreshRateSettings,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ParagraphBreakerSettings {
    pub hyphen_penalty: i32,
    pub stretch_tolerance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct BatterySettings {
    pub warn: f32,
    pub power_off: f32,
}

/// Configures structured logging to disk and optional OTLP export.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LoggingSettings {
    /// Enables logging output when set to true.
    pub enabled: bool,
    /// Minimum log level to record (for example: "info", "debug").
    pub level: String,
    /// Maximum number of rotated log files to keep.
    pub max_files: usize,
    /// Directory where JSON log files are written.
    pub directory: PathBuf,
    /// Optional OTLP endpoint; env vars override this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otlp_endpoint: Option<String>,
    /// Optional Pyroscope server URL for continuous profiling; env vars override this value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pyroscope_endpoint: Option<String>,
    /// Captures kernel logs via logread if kernel log capture is supported.
    pub enable_kern_log: bool,
    /// Captures D-Bus signals via the in-process zbus DbusMonitorTask when D-Bus log capture is supported.
    pub enable_dbus_log: bool,
}

/// OTA update settings.
///
/// Authentication is handled via GitHub device auth flow — no token configuration
/// is required in `Settings.toml`. The token is obtained interactively and
/// persisted to disk by the application.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct OtaSettings {}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinishedAction {
    Notify,
    Close,
    GoToNext,
}

impl fmt::Display for FinishedAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            FinishedAction::Notify => write!(f, "Notify"),
            FinishedAction::Close => write!(f, "Close"),
            FinishedAction::GoToNext => write!(f, "Go to Next"),
        }
    }
}

impl I18nDisplay for FinishedAction {
    fn to_i18n_string(&self) -> String {
        match self {
            FinishedAction::Notify => fl!("settings-finished-action-notify"),
            FinishedAction::Close => fl!("settings-finished-action-close"),
            FinishedAction::GoToNext => fl!("settings-finished-action-goto-next"),
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SouthEastCornerAction {
    NextPage,
    GoToPage,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BottomRightGestureAction {
    ToggleDithered,
    ToggleInverted,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SouthStripAction {
    ToggleBars,
    NextPage,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EastStripAction {
    PreviousPage,
    NextPage,
    None,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WestStripAction {
    PreviousPage,
    NextPage,
    None,
}

impl Default for RefreshRateSettings {
    fn default() -> Self {
        RefreshRateSettings {
            global: RefreshRatePair {
                regular: 8,
                inverted: 2,
            },
            by_kind: HashMap::new(),
        }
    }
}

impl Default for HomeSettings {
    fn default() -> Self {
        HomeSettings {
            address_bar: false,
            navigation_bar: true,
            max_levels: 3,
            max_trash_size: 32 * (1 << 20),
        }
    }
}

impl Default for ParagraphBreakerSettings {
    fn default() -> Self {
        ParagraphBreakerSettings {
            hyphen_penalty: HYPHEN_PENALTY,
            stretch_tolerance: STRETCH_TOLERANCE,
        }
    }
}

impl Default for ReaderSettings {
    fn default() -> Self {
        ReaderSettings {
            finished: FinishedAction::Close,
            south_east_corner: SouthEastCornerAction::GoToPage,
            bottom_right_gesture: BottomRightGestureAction::ToggleDithered,
            south_strip: SouthStripAction::ToggleBars,
            west_strip: WestStripAction::PreviousPage,
            east_strip: EastStripAction::NextPage,
            strip_width: 0.6,
            corner_width: 0.4,
            font_path: DEFAULT_FONT_PATH.to_string(),
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            min_font_size: DEFAULT_FONT_SIZE / 2.0,
            max_font_size: 3.0 * DEFAULT_FONT_SIZE / 2.0,
            text_align: DEFAULT_TEXT_ALIGN,
            margin_width: DEFAULT_MARGIN_WIDTH,
            min_margin_width: DEFAULT_MARGIN_WIDTH.saturating_sub(8),
            max_margin_width: DEFAULT_MARGIN_WIDTH.saturating_add(2),
            line_height: DEFAULT_LINE_HEIGHT,
            continuous_fit_to_width: true,
            ignore_document_css: false,
            dithered_kinds: [
                FileExtension::Cbz,
                FileExtension::Png,
                FileExtension::Jpg,
                FileExtension::Jpeg,
                FileExtension::Webp,
            ]
            .iter()
            .copied()
            .collect(),
            paragraph_breaker: ParagraphBreakerSettings::default(),
            refresh_rate: RefreshRateSettings::default(),
        }
    }
}

impl Default for ImportSettings {
    fn default() -> Self {
        ImportSettings {
            sync_metadata: true,
            metadata_kinds: ["epub", "pdf", "djvu"]
                .iter()
                .map(|k| k.to_string())
                .collect(),
            allowed_kinds: [
                FileExtension::Pdf,
                FileExtension::Djvu,
                FileExtension::Epub,
                FileExtension::Fb2,
                FileExtension::Txt,
                FileExtension::Xps,
                FileExtension::Oxps,
                FileExtension::Mobi,
                FileExtension::Cbz,
                FileExtension::Webp,
                FileExtension::Png,
                FileExtension::Jpg,
                FileExtension::Jpeg,
            ]
            .iter()
            .copied()
            .collect(),
        }
    }
}

impl ImportSettings {
    /// Returns `true` if `kind` is in the set of allowed file kinds.
    pub fn is_kind_allowed(&self, kind: FileExtension) -> bool {
        self.allowed_kinds.contains(&kind)
    }
}

impl Default for BatterySettings {
    fn default() -> Self {
        BatterySettings {
            warn: 10.0,
            power_off: 3.0,
        }
    }
}

impl Default for LoggingSettings {
    fn default() -> Self {
        LoggingSettings {
            enabled: true,
            level: "info".to_string(),
            max_files: 3,
            directory: PathBuf::from("logs"),
            otlp_endpoint: None,
            pyroscope_endpoint: None,
            enable_kern_log: false,
            enable_dbus_log: false,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            selected_library: 0,
            libraries: cfg_select! {
                feature = "emulator" => {
                    vec![LibrarySettings {
                        name: "Cadmus Source".to_string(),
                        path: PathBuf::from("."),
                        ..Default::default()
                    }]
                }
                _ => {
                    vec![
                        LibrarySettings {
                            name: "On Board".to_string(),
                            path: PathBuf::from(INTERNAL_CARD_ROOT),
                            hooks: vec![Hook {
                                path: PathBuf::from("Articles"),
                                program: PathBuf::from("bin/article_fetcher/article_fetcher"),
                                sort_method: Some(SortMethod::Added),
                                first_column: Some(FirstColumn::TitleAndAuthor),
                                second_column: Some(SecondColumn::Progress),
                            }],
                            ..Default::default()
                        },
                        LibrarySettings {
                            name: "Removable".to_string(),
                            path: PathBuf::from(EXTERNAL_CARD_ROOT),
                            ..Default::default()
                        },
                        LibrarySettings {
                            name: "Dropbox".to_string(),
                            path: PathBuf::from("/mnt/onboard/.kobo/dropbox"),
                            ..Default::default()
                        },
                        LibrarySettings {
                            name: "KePub".to_string(),
                            path: PathBuf::from("/mnt/onboard/.kobo/kepub"),
                            ..Default::default()
                        },
                    ]
                }
            },
            external_urls_queue: Some(PathBuf::from("bin/article_fetcher/urls.txt")),
            keyboard_layout: "English".to_string(),
            frontlight: true,
            wifi: false,
            inverted: false,
            sleep_cover: true,
            auto_share: false,
            rotation_lock: None,
            button_scheme: ButtonScheme::Natural,
            auto_suspend: 30.0,
            auto_power_off: 3.0,
            time_format: "%H:%M".to_string(),
            date_format: "%A, %B %-d, %Y".to_string(),
            intermissions: Intermissions {
                suspend: IntermissionDisplay::Logo,
                power_off: IntermissionDisplay::Logo,
                share: IntermissionDisplay::Logo,
            },
            home: HomeSettings::default(),
            reader: ReaderSettings::default(),
            import: ImportSettings::default(),
            dictionary: DictionarySettings::default(),
            sketch: SketchSettings::default(),
            calculator: CalculatorSettings::default(),
            battery: BatterySettings::default(),
            frontlight_levels: LightLevels::default(),
            frontlight_presets: Vec::new(),
            ota: OtaSettings::default(),
            logging: LoggingSettings::default(),
            settings_retention: 3,
            locale: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ota_settings_serializes_empty() {
        let settings = OtaSettings::default();
        let serialized = toml::to_string(&settings).expect("Failed to serialize");
        assert!(
            serialized.is_empty(),
            "OtaSettings should serialize to an empty string"
        );
    }

    #[test]
    fn test_intermissions_struct_serialization() {
        let intermissions = Intermissions {
            suspend: IntermissionDisplay::Blank,
            power_off: IntermissionDisplay::BlankInverted,
            share: IntermissionDisplay::Image(PathBuf::from("/custom/share.png")),
        };

        let serialized = toml::to_string(&intermissions).expect("Failed to serialize");

        assert!(
            serialized.contains("blank:"),
            "Should contain blank: for suspend"
        );
        assert!(
            serialized.contains("blank-inverted:"),
            "Should contain blank-inverted: for power-off"
        );
        assert!(
            serialized.contains("/custom/share.png"),
            "Should contain custom path for share"
        );
    }

    #[test]
    fn test_intermissions_struct_deserialization() {
        let toml_str = r#"
suspend = "blank:"
power-off = "blank-inverted:"
share = "/path/to/custom.png"
"#;

        let intermissions: Intermissions = toml::from_str(toml_str).expect("Failed to deserialize");

        assert!(
            matches!(intermissions.suspend, IntermissionDisplay::Blank),
            "suspend should deserialize to Blank"
        );
        assert!(
            matches!(intermissions.power_off, IntermissionDisplay::BlankInverted),
            "power_off should deserialize to BlankInverted"
        );
        assert!(
            matches!(
                intermissions.share,
                IntermissionDisplay::Image(ref path) if path == &PathBuf::from("/path/to/custom.png")
            ),
            "share should deserialize to Image with correct path"
        );
    }

    #[test]
    fn test_intermissions_struct_round_trip() {
        let original = Intermissions {
            suspend: IntermissionDisplay::Blank,
            power_off: IntermissionDisplay::BlankInverted,
            share: IntermissionDisplay::Image(PathBuf::from("/some/custom/image.jpg")),
        };

        let serialized = toml::to_string(&original).expect("Failed to serialize");
        let deserialized: Intermissions =
            toml::from_str(&serialized).expect("Failed to deserialize");

        assert_eq!(
            original.suspend, deserialized.suspend,
            "suspend should survive round trip"
        );
        assert_eq!(
            original.power_off, deserialized.power_off,
            "power_off should survive round trip"
        );
        assert_eq!(
            original.share, deserialized.share,
            "share should survive round trip"
        );
    }

    #[test]
    fn test_intermissions_reject_unsupported_calendar_selection() {
        let mut intermissions = Intermissions {
            suspend: IntermissionDisplay::Logo,
            power_off: IntermissionDisplay::Logo,
            share: IntermissionDisplay::Logo,
        };

        assert!(!intermissions.set_display(IntermKind::PowerOff, IntermissionDisplay::Calendar));
        assert!(!intermissions.set_display(IntermKind::Share, IntermissionDisplay::Calendar));
        assert!(intermissions.set_display(IntermKind::Suspend, IntermissionDisplay::Calendar));

        assert_eq!(
            intermissions[IntermKind::PowerOff],
            IntermissionDisplay::Logo
        );
        assert_eq!(intermissions[IntermKind::Share], IntermissionDisplay::Logo);
        assert_eq!(
            intermissions[IntermKind::Suspend],
            IntermissionDisplay::Calendar
        );
    }

    #[test]
    fn test_intermissions_accept_blank_selection_for_all_kinds() {
        let mut intermissions = Intermissions {
            suspend: IntermissionDisplay::Logo,
            power_off: IntermissionDisplay::Logo,
            share: IntermissionDisplay::Logo,
        };

        assert!(intermissions.set_display(IntermKind::Suspend, IntermissionDisplay::Blank));
        assert!(
            intermissions.set_display(IntermKind::PowerOff, IntermissionDisplay::BlankInverted)
        );
        assert!(intermissions.set_display(IntermKind::Share, IntermissionDisplay::Blank));

        assert_eq!(
            intermissions[IntermKind::Suspend],
            IntermissionDisplay::Blank
        );
        assert_eq!(
            intermissions[IntermKind::PowerOff],
            IntermissionDisplay::BlankInverted
        );
        assert_eq!(intermissions[IntermKind::Share], IntermissionDisplay::Blank);
    }

    #[test]
    fn test_intermissions_sanitize_replaces_unsupported_calendar() {
        let mut intermissions = Intermissions {
            suspend: IntermissionDisplay::Calendar,
            power_off: IntermissionDisplay::Calendar,
            share: IntermissionDisplay::Calendar,
        };

        assert!(intermissions.sanitize());

        assert_eq!(
            intermissions[IntermKind::Suspend],
            IntermissionDisplay::Calendar
        );
        assert_eq!(
            intermissions[IntermKind::PowerOff],
            IntermissionDisplay::Logo
        );
        assert_eq!(intermissions[IntermKind::Share], IntermissionDisplay::Logo);
    }

    #[test]
    fn test_allowed_kinds_deserializes_known_extensions() {
        let toml_str = r#"
sync-metadata = true
metadata-kinds = ["epub"]
allowed-kinds = ["epub", "pdf", "cbz"]
"#;
        let settings: ImportSettings = toml::from_str(toml_str).expect("Failed to deserialize");

        assert!(settings.allowed_kinds.contains(&FileExtension::Epub));
        assert!(settings.allowed_kinds.contains(&FileExtension::Pdf));
        assert!(settings.allowed_kinds.contains(&FileExtension::Cbz));
        assert_eq!(settings.allowed_kinds.len(), 3);
    }

    #[test]
    fn test_allowed_kinds_silently_drops_unknown_extensions() {
        let toml_str = r#"
sync-metadata = true
metadata-kinds = []
allowed-kinds = ["epub", "unknown-format", "another-unknown"]
"#;
        let settings: ImportSettings = toml::from_str(toml_str).expect("Failed to deserialize");

        assert!(settings.allowed_kinds.contains(&FileExtension::Epub));
        assert_eq!(settings.allowed_kinds.len(), 1);
    }

    #[test]
    fn test_dithered_kinds_deserializes_known_extensions() {
        let toml_str = r#"
finished = "close"
dithered-kinds = ["cbz", "png", "jpeg"]
"#;
        let settings: ReaderSettings = toml::from_str(toml_str).expect("Failed to deserialize");

        assert!(settings.dithered_kinds.contains(&FileExtension::Cbz));
        assert!(settings.dithered_kinds.contains(&FileExtension::Png));
        assert!(settings.dithered_kinds.contains(&FileExtension::Jpeg));
        assert_eq!(settings.dithered_kinds.len(), 3);
    }

    #[test]
    fn test_dithered_kinds_silently_drops_unknown_extensions() {
        let toml_str = r#"
finished = "close"
dithered-kinds = ["cbz", "unknown-format"]
"#;
        let settings: ReaderSettings = toml::from_str(toml_str).expect("Failed to deserialize");

        assert!(settings.dithered_kinds.contains(&FileExtension::Cbz));
        assert_eq!(settings.dithered_kinds.len(), 1);
    }

    #[test]
    fn test_file_extension_round_trip_via_from_str() {
        for ext in FileExtension::all() {
            let parsed = ext.as_str().parse::<FileExtension>().ok();
            assert_eq!(parsed, Some(*ext), "round trip failed for {:?}", ext);
        }
    }

    #[test]
    fn test_htm_extension_parses_as_html() {
        let parsed = "htm".parse::<FileExtension>();
        assert_eq!(parsed, Ok(FileExtension::Html));
    }

    #[test]
    fn test_html_extension_still_parses() {
        let parsed = "html".parse::<FileExtension>();
        assert_eq!(parsed, Ok(FileExtension::Html));
    }
}
