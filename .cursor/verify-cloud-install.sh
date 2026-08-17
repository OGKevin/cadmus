#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CADMUS_HOME="${CADMUS_HOME:-/home/ubuntu}"
export PATH="${CADMUS_HOME}/.local/bin:${CADMUS_HOME}/linaro-toolchain/bin:/usr/local/cargo/bin:${PATH}"

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
SQLITE_LIB="${ROOT}/target/cadmus-build-deps/${HOST_TRIPLE}/sqlite/lib/libsqlite3.a"
EPUB_PATH="${ROOT}/docs/book/epub/Cadmus Documentation.epub"
FONTS_DIR="${ROOT}/fonts"
ASSET_DIRS=(bin resources hyphenation-patterns)

for cmd in rustc cargo mdbook mdbook-epub mdbook-mermaid mdbook-gettext cargo-nextest node npm; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "missing command: $cmd" >&2
    exit 1
  fi
done

if [[ ! -f $SQLITE_LIB ]]; then
  echo "missing custom SQLite static library: $SQLITE_LIB" >&2
  exit 1
fi

if [[ ! -f $EPUB_PATH ]]; then
  echo "missing documentation EPUB: $EPUB_PATH" >&2
  exit 1
fi

if [[ ! -d $FONTS_DIR ]] || [[ -z "$(find "$FONTS_DIR" -maxdepth 1 -type f -name '*.ttf' -print -quit)" ]]; then
  echo "missing bundled fonts in: $FONTS_DIR" >&2
  exit 1
fi

for asset_dir in "${ASSET_DIRS[@]}"; do
  if [[ ! -d "${ROOT}/${asset_dir}" ]]; then
    echo "missing Plato asset directory: ${ROOT}/${asset_dir}" >&2
    exit 1
  fi
done

LIBCLANG_PATH="$(bash "${ROOT}/.cursor/libclang-path.sh")"
if [[ ! -d $LIBCLANG_PATH ]] || [[ -z "$(find "$LIBCLANG_PATH" -maxdepth 1 \( -name 'libclang.so' -o -name 'libclang-*.so' \) -print -quit)" ]]; then
  echo "missing libclang shared libraries in: $LIBCLANG_PATH" >&2
  exit 1
fi

if [[ ! -x "${CADMUS_HOME}/linaro-toolchain/bin/arm-linux-gnueabihf-gcc" ]]; then
  echo "missing Linaro ARM GCC" >&2
  exit 1
fi

MERMAID_DIR="${ROOT}/docs/src/mermaid-images"
expected_mermaid="$(grep -r '```mermaid' "${ROOT}/docs/src" --include='*.md' 2>/dev/null | wc -l)"
actual_mermaid="$(find "$MERMAID_DIR" -name '*.png' 2>/dev/null | wc -l)"
if [[ $expected_mermaid -gt 0 ]]; then
  if [[ $actual_mermaid -lt $expected_mermaid ]]; then
    echo "missing mermaid PNG renders for EPUB: expected ${expected_mermaid}, got ${actual_mermaid}" >&2
    echo "ensure Chrome headless deps are installed and PUPPETEER_ARGS is set for containers" >&2
    exit 1
  fi
fi

echo "Cursor Cloud install verification passed."
