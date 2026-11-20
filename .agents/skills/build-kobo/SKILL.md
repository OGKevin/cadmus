---
name: build-kobo
description: Cross-compile Cadmus for Kobo e-reader devices (ARM Linux). Use this when asked to build a Kobo release, cross-compile for ARM, or prepare a device binary.
---

Use `cadmuscli build-kobo` to cross-compile Cadmus for Kobo devices.
The Linaro ARM toolchain is supported on both Linux and macOS hosts.

## Basic usage

```sh
# Cross-compile for Kobo
cadmuscli build-kobo

# Build with specific Cargo feature flags
cadmuscli build-kobo --features test
```

## What it does

1. Verifies the Linaro ARM toolchain is available on `PATH`
2. Builds the custom ARM SQLite library when cold
3. Runs `cargo build --release --target arm-unknown-linux-gnueabihf -p cadmus`
   with the `kobo` feature plus any requested extras

Thirdparty dependencies are built by `crates/core/build.rs` through the
`build-deps` crate. Warm caches skip submodule initialization and C/C++
compilation.

## Output

The compiled binary is written to:
`target/arm-unknown-linux-gnueabihf/release/cadmus`

## Prerequisites

- **Linux or macOS** with the devenv-provided Linaro ARM tools on `PATH`:
  `arm-linux-gnueabihf-gcc` and `arm-linux-gnueabihf-ar`
- Generate the compile-time documentation and bundled-asset manifest inputs:

  ```bash
  cadmuscli docs --mdbook-only
  cadmuscli download-assets
  cadmuscli download-fonts
  ```

The documentation tools are supplied by devenv. `build-kobo` ensures ARM SQLite
automatically; a separate `cadmuscli setup --kobo` is optional.
