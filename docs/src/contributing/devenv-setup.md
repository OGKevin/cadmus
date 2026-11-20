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

2. Build the custom SQLite library (required before any Cargo compile):

   ```bash
   cadmuscli setup
   ```

   > [!NOTE]
   > Cadmus needs a custom SQLite build with `DELETE … LIMIT` support.
   > `libsqlite3-sys` runs before `cadmus-core`'s `build.rs`, so these
   > artifacts must already exist. Default `cadmuscli setup` (no flags)
   > builds for the host and Kobo targets.
   >
   > Other thirdparty C/C++ dependencies (MuPDF, libwebp, zlib, etc.) are
   > tracked as git submodules and built automatically by `build.rs` when
   > you run `cargo build` or `cadmuscli run-emulator`. Library inventory
   > and per-library notes: [Third-party libraries](thirdparty/index.md).
   > Patch tiers and apply order: [Patch tiers](thirdparty/patches.md).

3. Download the packaged runtime assets used by Kobo builds:

   ```bash
   cadmuscli download-assets
   cadmuscli download-fonts
   ```

   > [!NOTE]
   > `cadmus-core` generates some compile-time metadata from the bundled asset
   > directories. For Kobo builds, make sure `bin/`, `resources/`,
   > `hyphenation-patterns/`, and `fonts/` are present before
   > `cadmuscli build-kobo` so the generated asset list is complete.

4. Run the emulator:

   ```bash
   cadmuscli run-emulator
   ```

## Emulator GitHub Authentication

Debug builds (`cadmuscli run-emulator`) can skip GitHub device flow by setting
`GH_TOKEN` to a personal access token with `public_repo` (or the broader `repo`)
scope:

```bash
export GH_TOKEN=ghp_…
cadmuscli run-emulator
```

Cadmus reads `GH_TOKEN` first, trims whitespace, and ignores an empty value. If
the environment token is missing or rejected, it falls back to the token saved
under the emulator install directory (the same `.github_token` file device flow
writes). Release builds never read `GH_TOKEN`.

## Available Commands

Once inside the devenv shell, these commands are available:

| Command                     | Description                                                                     |
| --------------------------- | ------------------------------------------------------------------------------- |
| `cadmuscli setup`           | Build custom SQLite for host and Kobo (run before compile)                      |
| `cadmuscli download-assets` | Download packaged Plato runtime assets                                          |
| `cadmuscli download-fonts`  | Download bundled font files into `fonts/`                                       |
| `cadmuscli test`            | Run the test suite across the feature matrix                                    |
| `cadmuscli run-emulator`    | Run the emulator                                                                |
| `cadmuscli build-kobo`      | Cross-compile for Kobo device                                                   |
| `cadmuscli dist`            | Assemble the Kobo distribution directory                                        |
| `cadmuscli bundle`          | Package KoboRoot.tgz for installation (`--no-nickel --xz` adds KoboRoot.tar.xz) |
| `cadmuscli list-devices`    | List Cadmus devices advertised on the LAN                                       |
| `cadmus-dev-otel`           | Run emulator with tracing and profiling enabled                                 |
| `cadmus-process-status`     | Print stack URLs, process state, and health probes                              |
| `devenv up`                 | Start the supervised stack (observability + docs site)                          |
| `devenv down`               | Stop the stack                                                                  |
| `devenv processes wait`     | Wait until every process is settled; readiness must be checked per process      |
| `devenv processes status`   | Show a process phase; use `devenv processes status <name>` to check for `ready` |
| `devenv processes attach`   | Stream process status and logs until Ctrl-C                                     |
| `cadmuscli docs`            | Build docs site (mdBook, API docs, website)                                     |
| `cadmus-docs-serve`         | Next dev server through `devenv up website`                                     |
| `cadmus-storybook`          | Storybook through `devenv up storybook`                                         |
| `cadmus-docs-mdbook`        | mdBook preview through `devenv up mdbook`                                       |
| `cadmus-docs-preview`       | `wrangler dev` for `website/out` (Workers, matches CI)                          |
| `cadmus-translate`          | Generate the docs translation template (.pot)                                   |
| `cadmus-test-coverage`      | Run tests with coverage instrumentation                                         |
| `cadmus-coverage-show`      | Open project-wide HTML coverage report                                          |
| `cadmus-coverage-diff`      | Open patch coverage HTML vs `origin/$CADMUS_DEFAULT_BRANCH`                     |
| `cadmus-opencode-check`     | Typecheck and test the OpenCode plugins                                         |
| `cadmus-sccache`            | Inspect or restart the sccache build cache daemon                               |

