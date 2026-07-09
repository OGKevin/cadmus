---
name: build-cadmus-native
description: Build Cadmus on Linux and macOS hosts using xtask. Use when asked how to compile, test, lint, or run the project locally.
---

# Build Cadmus on Native Hosts (Linux / macOS)

## Critical prerequisite: generate the documentation EPUB

`cadmus-core` embeds the user documentation at compile time via `rust-embed`.
The macro points to `docs/book/epub/Cadmus Documentation.epub`. If that file
is missing, **every** `cargo check`, `cargo build`, or `cargo test` that
touches `cadmus-core` will fail with:

```text
error: #[derive(RustEmbed)] folder '…/docs/book/epub/' does not exist.
error[E0599]: no associated function named `get` found for struct `DocumentationAssets`
```

### Generate the EPUB

```bash
cargo xtask docs --mdbook-only
```

This installs mdBook and mdbook-epub (if missing), builds the mdBook sources,
and writes `docs/book/epub/Cadmus Documentation.epub`. You only need to rerun
it when documentation sources change.

## One-time setup

### Download runtime assets (optional)

```bash
cargo xtask download-assets
```

Pulls static assets (fonts, icons, etc.) from the latest GitHub release. Not
strictly required for compilation, but the emulator and some tests expect them.

Native dependencies (MuPDF, libwebp, and the C wrapper) are now built
automatically by `build.rs` when you run any Cargo command that compiles
`cadmus-core`.

## Daily workflow commands

| Goal                           | Command                                                             |
| ------------------------------ | ------------------------------------------------------------------- |
| Check formatting               | `cargo xtask fmt`                                                   |
| Run clippy                     | `cargo xtask clippy`                                                |
| Run tests (default features)   | `cargo xtask test --features default`                               |
| Run tests with coverage        | `cadmus-test-coverage --features default` (devenv)                  |
| View coverage (project-wide)   | `cadmus-coverage-show` (after test-coverage)                        |
| View coverage (patch diff)     | `cadmus-coverage-diff` (after test-coverage)                        |
| Run tests with telemetry       | `cargo xtask test --features "profiling + test + tracing"`          |
| Run the emulator               | `cargo xtask run-emulator` (builds the EPUB first if missing)       |
| Install the importer CLI       | `cargo xtask install-importer`                                      |
| Build docs portal (full)       | `cargo xtask docs`                                                  |

### Testing locally

The full feature matrix is large and slow. Run the complete matrix only in CI.
Locally, test the feature combination you are actively working with:

```bash
# Default features — fastest, covers most code
cargo xtask test --features default

# Specific feature you are adding or modifying
cargo xtask test --features "profiling + test + tracing"
```

> [!NOTE]
> The `telemetry` feature is excluded from the xtask matrix because it aliases
> `tracing + profiling` with no separate `cfg` branches. Use the expanded form
> (`profiling + test + tracing`) instead.

Use `cargo xtask ci matrix` to see all available feature combinations if you
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
cargo xtask test --coverage --features default
```

This writes `target/coverage/lcov.info`. Project HTML uses `cargo llvm-cov report --html`.
Patch HTML uses `diff-cover` (see `devenv.nix` scripts).

## What the xtask wrappers do

- **`fmt`** — runs `cargo fmt --check` (or `--apply` in CI) across the workspace
- **`clippy`** — iterates the full feature matrix; use `--features` to narrow it
- **`test`** — iterates the test feature matrix; `--coverage` enables llvm-cov instrumentation
- **`run-emulator`** — ensures the documentation EPUB exists, then runs `cargo run -p emulator`
- **`install-importer`** — runs `cargo install --path crates/importer`

## Common mistakes

| Mistake                                                       | Result                                                        | Fix                                                      |
| ------------------------------------------------------------- | ------------------------------------------------------------- | -------------------------------------------------------- |
| Running `cargo check` before `cargo xtask docs --mdbook-only` | `RustEmbed` folder-not-found error                            | Generate the EPUB first                                  |
| Running bare `cargo clippy` / `cargo test` directly           | May miss feature-gated code or use wrong feature combinations | Prefer `cargo xtask clippy` and `cargo xtask test`       |
| Running `cargo xtask test` without `--features`               | Runs the full (slow) CI matrix locally                        | Pass `--features default` or the specific combo you need |

## Platform notes

- **Linux**: Native dependencies are built from source automatically via `build.rs`; ensure `gcc`, `make`, `cmake`, and standard build tools are installed.
- **macOS**: Native dependencies are built from source automatically via `build.rs`; Xcode Command Line Tools must be installed. Full support including Kobo cross-compilation.
- The `build-kobo` command cross-compiles for ARM and is available on both Linux and macOS; it runs inside a containerised CI action in the main workflow.
