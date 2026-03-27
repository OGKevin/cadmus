---
name: build-kobo
description: Cross-compile Cadmus for Kobo e-reader devices (ARM Linux). Use this when asked to build a Kobo release, cross-compile for ARM, or prepare a device binary.
---

Use `cargo xtask build-kobo` to cross-compile Cadmus for Kobo devices.
This task is **Linux only** — the Linaro ARM toolchain consists of x86_64 Linux
ELF binaries that cannot run on macOS. Use Docker or a Linux VM on macOS.

## Basic usage

```sh
# Fast mode — downloads pre-built .so files (default)
cargo xtask build-kobo

# Slow mode — builds all thirdparty libraries from source (required for CI)
cargo xtask build-kobo --slow

# Skip library download entirely (when libs/ already exists)
cargo xtask build-kobo --skip

# Download thirdparty sources only, without building
cargo xtask build-kobo --download-only

# Build with specific Cargo feature flags
cargo xtask build-kobo --features test
```

## Build modes

| Mode                 | Flag                     | Description                                             |
| -------------------- | ------------------------ | ------------------------------------------------------- |
| Fast (default)       | _(none)_                 | Downloads pre-built `.so` files + MuPDF sources         |
| Slow                 | `--slow`                 | Builds all thirdparty libraries from source             |
| Slow + download only | `--slow --download-only` | Downloads all thirdparty sources without building       |
| Skip                 | `--skip`                 | Assumes `libs/` already exists; skips download entirely |

## What it does

1. Verifies the Linaro ARM toolchain is available on `PATH`
2. Downloads or builds thirdparty `.so` libraries into `libs/`
3. Builds the `mupdf_wrapper` C library for the ARM target
4. Runs `cargo build --release --target arm-unknown-linux-gnueabihf -p cadmus`

## Output

The compiled binary is written to:
`target/arm-unknown-linux-gnueabihf/release/cadmus`

## Prerequisites

- **Linux only** — exits with a clear error on macOS
- Linaro ARM toolchain on `PATH`: `arm-linux-gnueabihf-gcc`, `arm-linux-gnueabihf-ar`
  (provided by the devenv shell on Linux)
- Run `cargo xtask setup-native` first if MuPDF sources are not yet present
