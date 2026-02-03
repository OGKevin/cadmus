---
description: "Settings documentation sync policy"
applyTo: "{contrib/Settings-sample.toml,crates/core/src/settings/*}"
---

# Settings documentation

The settings reference lives at `docs/src/settings/index.md`.

All locations must be kept in sync:

- `contrib/Settings-sample.toml` - Sample configuration file with examples
- `crates/core/src/settings/*` - Settings source code files

## Keep settings documentation current

- The documentation should match the actual source code definitions in `crates/core/src/settings/*`.
- If settings structures, fields, or defaults change, review `docs/src/settings/*` to confirm the documentation still matches the implementation.
