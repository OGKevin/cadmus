mod preset;
pub mod versioned;

use crate::color::{Color, BLACK};
use crate::device::CURRENT_DEVICE;
use crate::frontlight::LightLevels;
use crate::metadata::{SortMethod, TextAlign};
use crate::unit::mm_to_px;
use fxhash::FxHashSet;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fmt::{self, Debug, Display};
use std::ops::{Index, IndexMut};
use std::path::PathBuf;

pub use self::preset::{guess_frontlight, LightPreset};

pub const SETTINGS_PATH: &str = "Settings.toml";
pub const DEFAULT_FONT_PATH: &str = "/mnt/onboard/fonts";
pub const INTERNAL_CARD_ROOT: &str = "/mnt/onboard";
pub const EXTERNAL_CARD_ROOT: &str = "/mnt/sd";
const LOGO_SPECIAL_PATH: &str = "logo:";
const COVER_SPECIAL_PATH: &str = "cover:";

/// How to display intermission screens.
/// Logo and Cover are special values that map to built-in images.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum IntermissionDisplay {
    /// Display the built-in logo image.
    Logo,
    /// Display the cover of the currently reading book.
    Cover,
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
            _ => IntermissionDisplay::Image(PathBuf::from(s)),
        })
    }
}

impl fmt::Display for IntermissionDisplay {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            IntermissionDisplay::Logo => write!(f, "Logo"),
            IntermissionDisplay::Cover => write!(f, "Cover"),
            IntermissionDisplay::Image(_) => write!(f, "Custom"),
        }
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
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryMode {
    Database,
    Filesystem,
}

impl Display for LibraryMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LibraryMode::Database => write!(f, "Database"),
            LibraryMode::Filesystem => write!(f, "Filesystem"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LibrarySettings {
    pub name: String,
    pub path: PathBuf,
    pub mode: LibraryMode,
    pub sort_method: SortMethod,
    pub first_column: FirstColumn,
    pub second_column: SecondColumn,
    pub thumbnail_previews: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<Hook>,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        LibrarySettings {
            name: "Unnamed".to_string(),
            path: env::current_dir()
                .ok()
                .unwrap_or_else(|| PathBuf::from("/")),
            mode: LibraryMode::Database,
            sort_method: SortMethod::Opened,
            first_column: FirstColumn::TitleAndAuthor,
            second_column: SecondColumn::Progress,
            thumbnail_previews: true,
            hooks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ImportSettings {
    pub unshare_trigger: bool,
    pub startup_trigger: bool,
    pub sync_metadata: bool,
    pub metadata_kinds: FxHashSet<String>,
    pub allowed_kinds: FxHashSet<String>,
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
    pub dithered_kinds: FxHashSet<String>,
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
}

/// Configuration for Over-the-Air (OTA) update feature.
///
/// Stores the GitHub personal access token required for downloading
/// build artifacts from pull requests.
///
/// # Security
///
/// The GitHub token is stored using `SecretString` from the `secrecy` crate,
/// which prevents accidental exposure in logs or debug output. The token is
/// automatically wrapped when loaded from the configuration file and unwrapped
/// only when needed for API authentication.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct OtaSettings {
    /// GitHub personal access token with workflow artifact read permissions.
    /// Required for authenticated API access to download build artifacts.
    ///
    /// When serialized, the token is stored as plain text in the configuration
    /// file. However, once loaded into memory, it is wrapped in `SecretString`
    /// to prevent accidental exposure.
    ///
    /// For development, you can set the `GH_TOKEN` environment variable to have it automatically
    /// loaded into the default settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_token: Option<SecretString>,
}

impl Serialize for OtaSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use secrecy::ExposeSecret;
        use serde::ser::SerializeStruct;

        let field_count = if self.github_token.is_some() { 1 } else { 0 };
        let mut state = serializer.serialize_struct("OtaSettings", field_count)?;
        if let Some(token) = &self.github_token {
            state.serialize_field("github-token", token.expose_secret())?;
        }
        state.end()
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinishedAction {
    Notify,
    Close,
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
            dithered_kinds: ["cbz", "png", "jpg", "jpeg"]
                .iter()
                .map(|k| k.to_string())
                .collect(),
            paragraph_breaker: ParagraphBreakerSettings::default(),
            refresh_rate: RefreshRateSettings::default(),
        }
    }
}

impl Default for ImportSettings {
    fn default() -> Self {
        ImportSettings {
            unshare_trigger: true,
            startup_trigger: true,
            sync_metadata: true,
            metadata_kinds: ["epub", "pdf", "djvu"]
                .iter()
                .map(|k| k.to_string())
                .collect(),
            allowed_kinds: [
                "pdf", "djvu", "epub", "fb2", "txt", "xps", "oxps", "mobi", "cbz",
            ]
            .iter()
            .map(|k| k.to_string())
            .collect(),
        }
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
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            selected_library: 0,
            #[cfg(feature = "emulator")]
            libraries: vec![LibrarySettings {
                name: "Cadmus Source".to_string(),
                path: PathBuf::from("."),
                ..Default::default()
            }],
            #[cfg(not(feature = "emulator"))]
            libraries: vec![
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
            ],
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
        }
    }
}

