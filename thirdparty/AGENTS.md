# Third-Party Libraries — Agent Context

Most subdirectories contain C/C++ dependencies cross-compiled for Kobo by
`crates/build-deps`: the `LIBRARY_NAMES` chain via `build/kobo/recipes.rs`, and
SQLite via `build/sqlite.rs`. Other submodules provide fonts, Rust libraries, or
documentation tooling and have different consumers.

## Build order

Libraries are built in dependency order as defined by `LIBRARY_NAMES` in
`crates/build-deps/src/versions.rs`. A library later in the list may link against
libraries earlier in the list. Respect this ordering when adding new entries.

## Per-library layout

Each subdirectory is a git submodule containing upstream source code. Build
patches and additional files are kept in `build-scripts/<lib>/`:

- Standard patch tiers — see [`build-scripts/AGENTS.md`](../build-scripts/AGENTS.md)
  and [`build-scripts/README.md`](../build-scripts/README.md):
  `generic/`, `native/` (`.gitkeep` when empty), and `kobo/`. Every patched
  library should use this layout even when only one tier has `.patch` files.
- `README-kobo.md` — Kobo-specific notes: patch provenance, deviations
  from upstream, and build quirks for the cross-compilation target.
- `README-cadmus.md` — project-specific notes: why the library is needed,
  what Cadmus-specific modifications exist, and integration context.
- Additional files (e.g. meson cross-file) when the upstream build system
  cannot be driven solely via environment variables.

## Patched libraries

Some libraries carry patches under `build-scripts/<lib>/generic/` and/or
`build-scripts/<lib>/kobo/`. Common reasons for patching:

- Replacing pkg-config dependency lookups with hard-coded paths to sibling
  thirdparty build directories (needed because pkg-config is unavailable
  during cross-compilation).
- Stripping build targets that are unnecessary on-device (e.g. CLI tools,
  data files) to reduce build time.
- Adding a device-specific cross-compilation target to the upstream build
  system.

When upgrading a patched library, verify the patch still applies cleanly
against the new source and regenerate it if the patched files changed
upstream.

## Version management and Renovate

Renovate monitors submodule commits via its `git-submodules` manager and opens
PRs when new upstream versions are available. When adding a submodule, add it
under `thirdparty/<name>` and set `branch` in `.gitmodules` to the release tag
or branch to track. That is enough for the submodule commit itself. If the same
revision is also pinned elsewhere (for example the `Cargo.toml` `[patch]` revs
or `devenv.nix`), add a regex manager and matching group in `renovate.jsonc`.

A Kobo C/C++ library additionally needs:

1. Build logic in `crates/build-deps/src/build/kobo/recipes.rs`.
2. Its name in `LIBRARY_NAMES` in `crates/build-deps/src/versions.rs`,
   respecting dependency order.

## Rules

- Never commit built artifacts — only source code via submodules and patches
  in `build-scripts/`.
- All Kobo C/C++ builds target `arm-linux-gnueabihf` with
  `-mcpu=cortex-a9 -mfpu=neon`.
- Record submodule upgrades as a new gitlink; edit `.gitmodules` only when its
  path, URL, or tracked version changes.
- Insert new Kobo C/C++ libraries at the correct position in `LIBRARY_NAMES`
  (respecting the dependency chain).
- Prefer a patch file over modifying build logic to work around upstream
  issues — patches make the delta explicit and reviewable.
