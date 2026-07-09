---
name: build-kobo
description: Cross-compile Cadmus for Kobo e-reader devices (ARM Linux). Use this when asked to build a Kobo release, cross-compile for ARM, or prepare a device binary.
---

Use `cargo xtask build-kobo` to cross-compile Cadmus for Kobo devices.
The Linaro ARM toolchain is supported on both Linux and macOS hosts.

## Basic usage

```sh
# Cross-compile for Kobo
cargo xtask build-kobo

# Build with specific Cargo feature flags
cargo xtask build-kobo --features test
```

## What it does

1. Verifies the Linaro ARM toolchain is available on `PATH`
2. Runs `cargo build --release --target arm-unknown-linux-gnueabihf -p cadmus`

Thirdparty dependencies (zlib, bzip2, libpng, libjpeg, openjpeg, jbig2dec, libwebp, freetype2, harfbuzz, gumbo, djvulibre, mupdf) are tracked as git submodules and built automatically by the `build-deps` crate's build.rs when needed. On a warm cache, no submodule initialisation or C/C++ compilation happens.

## Output

The compiled binary is written to:
`target/arm-unknown-linux-gnueabihf/release/cadmus`

## Prerequisites

- **Linux or macOS** — cross-compilation is supported on both platforms
- Linaro ARM toolchain on `PATH`: `arm-linux-gnueabihf-gcc`, `arm-linux-gnueabihf-ar`
  (provided by the devenv shell)