`devenv processes wait` returns when every process is settled. Settled includes
ready, exited, never-started, and `gave_up` processes; use
`devenv processes status <name>` to check whether a specific process is ready.

Run `cadmuscli --help` to see all available subcommands, or `cadmuscli <cmd> --help` for
options on a specific command. `cargo xtask` still works as a debug `cargo run` alias.
Entering the devenv shell installs a release `cadmuscli` onto `PATH` when `xtask/` sources
change or the binary is missing (`devenv tasks run cadmuscli:install`).

Devenv sets `CADMUS_WORKSPACE_ROOT` to the workspace root (same as
`TEST_ROOT_DIR`). Set it yourself only when running a PATH `cadmuscli`
outside devenv from outside the repository: it must be a Cadmus workspace
(`Cargo.toml` `[workspace]` whose `members` include `"xtask"`). Relative
paths resolve against the process current directory. A wrong value is an
error; `cadmuscli` does not search further. Inside the repo without devenv,
`CARGO_MANIFEST_DIR` (from the debug `cargo xtask` alias) or walking up from
cwd is enough.

Or have a look at the rustdocs for `xtask_lib` <a href="/api/xtask_lib/">here</a>.

## Tasks

The devenv environment uses [tasks](https://devenv.sh/tasks/) to manage build dependencies.
Tasks are defined in `devenv.nix` and can be run with `devenv tasks run <task>`.

### Available Tasks

| Task                | What it runs                                        | Dependencies        |
| ------------------- | --------------------------------------------------- | ------------------- |
| `cadmuscli:install` | `cargo build -p xtask --release` and link onto PATH | None                |
| `website:install`   | `npm install` in `website/`                         | None                |
| `opencode:install`  | `npm install` in `.opencode/`                       | None                |
| `docs:build`        | `cadmuscli docs --mdbook-only`                      | `cadmuscli:install` |
| `build:kobo`        | `cadmuscli build-kobo`                              | `docs:build`        |

`cadmuscli:install` builds the release binary; it does not call `cadmuscli`.
`website:install` and `opencode:install` only install npm dependencies.
`docs:build` and `build:kobo` invoke `cadmuscli`. `cadmuscli:install`,
`website:install` and `opencode:install` run before `devenv:enterShell`.
`cadmuscli:install` also runs before `docs:build`, and reruns when xtask
sources change or the PATH binary is missing.

### How Tasks Work

Tasks with dependencies automatically run their dependencies first. For example:

```bash
# This will first run docs:build (if needed), then build for Kobo
devenv tasks run build:kobo
```

`docs:build` uses `execIfModified` so it only rebuilds when documentation files
have changed. `website:install` uses `execIfModified` on `website/package.json`
and `website/package-lock.json`.

## Build Caching (sccache)

devenv sets `RUSTC_WRAPPER` to [sccache](https://github.com/mozilla/sccache), so
rustc output is cached in sccache's default local directory — `~/.cache/sccache`
on Linux, `~/Library/Caches/Mozilla.sccache` on macOS — capped at 50 GiB and
reused across builds, branches, and workspaces.

Incremental compilation is disabled (`CARGO_INCREMENTAL=0`). sccache cannot cache
incremental compiles, so leaving them on turns every dev build into a cache miss.

| Command                  | Purpose                                 |
| ------------------------ | --------------------------------------- |
| `cadmus-sccache stats`   | Advanced cache statistics               |
| `cadmus-sccache reset`   | Zero the counters                       |
| `cadmus-sccache restart` | Stop and start the daemon               |
| `cadmus-sccache log`     | Tail `.devenv/state/sccache-server.log` |

## Kobo Build Notes

- Run `cadmuscli setup` before host-side Cargo commands. `cadmuscli
build-kobo` also ensures the ARM SQLite artifacts, but running setup up
  front covers both native and device paths.
- `cadmuscli download-assets` and `cadmuscli download-fonts` must run before `cadmuscli build-kobo`.
- `cadmuscli docs` must run before `cadmuscli dist` or `cadmuscli kobo deploy`:
  `dist` copies `website/out` into `dist/website` and refuses to assemble any
  distribution without it. The shared Rust API docs (`_shared/api`) are stored
  there as `.gz` files only (about 22 MB instead of 131 MB); the device serves
  them with `Content-Encoding: gzip`, or inflates them for clients that do not
  accept gzip. `website/out` itself stays uncompressed for the hosted site.
- OTA updates delete Cadmus-owned bundled files before reboot, then Kobo
  extracts the new `KoboRoot.tgz` over the install directory.
- User files outside the generated Cadmus-owned asset list must be preserved.

See [Website](website/index.md) and [User guide](website/user-guide.md) for building, serving,
editing, and deploying the documentation site.

## Running Tests

Tests require the `TEST_ROOT_DIR` environment variable to be set. The easiest way to run the
full test matrix is:

```bash
cadmuscli test
```

This sets `TEST_ROOT_DIR` automatically and runs tests across all feature combinations. To run
a single feature combination:

```bash
cadmuscli test --features "emulator + test"
```

Or to run tests manually without cadmuscli:

```bash
TEST_ROOT_DIR=$(pwd) cargo test
```

`TEST_ROOT_DIR` is automatically configured in CI but must be set manually when running
`cargo test` directly.

## Test Coverage

CI runs instrumented tests across the full feature matrix and uploads merged reports to
[Codecov](https://app.codecov.io/github/ogkevin/cadmus). Patch coverage is enforced as a required status check — the
`codecov/patch` check must pass (auto target with 10% threshold) for PRs to be merged. Project
coverage remains informational with a 10% threshold.

CI also uploads nextest JUnit XML to [Codecov Test Analytics](https://docs.codecov.com/docs/test-analytics)
(one upload per feature-matrix shard, with Codecov flags). Doctests are not included in
JUnit output. View results in the Codecov Tests tab.

Locally, use the devenv coverage commands (requires `devenv shell`):

```bash
# 1. Run instrumented tests (writes target/coverage/lcov.info)
cadmus-test-coverage

# 2. View results without re-running tests
cadmus-coverage-show              # project-wide HTML in the browser
cadmus-coverage-diff              # patch HTML vs origin/$CADMUS_DEFAULT_BRANCH
cadmus-coverage-diff main         # or pass an explicit base branch
```

`CADMUS_DEFAULT_BRANCH` defaults to `master`. Override it in `devenv.local.nix` if needed.

Outside devenv, the equivalent cadmuscli invocation is:

```bash
cadmuscli test --coverage --features emulator
```

A device feature (`emulator`, `kobo`, or `deviceless`) is **required** to
compile `cadmus-core` — plain `cargo test` and `--features default` fail with
`compile_error!("A device feature must be enabled")`. Use `emulator` on the
host; use `kobo` only via the `build-kobo` skill (cross-compiles for ARM).

CI uploads require a `CODECOV_TOKEN` repository secret (one-time setup on codecov.io).

## Platform Support

### Linux (Full Support)

Linux provides full development capabilities including:

- Native development (emulator, tests)
- Cross-compilation for Kobo devices using the Linaro ARM toolchain
- Git hooks (actionlint, shellcheck, shfmt, rumdl, prettier)

The Linaro toolchain is automatically added to `PATH` and provides `arm-linux-gnueabihf-*` commands.

### macOS (Full Support)

macOS supports full development capabilities including:

- Native development (emulator, tests)
- Cross-compilation for Kobo devices using the Linaro ARM toolchain
- Git hooks (actionlint, shellcheck, shfmt, rumdl, prettier)

#### macOS-Specific Notes

**MuPDF build**: On macOS, the native build script manually gathers pkg-config CFLAGS for system
libraries because MuPDF's build system doesn't properly detect them on Darwin.

**Rust source path**: devenv only derives `RUST_SRC_PATH` when it builds the
toolchain itself, but the macOS toolchain is an override (`darwinRustToolchainFix`
in `devenv.nix`). The override therefore includes the `rust-src` component and
forces `RUST_SRC_PATH` to its merged sources, which `rust-analyzer` and
`cargo doc` need.

## Observability Stack

The devenv includes a full observability stack for development:

```bash
# Start all services in the foreground
devenv up

# In another terminal, check readiness, URLs, and health
cadmus-process-status --wait

# Then run the instrumented emulator
cadmus-dev-otel
```

`cadmus-dev-otel` checks that every observability process reports ready and
exits with an error if one does not.

Every service port is allocated by devenv. When a base port is already taken
(another worktree, `next dev`, a stray listener), devenv moves the service and
every consumer — service configs, Grafana datasources, probes, and the `*_URL`
environment variables — to the next free port together. devenv reserves the
ports while it evaluates the configuration and releases those reservations
when it launches the process manager, so a conflict appearing after that
release can still fail a service's startup.

| Service        | Base port | Purpose                    | Environment variable          |
| -------------- | --------- | -------------------------- | ----------------------------- |
| Grafana        | 3000      | Dashboards and exploration | `GRAFANA_URL`                 |
| Tempo          | 3200      | Distributed tracing        | `TEMPO_URL`                   |
| Loki           | 3100      | Log aggregation            | `LOKI_URL`                    |
| Prometheus     | 9090      | Metrics                    | `PROMETHEUS_URL`              |
| OTLP Collector | 4318      | Telemetry ingestion        | `OTEL_EXPORTER_OTLP_ENDPOINT` |
| Pyroscope      | 4040      | Continuous profiling       | `PYROSCOPE_SERVER_URL`        |

Base ports apply when no manager is running. The resolved ports come from the
shell banner, `cadmus-process-status`, or the environment variables — do not
hardcode them in bookmarks or scripts. A shell entered before the manager
started still reports the base ports: `cadmus-process-status` and
`cadmus-dev-otel` re-evaluate against the running manager themselves, and
other consumers follow the refresh recipe in
[Ports change between runs](#ports-change-between-runs).

### Docs site processes

The documentation site tools are supervised processes too, so they restart on
crash and report readiness:

| Process     | Base port | Start it with                              |
| ----------- | --------- | ------------------------------------------ |
| `website`   | 3001      | `devenv up website` / `cadmus-docs-serve`  |
| `mdbook`    | 3002      | `devenv up mdbook` / `cadmus-docs-mdbook`  |
| `storybook` | 6006      | `devenv up storybook` / `cadmus-storybook` |

`devenv up` (no argument) starts these alongside the observability stack.
`devenv up <name>` starts just that process and follows its output; add `-d`
to run detached.

The website listens on port 3001 because Grafana keeps port 3000.

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

### `libsqlite3-sys` fails with `sqlite3.h` not found

Custom SQLite has not been built yet. Inside `devenv shell`, run:

```bash
cadmuscli setup
```

Then retry the build or `cadmuscli run-emulator`.

### A stack process never becomes ready

`devenv processes status <name>` prints the phase (`starting`, `ready`,
`exited`, `gave_up`, …). Logs live under `$DEVENV_STATE/logs/`:

```bash
devenv processes logs <name> --lines 200
# or
tail -f "$DEVENV_STATE/logs/<name>.stdout.log"
```

`cadmus-process-status` reports every process phase and probes each health
endpoint, naming the process and log file on failure.

Loki's `/ready` returns 503 for the first ~15 seconds after an idle period
("Ingester not ready"); its probe allows six failures at a five-second period
before marking it unhealthy. Pyroscope's readiness probe is its `/ready`
endpoint: the metastore, segment-writer and ingester rolling-update waits are
disabled (safe for a single instance). Pyroscope records its allocated Raft
address and preserves the metastore index and Raft state across same-address
restarts; when the allocator moves the port, the incompatible index/Raft
state is removed once on the next start (profile data is separate and stays),
after which `/ready` turns 200 within seconds.

### Ports change between runs

Ports are stable only while the process manager runs. Starting or stopping
`devenv up` makes direnv reload the shell (devenv watches the manager pid
file), so `$GRAFANA_URL`, `$LOKI_URL`, … follow the running stack. Without a
manager, the environment reports the base ports.

Shells without the automatic direnv reload (or entered before the manager
started) keep stale values: `cadmus-process-status` and `cadmus-dev-otel`
re-evaluate against the running manager when you run them, but every other
consumer needs a refresh — re-enter the devenv shell (`direnv reload` or
`devenv shell`), or assign individual variables:

```bash
LOKI_URL="$(devenv shell -- printenv LOKI_URL)"
TEMPO_URL="$(devenv shell -- printenv TEMPO_URL)"
```

If the process manager is replaced while `cadmus-process-status` assembles its
report or while `cadmus-dev-otel` builds the docs, the command stops with a
retry message rather than using potentially stale ports. Run it again after
the stack settles. Both commands bound their process wait to 120 seconds so
they can report a failure instead of waiting indefinitely.

### `grafana` / `tempo` / `loki` / `pyroscope` are not on `PATH`

The service CLIs are no longer installed into the shell. Debug through
`cadmus-process-status` and the process logs, or fetch a CLI ad hoc with
`nix shell nixpkgs#tempo`.

## Local Configuration

Create `devenv.local.nix` to override settings without modifying the tracked configuration:

```nix
{ pkgs, ... }:

{
  env = {
    # Example: Set TEST_ROOT_DIR automatically
    TEST_ROOT_DIR = builtins.getEnv "PWD";

    # Default device for `cadmuscli kobo deploy` and `cadmuscli kobo restart`.
    # An explicit `--ip` still wins.
    KOBO_IP = "192.168.1.128";
  };
}
```

This file is gitignored and won't affect other contributors. After adding
`KOBO_IP`, re-enter the devenv shell (`direnv reload` or `devenv shell`) and
the `kobo` commands can run without `--ip`.

<!-- i18n:skip-end -->
