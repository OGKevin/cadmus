//! Scans a log once for the facts the filter choices need.
//!
//! The scan keeps the counts it has seen so far and can pick up from the last
//! complete line, which is how the Current run's summary resumes after the log
//! grew and how a snapshot is streamed while the scan runs.

use super::filter::level_from_name;
use buffa_types::google::protobuf::Timestamp;
use cadmus_api_proto::cadmus::api::public::v1::{LevelCount, LogLevel, LogSummary, TargetCount};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const BLOCK: usize = 1024 * 1024;

/// Bytes scanned between two streamed snapshots.
const SNAPSHOT_EVERY: u64 = 8 * 1024 * 1024;

#[derive(Deserialize)]
struct Head<'a> {
    #[serde(borrow, default)]
    timestamp: Cow<'a, str>,
    #[serde(borrow, default)]
    level: Cow<'a, str>,
    #[serde(borrow, default)]
    target: Cow<'a, str>,
}

/// What the scan has seen up to `offset`, the end of the last complete line.
#[derive(Clone, Default, Debug, PartialEq)]
pub(super) struct Tally {
    targets: FxHashMap<String, u64>,
    levels: [u64; 6],
    lines: u64,
    first_timestamp: String,
    last_timestamp: String,
    offset: u64,
}

impl Tally {
    fn record(&mut self, line: &[u8]) {
        self.lines += 1;
        if line.first() != Some(&b'{') {
            return;
        }
        let Ok(head) = serde_json::from_slice::<Head>(line) else {
            return;
        };
        if let Some(level) = level_from_name(head.level.as_bytes()) {
            self.levels[level as usize] += 1;
        }
        if !head.target.is_empty() {
            match self.targets.get_mut(head.target.as_ref()) {
                Some(count) => *count += 1,
                None => {
                    self.targets.insert(head.target.into_owned(), 1);
                }
            }
        }
        if !head.timestamp.is_empty() {
            if self.first_timestamp.is_empty() {
                self.first_timestamp.push_str(&head.timestamp);
            }
            self.last_timestamp.clear();
            self.last_timestamp.push_str(&head.timestamp);
        }
    }

    /// End of the last complete line counted.
    pub(super) fn offset(&self) -> u64 {
        self.offset
    }

    /// The snapshot to send. `size` is the file size the scan is working to.
    pub(super) fn to_proto(&self, size: u64, done: bool) -> LogSummary {
        let mut targets: Vec<TargetCount> = self
            .targets
            .iter()
            .map(|(target, lines)| TargetCount {
                target: target.clone(),
                lines: *lines,
                ..Default::default()
            })
            .collect();
        targets.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.target.cmp(&b.target)));
        let levels = [
            LogLevel::LOG_LEVEL_TRACE,
            LogLevel::LOG_LEVEL_DEBUG,
            LogLevel::LOG_LEVEL_INFO,
            LogLevel::LOG_LEVEL_WARN,
            LogLevel::LOG_LEVEL_ERROR,
        ]
        .into_iter()
        .filter(|level| self.levels[*level as usize] > 0)
        .map(|level| LevelCount {
            level: level.into(),
            lines: self.levels[level as usize],
            ..Default::default()
        })
        .collect();
        LogSummary {
            targets,
            levels,
            lines: self.lines,
            first_timestamp: timestamp_of(&self.first_timestamp).into(),
            last_timestamp: timestamp_of(&self.last_timestamp).into(),
            scanned: self.offset,
            size,
            done,
            ..Default::default()
        }
    }
}

/// The `Timestamp` a log line's RFC 3339 time names, or `None` when unparseable.
fn timestamp_of(text: &str) -> Option<Timestamp> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    Some(Timestamp {
        seconds: parsed.timestamp(),
        nanos: parsed.timestamp_subsec_nanos() as i32,
        ..Default::default()
    })
}

