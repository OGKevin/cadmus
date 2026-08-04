#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

git submodule update --init --recursive

cargo xtask setup --host

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
SQLITE_BASE="${ROOT}/target/cadmus-build-deps/${HOST_TRIPLE}/sqlite"

if [[ -f package-lock.json ]]; then
    npm ci
fi

EPUB_PATH="${ROOT}/docs/book/epub/Cadmus Documentation.epub"
if [[ ! -f "$EPUB_PATH" ]]; then
    cargo xtask docs --mdbook-only
fi

cargo fetch

BASHRC="${HOME}/.bashrc"
MARKER_START="# >>> cadmus-cloud-env >>>"
MARKER_END="# <<< cadmus-cloud-env <<<"

python3 - "$BASHRC" "$MARKER_START" "$MARKER_END" "$ROOT" "$HOST_TRIPLE" <<'PY'
import pathlib
import sys

bashrc, start, end, root, host_triple = sys.argv[1:6]
path = pathlib.Path(bashrc)
block = f"""{start}
# Managed by .cursor/cloud-install.sh — do not edit manually.
export CADMUS_ROOT="{root}"
export SQLITE3_STATIC=1
export SQLITE3_LIB_DIR="{root}/target/cadmus-build-deps/{host_triple}/sqlite/lib"
export SQLITE3_INCLUDE_DIR="{root}/target/cadmus-build-deps/{host_triple}/sqlite/include"
export PKG_CONFIG_PATH_x86_64_unknown_linux_gnu="{root}/target/cadmus-build-deps/x86_64-unknown-linux-gnu/sqlite/lib/pkgconfig"
export PKG_CONFIG_PATH_arm_unknown_linux_gnueabihf="{root}/target/cadmus-build-deps/arm-unknown-linux-gnueabihf/sqlite/lib/pkgconfig"
export SQLX_OFFLINE=true
export PKG_CONFIG_ALLOW_CROSS=1
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
export DISPLAY=:1
export PATH="$HOME/linaro-toolchain/bin:$HOME/.local/bin:/usr/local/cargo/bin:$PATH"
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
{end}
"""

text = path.read_text() if path.exists() else ""
if start in text and end in text:
    before, rest = text.split(start, 1)
    _, after = rest.split(end, 1)
    text = before + block + after
else:
    text = text.rstrip() + "\n\n" + block + "\n"

path.write_text(text)
PY
