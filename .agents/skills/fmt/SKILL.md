---
name: fmt
description: Check or apply rustfmt formatting across the workspace. Use this when asked to format code, check formatting, or fix rustfmt issues.
---

Always use `cargo xtask fmt` to format or check formatting in this project —
never bare `cargo fmt`.

## Basic usage

```sh
# Check formatting (default — exits non-zero if any file would change)
cargo xtask fmt

# Apply formatting in place
cargo xtask fmt --apply
```

## When to use each mode

| Goal                   | Command                   |
| ---------------------- | ------------------------- |
| CI formatting gate     | `cargo xtask fmt`         |
| Fix formatting locally | `cargo xtask fmt --apply` |

## Notes

- Without `--apply` the command runs `cargo fmt --all --check`, which is the
  correct mode for CI.
- Always run `cargo xtask fmt --apply` before committing to avoid CI failures.