/// Continues `tally` through the rest of `path`, calling `snapshot` as it goes.
///
/// `snapshot` receives the tally after every [`SNAPSHOT_EVERY`] bytes and
/// returns `false` to stop, as a request whose client is gone does. The
/// returned flag says whether the scan reached the end of the file. An
/// unterminated last line is left for the next scan.
///
/// # Errors
///
/// Returns the I/O error of the file.
pub(super) fn scan(
    path: &Path,
    mut tally: Tally,
    mut snapshot: impl FnMut(&Tally) -> bool,
) -> io::Result<(Tally, bool)> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(tally.offset))?;
    let mut block = vec![0u8; BLOCK];
    let mut carry: Vec<u8> = Vec::new();
    let mut since_snapshot = 0u64;
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            return Ok((tally, true));
        }
        let data = &block[..read];
        let block_start = tally.offset + carry.len() as u64;
        let mut from = 0;
        let mut complete = true;
        if !carry.is_empty() {
            match memchr::memchr(b'\n', data) {
                Some(index) => {
                    carry.extend_from_slice(&data[..index]);
                    tally.record(&carry);
                    carry.clear();
                    from = index + 1;
                }
                None => {
                    carry.extend_from_slice(data);
                    complete = false;
                }
            }
        }
        if complete {
            while let Some(index) = memchr::memchr(b'\n', &data[from..]) {
                tally.record(&data[from..from + index]);
                from += index + 1;
            }
            carry.clear();
            if from < read {
                carry.extend_from_slice(&data[from..]);
            }
            tally.offset = block_start + from as u64;
        }
        since_snapshot += read as u64;
        if since_snapshot >= SNAPSHOT_EVERY {
            since_snapshot = 0;
            if !snapshot(&tally) {
                return Ok((tally, false));
            }
        }
    }
}

/// How a file looked when it was scanned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Stamp {
    pub(super) size: u64,
    pub(super) modified: Option<SystemTime>,
}

impl Stamp {
    pub(super) fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

/// What a request has to do to summarise a file.
pub(super) enum Plan {
    /// The file is as the cached scan saw it.
    Cached(Tally),
    /// Scan from this tally: empty for a first scan, partial for a resumed one.
    Scan(Tally),
}

/// Finished scans, kept for the Companion session.
#[derive(Default)]
pub(in crate::api) struct SummaryCache {
    entries: Mutex<FxHashMap<PathBuf, (Stamp, Tally)>>,
}

impl SummaryCache {
    /// Decides how to summarise `path` given its current `stamp`.
    ///
    /// An unchanged file comes from the cache. A grown Current run resumes
    /// after the lines already counted. Anything else starts over, because a
    /// file that shrank or changed in place may no longer hold those lines.
    pub(super) fn plan(&self, path: &Path, stamp: Stamp, current: bool) -> Plan {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match entries.get(path) {
            Some((cached, tally)) if *cached == stamp => Plan::Cached(tally.clone()),
            Some((cached, tally)) if current && stamp.size > cached.size => {
                Plan::Scan(tally.clone())
            }
            _ => Plan::Scan(Tally::default()),
        }
    }

