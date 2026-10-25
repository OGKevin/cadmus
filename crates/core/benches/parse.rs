//! Benchmarks for chapter parsing: xml5ever versus the hand-rolled parser.
//!
//! Compares the production xml5ever parser ([`parse_xml`]) with [`XmlParser`],
//! the hand-written parser it replaced and that parity tests still exercise, so
//! a slower chapter parse can be attributed to the parser swap rather than to
//! layout or rendering.
//!
//! Both parsers build an [`XmlTree`] from the same source string. Criterion
//! measures the parse only: the EPUB is read and decompressed during setup, so
//! file I/O is never timed.
//!
//! Inputs are the `.xhtml` / `.html` / `.htm` entries of the documentation
//! EPUB at `docs/book/epub/Cadmus Documentation.epub`.
//!
//! Run with the `bench` feature that compiles [`XmlParser`]. It selects the
//! test device, so it pairs with `deviceless` rather than `emulator`:
//!
//! ```text
//! cargo bench -p cadmus-core --features deviceless,bench --bench parse
//! ```

use cadmus_core::document::html::dom::XmlTree;
use cadmus_core::document::html::xml::{XmlParser, parse_xml};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::fs::File;
use std::hint::black_box;
use std::io::Read;
use std::path::PathBuf;
use zip::ZipArchive;

/// A chapter of the documentation EPUB: its archive name and source text.
type Chapter = (String, String);

/// The EPUB the repository ships as user documentation, relative to this
/// crate's manifest directory.
const EPUB_PATH: &str = "../../docs/book/epub/Cadmus Documentation.epub";

/// Reads every HTML-like entry of the documentation EPUB, sorted from largest
/// to smallest so the first entry is the heaviest parse the documentation has.
fn load_chapters() -> Vec<Chapter> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(EPUB_PATH);
    let file = File::open(&path).unwrap_or_else(|error| {
        panic!(
            "failed to open {} ({error}); build the docs EPUB first",
            path.display()
        )
    });

    let mut archive = ZipArchive::new(file).expect("documentation EPUB must be a readable zip");
    let mut chapters = Vec::new();

    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let name = match entry.name() {
            Ok(name) => name.to_string(),
            Err(_) => continue,
        };
        if !is_chapter(&name, entry.is_dir()) {
            continue;
        }

        let mut source = String::new();
        if entry.read_to_string(&mut source).is_err() {
            continue;
        }
        chapters.push((name, source));
    }

    chapters.sort_by_key(|(_, source)| std::cmp::Reverse(source.len()));
    chapters
}

/// Reports whether an archive entry is a chapter the bench should parse.
fn is_chapter(name: &str, is_dir: bool) -> bool {
    if is_dir {
        return false;
    }
    let name = name.to_ascii_lowercase();
    name.ends_with(".xhtml") || name.ends_with(".html") || name.ends_with(".htm")
}

/// Parses `source` with the hand-rolled parser under test.
fn parse_hand_rolled(source: &str) -> XmlTree {
    XmlParser::new(source).parse()
}

/// Parses `source` with the production xml5ever parser.
fn parse_xml5ever(source: &str) -> XmlTree {
    parse_xml(source)
}

fn bench_parsers(c: &mut Criterion) {
    let chapters = load_chapters();
    let (largest_name, largest_source) = chapters
        .first()
        .expect("documentation EPUB must contain at least one chapter");
    let total_bytes: usize = chapters.iter().map(|(_, source)| source.len()).sum();

    let mut group = c.benchmark_group("chapter_largest");
    group.throughput(Throughput::Bytes(largest_source.len() as u64));
    group.bench_with_input(
        BenchmarkId::new("xml5ever", largest_name.as_str()),
        largest_source,
        |b, source| b.iter(|| black_box(parse_xml5ever(black_box(source)))),
    );
    group.bench_with_input(
        BenchmarkId::new("hand_rolled", largest_name.as_str()),
        largest_source,
        |b, source| b.iter(|| black_box(parse_hand_rolled(black_box(source)))),
    );
    group.finish();

    let mut group = c.benchmark_group("chapters_all");
    group.throughput(Throughput::Bytes(total_bytes as u64));
    group.bench_function("xml5ever", |b| {
        b.iter(|| {
            for (_, source) in &chapters {
                black_box(parse_xml5ever(black_box(source)));
            }
        })
    });
    group.bench_function("hand_rolled", |b| {
        b.iter(|| {
            for (_, source) in &chapters {
                black_box(parse_hand_rolled(black_box(source)));
            }
        })
    });
    group.finish();
}

criterion_group!(benches, bench_parsers);
criterion_main!(benches);
