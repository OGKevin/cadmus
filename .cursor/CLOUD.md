# Cursor Cloud — Cadmus dev environment

Applies only to Cursor Cloud agents. Coding conventions and testing policy:
[AGENTS.md](../AGENTS.md). Command how-tos: [.agents/skills/](../.agents/skills/).

## Layout

| File                                                                        | Role                                                                                                                                                                                                                   |
| --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [.cursor/Dockerfile](Dockerfile)                                            | Baked Ubuntu image: apt (incl. clang/gettext), rustup, Linaro, mdbook/mdbook-epub/mdbook-mermaid, cargo-nextest, Node. `ENV` pins match CI (`MDBOOK_*`, `NEXTEST_VERSION`, `NODE_VERSION`). No project `COPY`.         |
| [.cursor/cloud-install.sh](cloud-install.sh)                                | Idempotent boot install: submodules, host setup, assets/fonts, npm/EPUB/fetch.<br>Writes `/etc/profile.d/cadmus-cloud-env.sh`; ubuntu/root bashrc source it.                                                           |
| [.cursor/environment.json](environment.json)                                | Wires Dockerfile build (`context: ..`) and `install`; `agentCanUpdateSnapshot` lets setup agents promote snapshots.                                                                                                    |
| [.cursor/verify-cloud-install.sh](verify-cloud-install.sh)                  | Post-install assertions (used by CI).                                                                                                                                                                                  |
| [.github/workflows/cursor-cloud.yml](../.github/workflows/cursor-cloud.yml) | Path-filtered CI: `check-jsonschema`, `docker build`, `container-structure-test`, `cloud-install` + verify.                                                                                                            |

## Boot sequence

1. Cursor builds or restores from a snapshot/checkpoint based on [environment.json](environment.json).
2. The Dockerfile layer provides `/home/ubuntu` toolchain paths and global `ENV` pins.
3. [cloud-install.sh](cloud-install.sh) runs from the repo root (`install` in environment.json).
4. [cloud-install.sh](cloud-install.sh) writes `/etc/profile.d/cadmus-cloud-env.sh` and installs a managed block in `/home/ubuntu/.bashrc` and `/root/.bashrc` that sources it (login shells also pick up profile.d via `/etc/profile`).

Use `CADMUS_HOME=/home/ubuntu` in the image so root-driven snapshot builds find Dockerfile-installed tools under `/home/ubuntu/.local/bin`.

## Build environment variables

Defined in `/etc/profile.d/cadmus-cloud-env.sh` (managed by cloud-install). Non-login interactive shells load it via the managed bashrc snippet; if `SQLITE3_INCLUDE_DIR` is empty, run `source /etc/profile.d/cadmus-cloud-env.sh` (or open a login shell).

Before emulator smoke tests, confirm the vars are set (especially `SQLITE3_INCLUDE_DIR` and `DISPLAY=:1`), then run `DISPLAY=:1 cargo xtask run-emulator` from the repo root.

Exports include:

- `CADMUS_ROOT`, `SQLITE3_STATIC=1`, `SQLITE3_LIB_DIR`, `SQLITE3_INCLUDE_DIR`
- `PKG_CONFIG_PATH_x86_64_unknown_linux_gnu`, `PKG_CONFIG_PATH_arm_unknown_linux_gnueabihf`
- `SQLX_OFFLINE=true`, `PKG_CONFIG_ALLOW_CROSS=1`, `LIBCLANG_PATH` (detected via [libclang-path.sh](libclang-path.sh); matches CI apt action logic)
- `DISPLAY=:1`
- `PATH` includes `/home/ubuntu/.local/bin`, `/home/ubuntu/linaro-toolchain/bin`, `/usr/local/cargo/bin`

## Smoke bring-up

Cloud Agent terminals often run as **root** with a non-login interactive shell. If builds fail looking for custom SQLite headers, check `SQLITE3_INCLUDE_DIR`; when empty, `source /etc/profile.d/cadmus-cloud-env.sh`. Verify env before starting the emulator (`DISPLAY=:1 cargo xtask run-emulator`).

## Renovate pins

| Tool                                  | Where pinned                                                        | Renovate                                                                                                                                                                   |
| ------------------------------------- | ------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| mdbook / mdbook-epub / mdbook-mermaid | `.cursor/Dockerfile`, CI action, `cloud-install.sh` fallbacks       | `custom.regex` + `mdbook` group                                                                                                                                            |
| mdbook-i18n-helpers                   | `thirdparty/mdbook-i18n-helpers` git submodule (+ `devenv.nix` rev) | `git-submodules` + `mdbook-i18n-helpers` group; [cloud-install.sh](cloud-install.sh) and CI read `git rev-parse HEAD:thirdparty/mdbook-i18n-helpers` after submodules init |
| nextest / Node                        | Dockerfile + `cargo.yml`                                            | `custom.regex` groups                                                                                                                                                      |
| container-structure-test              | `cursor-cloud.yml`                                                  | `custom.regex`                                                                                                                                                             |

Do not Renovate-track: apt lists, floating rustup stable, Linaro 4.9.4 tarball URL.

## First-build recovery

If a fresh snapshot fails to compile, see the relevant skill:

- Custom SQLite: `cargo xtask setup --host` (cloud-install runs this; rerun manually if needed)
- EPUB: `build-cadmus-native` skill (`cargo xtask docs --mdbook-only`)
- Plato assets: `cargo xtask download-assets` if `bin/`, `resources/`, or `hyphenation-patterns/` are missing (cloud-install fetches these on first boot)
- Fonts: `cargo xtask download-fonts` (cloud-install runs this; rerun manually if `fonts/` is incomplete)

## Emulator

X server on `DISPLAY=:1`. `cargo xtask run-emulator` builds the EPUB if missing,
then launches the emulator — prefix with `DISPLAY=:1` from the workspace root.
See the `build-cadmus-native` skill for details.

### Visual smoke evidence

When sharing emulator screenshots or other ephemeral visual proof:

- **Do not** create GitHub Releases, tags, or release assets to host temporary images.
- **Do not** invent other temporary public hosting hacks for the same purpose.
- **Preferred delivery:** Cursor Cloud run artifacts, or attach images in chat to the operator. A normal PR comment with images is an acceptable last resort for in-repo sharing — still never a Release.
- **If evidence cannot be delivered cleanly:** stop, report the struggle clearly, and let the operator decide. Do not work around with Releases or similar.
