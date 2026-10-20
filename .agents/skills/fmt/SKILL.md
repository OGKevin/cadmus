---
name: fmt
description: Check or apply rustfmt formatting across the workspace. Use this when asked to format code, check formatting, or fix rustfmt issues.
---

Always use `cadmuscli fmt` to format or check formatting in this project —
never bare `cargo fmt`.

## Basic usage

```sh
# Check formatting (default — exits non-zero if any file would change)
cadmuscli fmt

# Apply formatting in place
cadmuscli fmt --apply
```

## When to use each mode

| Goal                   | Command                 |
| ---------------------- | ----------------------- |
| CI formatting gate     | `cadmuscli fmt`         |
| Fix formatting locally | `cadmuscli fmt --apply` |

## Notes

- Without `--apply` the command runs `cargo fmt --all --check`, which is the
  correct mode for CI.
- Always run `cadmuscli fmt --apply` before committing to avoid CI failures.
