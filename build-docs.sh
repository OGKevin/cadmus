#!/usr/bin/env bash

# Build documentation
echo "Building mdBook documentation..."
cd docs && mdbook build && cd ..

echo "Building Rust API documentation..."
cargo doc --no-deps --document-private-items

ln -sf "$(cargo metadata --format-version=1 --no-deps | jq -r '.target_directory')/doc" "docs-portal/static/api"
ln -sf "$(cargo metadata --format-version=1 --no-deps | jq -r '.workspace_root')/docs/book/html" "docs-portal/static/guide"

echo "Building Zola documentation portal..."
cd docs-portal
if [ "$CF_PAGES_BRANCH" = "main" ]; then
	zola build
else
	zola build --base-url "$CF_PAGES_URL"
fi

echo "✓ Documentation build complete"
