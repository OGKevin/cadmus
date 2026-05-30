#[cfg(feature = "bench")]
use std::io::Write;
#[cfg(feature = "bench")]
use std::time::Duration;

#[cfg(feature = "bench")]
use cadmus_core::helpers::Fingerprint;
#[cfg(feature = "bench")]
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
#[cfg(feature = "bench")]
use tempfile::NamedTempFile;

/// Creates a temporary file filled with `size` bytes of repeating pattern data.
#[cfg(feature = "bench")]
fn create_temp_file(size: usize) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("failed to create temp file");
    let chunk: Vec<u8> = (0..256u16).map(|i| i as u8).cycle().take(size).collect();
    file.write_all(&chunk).expect("failed to write temp file");
    file.flush().expect("failed to flush temp file");
    file
}

#[cfg(feature = "bench")]
fn bench_fingerprint(c: &mut Criterion) {
    let sizes: &[(usize, &str)] = &[
        (1_024, "1KB"),
        (10_240, "10KB"),
        (102_400, "100KB"),
        (204_800, "200KB"),
        (512_000, "500KB"),
        (716_800, "700KB"),
        (1_048_576, "1MB"),
        (209_715_200, "200MB"),
        (524_288_000, "500MB"),
        (734_003_200, "700MB"),
        (1_073_741_824, "1GB"),
    ];

    let mut group = c.benchmark_group("fingerprint");

    for (size, label) in sizes {
        let file = create_temp_file(*size);
        let path = file.path().to_path_buf();

        group.bench_with_input(BenchmarkId::new("size", label), label, |b, _| {
            b.iter(|| {
                let _fp = path.fingerprint().expect("fingerprint failed");
            });
        });
    }

    group.finish();
}

#[cfg(feature = "bench")]
criterion_group!(
    name = benches;
    config = Criterion::default().measurement_time(Duration::from_secs(10)).sample_size(50);
    targets = bench_fingerprint
);
#[cfg(feature = "bench")]
criterion_main!(benches);

#[cfg(not(feature = "bench"))]
fn main() {}
