---
name: clippy
description: Run cargo clippy across the full feature matrix. Use this when asked to lint, check for warnings, or run clippy on the project.
---

Always use `cargo xtask clippy` to lint this project — never bare `cargo clippy`.
The xtask drives clippy across every feature combination in the workspace matrix
so that code behind `#[cfg(not(feature = "..."))]` gates is also checked.

## Basic usage

```sh
# Lint all feature combinations (default)
cargo xtask clippy

# Lint a specific feature combination (comma-separated, like cargo)
cargo xtask clippy --features "otel,test"
```

## Reviewdog (local reporter)

To see only the warnings introduced by your changes, pipe clippy through
reviewdog's local reporter. Pass a branch name or commit hash to diff against:

```sh
# Diff against master branch
cargo xtask clippy --diff-branch master

# Diff against a specific commit
cargo xtask clippy --diff-branch abc1234
```

Reviewdog must be on `PATH` (provided by the devenv shell).

## Notes

- The feature matrix is derived dynamically from workspace `Cargo.toml` files —
  no manual updates needed when new features are added.
- `--features` accepts comma-separated feature names in any order (e.g. `"otel,test"`).
- Passing an unknown feature combination exits with an error listing all valid labels.
