# Documentation Deployment

Cadmus documentation is deployed to Cloudflare Pages.

## URLs

- **Production**: <https://cadmus-dt6.pages.dev/>
- **PR Preview**: `https://pr-{NUMBER}.cadmus-dt6.pages.dev/`

## Reviewing Documentation Changes

When you open a pull request that modifies documentation files, a preview deployment is automatically created. The PR will show a deployment status with a link to the preview URL.

Preview URLs follow the pattern: `https://pr-{NUMBER}.cadmus-dt6.pages.dev/`

## Local Development

### Building and Serving

Build and serve documentation locally:

```bash
devenv shell
cargo xtask docs        # Build all documentation
cadmus-docs-serve       # Serve at http://localhost:1111
```

`cargo xtask docs` handles the full pipeline: installing Mermaid assets, building mdBook,
generating Rust API docs, and assembling the Zola portal. Pass `--mdbook-only` to skip the
Zola step when you only need to check the mdBook output.

To serve with live reload after building:

```bash
cd docs-portal && zola serve --base-url http://localhost
```

## Build Process

Documentation is built from three sources:

1. **mdBook** (`docs/`) - User and contributor guides
2. **Cargo doc** (`crates/`) - Rust API documentation
3. **Zola** (`docs-portal/`) - Documentation portal that combines everything

The build is orchestrated by `cargo xtask docs` (see `xtask/src/tasks/docs.rs`). The GitHub
Actions workflow (`.github/workflows/cadmus-docs.yml`) runs this command automatically on every
push to `main` and for every pull request.
