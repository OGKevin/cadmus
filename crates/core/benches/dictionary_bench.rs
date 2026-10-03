#[cfg(feature = "bench")]
mod reader_dict_fixture;

#[cfg(feature = "bench")]
use std::time::Duration;

#[cfg(feature = "bench")]
use cadmus_core::db::Database;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::Metadata;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::db_index::DbIndexReader;
#[cfg(feature = "bench")]
use cadmus_core::dictionary::indexing::{Entry, IndexReader, normalize};
#[cfg(feature = "bench")]
use criterion::{Criterion, criterion_group, criterion_main};
#[cfg(feature = "bench")]
use tokio::runtime::Runtime;

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
async fn db_index_fuzzy_scoped(db: &Database, headword: &str) -> usize {
    let reader = DbIndexReader::new(db, Some(READER_DICT_BENCH_DICT_ID));
    reader.find(headword, true).await.len()
}

#[cfg(feature = "bench")]
async fn db_index_fuzzy_global(db: &Database, headword: &str) -> usize {
    let reader = DbIndexReader::new(db, None);
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
criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10)).sample_size(10);
    targets = bench_normalize, bench_fuzzy_reader_dict_en
);
#[cfg(feature = "bench")]
criterion_main!(benches);

#[cfg(not(feature = "bench"))]
fn main() {}
