#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CADMUS_HOME="${CADMUS_HOME:-/home/ubuntu}"
export PATH="${CADMUS_HOME}/.local/bin:${CADMUS_HOME}/linaro-toolchain/bin:/usr/local/cargo/bin:${PATH}"

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
SQLITE_LIB="${ROOT}/target/cadmus-build-deps/${HOST_TRIPLE}/sqlite/lib/libsqlite3.a"
EPUB_PATH="${ROOT}/docs/book/epub/Cadmus Documentation.epub"

for cmd in rustc cargo mdbook mdbook-epub mdbook-mermaid mdbook-gettext cargo-nextest node npm; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "missing command: $cmd" >&2
        exit 1
    fi
done

if [[ ! -f "$SQLITE_LIB" ]]; then
    echo "missing custom SQLite static library: $SQLITE_LIB" >&2
    exit 1
fi

if [[ ! -f "$EPUB_PATH" ]]; then
    echo "missing documentation EPUB: $EPUB_PATH" >&2
    exit 1
fi

if [[ ! -x "${CADMUS_HOME}/linaro-toolchain/bin/arm-linux-gnueabihf-gcc" ]]; then
    echo "missing Linaro ARM GCC" >&2
    exit 1
fi

echo "Cursor Cloud install verification passed."
