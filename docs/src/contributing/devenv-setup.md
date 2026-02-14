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

2. Run the one-time setup to build native dependencies:

   ```bash
   cadmus-setup-native
   ```

3. Run the emulator:

   ```bash
   ./run-emulator.sh
   ```

## Available Commands

Once inside the devenv shell, these commands are available:

| Command               | Description                                      |
| --------------------- | ------------------------------------------------ |
| `cadmus-setup-native` | Build MuPDF for native development (run once)    |
| `cargo test`          | Run the test suite                               |
| `./run-emulator.sh`   | Run the emulator                                 |
| `cadmus-build-kobo`   | Build for Kobo device (Linux only)               |
| `cadmus-dev-otel`     | Run emulator with OpenTelemetry instrumentation  |
| `devenv up`           | Start observability stack (Grafana, Tempo, Loki) |

## Tasks

The devenv environment uses [tasks](https://devenv.sh/tasks/) to manage build dependencies.
Tasks are defined in `devenv.nix` and can be run with `devenv tasks run <task>`.

### Available Tasks

| Task          | Description                                               | Dependencies |
| ------------- | --------------------------------------------------------- | ------------ |
| `docs:build`  | Build documentation EPUB (only rebuilds if files changed) | None         |
| `deps:native` | Build MuPDF and wrapper for native development            | None         |
| `build:kobo`  | Build for Kobo device (Linux only)                        | `docs:build` |

### How Tasks Work

Tasks with dependencies automatically run their dependencies first. For example:

```bash
# This will first run docs:build (if needed), then build for Kobo
devenv tasks run build:kobo
```

The `docs:build` task uses `execIfModified` to only rebuild when documentation files have actually changed.

## Running Tests

Tests require the `TEST_ROOT_DIR` environment variable to be set:

```bash
TEST_ROOT_DIR=$(pwd) cargo test
```

This is automatically configured in CI but must be set manually for local testing.

## Platform Support

### Linux (Full Support)

Linux provides full development capabilities including:

- Native development (emulator, tests)
- Cross-compilation for Kobo devices using the Linaro ARM toolchain
- Git hooks (actionlint, shellcheck, shfmt, markdownlint, prettier)

The Linaro toolchain is automatically added to `PATH` and provides `arm-linux-gnueabihf-*` commands.

### macOS (Native Development Only)

macOS supports native development but has some limitations:

| Feature           | Status        | Notes                          |
| ----------------- | ------------- | ------------------------------ |
| Native builds     | Supported     | Emulator and tests work        |
| Cross-compilation | Not supported | Linaro toolchain is Linux-only |

#### macOS-Specific Notes

**Cross-compilation for Kobo**: The Linaro ARM cross-compilation toolchain consists of x86_64 Linux
ELF binaries that cannot run on macOS. To build for Kobo devices on macOS, use Docker with a Linux
container or a Linux VM.

**MuPDF build**: On macOS, the native setup script manually gathers pkg-config CFLAGS for system
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

For more details on telemetry, see [OpenTelemetry Integration](telemetry.md).

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
