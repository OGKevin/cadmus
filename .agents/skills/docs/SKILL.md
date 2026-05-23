---
name: docs
description: Build the full Cadmus documentation portal (mdBook + cargo doc + Zola). Use this when asked to build, preview, or update the documentation site.
---

Always use `cargo xtask docs` to build documentation in this project.

## Basic usage

```sh
# Build the complete documentation portal
cargo xtask docs

# Build only the mdBook output (skips Zola portal — faster for local preview)
cargo xtask docs --mdbook-only
```

## Output

The final portal is written to `docs-portal/public/`.

## Prerequisites

The following tools must be on `PATH` (all provided by the devenv shell):

- `mdbook`
- `mdbook-mermaid`
- `zola`
- `cargo`
- `git`
