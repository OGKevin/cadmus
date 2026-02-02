# OpenTelemetry Integration

Cadmus supports exporting logs and traces to OpenTelemetry-compatible backends when built with the `otel` feature flag.

## Overview

The OpenTelemetry (OTEL) integration allows Cadmus to export structured logs and distributed traces to observability platforms like Grafana Loki/Tempo, or any OTLP-compatible service. This is useful for monitoring application behavior, debugging issues, and analyzing performance.

## Architecture

The telemetry system consists of two main components:

- **Logging**: JSON-structured logs written to disk via `tracing_subscriber`
- **OTLP Export**: Optional export of logs and traces to a remote OTLP endpoint

Each Cadmus run is assigned a unique Run ID (UUID v7) that ties together all logs and traces for that session.

## Building with OTEL Support

To enable OpenTelemetry, build Cadmus with the `otel` feature:

```bash
cargo build --features otel
```

## Configuration

### Settings File

Configure OpenTelemetry in your `Settings.toml`:

```toml
[logging]
enabled = true
level = "info"
max-files = 3
directory = "logs"
otlp-endpoint = "https://otel.example.com:4318"
```

#### Configuration Options

- **enabled**: Enable or disable logging entirely
- **level**: Minimum log level (`trace`, `debug`, `info`, `warn`, `error`)
- **max-files**: Number of log files to retain (0 = keep all)
- **directory**: Path to log directory (relative to installation directory)
- **otlp-endpoint**: OTLP HTTP endpoint URL (optional)

### Environment Variables

You can override the OTLP endpoint using an environment variable:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://otel.example.com:4318"
./cadmus
```

Environment variables take precedence over `Settings.toml` configuration.

### Log Level Control

The log level can be controlled via the `RUST_LOG` environment variable, which overrides the `level` setting:

```bash
# Enable debug logs for all modules
export RUST_LOG=debug
./cadmus

# Enable trace logs only for specific modules
export RUST_LOG=cadmus::view=trace,info
./cadmus
```

## Resource Attributes

Each telemetry export includes the following resource attributes:

- **service.name**: Always `cadmus`
- **service.version**: Git version from build metadata
- **cadmus.run_id**: Unique identifier for the application run
- **hostname**: System hostname

## Log File Format

Logs are written as newline-delimited JSON to files named:

```text
cadmus-<run_id>.json
```

Each log entry includes:

- **timestamp**: ISO 8601 formatted timestamp
- **level**: Log level (TRACE, DEBUG, INFO, WARN, ERROR)
- **target**: Module path where the log originated
- **fields**: Structured log data
- **spans**: Active tracing spans providing context
