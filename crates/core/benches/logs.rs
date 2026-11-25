//! Throughput of the Companion log viewer's reads on a large Run log.
//!
//! The fixture is a JSON-lines file shaped after the 744 MB log of issue
//! #1216: about 98 % of the lines come from one noisy dependency target, the
//! rest spread over about fifty other targets with nested fields, plus a few
//! lines that are not JSON. It is written on the first measured iteration to
//! `CADMUS_BENCH_DIR` (default: the system temporary directory) as
//! `cadmus-logs-bench.json` and overwritten by the next run. A name filter or
//! `--load-baseline` that selects no measured closure never writes it.
//! `CADMUS_BENCH_LOG_BYTES` sets its size, 1 GiB by default; use a small value
//! for a quick local run. `CADMUS_BENCH_BUNDLE_BYTES` sets how much of it the
//! two deflate cases compress, 16 MiB by default.
//!
//! On a Kobo the temporary directory is RAM, so `cadmuscli kobo bench` points
//! `CADMUS_BENCH_DIR` at flash.

use cadmus_core::api::logs::bench::{
    LogFilter, LogLevel, bundle_size, last_page, scan_all, summarize,
};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

const DEFAULT_BYTES: u64 = 1 << 30;

const DEFAULT_BUNDLE_BYTES: u64 = 16 << 20;

/// Space left free on the filesystem beyond the fixture.
const FREE_MARGIN: u64 = 64 << 20;

const NOISE: &str = r#","level":"DEBUG","fields":{"message":"processing in state Cell { value: Data }","log.target":"xml5ever::tokenizer","log.module_path":"xml5ever::tokenizer","log.file":"xml5ever-0.41.0/src/tokenizer/mod.rs","log.line":1000},"target":"xml5ever::tokenizer"}"#;

/// Deflate levels the bundle cases compare.
const BUNDLE_LEVELS: [i64; 2] = [1, 6];

/// One bit per bundle case, so its compressed size is printed once.
static REPORTED: AtomicU8 = AtomicU8::new(0);

