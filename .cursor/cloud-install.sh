#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CADMUS_HOME="${CADMUS_HOME:-/home/ubuntu}"
export HOME="${CADMUS_HOME}"
export PATH="${CADMUS_HOME}/.local/bin:${CADMUS_HOME}/linaro-toolchain/bin:/usr/local/cargo/bin:${PATH}"
export SQLX_OFFLINE=true
export PKG_CONFIG_ALLOW_CROSS=1
export PUPPETEER_ARGS="${PUPPETEER_ARGS:---no-sandbox --disable-setuid-sandbox}"

git submodule update --init --recursive

MDBOOK_I18N_HELPERS_REV="$(git rev-parse HEAD:thirdparty/mdbook-i18n-helpers)"
cargo xtask ci install-doc-tools \
  --mdbook-version "${MDBOOK_VERSION:-0.5.4}" \
  --mdbook-epub-rev "${MDBOOK_EPUB_REV:-21a1c8134134201a2d555313447c96e56e2a8996}" \
  --mdbook-mermaid-version "${MDBOOK_MERMAID_VERSION:-0.17.1}" \
  --mdbook-i18n-helpers-rev "${MDBOOK_I18N_HELPERS_REV}"

cargo xtask setup --host

LIBCLANG_PATH="$(bash "${ROOT}/.cursor/libclang-path.sh")"
export LIBCLANG_PATH

cargo xtask download-assets
cargo xtask download-fonts

HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"

if [[ -f package-lock.json ]]; then
  npm ci
fi

EPUB_PATH="${ROOT}/docs/book/epub/Cadmus Documentation.epub"
if [[ ! -f $EPUB_PATH ]]; then
  cargo xtask docs --mdbook-only
fi

cargo fetch

MARKER_START="# >>> cadmus-cloud-env >>>"
MARKER_END="# <<< cadmus-cloud-env <<<"
PROFILE_D="/etc/profile.d/cadmus-cloud-env.sh"
BASHRCS=("${HOME}/.bashrc" "/root/.bashrc")

python3 - "$PROFILE_D" "$MARKER_START" "$MARKER_END" "$ROOT" "$HOST_TRIPLE" "$LIBCLANG_PATH" "${BASHRCS[@]}" <<'PY'
import os
import pathlib
import subprocess
import sys

profile_d, start, end, root, host_triple, libclang_path = sys.argv[1:7]
bashrcs = sys.argv[7:]

profile_content = f"""# Managed by .cursor/cloud-install.sh — do not edit manually.
export CADMUS_ROOT="{root}"
export SQLITE3_STATIC=1
export SQLITE3_LIB_DIR="{root}/target/cadmus-build-deps/{host_triple}/sqlite/lib"
export SQLITE3_INCLUDE_DIR="{root}/target/cadmus-build-deps/{host_triple}/sqlite/include"
export PKG_CONFIG_PATH_x86_64_unknown_linux_gnu="{root}/target/cadmus-build-deps/x86_64-unknown-linux-gnu/sqlite/lib/pkgconfig"
export PKG_CONFIG_PATH_arm_unknown_linux_gnueabihf="{root}/target/cadmus-build-deps/arm-unknown-linux-gnueabihf/sqlite/lib/pkgconfig"
export SQLX_OFFLINE=true
export PKG_CONFIG_ALLOW_CROSS=1
export LIBCLANG_PATH="{libclang_path}"
export DISPLAY=:1
export PUPPETEER_ARGS="--no-sandbox --disable-setuid-sandbox"
export CADMUS_HOME="/home/ubuntu"
export PATH="$CADMUS_HOME/.local/bin:$CADMUS_HOME/linaro-toolchain/bin:/usr/local/cargo/bin:$HOME/.local/bin:$PATH"
export NVM_DIR="$CADMUS_HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"
"""

bashrc_block = f"""{start}
# Managed by .cursor/cloud-install.sh — do not edit manually.
[ -f {profile_d} ] && . {profile_d}
{end}
"""


def write_path(path: pathlib.Path, content: str) -> None:
    if os.geteuid() == 0:
        path.write_text(content)
        return
    subprocess.run(
        ["sudo", "tee", str(path)],
        input=content.encode(),
        check=True,
        stdout=subprocess.DEVNULL,
    )


def read_path(path: pathlib.Path) -> str:
    try:
        return path.read_text()
    except (FileNotFoundError, PermissionError, OSError):
        if os.geteuid() == 0:
            return ""
        result = subprocess.run(["sudo", "cat", str(path)], capture_output=True)
        if result.returncode != 0:
            return ""
        return result.stdout.decode()


write_path(pathlib.Path(profile_d), profile_content)

for bashrc in bashrcs:
    path = pathlib.Path(bashrc)
    text = read_path(path)
    if start in text and end in text:
        before, rest = text.split(start, 1)
        _, after = rest.split(end, 1)
        text = before + bashrc_block + after
    else:
        text = text.rstrip() + "\n\n" + bashrc_block + "\n"
    write_path(path, text)
PY
