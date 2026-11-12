#[cfg(feature = "bench")]
mod reader_dict_fixture;

#[cfg(feature = "bench")]
use std::time::Duration;

#[cfg(feature = "bench")]
use cadmus_core::context::DICTIONARIES_DIRNAME;
#[cfg(feature = "bench")]
use cadmus_core::device::inhibitor::Inhibitor;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::DictionaryDb;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::Metadata;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::db_index::DbIndexReader;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::indexing::{Entry, IndexReader, normalize};
#[cfg(feature = "bench")]
use cadmus_core::task::BackgroundTask;
#[cfg(feature = "bench")]
use cadmus_core::task::DictionaryIndexTask;
#[cfg(feature = "bench")]
use cadmus_core::view::hub_channel;
#[cfg(feature = "bench")]
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
#[cfg(feature = "bench")]
use tokio::runtime::Runtime;
#[cfg(feature = "bench")]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "bench")]
const READER_DICT_BENCH_DICT_ID: i64 = 1;

#[cfg(feature = "bench")]
fn make_sorted_entries(n: usize) -> Vec<Entry> {
    (0..n)
        .map(|i| Entry {
            headword: format!("word{:06}", i),
            offset: i as u64,
            size: 10,
            original: None,
        })
        .collect()
}

#[cfg(feature = "bench")]
fn make_unsorted_entries(n: usize) -> Vec<Entry> {
    let mut entries = make_sorted_entries(n);
    entries.reverse();
    entries
}

#[cfg(feature = "bench")]
fn make_entries_needing_transform(n: usize, sorted: bool) -> Vec<Entry> {
    let mut entries: Vec<Entry> = (0..n)
        .map(|i| Entry {
            headword: format!("WORD-{:06}", i),
            offset: i as u64,
            size: 10,
            original: None,
        })
        .collect();

    if !sorted {
        entries.reverse();
    }

    entries
}

#[cfg(feature = "bench")]
fn bench_normalize(c: &mut Criterion) {
    let metadata_no_transform = Metadata {
        all_chars: true,
        case_sensitive: true,
    };

    let metadata_with_transform = Metadata {
        all_chars: false,
        case_sensitive: false,
    };

    let mut group = c.benchmark_group("normalize");

    group.bench_function("sorted_no_transform_10k", |b| {
        let entries = make_sorted_entries(10_000);
        b.iter(|| normalize(&entries, &metadata_no_transform))
    });

    group.bench_function("sorted_with_transform_10k", |b| {
        let entries = make_entries_needing_transform(10_000, true);
        b.iter(|| normalize(&entries, &metadata_with_transform))
    });

    group.bench_function("unsorted_no_transform_10k", |b| {
        let entries = make_unsorted_entries(10_000);
        b.iter(|| normalize(&entries, &metadata_no_transform))
    });

    group.bench_function("unsorted_with_transform_10k", |b| {
        let entries = make_entries_needing_transform(10_000, false);
        b.iter(|| normalize(&entries, &metadata_with_transform))
    });

    group.bench_function("large_unsorted_with_transform_100k", |b| {
        let entries = make_entries_needing_transform(100_000, false);
        b.iter(|| normalize(&entries, &metadata_with_transform))
    });

    group.finish();
}

#[cfg(feature = "bench")]
async fn db_index_fuzzy_scoped(db: &DictionaryDb, headword: &str) -> usize {
    let reader = DbIndexReader::new(db.pool(), Some(READER_DICT_BENCH_DICT_ID));
    reader.find(headword, true).await.len()
}

#[cfg(feature = "bench")]
async fn db_index_fuzzy_global(db: &DictionaryDb, headword: &str) -> usize {
    let reader = DbIndexReader::new(db.pool(), None);
    reader.find(headword, true).await.len()
}

#[cfg(feature = "bench")]
fn bench_fuzzy_reader_dict_en(c: &mut Criterion) {
    if std::env::var("CADMUS_DICT_BENCH_SKIP_READER_DICT").is_ok() {
        return;
    }

    let rt = Runtime::new().expect("tokio runtime");
    let headword = reader_dict_fixture::default_reader_dict_headword();
    let fixture = rt.block_on(reader_dict_fixture::init_reader_dict_fixture(&headword));

    let mut group = c.benchmark_group("fuzzy_prefix_reader_dict_en");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));

    let label = format!("{}_entries", fixture.entry_count);
    let db = &fixture.db;
    let headword = fixture.headword.clone();

    group.bench_with_input(
        criterion::BenchmarkId::new("db_index_fuzzy_scoped", &label),
        &headword,
        |b, headword| b.iter(|| rt.block_on(db_index_fuzzy_scoped(db, headword))),
    );
    group.bench_with_input(
        criterion::BenchmarkId::new("db_index_fuzzy_global", &label),
        &headword,
        |b, headword| b.iter(|| rt.block_on(db_index_fuzzy_global(db, headword))),
    );

    group.finish();
}