/// Log bytes the deflate cases compress.
fn bundle_bytes() -> u64 {
    std::env::var("CADMUS_BENCH_BUNDLE_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_BUNDLE_BYTES)
}

fn fixture_bytes() -> u64 {
    std::env::var("CADMUS_BENCH_LOG_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_BYTES)
}

fn fixture_dir() -> PathBuf {
    std::env::var_os("CADMUS_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn free_bytes(dir: &Path) -> u64 {
    let stats = nix::sys::statvfs::statvfs(dir).expect("statvfs of CADMUS_BENCH_DIR");
    stats.blocks_available() as u64 * stats.fragment_size() as u64
}

/// Deterministic xorshift, so every run reads the same bytes.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

struct Fixture {
    path: PathBuf,
    size: u64,
}

static FIXTURE: LazyLock<Fixture> =
    LazyLock::new(|| write_fixture(&fixture_dir(), fixture_bytes()));

/// The fixture, written by the first measured closure that needs it.
fn fixture() -> &'static Fixture {
    &FIXTURE
}

fn timestamp(line: u64) -> String {
    let seconds = line / 1000;
    format!(
        "2026-11-16T{:02}:{:02}:{:02}.{:06}Z",
        7 + seconds / 3600 % 17,
        seconds / 60 % 60,
        seconds % 60,
        line % 1000 * 1000
    )
}

fn other_line(rng: &mut Rng, line: u64) -> String {
    let target = match rng.next() % 50 {
        0..=4 => "cadmus_core::library::import".to_owned(),
        5..=7 => "cadmus_core::device::wifi".to_owned(),
        n => format!("cadmus_core::module{n}::sub{}", rng.next() % 4),
    };
    let level = match rng.next() % 1000 {
        0 => "ERROR",
        1..=9 => "WARN",
        10..=200 => "INFO",
        201..=500 => "DEBUG",
        _ => "TRACE",
    };
    let message = if target.ends_with("wifi") {
        "Wi-Fi state changed to connected"
    } else {
        "operation finished"
    };
    format!(
        r#"{{"timestamp":"{}","level":"{level}","fields":{{"message":"{message}","path":"/mnt/onboard/Books/{}.epub","count":{},"nested":{{"ok":true,"id":{}}}}},"target":"{target}","span":{{"name":"job"}}}}"#,
        timestamp(line),
        rng.next() % 5000,
        rng.next() % 100,
        rng.next() % 1_000_000
    )
}

/// Writes the fixture, stopping with a message that names `CADMUS_BENCH_DIR`
/// when the filesystem cannot hold it.
fn write_fixture(dir: &Path, bytes: u64) -> Fixture {
    fs::create_dir_all(dir).expect("create CADMUS_BENCH_DIR");
    let have = free_bytes(dir);
    assert!(
        have >= bytes + FREE_MARGIN,
        "{} needs {} bytes free for the {bytes}-byte log fixture but has {have}; set CADMUS_BENCH_DIR to a directory on a larger filesystem or lower CADMUS_BENCH_LOG_BYTES",
        dir.display(),
        bytes + FREE_MARGIN,
    );
    let path = dir.join("cadmus-logs-bench.json");
    let mut fixture = Fixture { path, size: 0 };
    let mut out = BufWriter::with_capacity(1 << 20, File::create(&fixture.path).expect("create"));
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut line = 0u64;
    while fixture.size < bytes {
        let row = match rng.next() % 1000 {
            0 => "cadmus.sh: restarting after a crash".to_owned(),
            1..=19 => other_line(&mut rng, line),
            _ => format!(r#"{{"timestamp":"{}"{NOISE}"#, timestamp(line)),
        };
        out.write_all(row.as_bytes()).expect("write fixture");
        out.write_all(b"\n").expect("write fixture");
        fixture.size += row.len() as u64 + 1;
        line += 1;
    }
    out.flush().expect("flush fixture");
    fixture
}

fn filter(levels: &[LogLevel], targets: &[&str], text: &str) -> LogFilter {
    LogFilter {
        levels: levels.iter().map(|level| (*level).into()).collect(),
        targets: targets.iter().map(|target| (*target).to_owned()).collect(),
        text: text.to_owned(),
        ..Default::default()
    }
}

fn bench_logs(c: &mut Criterion) {
    let configured = fixture_bytes();
    let mut group = c.benchmark_group("logs");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(30));

    let unfiltered = LogFilter::default();
    group.throughput(Throughput::Elements(200));
    group.bench_function("page/newest_200_unfiltered", |b| {
        b.iter(|| black_box(last_page(&fixture().path, 200, &unfiltered).expect("page")));
    });

    group.throughput(Throughput::Bytes(configured));
    let scans = [
        ("level_error", filter(&[LogLevel::LOG_LEVEL_ERROR], &[], "")),
        ("target_prefix", filter(&[], &["cadmus_core::library"], "")),
        ("text_case_insensitive", filter(&[], &[], "WI-FI")),
        (
            "level_target_text",
            filter(
                &[LogLevel::LOG_LEVEL_WARN, LogLevel::LOG_LEVEL_ERROR],
                &["cadmus_core::device"],
                "wi-fi",
            ),
        ),
    ];
    for (name, filter) in &scans {
        group.bench_function(format!("scan_all/{name}"), |b| {
            b.iter(|| black_box(scan_all(&fixture().path, filter).expect("scan")));
        });
    }

    group.bench_function("summarize", |b| {
        b.iter(|| black_box(summarize(&fixture().path).expect("summary")));
    });

    let bundled = bundle_bytes();
    group.throughput(Throughput::Bytes(bundled));
    for (index, level) in BUNDLE_LEVELS.into_iter().enumerate() {
        group.bench_function(format!("bundle/deflate_level_{level}"), |b| {
            b.iter(|| {
                let fixture = fixture();
                let limit = bundled.min(fixture.size);
                let compressed = bundle_size(&fixture.path, limit, level).expect("bundle");
                let bit = 1 << index;
                if REPORTED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
                    eprintln!(
                        "bundle/deflate_level_{level}: {limit} bytes of log compress to {compressed} bytes"
                    );
                }
                black_box(compressed)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_logs);
criterion_main!(benches);
