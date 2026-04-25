# Logging

Cadmus writes structured JSON logs to disk and can export logs to an OTLP
backend when the `tracing` feature is enabled.

## What it does

- Writes newline-delimited JSON log files to the configured log directory
- Adds run metadata so a single app session can be traced through log output
- Optionally exports logs to an OpenTelemetry collector

## Feature flag

- `tracing` enables OTLP log export and shared tracing/logging context

## Log file format

Cadmus writes newline-delimited JSON files named like this:

```text
cadmus-<run_id>.json
```

Each record includes these fields:

- `timestamp`
- `level`
- `target`
- `fields`
- `spans`

The `spans` array carries active tracing context so log records can be matched
back to the traced operation that emitted them.

## Resource attributes

When OTLP export is enabled, log records include the same resource metadata as
traces:

- `service.name = cadmus`
- `service.version = <git describe output>`
- `cadmus.run_id = <uuid-v7>`
- `hostname = <system hostname>`

## Configuration

See the [settings reference](../../settings/index.md#logging) for the full
logging configuration. The main options are:

- `logging.enabled`
- `logging.level`
- `logging.max-files`
- `logging.directory`
- `logging.otlp-endpoint`

`OTEL_EXPORTER_OTLP_ENDPOINT` overrides `logging.otlp-endpoint` when both are
set.

`RUST_LOG` overrides the configured log level. This is useful when you need
trace-level output for a single subsystem without editing `Settings.toml`.

```bash
# Enable debug logs globally
RUST_LOG=debug cargo run --features tracing

# Enable trace logs for a specific module
RUST_LOG=cadmus_core::view=trace,info cargo run --features tracing
```

## Runtime behavior

When `tracing` is enabled, log export is initialized during app startup and
shut down during app exit.

Cadmus always writes local JSON logs. OTLP export is an additional sink layered
on top when an endpoint is configured.

## Related docs

- [Tracing](tracing.md)
- [Profiling](profiling.md)
