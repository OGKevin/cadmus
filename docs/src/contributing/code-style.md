<!-- i18n:skip-start -->

# Code Style and Linting

Cadmus enforces a consistent code style across all languages using
[treefmt](https://treefmt.com/) for formatting and several linters for static
analysis. The pre-commit hooks run all checks automatically, and CI enforces the
same rules on every pull request.

## Formatters and Linters

| Tool         | Languages / Files                     | Key Configuration                          |
| ------------ | ------------------------------------- | ------------------------------------------ |
| `rustfmt`    | Rust (`*.rs`)                         | Workspace `rustfmt.toml`                   |
| `prettier`   | JSON, YAML, CSS, JS (not docs `*.md`) | `.prettierrc.json`, `.prettierignore`      |
| `shfmt`      | Shell (`*.sh`, `*.bash`)              | `-i 2 -ci` (2-space, case-indent)          |
| `shellcheck` | Shell (`*.sh`, `*.bash`)              | `.editorconfig`                            |
| `yamllint`   | YAML                                  | `extends: default`, several rules disabled |
| `rumdl`      | Markdown (`*.md`)                     | `.rumdl.toml` (lint + format via `--fix`)  |
| `actionlint` | GitHub Actions workflows              | `-ignore "rust-toolchain"`                 |
| `clippy`     | Rust                                  | `-D warnings`                              |

## Running treefmt

All formatters run through `treefmt`. Inside the devenv shell:

```bash
# Format all files tracked by treefmt
treefmt

# Check without writing (dry run)
treefmt --fail-on-change
```

The pre-commit hook (`git-hooks.hooks.treefmt`) runs `treefmt --fail-on-change`
automatically on every commit, so format issues are caught before they reach CI.

Markdown is formatted by rumdl (`rumdl check --fix` via treefmt). Docs paths in
`.prettierignore` are excluded from Prettier so i18n list nesting is preserved.
CI runs `rumdl check` (lint only) via reviewdog in `docs-lint.yml`.

## Rust Style

### Formatting

`rustfmt` with the workspace configuration handles formatting automatically.
Run it via treefmt or directly:

```bash
cargo fmt
```

### Linting (Clippy)

Clippy runs with `-D warnings` — all warnings are errors:

```bash
cargo xtask clippy
```

Clippy runs across every feature flag combination in CI. When adding a new
feature flag, update `.github/workflows/cargo.yml` to include the new matrix
entries.

### Key Conventions

- Prefer `?` over `unwrap()` / `expect()` in library and app code.
- Use iterators over index-based loops.
- Use `&str` over `String` in function parameters when ownership is not needed.
- Prefer borrowing over cloning.
- Avoid premature `collect()` — keep iterators lazy.
- Use newtype wrappers over raw primitives for domain concepts.

## Shell Style

Shell scripts are formatted with `shfmt` (`-i 2 -ci`) and checked with
`shellcheck`. The `-ci` flag indents `case` statement arms relative to the
`case` keyword:

```bash
# Correct — case arms indented with -ci
case "${VAR}" in
  pattern)
    do_something
    ;;
  *)
    fallback
    ;;
esac
```

Scripts must declare their shell variant. For bash scripts, use:

```bash
#! /bin/bash
```

## Structured Logging

Use the `tracing` crate with structured fields — never string formatting for
log data:

```rust
// Correct
tracing::debug!(pr_number, count, "Found artifacts");

// Wrong
tracing::debug!("[OTA] Found {} artifacts for PR #{}", count, pr_number);
```

See [Logging](telemetry/logging.md) for log level guidance.

## Comments

Comment **why**, not **what**. Most code needs no comments — good naming is
preferred. The rules:

- No inline comments — if one feels necessary, extract the code into a
  well-named function instead.
- No commented-out code.
- No changelog comments (`Modified by X on date`).
- No decorative dividers (`//=====`).
- `TODO`, `FIXME`, `HACK`, `NOTE` annotations are fine with context.

Public API items must have doc comments.

## CI Checks

The following CI workflows enforce style:

| Workflow          | What it checks                                    |
| ----------------- | ------------------------------------------------- |
| `cargo.yml`       | `rustfmt`, `clippy` (full feature matrix), tests  |
| `actions-lint.yml`| `actionlint`, prettier for workflow YAML          |
| `shell.yml`       | `shellcheck`, `shfmt`                             |
| `website.yml`     | prettier, eslint, stylelint (plus typecheck/build)|
| `docs-lint.yml`   | `rumdl` (docs and markdown files)                 |

Inline review comments are posted by matching `*-report.yml` workflows on
`workflow_run` (see `.github/workflows/AGENTS.md`). Collect jobs fail the PR
check when findings remain; report jobs post with `-filter-mode=added` so only
lines changed in the PR are flagged. Running `treefmt` locally before pushing
will catch everything.

<!-- i18n:skip-end -->
