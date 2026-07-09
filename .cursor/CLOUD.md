# Cursor Cloud — Cadmus dev environment

Applies only to Cursor Cloud agents. Coding conventions and testing policy:
[AGENTS.md](../AGENTS.md). Command how-tos: [.agents/skills/](../.agents/skills/).

## Environment overview

- No Nix/devenv; toolchain is pre-baked in the snapshot (Rust, native libs,
  mdbook stack, cargo-nextest, Linaro ARM cross toolchain).
- VM boot runs the `install` script in [.cursor/environment.json](environment.json):
  `git submodule update --init --recursive`.

## Build environment variables

Exported in `~/.bashrc` — re-source in non-login shells:

- `SQLITE3_STATIC=1`, `SQLITE3_LIB_DIR`, `SQLITE3_INCLUDE_DIR`
- `PKG_CONFIG_PATH_x86_64_unknown_linux_gnu`, `SQLX_OFFLINE=true`
- Kobo cross: `~/linaro-toolchain/bin` on `PATH`, `PKG_CONFIG_ALLOW_CROSS=1`,
  `PKG_CONFIG_PATH_arm_unknown_linux_gnueabihf`

## First-build recovery

If a fresh snapshot fails to compile, see the relevant skill:

- Custom SQLite: `cargo xtask setup --host` (before any `cargo build`)
- EPUB: `build-cadmus-native` skill
- Kobo assets: `cargo xtask download-assets` if `bin/`, `resources/`,
  `hyphenation-patterns/` are missing

## Emulator

X server on `DISPLAY=:1`. `cargo xtask run-emulator` builds the EPUB if missing,
then launches the emulator — prefix with `DISPLAY=:1` from the workspace root.
See the `build-cadmus-native` skill for details.
