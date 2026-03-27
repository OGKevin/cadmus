---
name: test
description: Run cargo tests across the full feature matrix. Use this when asked to run tests, check test coverage, or verify the test suite passes.
---

Always use `cargo xtask test` to run tests in this project — never bare
`cargo test` or `cargo nextest run`.
The xtask drives both `cargo nextest run` and `cargo test --doc` across every
feature combination in the workspace matrix.

## Basic usage

```sh
# Run all tests across all feature combinations (default)
cargo xtask test

# Run tests for a specific feature combination (comma-separated, like cargo)
cargo xtask test --features "otel,test"
```

## What runs for each matrix entry

1. `cargo nextest run --all-targets` — parallel test execution with per-test output
2. `cargo test --doc` — doctests (nextest does not execute these)

## Notes

- The feature matrix is derived dynamically from workspace `Cargo.toml` files —
  no manual updates needed when new features are added.
- `--features` accepts comma-separated feature names in any order (e.g. `"otel,test"`).
- Passing an unknown feature combination exits with an error listing all valid labels.
- `TEST_ROOT_DIR` is set to the workspace root automatically so integration
  tests can locate fixture files regardless of working directory.
- `cargo-nextest` must be installed (`cargo install cargo-nextest`).
