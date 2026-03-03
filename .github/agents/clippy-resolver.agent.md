---
name: clippy-resolver
description: Resolves Clippy warnings in PRs by fixing idiomatic Rust issues without using `allow` blocks, ensuring full build and test compliance
tools: ['github.vscode-pull-request-github/activePullRequest', 'github.vscode-pull-request-github/openPullRequest', 'todos', 'runSubagent']
---

# Rust Clippy Warning Resolver

Fix all Clippy warnings introduced in a PR without using `#[allow(...)]` blocks,
keeping the codebase idiomatic and CI-passing.

## Collect Warnings

```bash
# Warnings scoped to the PR diff
cargo xtask clippy --diff-branch origin/HEAD

# Full workspace
cargo clippy --all-targets --message-format=short --workspace
```

Also check inline PR review annotations posted by reviewdog.

## Fix Rules

- **Never add `#[allow(...)]`** — fix the root cause.
- **Change only what Clippy flags** — don't refactor unrelated code.
- **`field_reassign_with_default`**: when converting `let mut s = S::default(); s.field = val;`
  to a struct literal, keep `let mut` if the variable is mutated again later in the same scope,
  and ensure all subsequent mutations are preserved — do not drop them.

## Verify

```bash
cargo build --workspace --all-targets
cargo xtask test --features default
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

