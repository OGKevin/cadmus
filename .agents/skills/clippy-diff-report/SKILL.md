---
name: clippy-diff-report
description: Run clippy locally and report only issues in the current diff, matching the CI reviewdog flow. Use when asked how to lint changed code or get clippy feedback on a branch before pushing.
---

# Run Clippy on the Current Diff Only

## The problem

Running `cargo clippy` on the whole workspace produces hundreds of warnings
that may have nothing to do with your changes. The CI workflow solves this
with reviewdog — a tool that filters clippy output to only lines touched by
the PR diff.

You can run the same filter locally before pushing.

## Prerequisites

- `reviewdog` on `PATH` (provided by the devenv shell)
- Documentation EPUB already built (`cargo xtask docs --mdbook-only`)
- Native dependencies already built (`cargo xtask setup --host`)

## Run clippy filtered to your diff

```bash
cargo xtask clippy --features emulator --github-report --diff-branch master
```

This:

1. Runs `cargo clippy --message-format=short` for the `emulator` feature set
2. Pipes output through `reviewdog` with `-filter-mode=added`
3. Reviewdog diffs against `master` and prints only warnings on changed lines

### Why `--features emulator`

`cadmus-core` requires a device feature (`emulator`, `kobo`, or `deviceless`)
to compile — `default` is empty and `--features default` fails. For native
host linting use `emulator` (it matches what `run-emulator` builds). The full
feature matrix is large and slow; run it only in CI. Use `cargo xtask ci
matrix` to see all available labels if you need a specific one.

### Check all feature combinations you touched

If your changes span multiple feature sets (e.g. you added `#[cfg(feature =
"tracing")]` code), run clippy for each relevant combination — every
combination must include a device feature:

```bash
cargo xtask clippy --features emulator --github-report --diff-branch master
cargo xtask clippy --features "emulator + profiling + test + tracing" --github-report --diff-branch master
```

> [!NOTE]
> The `telemetry` feature is excluded from the xtask matrix because it aliases
> `tracing + profiling` with no separate `cfg` branches. Use the expanded form
> (`emulator + profiling + test + tracing`) instead.

## How it works

The xtask wrapper constructs a reviewdog invocation equivalent to:

```bash
reviewdog \
  -f=clippy \
  -filter-mode=added \
  -fail-on-error=false \
  -reporter=local \
  -diff="git diff --no-ext-diff master"
```

- `-filter-mode=added` — only reports diagnostics on lines that appear in the
  diff (new or modified lines)
- `-reporter=local` — prints to terminal instead of posting GitHub comments
- `-diff=...` — tells reviewdog which diff to filter against

## Common use cases

| Goal                                                   | Command                                                                                                      |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------ |
| Check emulator-feature changes before pushing          | `cargo xtask clippy --features emulator --github-report --diff-branch master`                                |
| Check telemetry-related changes                        | `cargo xtask clippy --features "emulator + profiling + test + tracing" --github-report --diff-branch master` |
| See raw (unfiltered) clippy output for one feature set | `cargo xtask clippy --features emulator`                                                                     |
| Run the full matrix (slow — CI only)                   | `cargo xtask clippy --github-report --diff-branch master` (omits `--features`)                               |

## Fixing warnings

When resolving Clippy warnings introduced by a change:

- **Never add `#[allow(...)]`** — fix the root cause.
- **Change only what Clippy flags** — do not refactor unrelated code.
- **`field_reassign_with_default`**: when converting
  `let mut s = S::default(); s.field = val;` to a struct literal, keep
  `let mut` if the variable is mutated again later in the same scope, and
  ensure all subsequent mutations are preserved — do not drop them.

After fixes, re-run the filtered clippy command above, then complete the
verification sequence in [AGENTS.md](../../../AGENTS.md) (fmt → clippy →
tests with `--features emulator` → `build-kobo`).

## Troubleshooting

### "reviewdog not found"

- **Using devenv**: reviewdog is already provided by the devenv shell. Ensure
  you are inside the shell (`direnv allow` or `devenv shell`).
- **Not using devenv**: Install reviewdog yourself. The simplest way is via
  Homebrew (`brew install reviewdog`) or by downloading the binary from
  <https://github.com/reviewdog/reviewdog/releases>. If you are unsure how to
  install it on your platform, ask the operator for help.

### "diff-branch: no merge base"

Your branch may be old. Rebase or merge master first:

```bash
git fetch origin master
git rebase origin/master
```

### Too much noise from unrelated files

You probably forgot `--diff-branch`. Without it, xtask falls back to the
`github-pr-review` reporter which tries to fetch the PR diff from GitHub and
will fail locally.