impl Default for OtaSettings {
    /// Creates a default `OtaSettings` instance, attempting to read the GitHub token
    /// from the `GH_TOKEN` environment variable. If the variable is set, its value is wrapped
    /// in a `SecretString` and used as the default token.
    fn default() -> Self {
        env::var("GH_TOKEN")
            .ok()
            .map(|token| OtaSettings {
                github_token: Some(SecretString::from(token)),
            })
            .unwrap_or(OtaSettings { github_token: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn test_ota_settings_secret_serialization() {
        let test_token = "ghp_test1234567890abcdefghijklmnopqrstuvwxyz";

        let mut settings = OtaSettings::default();
        settings.github_token = Some(SecretString::from(test_token.to_string()));

        let serialized = toml::to_string(&settings).expect("Failed to serialize");

        assert!(
            serialized.contains("github-token"),
            "Serialized output should contain github-token field"
        );
        assert!(
            serialized.contains(test_token),
            "Serialized output should contain the actual token value for file storage"
        );
        assert!(
            !serialized.contains("REDACTED"),
            "Serialized output should not contain REDACTED placeholder"
        );
    }

    #[test]
    fn test_ota_settings_secret_deserialization() {
        let test_token = "ghp_test1234567890abcdefghijklmnopqrstuvwxyz";
        let toml_str = format!(r#"github-token = "{}""#, test_token);

        let settings: OtaSettings = toml::from_str(&toml_str).expect("Failed to deserialize");

        assert!(
            settings.github_token.is_some(),
            "Token should be present after deserialization"
        );

        let token = settings.github_token.as_ref().unwrap();
        assert_eq!(
            token.expose_secret(),
            test_token,
            "Deserialized token should match original"
        );
    }

    #[test]
    fn test_ota_settings_secret_round_trip() {
        let test_token = "ghp_roundtrip_test_token_1234567890";

        let mut original = OtaSettings::default();
        original.github_token = Some(SecretString::from(test_token.to_string()));

        let serialized = toml::to_string(&original).expect("Failed to serialize");
        let deserialized: OtaSettings = toml::from_str(&serialized).expect("Failed to deserialize");

        assert!(
            deserialized.github_token.is_some(),
            "Token should survive round trip"
        );

        assert_eq!(
            deserialized.github_token.as_ref().unwrap().expose_secret(),
            original.github_token.as_ref().unwrap().expose_secret(),
            "Token value should be identical after round trip"
        );
    }

    #[test]
    fn test_ota_settings_secret_not_in_debug() {
        let test_token = "ghp_secret_should_not_appear_in_debug";

        let mut settings = OtaSettings::default();
        settings.github_token = Some(SecretString::from(test_token.to_string()));

        let debug_output = format!("{:?}", settings);

        assert!(
            !debug_output.contains(test_token),
            "Debug output should NOT contain the actual token value. Got: {}",
            debug_output
        );
    }

    #[test]
    fn test_ota_settings_none_token() {
        let settings = OtaSettings::default();

        let serialized = toml::to_string(&settings).expect("Failed to serialize");

        assert!(
            !serialized.contains("github-token"),
            "Serialized output should not contain github-token field when None"
        );

        let deserialized: OtaSettings = toml::from_str("").expect("Failed to deserialize empty");

        assert!(
            deserialized.github_token.is_none(),
            "Deserialized settings should have None token"
        );
    }

    #[test]
    fn test_intermissions_struct_serialization() {
        let intermissions = Intermissions {
            suspend: IntermissionDisplay::Logo,
            power_off: IntermissionDisplay::Cover,
            share: IntermissionDisplay::Image(PathBuf::from("/custom/share.png")),
        };

        let serialized = toml::to_string(&intermissions).expect("Failed to serialize");

        assert!(
            serialized.contains("logo:"),
            "Should contain logo: for suspend"
        );
        assert!(
            serialized.contains("cover:"),
            "Should contain cover: for power-off"
        );
        assert!(
            serialized.contains("/custom/share.png"),
            "Should contain custom path for share"
        );
    }

    #[test]
    fn test_intermissions_struct_deserialization() {
        let toml_str = r#"
suspend = "logo:"
power-off = "cover:"
share = "/path/to/custom.png"
"#;

        let intermissions: Intermissions = toml::from_str(toml_str).expect("Failed to deserialize");

        assert!(
            matches!(intermissions.suspend, IntermissionDisplay::Logo),
            "suspend should deserialize to Logo"
        );
        assert!(
            matches!(intermissions.power_off, IntermissionDisplay::Cover),
            "power_off should deserialize to Cover"
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
            suspend: IntermissionDisplay::Logo,
            power_off: IntermissionDisplay::Cover,
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
    fn test_ota_default_from_env() {
        let test_token = "ghp_env_default_test_token_1234567890";
        env::set_var("GH_TOKEN", test_token);

        let settings = OtaSettings::default();

        assert!(
            settings.github_token.is_some(),
            "Default OtaSettings should read GH_TOKEN from environment"
        );

        let token = settings.github_token.as_ref().unwrap();
        assert_eq!(
            token.expose_secret(),
            test_token,
            "Token from environment should match expected value"
        );

        env::remove_var("GH_TOKEN");
    }
}