#[cfg(feature = "bench")]
enum IndexWriteFixture {
    Generated {
        rows: usize,
        transformed_headwords: bool,
    },
    ReaderDict {
        index_path: std::path::PathBuf,
        rows: usize,
    },
}

#[cfg(feature = "bench")]
struct IndexWriteWorkload {
    label: &'static str,
    fixture: IndexWriteFixture,
}

/// Writes a generated `.index` fixture the production task indexes from scratch.
///
/// The `00-database-allchars` line makes `normalize` keep punctuation; uppercase
/// headwords additionally force it to store the source word in `original`,
/// which is what separates the two storage shapes.
#[cfg(feature = "bench")]
fn write_index_file(dict_dir: &std::path::Path, rows: usize, transformed_headwords: bool) {
    let mut content = String::with_capacity(rows * 16);
    content.push_str("00-database-allchars\tA\tA\n");
    for i in 0..rows {
        let case = if transformed_headwords { 'W' } else { 'w' };
        content.push_str(&format!("{case}ord{i:07}\tA\tA\n"));
    }
    std::fs::write(dict_dir.join("generated.index"), content).expect("write index fixture");
}

/// Drives the production indexing path — [`DictionaryIndexTask::run`] over an
/// on-disk `.index` file — against a database created fresh for every
/// iteration so growth and duplicate suppression cannot distort samples.
///
/// The full-size workload reuses the Reader-Dict-en index that the fuzzy
/// benchmark downloads and caches; the smaller generated workloads exist so
/// `original` coverage can be toggled independently of the dictionary data.
#[cfg(feature = "bench")]
fn bench_index_batch_writes(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    let reader_headword = reader_dict_fixture::default_reader_dict_headword();
    let reader_dict = rt.block_on(reader_dict_fixture::init_reader_dict_fixture(
        &reader_headword,
    ));
    let reader_index = rt.block_on(reader_dict_fixture::reader_dict_index_path());

    let workloads = [
        IndexWriteWorkload {
            label: "insert_10k",
            fixture: IndexWriteFixture::Generated {
                rows: 10_000,
                transformed_headwords: false,
            },
        },
        IndexWriteWorkload {
            label: "insert_10k_original",
            fixture: IndexWriteFixture::Generated {
                rows: 10_000,
                transformed_headwords: true,
            },
        },
        IndexWriteWorkload {
            label: "insert_100k",
            fixture: IndexWriteFixture::Generated {
                rows: 100_000,
                transformed_headwords: false,
            },
        },
        IndexWriteWorkload {
            label: "insert_100k_original",
            fixture: IndexWriteFixture::Generated {
                rows: 100_000,
                transformed_headwords: true,
            },
        },
        IndexWriteWorkload {
            label: "insert_1_5m",
            fixture: IndexWriteFixture::Generated {
                rows: 1_500_000,
                transformed_headwords: false,
            },
        },
        IndexWriteWorkload {
            label: "insert_reader_dict",
            fixture: IndexWriteFixture::ReaderDict {
                index_path: reader_index,
                rows: reader_dict.entry_count,
            },
        },
    ];

    let mut group = c.benchmark_group("index_batch_writes");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    group.warm_up_time(Duration::from_secs(1));

    for workload in workloads {
        let dir = tempfile::tempdir().expect("index fixture tempdir");
        let dict_dir = dir.path().join(DICTIONARIES_DIRNAME);
        std::fs::create_dir_all(&dict_dir).expect("create dictionaries dir");

        let elements = match workload.fixture {
            IndexWriteFixture::Generated {
                rows,
                transformed_headwords,
            } => {
                write_index_file(&dict_dir, rows, transformed_headwords);
                rows as u64 + 1
            }
            IndexWriteFixture::ReaderDict {
                ref index_path,
                rows,
            } => {
                std::fs::copy(index_path, dict_dir.join("reader-dict.index"))
                    .expect("copy Reader-Dict-en index");
                rows as u64
            }
        };

        group.throughput(Throughput::Elements(elements));
        group.bench_function(workload.label, |b| {
            b.iter(|| {
                rt.block_on(async {
                    let db = DictionaryDb::open(":memory:")
                        .await
                        .expect("dictionary database");
                    let mut task = DictionaryIndexTask::new(
                        db.pool().clone(),
                        dir.path().to_path_buf(),
                        Inhibitor::noop(),
                    );
                    let (hub, _rx) = hub_channel();
                    let shutdown = CancellationToken::new();
                    task.run(&hub, &shutdown).await;
                });
            });
        });
    }

    group.finish();
}

#[cfg(feature = "bench")]
criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10)).sample_size(10);
    targets = bench_normalize, bench_fuzzy_reader_dict_en, bench_index_batch_writes
);
#[cfg(feature = "bench")]
criterion_main!(benches);

#[cfg(not(feature = "bench"))]
fn main() {}
