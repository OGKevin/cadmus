<!-- i18n:skip-start -->

# Benchmarks

Performance work is measured with [criterion](https://crates.io/crates/criterion)
benchmarks. A host number says little about a Kobo's CPU, flash and RAM, so the
same benchmarks can also be run on a real device.

## Setup

Benchmarks live in `crates/core/benches/`. Each one is a criterion target with
`harness = false`, declared in `crates/core/Cargo.toml`, so a new benchmark is a
new file plus a `[[bench]]` entry.

## Running on the host

```sh
cadmuscli bench
```

This runs `cargo bench --features bench` for the workspace. Criterion writes its
reports to `target/criterion/`. Arguments to criterion, such as a name filter,
are easiest to pass by calling `cargo bench` directly.

## Running on a Kobo

```sh
# Run the `dictionary_bench` benchmark on the device.
cadmuscli kobo bench --ip 192.168.1.128 --bench dictionary_bench

# Arguments after `--` reach the benchmark binary unchanged.
cadmuscli kobo bench --bench dictionary_bench -- normalize/sorted_no_transform_10k
```

`--bench` takes any bench target of the workspace (`cargo metadata` lists them).
An unknown name fails before building and prints the available ones. The SSH
options, the `KOBO_IP` default and the first-connect host key handling are the
same as for `kobo deploy`; see [SSH](ssh.md).

The benchmark gets these environment variables:

- `CADMUS_BENCH_DIR` points at `<name>/scratch` on flash. `/tmp` is RAM on a
  Kobo, so a large fixture must never go there. The directory is wiped before
  the run and removed afterwards, even when the benchmark fails. The free space
  on the device is printed first.
- `CRITERION_HOME` points at `<name>/criterion`, which is wiped before each run.
- `LD_LIBRARY_PATH` points at the synced `libs/`.

Plots are not generated on the device: `--noplot` is passed by default, since
rendering is slow on a Kobo. After the run, the criterion output is transferred
back from the device to `target/kobo-bench/<name>/criterion/` on the host. The
numbers are in
`target/kobo-bench/<name>/criterion/<group>/<bench>/new/estimates.json`, and the
command prints how to render the HTML report on your machine from that data:

```sh
CRITERION_HOME=target/kobo-bench/<name>/criterion \
  cargo bench -p <package> --features deviceless,bench --bench <name> -- \
  --load-baseline new --baseline base
```

`--load-baseline` makes criterion analyse the copied samples instead of
measuring again, so the report shows the Kobo's numbers; the host only builds
and starts the benchmark. Criterion panics for a benchmark that has no copied
sample, so the printed command repeats the filters you passed to the device run.
Open `target/kobo-bench/<name>/criterion/report/index.html` afterwards.

### Writing a benchmark that runs on the device

The benchmark binary runs on the Kobo, away from the source tree. The rules for
fixtures and scratch data are in `crates/core/benches/AGENTS.md`.

<!-- i18n:skip-end -->
