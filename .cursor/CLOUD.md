# Cursor Cloud — Cadmus dev environment

Applies only to Cursor Cloud agents. Coding conventions and testing policy:
[AGENTS.md](../AGENTS.md). Command how-tos: [.agents/skills/](../.agents/skills/).

## Layout

| File | Role |
| ---- | ---- |
| [.cursor/Dockerfile](Dockerfile) | Baked Ubuntu image: apt (incl. clang/gettext), rustup, Linaro, mdbook/mdbook-epub/mdbook-mermaid, cargo-nextest, Node. `ENV` pins match CI (`MDBOOK_*`, `NEXTEST_VERSION`, `NODE_VERSION`). No project `COPY`. |
| [.cursor/cloud-install.sh](cloud-install.sh) | Idempotent install on each agent boot: submodules, `cargo xtask ci install-doc-tools`, `cargo xtask setup --host`, `npm ci`, EPUB if missing, `cargo fetch`, `~/.bashrc` exports. |
| [.cursor/environment.json](environment.json) | Wires Dockerfile build (`context: ..`) and `install`; `agentCanUpdateSnapshot` lets setup agents promote snapshots. |
| [.cursor/verify-cloud-install.sh](verify-cloud-install.sh) | Post-install assertions (used by CI). |
| [.github/workflows/cursor-cloud.yml](../.github/workflows/cursor-cloud.yml) | Path-filtered CI: `check-jsonschema`, `docker build`, `container-structure-test`, `cloud-install` + verify. |

## Boot sequence

1. Cursor builds or restores from a snapshot/checkpoint based on [environment.json](environment.json).
2. The Dockerfile layer provides `/home/ubuntu` toolchain paths and global `ENV` pins.
3. [cloud-install.sh](cloud-install.sh) runs from the repo root (`install` in environment.json).
4. `~/.bashrc` gains a managed block with SQLite, `PKG_CONFIG`, `SQLX_OFFLINE`, `LIBCLANG_PATH`, `DISPLAY`, and `PATH`.

Use `CADMUS_HOME=/home/ubuntu` in the image so root-driven snapshot builds find Dockerfile-installed tools under `/home/ubuntu/.local/bin`.

## Build environment variables

Exported in `~/.bashrc` (managed by cloud-install) — re-source in non-login shells:

- `CADMUS_ROOT`, `SQLITE3_STATIC=1`, `SQLITE3_LIB_DIR`, `SQLITE3_INCLUDE_DIR`
- `PKG_CONFIG_PATH_x86_64_unknown_linux_gnu`, `PKG_CONFIG_PATH_arm_unknown_linux_gnueabihf`
- `SQLX_OFFLINE=true`, `PKG_CONFIG_ALLOW_CROSS=1`, `LIBCLANG_PATH=/usr/lib/llvm-18/lib`
- `DISPLAY=:1`
- `PATH` includes `/home/ubuntu/.local/bin`, `/home/ubuntu/linaro-toolchain/bin`, `/usr/local/cargo/bin`

## Renovate pins

| Tool | Where pinned | Renovate |
| ---- | ------------ | -------- |
| mdbook / mdbook-epub / mdbook-mermaid | `.cursor/Dockerfile`, CI action, `cloud-install.sh` fallbacks | `custom.regex` + `mdbook` group |
| mdbook-i18n-helpers | `thirdparty/mdbook-i18n-helpers` git submodule (+ `devenv.nix` rev) | `git-submodules` + `mdbook-i18n-helpers` group; [cloud-install.sh](cloud-install.sh) and CI read `git rev-parse HEAD:thirdparty/mdbook-i18n-helpers` after submodules init |
| nextest / Node | Dockerfile + `cargo.yml` | `custom.regex` groups |
| container-structure-test | `cursor-cloud.yml` | `custom.regex` |

Do not Renovate-track: apt lists, floating rustup stable, Linaro 4.9.4 tarball URL.

## First-build recovery

If a fresh snapshot fails to compile, see the relevant skill:

- Custom SQLite: `cargo xtask setup --host` (cloud-install runs this; rerun manually if needed)
- EPUB: `build-cadmus-native` skill (`cargo xtask docs --mdbook-only`)
- Kobo assets: `cargo xtask download-assets` if `bin/`, `resources/`, `hyphenation-patterns/` are missing (not part of cloud-install)

## Emulator

X server on `DISPLAY=:1`. `cargo xtask run-emulator` builds the EPUB if missing,
then launches the emulator — prefix with `DISPLAY=:1` from the workspace root.
See the `build-cadmus-native` skill for details.
