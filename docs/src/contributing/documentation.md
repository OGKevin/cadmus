# Documentation Deployment

Cadmus documentation is deployed to Cloudflare Pages.

## URLs

- **Production**: <https://cadmus-dt6.pages.dev/>
- **PR Preview**: `https://pr-{NUMBER}.cadmus-dt6.pages.dev/`

## Reviewing Documentation Changes

When you open a pull request that modifies documentation files, a preview deployment is automatically created. The PR will show a deployment status with a link to the preview URL.

Preview URLs follow the pattern: `https://pr-{NUMBER}.cadmus-dt6.pages.dev/`

## Local Development

### Prerequisites

The documentation uses [mdbook-mermaid](https://github.com/badboy/mdbook-mermaid) for Mermaid diagram support. You need to install the Mermaid JavaScript assets before building:

```bash
# Install mdbook-mermaid assets (required for Mermaid diagrams)
mdbook-mermaid install docs
```

This command downloads and installs the minified Mermaid JavaScript into the `docs/` directory. It only needs to be run once (or after updating the mdbook-mermaid version).

### Building and Serving

Build and serve documentation locally:

```bash
devenv shell
cadmus-docs-build    # Build all documentation
cadmus-docs-serve    # Serve at http://localhost:1111
```

Or manually:

```bash
# First, install mermaid assets (required)
mdbook-mermaid install docs

# Then build
cd docs && mdbook build && cd ..
cargo doc --no-deps --document-private-items
cd docs-portal && zola serve --base-url http://localhost
```

## Build Process

Documentation is built from three sources:

1. **mdBook** (`docs/`) - User and contributor guides
2. **Cargo doc** (`crates/`) - Rust API documentation
3. **Zola** (`docs-portal/`) - Documentation portal that combines everything

The GitHub Actions workflow (`.github/workflows/cadmus-docs.yml`) handles building and deploying automatically on every push to `main` and for every pull request.
