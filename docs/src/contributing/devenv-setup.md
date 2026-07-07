<!-- i18n:skip-start -->

# Development Environment Setup

Cadmus uses [devenv](https://devenv.sh/) with Nix to provide a reproducible development environment.
This guide covers setup on both Linux and macOS.

## Prerequisites

1. Install Nix with flakes enabled. The easiest way is using the [Determinate Nix Installer](https://github.com/DeterminateSystems/nix-installer).
2. Install [devenv](https://devenv.sh/getting-started).

## Quick Start

1. Clone the repository and enter the devenv shell:

   ```bash
   git clone https://github.com/OGKevin/cadmus.git
   cd cadmus
   devenv shell
   ```

2. Download the packaged runtime assets used by Kobo builds:

   ```bash
   cargo xtask download-assets
   ```

   > [!NOTE]
   > `cadmus-core` generates some compile-time metadata from the bundled asset
   > directories. For Kobo builds, make sure `bin/`, `resources/`, and
   > `hyphenation-patterns/` are present before `cargo xtask build-kobo` so the
   > generated asset list is complete.
   >
   > Thirdparty C/C++ dependencies (MuPDF, libwebp, zlib, etc.) are tracked as git
   > submodules and built automatically by `build.rs` when you run `cargo build` or
   > `cargo xtask run-emulator`. No separate setup step is required.

3. Run the emulator:

   ```bash
   cargo xtask run-emulator
   ```

## Available Commands

Once inside the devenv shell, these commands are available:

| Command                       | Description                                      |
| ----------------------------- | ------------------------------------------------ |
| `cargo xtask download-assets` | Download packaged Plato runtime assets           |
| `cargo xtask test`            | Run the test suite across the feature matrix     |
| `cargo xtask run-emulator`    | Run the emulator                                 |
| `cargo xtask build-kobo`      | Cross-compile for Kobo device                    |
| `cargo xtask dist`            | Assemble the Kobo distribution directory         |
| `cargo xtask bundle`          | Package KoboRoot.tgz for installation            |
| `cadmus-dev-otel`             | Run emulator with tracing and profiling enabled  |
| `devenv up`                   | Start observability stack (Grafana, Tempo, Loki) |
| `cargo xtask docs`            | Build documentation portal (mdBook + Cargo docs) |
| `cadmus-docs-serve`           | Serve documentation portal locally on port 1111  |
| `cadmus-translate`            | Generate the docs translation template (.pot)    |

Run `cargo xtask --help` to see all available subcommands, or `cargo xtask <cmd> --help` for
options on a specific command.

Or have a look at the rustdocs for `xtask` <a href="/api/xtask/">here</a>.

## Tasks

The devenv environment uses [tasks](https://devenv.sh/tasks/) to manage build dependencies.
Tasks are defined in `devenv.nix` and can be run with `devenv tasks run <task>`.

### Available Tasks

| Task         | Description                                               | Dependencies |
| ------------ | --------------------------------------------------------- | ------------ |
| `docs:build` | Build documentation EPUB (only rebuilds if files changed) | None         |
| `build:kobo` | Build for Kobo device                                     | `docs:build` |

All tasks delegate to `cargo xtask` under the hood.

### How Tasks Work

Tasks with dependencies automatically run their dependencies first. For example:

```bash
# This will first run docs:build (if needed), then build for Kobo
devenv tasks run build:kobo
```

The `docs:build` task uses `execIfModified` to only rebuild when documentation files have actually changed.

## Kobo Build Notes

- `cargo xtask download-assets` must run before `cargo xtask build-kobo`.
- OTA updates delete Cadmus-owned bundled files before reboot, then Kobo
  extracts the new `KoboRoot.tgz` over the install directory.
- User files outside the generated Cadmus-owned asset list must be preserved.

## Documentation Portal

Cadmus provides a unified documentation portal that combines user guides, API reference, and
contribution guides in one place.

### Building and Serving Locally

To build the documentation portal:

```bash
cargo xtask docs
```

This runs the full build pipeline:

1. Builds the mdBook user guide (`docs/book/html/`)
2. Generates Rust API documentation (`target/doc/`)
3. Builds the Zola landing page and integrates all documentation

To serve the portal locally with live reload:

```bash
cadmus-docs-serve
```

The portal will be available at <http://localhost:1111> with automatic rebuilds when you change
documentation files or Rust code.

### Documentation Structure

The portal provides three integrated sections:

- **Landing Page** (`/en/`, `/fr/`, …) - Overview and feature highlights
- **User Guide** (`/en/guide/`, `/fr/guide/`, …) - User-facing documentation from mdBook
- **API Reference** (`/en/api/cadmus_core/`) - Auto-generated Rust API documentation

All three sections are deployed as a single artifact to GitHub Pages at
<https://ogkevin.github.io/cadmus/>.

### Continuous Integration

Documentation is automatically built and validated on every pull request and deployed on push
to `main` or `master`. The CI pipeline checks:

- mdBook documentation compiles
- Rust code documentation is valid
- Zola landing page builds successfully

## Running Tests

Tests require the `TEST_ROOT_DIR` environment variable to be set. The easiest way to run the
full test matrix is:

```bash
cargo xtask test
```

This sets `TEST_ROOT_DIR` automatically and runs tests across all feature combinations. To run
a single feature combination:

```bash
cargo xtask test --features "emulator + test"
```

Or to run tests manually without xtask:

```bash
TEST_ROOT_DIR=$(pwd) cargo test
```

`TEST_ROOT_DIR` is automatically configured in CI but must be set manually when running
`cargo test` directly.

## Platform Support

### Linux (Full Support)

Linux provides full development capabilities including:

- Native development (emulator, tests)
- Cross-compilation for Kobo devices using the Linaro ARM toolchain
- Git hooks (actionlint, shellcheck, shfmt, markdownlint, prettier)

The Linaro toolchain is automatically added to `PATH` and provides `arm-linux-gnueabihf-*` commands.

### macOS (Full Support)

macOS supports full development capabilities including:

- Native development (emulator, tests)
- Cross-compilation for Kobo devices using the Linaro ARM toolchain
- Git hooks (actionlint, shellcheck, shfmt, markdownlint, prettier)

#### macOS-Specific Notes

**MuPDF build**: On macOS, the native build script manually gathers pkg-config CFLAGS for system
libraries because MuPDF's build system doesn't properly detect them on Darwin.

## Observability Stack

The devenv includes a full observability stack for development:

```bash
# Start all services
devenv up

# In another terminal, run the instrumented emulator
cadmus-dev-otel
```

Services available after `devenv up`:

| Service        | URL                     | Purpose                    |
| -------------- | ----------------------- | -------------------------- |
| Grafana        | <http://localhost:3000> | Dashboards and exploration |
| Tempo          | <http://localhost:3200> | Distributed tracing        |
| Loki           | <http://localhost:3100> | Log aggregation            |
| Prometheus     | <http://localhost:9090> | Metrics                    |
| OTLP Collector | <http://localhost:4318> | Telemetry ingestion        |
| Pyroscope      | <http://localhost:4040> | Continuous profiling       |

For more details on telemetry, see [Telemetry](telemetry/index.md).

## Troubleshooting

### Shell takes a long time to start

The first `devenv shell` invocation downloads and builds dependencies, which can take several
minutes. Subsequent invocations are cached and should be fast.

### Tests fail with "TEST_ROOT_DIR must be set"

Set the environment variable before running tests:

```bash
TEST_ROOT_DIR=$(pwd) cargo test
```

## Local Configuration

Create `devenv.local.nix` to override settings without modifying the tracked configuration:

```nix
{ pkgs, ... }:

{
  env = {
    # Example: Set TEST_ROOT_DIR automatically
    TEST_ROOT_DIR = builtins.getEnv "PWD";
  };
}
```

This file is gitignored and won't affect other contributors.

<!-- i18n:skip-end -->
