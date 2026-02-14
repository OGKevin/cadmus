#!/usr/bin/env bash

set -e

# Build documentation
echo "Building mdBook documentation..."
cd docs && mdbook build && cd ..

echo "Building Rust API documentation..."
cargo doc --no-deps --document-private-items

ln -sf "$(cargo metadata --format-version=1 --no-deps | jq -r '.target_directory')/doc" "docs-portal/static/api"
ln -sf "$(cargo metadata --format-version=1 --no-deps | jq -r '.workspace_root')/docs/book/html" "docs-portal/static/guide"

echo "Building Zola documentation portal..."
cd docs-portal
zola build

echo "✓ Documentation build complete"
