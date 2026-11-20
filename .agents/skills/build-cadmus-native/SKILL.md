---
name: build-cadmus-native
description: Build Cadmus on Linux and macOS hosts using cadmuscli. Use when asked how to compile, test, lint, or run the project locally.
---

# Build Cadmus on Native Hosts (Linux / macOS)

## Prerequisites

Run commands from the repository root. The devenv shell provides the required
build and documentation tools.

### Documentation EPUB

`cadmus-core` embeds `docs/book/epub/Cadmus Documentation.epub` at compile time.
Generate it before any command that compiles `cadmus-core`, including
`cadmuscli clippy`, `cadmuscli test`, and direct Cargo commands. Only
`cadmuscli run-emulator` builds a missing EPUB automatically:

```bash
cadmuscli docs --mdbook-only
```

This uses the devenv-provided documentation tools (see the `docs` skill); it
does not install them. Rerun it after documentation changes.

### Custom SQLite

Direct `cargo build`, `cargo check`, and `cargo test` need the UDL-enabled
SQLite library before `libsqlite3-sys` runs:

```bash
cadmuscli setup --host
```

`cadmuscli test`, `cadmuscli clippy`, and `cadmuscli run-emulator` perform this
SQLite preflight automatically. Bare `cadmuscli setup` builds both host and Kobo
targets; use `--host` for native-only work.

Other native dependencies, including MuPDF, libwebp, and the C wrapper, are
built automatically by `crates/core/build.rs`.

## Daily workflow commands

| Goal                          | Command                                                             |
| ----------------------------- | ------------------------------------------------------------------- |
| Check formatting              | `cadmuscli fmt`                                                     |
| Run clippy                    | `cadmuscli clippy`                                                  |
| Run tests (emulator features) | `cadmuscli test --features emulator`                                |
| Run tests with coverage       | `cadmus-test-coverage --features emulator` (devenv)                 |
| View coverage (project-wide)  | `cadmus-coverage-show` (after test-coverage)                        |
| View coverage (patch diff)    | `cadmus-coverage-diff` (after test-coverage)                        |
| Run tests with telemetry      | `cadmuscli test --features "emulator + profiling + test + tracing"` |
| Run the emulator              | `cadmuscli run-emulator` (builds the EPUB first if missing)         |
| Build docs portal (full)      | `cadmuscli docs`                                                    |

### A device feature is required

`cadmus-core` emits `compile_error!("A device feature must be enabled")`
(`crates/core/src/lib.rs`) unless one of `emulator`, `kobo`, or `deviceless`
is enabled. The `cadmus` crate has `default = []`, so plain `cargo
build`, `--features default`, and `cadmuscli test --features default` all
fail. For native host development use `emulator` (what `run-emulator` uses);
use `kobo` only via the `build-kobo` skill (it cross-compiles for ARM).

### Testing locally

The full feature matrix is large and slow. Run the complete matrix only in CI.
Locally, test the feature combination you are actively working with — every
combination must include a device feature:

```bash
# Emulator device feature — fastest, covers most code
cadmuscli test --features emulator

# Add the feature you are modifying on top of a device feature
cadmuscli test --features "emulator + profiling + test + tracing"
```

> [!NOTE]
> The `telemetry` feature is excluded from the xtask matrix because it aliases
> `tracing + profiling` with no separate `cfg` branches. Use the expanded form
> (`emulator + profiling + test + tracing`) instead.

Use `cadmuscli ci matrix` to see all available feature combinations if you
need to verify a specific one.

### Coverage locally

In a devenv shell (`cadmus-test-coverage`, `cadmus-coverage-show`, `cadmus-coverage-diff`):

```bash
cadmus-test-coverage
cadmus-coverage-show
cadmus-coverage-diff
```

Without devenv:

```bash
cadmuscli test --coverage --features emulator
```

This writes `target/coverage/lcov.info`. Project HTML uses `cargo llvm-cov report --html`.
Patch HTML uses `diff-cover` (see `devenv.nix` scripts).

## sccache

devenv sets `RUSTC_WRAPPER=sccache`. If setup or a compile panics inside
sccache, retry that command with the wrapper unset:

```bash
devenv shell -- env -u RUSTC_WRAPPER cadmuscli setup
```

If the failure is a client/server version mismatch after an upgrade, run
`cadmus-sccache restart` first (see `docs/src/contributing/devenv-setup.md`).

## Platform notes

- **Linux**: Native dependencies are built from source automatically via `build.rs`; ensure `gcc`, `make`, `cmake`, and standard build tools are installed.
- **macOS**: Native dependencies are built from source automatically via `build.rs`; Xcode Command Line Tools must be installed. Full support including Kobo cross-compilation.
- The `build-kobo` command cross-compiles for ARM and is available on both Linux and macOS; it runs inside a containerised CI action in the main workflow.
