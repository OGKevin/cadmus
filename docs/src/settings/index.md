# Settings

Cadmus reads settings from `Settings/Settings-*.toml`.
Settings can be changed via **Main Menu → Settings**, which opens the built-in settings editor.

**Legend:**

- ✏️ Editable in the settings editor
- 🔑 Required for feature to work
- 🧪 Only available in test builds
- 📱 Kobo

## General Settings

### `keyboard-layout`

✏️

Keyboard layout to use for text input.

- Possible values: `"English"`, `"Russian"`.

```toml
keyboard-layout = "English"
```

### `sleep-cover`

✏️

Handle the magnetic sleep cover event.

```toml
sleep-cover = true
```

### `auto-share`

✏️

Automatically enter shared mode when connected to a computer, skipping the
"Share storage via USB?" prompt.

> [!TIP]
> Turn this on if you update Cadmus via USB often — you won't have to
> confirm the sharing dialog each time you plug in.

```toml
auto-share = false
```

### `auto-suspend`

✏️

Number of minutes of inactivity after which the device will automatically go to sleep.

- Zero means never.

```toml
auto-suspend = 30.0
```

### `auto-power-off`

✏️

Delay in days after which a suspended device will power off.

- Zero means never.

```toml
auto-power-off = 3.0
```

### `button-scheme`

✏️

Defines how the back and forward buttons are mapped to page forward and page backward actions.

- Possible values: `"natural"`, `"inverted"`.

```toml
button-scheme = "natural"
```

### `locale`

✏️

The preferred language for the user interface, using BCP 47 format (e.g., `"en-US"`, `"de-DE"`).

This setting is optional. When not set, `en-GB` is used.

```toml
locale = "en-GB"
```

## Reader

Settings that control the reading experience.

### `reader.finished`

✏️

What to do when you finish reading a book.

Possible values:

- `"notify"` (show a notification)
- `"close"` (close the book and go back)
- `"go-to-next"` (open the next book in the library).

```toml
[reader]
finished = "close"
```

## Libraries

✏️

Document library configuration. Each library has a name, path, and mode.

```toml
[[libraries]]
name = "On Board"
path = "/mnt/onboard"
mode = "database"
```

### `libraries.name`

✏️

Display name for the library.

### `libraries.path`

✏️

Directory path containing documents.

### `libraries.mode`

✏️

Library indexing mode.

- Possible values: `"database"`, `"filesystem"`.

### `libraries.finished`

✏️

Override the `reader.finished` setting for this specific library.
When set, this takes precedence over the global reader setting.

Possible values:

- `"notify"`
- `"close"`
- `"go-to-next"`.
- Leave unset to inherit the global `reader.finished` setting.

```toml
[[libraries]]
name = "KePub"
path = "/mnt/onboard/.kobo/kepub"
finished = "go-to-next"
```

## Intermissions

✏️

Defines the images displayed when entering an intermission state.

```toml
[intermissions]
suspend = "logo:"
power-off = "logo:"
share = "logo:"
```

### `intermissions.suspend`

✏️

Image displayed when the device enters sleep mode.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), or a path to a custom image file.

### `intermissions.power-off`

✏️

Image displayed when the device powers off.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), or a path to a custom image file.

### `intermissions.share`

✏️

Image displayed when entering USB sharing mode.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), or a path to a custom image file.

## OTA

The OTA feature downloads builds from GitHub.

Authentication for main branch and PR builds uses **GitHub device auth flow**.
When you select a build that requires authentication,
Cadmus will display a short code and a URL. Visit
`github.com/login/device` on any device, enter the code, and Cadmus will
automatically continue the download once you authorize.

The token is saved to disk after the first authorization so you will not be
prompted again on subsequent downloads.

For step-by-step instructions with screenshots, see the
[OTA updates](../installation/ota.md) guide.

## Telemetry

Cadmus writes JSON logs to disk. When the build enables the `otel` feature, it
can also export logs to an OpenTelemetry endpoint.

These settings are available in the **Settings → Telemetry** menu.

> [!IMPORTANT]
> Changes to these settings only take effect after
> restarting Cadmus. The application initializes telemetry on startup.

### `logging`

```toml
[logging]
enabled = true
level = "info"
max-files = 3
directory = "logs"
# otlp-endpoint = "https://otel.example.com:4318"
```

### `logging.enabled`

✏️

Enable or disable structured JSON logging.

```toml
[logging]
enabled = true
```

### `logging.level`

✏️

Minimum log level to record.

- Possible values: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`.

```toml
[logging]
level = "info"
```

### `logging.max-files`

Number of log files to keep. Only the most recent N files are kept — older ones
are deleted automatically when Cadmus starts.

- Default: `3`
- Set to `0` to keep all log files.

```toml
[logging]
max-files = 3
```

### `logging.otlp-endpoint`

✏️ (only when the `otel` feature is enabled)

Optional OTLP endpoint for exporting logs to an OpenTelemetry collector.

```toml
[logging]
otlp-endpoint = "https://otel.example.com:4318"
```

Environment override:

- `OTEL_EXPORTER_OTLP_ENDPOINT` takes precedence over `logging.otlp-endpoint`.

### `logging.enable-kern-log`

🧪 📱 ✏️

Captures kernel logs via `logread -F` and forwards them to structured logging
with the target `cadmus_core::logging:kern`.

```toml
[logging]
enable-kern-log = false
```

## Settings Retention

Cadmus stores each version's settings in a separate file in the `Settings/` directory (for example, `Settings-v1.2.3.toml`).
This ensures backward and forward compatibility when you upgrade.

### `settings-retention`

Number of recent version settings files to keep. Only the most recent N version files are kept. When a new version is saved, older versions beyond this limit are deleted automatically.

- Default: `3`
- Set to `0` to keep all version files

```toml
settings-retention = 3
```