    /// Keeps a scan that reached the end of the file.
    pub(super) fn store(&self, path: &Path, stamp: Stamp, tally: Tally) {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(path.to_path_buf(), (stamp, tally));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn json(level: &str, target: &str, time: &str) -> String {
        format!(
            r#"{{"timestamp":"{time}","level":"{level}","fields":{{"message":"m","target":"decoy"}},"target":"{target}"}}"#
        )
    }

    fn log_of(lines: &[String]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file
    }

    fn scanned(path: &Path, tally: Tally) -> Tally {
        scan(path, tally, |_| true).unwrap().0
    }

    fn ts(text: &str) -> Timestamp {
        timestamp_of(text).expect("valid test time")
    }

    #[test]
    fn counts_targets_levels_lines_and_the_time_range() {
        let file = log_of(&[
            json("INFO", "a::b", "2026-11-25T10:00:00Z"),
            json("WARN", "a::b", "2026-11-25T10:00:01Z"),
            "plain launcher output".to_owned(),
            json("INFO", "c", "2026-11-25T10:00:02Z"),
        ]);
        let summary = scanned(file.path(), Tally::default()).to_proto(0, true);
        assert_eq!(summary.lines, 4);
        let targets: Vec<(&str, u64)> = summary
            .targets
            .iter()
            .map(|t| (t.target.as_str(), t.lines))
            .collect();
        assert_eq!(targets, [("a::b", 2), ("c", 1)]);
        let levels: Vec<(LogLevel, u64)> = summary
            .levels
            .iter()
            .map(|l| (l.level.as_known().unwrap(), l.lines))
            .collect();
        assert_eq!(
            levels,
            [(LogLevel::LOG_LEVEL_INFO, 2), (LogLevel::LOG_LEVEL_WARN, 1)]
        );
        assert_eq!(
            summary.first_timestamp.as_option().map(|time| time.seconds),
            Some(ts("2026-11-25T10:00:00Z").seconds)
        );
        assert_eq!(
            summary.last_timestamp.as_option().map(|time| time.seconds),
            Some(ts("2026-11-25T10:00:02Z").seconds)
        );
        assert!(summary.done);
    }

    #[test]
    fn an_unparseable_timestamp_leaves_the_summary_times_unset() {
        let file = log_of(&[json("INFO", "a", "not-a-time"), json("INFO", "a", "")]);
        let summary = scanned(file.path(), Tally::default()).to_proto(0, true);
        assert!(summary.first_timestamp.is_unset());
        assert!(summary.last_timestamp.is_unset());
    }

    #[test]
    fn the_summary_keeps_the_first_and_last_time_in_file_order() {
        let file = log_of(&[
            json("INFO", "a", "2026-11-25T10:00:02Z"),
            json("INFO", "a", "2026-11-25T10:00:01Z"),
            json("INFO", "a", "2026-11-25T10:00:03Z"),
        ]);
        let summary = scanned(file.path(), Tally::default()).to_proto(0, true);
        assert_eq!(
            summary.first_timestamp.as_option().map(|time| time.seconds),
            Some(ts("2026-11-25T10:00:02Z").seconds)
        );
        assert_eq!(
            summary.last_timestamp.as_option().map(|time| time.seconds),
            Some(ts("2026-11-25T10:00:03Z").seconds)
        );
    }

    #[test]
    fn resuming_counts_only_the_new_lines() {
        let mut file = log_of(&[json("INFO", "a", "t1"), json("INFO", "a", "t2")]);
        let first = scanned(file.path(), Tally::default());
        assert_eq!(first.lines, 2);
        writeln!(file, "{}", json("ERROR", "b", "t3")).unwrap();
        let resumed = scanned(file.path(), first.clone());
        assert_eq!(resumed.lines, 3);
        assert_eq!(resumed, scanned(file.path(), Tally::default()));
        assert_eq!(resumed.offset, file.as_file().metadata().unwrap().len());
    }

    #[test]
    fn an_unterminated_last_line_is_left_for_the_next_scan() {
        let mut file = log_of(&[json("INFO", "a", "t1")]);
        write!(file, "{{\"level\":\"INFO\"").unwrap();
        let tally = scanned(file.path(), Tally::default());
        assert_eq!(tally.lines, 1);
        writeln!(file, ",\"target\":\"b\"}}").unwrap();
        let tally = scanned(file.path(), tally);
        assert_eq!(tally.lines, 2);
        assert_eq!(tally.targets.get("b"), Some(&1));
    }

    #[test]
    fn snapshots_stream_while_scanning_and_stop_when_declined() {
        let line = json("INFO", "a", "t");
        let count = (SNAPSHOT_EVERY as usize / (line.len() + 1)) * 2 + 10;
        let file = log_of(&vec![line; count]);
        let mut seen = Vec::new();
        let (tally, finished) = scan(file.path(), Tally::default(), |tally| {
            seen.push(tally.lines);
            true
        })
        .unwrap();
        assert!(finished);
        assert!(seen.len() >= 2, "{seen:?}");
        assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(seen.iter().all(|lines| *lines < tally.lines));

        let (stopped, finished) = scan(file.path(), Tally::default(), |_| false).unwrap();
        assert!(!finished);
        assert!(stopped.lines < tally.lines);
    }

    #[test]
    fn an_unchanged_file_is_served_from_the_cache() {
        let file = log_of(&[json("INFO", "a", "t")]);
        let cache = SummaryCache::default();
        let stamp = Stamp::of(&file.as_file().metadata().unwrap());
        let tally = scanned(file.path(), Tally::default());
        cache.store(file.path(), stamp, tally.clone());
        match cache.plan(file.path(), stamp, false) {
            Plan::Cached(cached) => assert_eq!(cached, tally),
            Plan::Scan(_) => panic!("an unchanged file must not be scanned again"),
        }
    }

    #[test]
    fn a_grown_current_run_resumes_but_a_grown_older_log_starts_over() {
        let file = log_of(&[json("INFO", "a", "t")]);
        let cache = SummaryCache::default();
        let before = Stamp::of(&file.as_file().metadata().unwrap());
        let tally = scanned(file.path(), Tally::default());
        cache.store(file.path(), before, tally.clone());
        let grown = Stamp {
            size: before.size + 10,
            ..before
        };
        match cache.plan(file.path(), grown, true) {
            Plan::Scan(resume) => assert_eq!(resume, tally),
            Plan::Cached(_) => panic!("a changed file is not cached"),
        }
        match cache.plan(file.path(), grown, false) {
            Plan::Scan(fresh) => assert_eq!(fresh, Tally::default()),
            Plan::Cached(_) => panic!("a changed file is not cached"),
        }
        let shrunk = Stamp {
            size: before.size - 1,
            ..before
        };
        match cache.plan(file.path(), shrunk, true) {
            Plan::Scan(fresh) => assert_eq!(fresh, Tally::default()),
            Plan::Cached(_) => panic!("a changed file is not cached"),
        }
    }
}
