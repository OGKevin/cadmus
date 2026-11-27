//! Line filter for the log viewer.
//!
//! Run logs are tracing JSON lines. A line is parsed only when byte searches
//! cannot rule it out, so a selective filter reads a large log at close to disk
//! speed. Lines that are not JSON have no level or target and match only text.

use cadmus_api_proto::cadmus::api::public::v1::{LogFilter, LogLevel};
use memchr::{memchr2, memmem};
use serde::Deserialize;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::borrow::Cow;
use std::fmt;

const LEVEL_KEY: &[u8] = b"\"level\":\"";
const TARGET_KEY: &[u8] = b"\"target\":\"";
const MAX_VALUE_DEPTH: u8 = 128;

/// Bit of `level` in a level mask.
fn level_bit(level: LogLevel) -> u8 {
    match level {
        LogLevel::LOG_LEVEL_TRACE => 1 << 1,
        LogLevel::LOG_LEVEL_DEBUG => 1 << 2,
        LogLevel::LOG_LEVEL_INFO => 1 << 3,
        LogLevel::LOG_LEVEL_WARN => 1 << 4,
        LogLevel::LOG_LEVEL_ERROR => 1 << 5,
        LogLevel::LOG_LEVEL_UNSPECIFIED => 0,
    }
}

/// Level a tracing JSON line names, as written (`INFO`).
pub(super) fn level_from_name(name: &[u8]) -> Option<LogLevel> {
    match name {
        b"TRACE" => Some(LogLevel::LOG_LEVEL_TRACE),
        b"DEBUG" => Some(LogLevel::LOG_LEVEL_DEBUG),
        b"INFO" => Some(LogLevel::LOG_LEVEL_INFO),
        b"WARN" => Some(LogLevel::LOG_LEVEL_WARN),
        b"ERROR" => Some(LogLevel::LOG_LEVEL_ERROR),
        _ => None,
    }
}

/// Lowercase `text`, folding Greek final sigma to sigma so both spellings meet.
fn fold(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains('ς') {
        lower.replace('ς', "σ")
    } else {
        lower
    }
}

/// Text to find, lowercased once.
struct Text {
    lower: String,
    /// ASCII without characters JSON escapes, so the raw line bytes contain it
    /// whenever a string value does.
    byte_searchable: bool,
}

impl Text {
    fn new(text: &str) -> Self {
        let lower = fold(text);
        let byte_searchable = lower
            .bytes()
            .all(|byte| byte.is_ascii() && byte >= 0x20 && byte != b'"' && byte != b'\\');
        Self {
            lower,
            byte_searchable,
        }
    }

    fn in_bytes(&self, haystack: &[u8]) -> bool {
        self.lower.is_ascii() && find_ascii_ci(haystack, self.lower.as_bytes())
    }

    fn in_str(&self, haystack: &str) -> bool {
        if self.lower.is_ascii() {
            find_ascii_ci(haystack.as_bytes(), self.lower.as_bytes())
        } else if haystack.is_ascii() {
            false
        } else {
            fold(haystack).contains(&self.lower)
        }
    }
}

/// Case-insensitive search for an ASCII-lowercase `needle`.
fn find_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    let upper = first.to_ascii_uppercase();
    let mut from = 0;
    while let Some(index) = memchr2(first, upper, &haystack[from..]) {
        let after = from + index + 1;
        if haystack
            .get(after..after + rest.len())
            .is_some_and(|tail| tail.eq_ignore_ascii_case(rest))
        {
            return true;
        }
        from = after;
    }
    false
}

/// The keys of a tracing JSON line a filter looks at.
#[derive(Deserialize)]
struct Record<'a> {
    #[serde(borrow, default)]
    target: Cow<'a, str>,
    #[serde(borrow, default)]
    fields: Option<&'a RawValue>,
}

