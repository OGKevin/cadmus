# Settings

Cadmus reads settings from `Settings.toml`.

## OTA

The OTA feature downloads builds from GitHub CI artifacts.

### `ota.github-token`

GitHub personal access token used to access CI artifacts.

- Required to use OTA.
- Configure it under the `[ota]` section.

```toml
[ota]
github-token = "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
```

## Logging

Cadmus writes JSON logs to disk. When the build enables the `otel` feature, it
can also export logs to an OpenTelemetry endpoint.

### `logging`

```toml
[logging]
enabled = true
level = "info"
max-files = 3
directory = "logs"
# otlp-endpoint = "https://otel.example.com:4318"
```

Environment overrides:

- `OTEL_EXPORTER_OTLP_ENDPOINT` takes precedence over `logging.otlp-endpoint`.
