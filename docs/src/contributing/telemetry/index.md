<!-- i18n:skip-start -->

# Telemetry

Cadmus has three related observability paths:

- `logging` for structured log export and local log files
- `tracing` for distributed tracing and OTLP export
- `profiling` for continuous profiling with Pyroscope

Use this section when you need to understand how those features fit together,
what each one depends on, and how to run them locally.

Cadmus assigns each run a unique Run ID using UUID v7. That Run ID ties
together local log files, OTLP exports, and profiling data for the same app
session.

## Pages

- [Logging](logging.md)
- [Tracing](tracing.md)
- [Profiling](profiling.md)

## Feature flags

- `tracing` enables structured logs, distributed traces, and OTLP export.
- `profiling` enables heap and CPU profiling with Pyroscope.
- `telemetry` enables both `tracing` and `profiling` together in the app.

## Architecture

The telemetry stack is split into three layers:

- Logging writes newline-delimited JSON to disk and can export log records over
  OTLP.
- Tracing creates spans around instrumented operations and exports them over
  OTLP.
- Profiling samples CPU and heap activity and pushes profiles to Pyroscope.

When `telemetry` is enabled, Cadmus runs all three paths together. In local
development this matches the default observability stack exposed by
`devenv up`.

## Local setup

The development environment includes a full observability stack. Use the
`cadmus-dev-otel` command to run the emulator with telemetry enabled.

<!-- i18n:skip-end -->