/// The line's target alone, so a target-only filter never parses its fields.
#[derive(Deserialize)]
struct TargetOnly<'a> {
    #[serde(borrow, default)]
    target: Cow<'a, str>,
}

/// A JSON string deserialized from a raw value.
#[derive(Deserialize)]
struct JsonString<'a>(#[serde(borrow)] Cow<'a, str>);

/// A parsed [`LogFilter`].
pub(super) struct LineFilter {
    levels: u8,
    targets: Vec<String>,
    target_searches: Option<Vec<Vec<u8>>>,
    text: Option<Text>,
}

impl LineFilter {
    pub(super) fn new(filter: Option<&LogFilter>) -> Self {
        let Some(filter) = filter else {
            return Self {
                levels: 0,
                targets: Vec::new(),
                target_searches: None,
                text: None,
            };
        };
        let levels = filter
            .levels
            .iter()
            .filter_map(|level| level.as_known())
            .fold(0, |mask, level| mask | level_bit(level));
        let targets: Vec<String> = filter
            .targets
            .iter()
            .filter(|target| !target.is_empty())
            .cloned()
            .collect();
        let target_searches = (!targets.is_empty()
            && targets.iter().all(|target| {
                target
                    .bytes()
                    .all(|byte| byte.is_ascii() && byte >= 0x20 && byte != b'"' && byte != b'\\')
            }))
        .then(|| {
            targets
                .iter()
                .map(|target| [TARGET_KEY, target.as_bytes()].concat())
                .collect()
        });
        let text = Some(filter.text.as_str())
            .filter(|text| !text.is_empty())
            .map(Text::new);
        Self {
            levels,
            targets,
            target_searches,
            text,
        }
    }

    /// Whether `line`, without its newline, passes every set condition.
    pub(super) fn matches(&self, line: &[u8]) -> bool {
        if self.levels == 0 && self.targets.is_empty() && self.text.is_none() {
            return true;
        }
        if line.first() != Some(&b'{') {
            return self.matches_plain(line);
        }
        if self.levels != 0 && !self.level_matches(line) {
            return false;
        }
        if self.targets.is_empty() && self.text.is_none() {
            return true;
        }
        if let Some(searches) = &self.target_searches
            && !searches
                .iter()
                .any(|search| memmem::find(line, search).is_some())
        {
            return false;
        }
        if let Some(text) = &self.text
            && text.byte_searchable
            && !text.in_bytes(line)
        {
            return false;
        }
        match self.text {
            None => serde_json::from_slice::<TargetOnly>(line)
                .is_ok_and(|record| self.target_matches(&record.target)),
            Some(_) => match serde_json::from_slice::<Record>(line) {
                Ok(record) => self.record_matches(&record),
                Err(_) => self.matches_plain(line),
            },
        }
    }

    fn matches_plain(&self, line: &[u8]) -> bool {
        if self.levels != 0 || !self.targets.is_empty() {
            return false;
        }
        match &self.text {
            Some(text) if text.lower.is_ascii() => text.in_bytes(line),
            Some(_) if line.is_ascii() => false,
            Some(text) => text.in_str(&String::from_utf8_lossy(line)),
            None => true,
        }
    }

    /// The first `"level"` key is the line's own: tracing writes it before
    /// `fields`, and a quote inside any string value is escaped.
    fn level_matches(&self, line: &[u8]) -> bool {
        let Some(at) = memmem::find(line, LEVEL_KEY) else {
            return false;
        };
        let value = &line[at + LEVEL_KEY.len()..];
        let end = memchr::memchr(b'"', value).unwrap_or(value.len());
        level_from_name(&value[..end]).is_some_and(|level| self.levels & level_bit(level) != 0)
    }

    /// Whether `target` is one of the selected Log targets or below one.
    fn target_matches(&self, target: &str) -> bool {
        self.targets.is_empty()
            || self.targets.iter().any(|wanted| {
                target == wanted
                    || target
                        .strip_prefix(wanted.as_str())
                        .is_some_and(|rest| rest.starts_with("::"))
            })
    }

