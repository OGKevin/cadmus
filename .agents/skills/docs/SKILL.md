---
name: docs
description: Build the full Cadmus documentation website (mdBook + cargo doc + Storybook + Next.js). Use this when asked to build, preview, or update the documentation site.
---

Always use `cargo xtask docs` to build documentation in this project.

## Basic usage

```sh
# Build the complete documentation website
cargo xtask docs

# Build only the mdBook output (skips website build — faster for local preview)
cargo xtask docs --mdbook-only
```

## Output

The final website is written to `website/out/`.

## Prerequisites

The following tools must be on `PATH` (all provided by the devenv shell):

- `mdbook`
- `mdbook-mermaid`
- `npm` / `node`
- `cargo`
- `git`
