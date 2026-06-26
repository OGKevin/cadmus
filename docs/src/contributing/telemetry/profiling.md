<!-- i18n:skip-start -->

# Profiling

Cadmus supports continuous profiling with Pyroscope when the `profiling`
feature is enabled.

## What it does

- Uses jemalloc heap profiling for allocation data
- Uses pprof for CPU profiling data
- Pushes both profile types to Pyroscope

## Feature flags

- `profiling` enables profiling support in the codebase
- `telemetry` enables profiling together with tracing

## Runtime behavior

Profiling is initialized during app startup and shut down during app exit.
If a Pyroscope endpoint is configured, Cadmus starts collecting profiles right
away so early startup work is included.

The app configures jemalloc as the global allocator when `profiling` is enabled
and enables heap profiling at process startup. CPU profiling runs alongside it
through the Pyroscope agent.

## Configuration

See the [settings reference](../../settings/index.md#logging) for the full
logging and profiling configuration.

- `logging.pyroscope-endpoint`
- `PYROSCOPE_SERVER_URL`

`PYROSCOPE_SERVER_URL` overrides `logging.pyroscope-endpoint` when both are
set.

To build Cadmus with profiling support:

```bash
cargo build --features profiling
```

To build Cadmus with tracing and profiling together:

```bash
cargo build --features telemetry
```

## Profile types

Cadmus currently exports:

- heap allocation profiles via jemalloc
- CPU profiles via pprof-rs

Both profile streams are pushed to the configured Pyroscope server.

## Local development

The `cadmus-dev-otel` command starts the emulator with profiling enabled and
the local Pyroscope service available at <http://localhost:4040>.

With the full devenv stack running, traces go to Tempo, logs go to Loki, and
profiles go to Pyroscope. That makes it possible to correlate a single run
across all three observability backends.

<!-- i18n:skip-end -->