    fn record_matches(&self, record: &Record<'_>) -> bool {
        if !self.target_matches(&record.target) {
            return false;
        }
        self.text.as_ref().is_none_or(|text| {
            record
                .fields
                .is_some_and(|fields| raw_contains(fields, text))
        })
    }
}

/// Whether a value in `raw` contains `text`, comparing numbers by their raw text.
fn raw_contains(raw: &RawValue, text: &Text) -> bool {
    raw_contains_at(raw, text, 1)
}

fn string_contains(json: &str, text: &Text) -> bool {
    serde_json::from_str::<JsonString<'_>>(json).is_ok_and(|value| text.in_str(&value.0))
}

fn array_contains(json: &str, text: &Text, depth: u8) -> bool {
    if depth > MAX_VALUE_DEPTH {
        return false;
    }
    serde_json::from_str::<Vec<&RawValue>>(json).map_or_else(
        |_| text.in_str(json),
        |items| {
            items
                .iter()
                .any(|item| raw_contains_at(item, text, depth + 1))
        },
    )
}

fn object_contains(json: &str, text: &Text, depth: u8) -> bool {
    if depth > MAX_VALUE_DEPTH {
        return false;
    }
    let mut deserializer = serde_json::Deserializer::from_str(json);
    ObjectSearch { text, depth }
        .deserialize(&mut deserializer)
        .unwrap_or_else(|_| text.in_str(json))
}

fn raw_contains_at(raw: &RawValue, text: &Text, depth: u8) -> bool {
    if depth > MAX_VALUE_DEPTH {
        return false;
    }
    let json = raw.get();
    match json.as_bytes().first() {
        Some(b'"') => string_contains(json, text),
        Some(b't') => text.in_str("true"),
        Some(b'f') => text.in_str("false"),
        Some(b'n') => false,
        Some(b'[') => array_contains(json, text, depth + 1),
        Some(b'{') => object_contains(json, text, depth + 1),
        Some(_) => text.in_str(json),
        None => false,
    }
}

/// Searches an object's values without allocating its keys.
struct ObjectSearch<'text> {
    text: &'text Text,
    depth: u8,
}

