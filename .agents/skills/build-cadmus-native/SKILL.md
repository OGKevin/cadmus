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

### 1. Native dependencies (MuPDF + C wrapper)

```bash
cargo xtask setup-native
```

This downloads MuPDF sources, applies Cadmus-specific patches, and compiles the
C wrapper library. It is **required** before any compilation or test run.

### 2. (Optional) Download runtime assets

```bash
cargo xtask download-assets
```

Pulls static assets (fonts, icons, etc.) from the latest GitHub release. Not
strictly required for compilation, but the emulator and some tests expect them.

## Daily workflow commands

| Goal                         | Command                                                    |
| ---------------------------- | ---------------------------------------------------------- |
| Check formatting             | `cargo xtask fmt`                                          |
| Run clippy                   | `cargo xtask clippy`                                       |
| Run tests (default features) | `cargo xtask test --features default`                      |
| Run tests with telemetry     | `cargo xtask test --features "profiling + test + tracing"` |
| Run the emulator             | `cargo xtask run-emulator`                                 |
| Install the importer CLI     | `cargo xtask install-importer`                             |
| Build docs portal (full)     | `cargo xtask docs`                                         |

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

## What the xtask wrappers do

- **`fmt`** — runs `cargo fmt --check` (or `--apply` in CI) across the workspace
- **`clippy`** — iterates the full feature matrix; use `--features` to narrow it
- **`test`** — iterates the test feature matrix; native deps are assumed built
- **`run-emulator`** — ensures `setup-native` has run, then `cargo run -p emulator`
- **`install-importer`** — ensures `setup-native` has run, then `cargo install --path crates/importer`

## Common mistakes

| Mistake                                                       | Result                                                        | Fix                                                      |
| ------------------------------------------------------------- | ------------------------------------------------------------- | -------------------------------------------------------- |
| Running `cargo check` before `cargo xtask docs --mdbook-only` | `RustEmbed` folder-not-found error                            | Generate the EPUB first                                  |
| Running `cargo test` before `cargo xtask setup-native`        | Linker errors for missing MuPDF / wrapper                     | Run `setup-native` first                                 |
| Running bare `cargo clippy` / `cargo test` directly           | May miss feature-gated code or use wrong feature combinations | Prefer `cargo xtask clippy` and `cargo xtask test`       |
| Running `cargo xtask test` without `--features`               | Runs the full (slow) CI matrix locally                        | Pass `--features default` or the specific combo you need |

## Platform notes

- **Linux**: `setup-native` builds MuPDF from source; ensure `gcc`, `make`, `cmake`, and standard build tools are installed.
- **macOS**: `setup-native` builds MuPDF from source; Xcode Command Line Tools must be installed.
- The `build-kobo` command is **Linux-only** (cross-compiles for ARM); it is not covered here because it runs inside a containerised CI action.
