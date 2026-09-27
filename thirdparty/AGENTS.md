# Third-Party Libraries — Agent Context

## What this directory is

Thirdparty C/C++ dependencies for cross-compiling to ARM (Kobo). Each
subdirectory is a git submodule tracked in `.gitmodules`. All build logic
lives in the `crates/build-deps` crate.

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

Thirdparty dependencies are tracked as git submodules in `.gitmodules`. Each
submodule pins a specific commit from the upstream repository.

Renovate monitors submodule commits via its `git-submodules` manager and
opens PRs when new upstream versions are available. When adding a new
library:

1. Add the submodule under `thirdparty/<name>` and pin it to a release branch
   or tag in `.gitmodules`.
2. Add build logic in `crates/build-deps/src/build/kobo/recipes.rs`.
3. Add the library name to `LIBRARY_NAMES` in `crates/build-deps/src/versions.rs`,
   respecting dependency order.

Renovate's `git-submodules` manager will automatically track the new
submodule — no manual Renovate configuration is required.

## Rules

- Never commit built artifacts — only source code via submodules and patches
  in `build-scripts/`.
- All builds target `arm-linux-gnueabihf` with `-mcpu=cortex-a9 -mfpu=neon`.
- Update the submodule commit in `.gitmodules` when upgrading a library.
- Insert new libraries at the correct position in `LIBRARY_NAMES` (respecting
  the dependency chain).
- Prefer a patch file over modifying build logic to work around upstream
  issues — patches make the delta explicit and reviewable.