impl<'de> DeserializeSeed<'de> for ObjectSearch<'_> {
    type Value = bool;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ObjectSearch<'_> {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_key::<IgnoredAny>()?.is_some() {
            let value: &'de RawValue = map.next_value()?;
            if raw_contains_at(value, self.text, self.depth) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO_WIFI: &[u8] = br#"{"timestamp":"2026-11-25T10:00:00Z","level":"INFO","fields":{"message":"Wi-Fi up","ssid":"Home","attempt":3},"target":"cadmus_core::device::wifi"}"#;
    const WARN_LIBRARY: &[u8] = br#"{"timestamp":"2026-11-25T10:00:01Z","level":"WARN","fields":{"message":"slow import","path":"/mnt/onboard/Caf\u00e9"},"target":"cadmus_core::library"}"#;
    const ERROR_QUOTED: &[u8] = br#"{"timestamp":"2026-11-25T10:00:02Z","level":"ERROR","fields":{"message":"said \"hello\" to \\wifi"},"target":"cadmus_core::view"}"#;
    const INFO_DEVICES: &[u8] = br#"{"timestamp":"2026-11-25T10:00:03Z","level":"INFO","fields":{"message":"listed"},"target":"cadmus_core::devices"}"#;
    fn filter(levels: &[LogLevel], targets: &[&str], text: &str) -> LineFilter {
        LineFilter::new(Some(&LogFilter {
            levels: levels.iter().map(|level| (*level).into()).collect(),
            targets: targets.iter().map(|target| (*target).to_owned()).collect(),
            text: text.to_owned(),
            ..Default::default()
        }))
    }

    #[test]
    fn an_empty_filter_keeps_every_line_including_plain_text() {
        let keep = filter(&[], &[], "");
        assert!(keep.matches(INFO_WIFI));
        assert!(keep.matches(b"panicked at src/main.rs"));
        assert!(keep.matches(b""));
    }

    #[test]
    fn levels_select_by_the_lines_own_level() {
        let warn_or_error = filter(
            &[LogLevel::LOG_LEVEL_WARN, LogLevel::LOG_LEVEL_ERROR],
            &[],
            "",
        );
        assert!(!warn_or_error.matches(INFO_WIFI));
        assert!(warn_or_error.matches(WARN_LIBRARY));
        assert!(warn_or_error.matches(ERROR_QUOTED));
    }

    #[test]
    fn a_field_named_level_does_not_change_the_lines_level() {
        let line = br#"{"timestamp":"t","level":"INFO","fields":{"level":"ERROR"},"target":"a"}"#;
        assert!(!filter(&[LogLevel::LOG_LEVEL_ERROR], &[], "").matches(line));
        assert!(filter(&[LogLevel::LOG_LEVEL_INFO], &[], "").matches(line));
    }

    #[test]
    fn a_target_keeps_itself_and_its_sub_targets_but_no_siblings() {
        let device = filter(&[], &["cadmus_core::device"], "");
        assert!(device.matches(INFO_WIFI));
        assert!(!device.matches(INFO_DEVICES));
        let exact = filter(&[], &["cadmus_core::device::wifi"], "");
        assert!(exact.matches(INFO_WIFI));
        let library = filter(&[], &["cadmus_core::library"], "");
        assert!(library.matches(WARN_LIBRARY));
        assert!(!library.matches(INFO_WIFI));
    }

    #[test]
    fn a_target_with_json_escapes_matches_its_decoded_target() {
        let line = br#"{"timestamp":"t","level":"INFO","fields":{"message":"x"},"target":"cadmus_core::Caf\u00e9"}"#;
        assert!(filter(&[], &["cadmus_core::Café"], "").matches(line));
        assert!(!filter(&[], &["cadmus_core::Cafe"], "").matches(line));
    }

    #[test]
    fn an_unspecified_level_and_an_empty_target_are_ignored() {
        let levels_only = filter(&[LogLevel::LOG_LEVEL_UNSPECIFIED], &[], "");
        assert!(levels_only.matches(INFO_WIFI));
        assert!(levels_only.matches(b"plain"));
        let blank_target = filter(&[], &[""], "");
        assert!(blank_target.matches(INFO_WIFI));
        let blank_with_other = filter(&[], &["", "cadmus_core::library"], "");
        assert!(blank_with_other.matches(WARN_LIBRARY));
        assert!(!blank_with_other.matches(INFO_WIFI));
    }

    #[test]
    fn a_target_named_in_a_field_is_not_the_lines_target() {
        let line = br#"{"timestamp":"t","level":"INFO","fields":{"message":"x","target":"cadmus_core::library"},"target":"cadmus_core::view"}"#;
        assert!(!filter(&[], &["cadmus_core::library"], "").matches(line));
    }

    #[test]
    fn text_matches_the_message_and_any_field_value_in_any_case() {
        assert!(filter(&[], &[], "WI-FI").matches(INFO_WIFI));
        assert!(filter(&[], &[], "home").matches(INFO_WIFI));
        assert!(filter(&[], &[], "3").matches(INFO_WIFI));
        assert!(!filter(&[], &[], "ssid").matches(INFO_WIFI));
        assert!(!filter(&[], &[], "cadmus_core").matches(INFO_WIFI));
    }

    #[test]
    fn text_folds_unicode_case() {
        assert!(filter(&[], &[], "CAFÉ").matches(WARN_LIBRARY));
        assert!(filter(&[], &[], "café").matches(WARN_LIBRARY));
    }

    #[test]
    fn greek_final_sigma_matches_sigma_in_either_direction() {
        let upper = br#"{"timestamp":"t","level":"INFO","fields":{"message":"\u039b\u039f\u0393\u039f\u03a3"},"target":"a"}"#;
        assert!(filter(&[], &[], "λογος").matches(upper));
        let lower = br#"{"timestamp":"t","level":"INFO","fields":{"message":"\u03bb\u03bf\u03b3\u03bf\u03c2"},"target":"a"}"#;
        assert!(filter(&[], &[], "λογοσ").matches(lower));
    }

    #[test]
    fn a_non_ascii_needle_does_not_match_an_ascii_value() {
        assert!(!filter(&[], &[], "café").matches(INFO_WIFI));
        assert!(!filter(&[], &[], "café").matches(b"plain ascii"));
    }

    #[test]
    fn numbers_match_the_raw_json_text_rather_than_a_reprint() {
        let line = br#"{"timestamp":"t","level":"INFO","fields":{"ratio":1.50,"scale":1e2,"big":12345678901234567890},"target":"a"}"#;
        assert!(filter(&[], &[], "1.50").matches(line));
        assert!(filter(&[], &[], "1e2").matches(line));
        assert!(filter(&[], &[], "12345678901234567890").matches(line));
    }

    #[test]
    fn text_searches_nested_values_and_leaves() {
        let line = br#"{"timestamp":"t","level":"INFO","fields":{"items":[{"message":"deep"},{"ok":true}],"none":null},"target":"a"}"#;
        assert!(filter(&[], &[], "deep").matches(line));
        assert!(filter(&[], &[], "TRUE").matches(line));
        assert!(!filter(&[], &[], "null").matches(line));
    }

    #[test]
    fn text_with_json_escapes_matches_the_decoded_value() {
        assert!(filter(&[], &[], "said \"hello\"").matches(ERROR_QUOTED));
        assert!(filter(&[], &[], "\\wifi").matches(ERROR_QUOTED));
    }

    #[test]
    fn plain_lines_match_only_text() {
        let plain = b"cadmus.sh: CRASH_COUNT -ge 3, panicked at main";
        assert!(filter(&[], &[], "Panicked").matches(plain));
        assert!(!filter(&[], &[], "absent").matches(plain));
        assert!(!filter(&[LogLevel::LOG_LEVEL_INFO], &[], "").matches(plain));
        assert!(!filter(&[], &["cadmus"], "").matches(plain));
    }

    #[test]
    fn a_malformed_json_line_is_plain_text() {
        let broken = br#"{"timestamp":"t","level":"INFO","fields":{"message":"wifi"#;
        assert!(filter(&[], &[], "wifi").matches(broken));
        assert!(!filter(&[], &["cadmus"], "wifi").matches(broken));
    }

    #[test]
    fn all_set_conditions_must_hold() {
        let combined = filter(
            &[LogLevel::LOG_LEVEL_INFO],
            &["cadmus_core::device"],
            "home",
        );
        assert!(combined.matches(INFO_WIFI));
        let wrong_level = filter(
            &[LogLevel::LOG_LEVEL_WARN],
            &["cadmus_core::device"],
            "home",
        );
        assert!(!wrong_level.matches(INFO_WIFI));
    }

    #[test]
    fn two_targets_keep_lines_from_either() {
        let either = filter(&[], &["cadmus_core::library", "cadmus_core::device"], "");
        assert!(either.matches(WARN_LIBRARY));
        assert!(either.matches(INFO_WIFI));
        assert!(!either.matches(ERROR_QUOTED));
    }

    #[test]
    fn an_empty_target_list_keeps_every_target() {
        let every = filter(&[], &[], "");
        assert!(every.matches(INFO_WIFI));
        assert!(every.matches(WARN_LIBRARY));
        assert!(every.matches(ERROR_QUOTED));
    }
}
