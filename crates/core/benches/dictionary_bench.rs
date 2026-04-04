use std::time::Duration;

use cadmus_core::dictionary::indexing::{normalize, Entry};
use cadmus_core::dictionary::Metadata;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

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

fn make_unsorted_entries(n: usize) -> Vec<Entry> {
    let mut entries = make_sorted_entries(n);
    entries.reverse();
    entries
}

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

    group.bench_with_input(
        BenchmarkId::new("sorted_no_transform", 10_000),
        &make_sorted_entries(10_000),
        |b, entries| b.iter(|| normalize(entries, &metadata_no_transform)),
    );

    group.bench_with_input(
        BenchmarkId::new("sorted_with_transform", 10_000),
        &make_entries_needing_transform(10_000, true),
        |b, entries| b.iter(|| normalize(entries, &metadata_with_transform)),
    );

    group.bench_with_input(
        BenchmarkId::new("unsorted_no_transform", 10_000),
        &make_unsorted_entries(10_000),
        |b, entries| b.iter(|| normalize(entries, &metadata_no_transform)),
    );

    group.bench_with_input(
        BenchmarkId::new("unsorted_with_transform", 10_000),
        &make_entries_needing_transform(10_000, false),
        |b, entries| b.iter(|| normalize(entries, &metadata_with_transform)),
    );

    group.bench_with_input(
        BenchmarkId::new("large_unsorted_with_transform", 100_000),
        &make_entries_needing_transform(100_000, false),
        |b, entries| b.iter(|| normalize(entries, &metadata_with_transform)),
    );

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10)).sample_size(10);
    targets = bench_normalize
);
criterion_main!(benches);
