---
name: setup-native
description: Build MuPDF and the mupdf_wrapper C library for native development. Use this when setting up a local dev environment, after a clean checkout, or when MuPDF sources are missing or stale.
---

Use `cargo xtask setup-native` to prepare the native build environment.
This must be run before `cargo test`, `cargo build`, or `cargo xtask run-emulator`
on a fresh checkout or after the required MuPDF version changes.

## Basic usage

```sh
# Download MuPDF sources and build the native wrapper (idempotent)
cargo xtask setup-native

# Force a re-download of MuPDF sources even if the correct version is present
cargo xtask setup-native --force
```

## What it does

1. Downloads MuPDF sources at the pinned version (`1.27.0`) into `thirdparty/mupdf/`
   if not already present or if the version does not match
2. Builds the `mupdf_wrapper` C static library for the native platform
3. Compiles MuPDF using system libraries
4. Creates symlinks in `target/mupdf_wrapper/<platform>/` so the Rust build
   script can find the static libraries

## Platform notes

- **macOS**: collects system library CFLAGS via `pkg-config` automatically
- **Linux**: MuPDF's build system detects system libraries automatically
- Cross-compilation for Kobo (ARM) is handled separately by `cargo xtask build-kobo`

## When to run

| Situation            | Command                            |
| -------------------- | ---------------------------------- |
| Fresh checkout       | `cargo xtask setup-native`         |
| MuPDF version bumped | `cargo xtask setup-native --force` |
| Sources corrupted    | `cargo xtask setup-native --force` |

## Prerequisites

The following tools must be on `PATH` (all provided by the devenv shell):

- `make`
- `pkg-config`
- `ar`
- System libraries: `freetype2`, `harfbuzz`, `libopenjp2`, `libjpeg`, `zlib`,
  `jbig2dec`, `gumbo` (macOS: install via Homebrew; Linux: install via apt)
