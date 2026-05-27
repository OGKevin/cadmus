# Third-Party Libraries — Agent Context

## What this directory is

Build scripts and patches for cross-compiling C/C++ dependencies to ARM
(Kobo). Source code is **not** committed — it is downloaded at build time
via `cargo xtask build-kobo`.

## Build order

Libraries are built in dependency order as defined by `LIBRARY_NAMES` in
`xtask/src/tasks/util/thirdparty.rs`. A library later in the list may link
against libraries earlier in the list. Respect this ordering when adding new
entries.

## Per-library layout

Each subdirectory may contain:

- `build-kobo.sh` — invoked by xtask to configure and compile the library
  for `arm-linux-gnueabihf` with `-mcpu=cortex-a9 -mfpu=neon`.
- `kobo.patch` — applied before building. Used when upstream sources need
  modification for the cross-compilation environment.
- Additional patches named `*-kobo.patch` — when a library requires
  multiple patches (e.g. from different origins), each gets a descriptive
  name with a `-kobo` suffix.
- `README-kobo.md` — Kobo-specific notes: patch provenance, deviations
  from upstream, and build quirks for the cross-compilation target.
- `README-cadmus.md` — project-specific notes: why the library is needed,
  what Cadmus-specific modifications exist, and integration context.
- Additional files (e.g. `Makefile-kobo`, meson cross-file) when the
  upstream build system cannot be driven solely via environment variables.

## Patched libraries

Some libraries carry a `kobo.patch`. Common reasons for patching:

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

Library versions are defined as constants in
`xtask/src/tasks/util/thirdparty.rs`. This file is the **single source of
truth** for download URLs.

Version updates are managed via `renovate.json` — Renovate regex managers
match the version constants and open PRs when new upstream releases are
available. When adding a new library:

1. Add a version constant to `thirdparty.rs`.
2. Check `renovate.json` for an existing regex manager that would cover the
   new constant. If none exists, add one so the library receives automated
   update PRs.

## Rules

- Never commit downloaded source trees — only build scripts and patches.
- All builds target `arm-linux-gnueabihf` with `-mcpu=cortex-a9 -mfpu=neon`.
- Update the version constant in `xtask/src/tasks/util/thirdparty.rs` when
  upgrading — do not hard-code URLs in build scripts.
- Insert new libraries at the correct position in `LIBRARY_NAMES` (respecting
  the dependency chain).
- Prefer a `kobo.patch` over modifying `build-kobo.sh` to work around
  upstream issues — patches make the delta explicit and reviewable.
