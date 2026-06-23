# Settings

Cadmus reads settings from `Settings/Settings-*.toml`.
Settings can be changed via **Main Menu → Settings**, which opens the built-in settings editor.

**Legend:**

- ✏️ Editable in the settings editor
- 🔑 Required for feature to work
- 🧪 Only available in test builds
- 📱 Kobo

## Example Full Config

<details>
<summary>Expand Me</summary>

<!-- i18n:skip-start -->

```toml
{{#include ../../../contrib/Settings-sample.toml}}
```

<!-- i18n:skip-end -->

</details>

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

### `auto-time`

✏️

Automatically synchronize the device time via NTP when WiFi connects. This will also set the correct timezone. Uses time.cloudflare.com and ipapi.co.

```toml
auto-time = false
```

### `auto-frontlight`

✏️

Automatically adjust the frontlight warmth and brightness based on the sun's position at the device's location.

- During the day warmth is at its minimum.
- Around sunrise and sunset warmth ramps gradually between zero and full.
- After sunset brightness is reduced to `auto-frontlight-night-brightness` and warmth stays at its maximum until sunrise.

Coordinates are auto-detected during each time sync (via ipapi.co) and stored in `auto-frontlight-last-coordinates`. Set `auto-frontlight-manual-coordinates` to override the detected location.

```toml
auto-frontlight = false
```

### `auto-frontlight-night-brightness`

✏️

Frontlight brightness level (0.0–100.0) applied when the sun is below the horizon.

This setting is optional. When not set, a default of `1.0` is used.

```toml
auto-frontlight-night-brightness = 10.0
```

### `auto-frontlight-manual-coordinates`

✏️

GPS coordinates `[latitude, longitude]` to use for sun-position calculations instead of the auto-detected location. Takes priority over `auto-frontlight-last-coordinates`.

This setting is optional.

```toml
auto-frontlight-manual-coordinates = [51.5074, -0.1278]
```

### `auto-frontlight-last-coordinates`

GPS coordinates `[latitude, longitude]` last detected during a time sync. Written automatically — do not edit this by hand; set `auto-frontlight-manual-coordinates` to override the location instead.

This setting is optional and managed automatically.

```toml
# auto-frontlight-last-coordinates = [48.8566, 2.3522]
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

### `startup-mode`

✏️

What to show when Cadmus starts.

- `"home"` — open the home screen (default).
- `"last-file"` — re-open the last book you were reading. If there is no
  unfinished book in the selected library, the home screen is shown instead.

```toml
startup-mode = "home"
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

### `reader.dithered-kinds`

✏️

File extensions rendered with dithering by default.

```toml
[reader]
dithered-kinds = ["cbz", "png", "jpg", "jpeg", "webp"]
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

Setting this to `"calendar:"` also enables the calendar refresh: every 5
minutes, the device wakes, shows the calendar, and then goes back to sleep
automatically.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), `"calendar:"` (built-in calendar), or a path to a custom image file.

### `intermissions.power-off`

✏️

Image displayed when the device powers off.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), or a path to a custom image file.

### `intermissions.share`

✏️

Image displayed when entering USB sharing mode.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), or a path to a custom image file.

## Import

These settings control how Cadmus imports documents from your device.
They are available in the **Settings → Import** menu.

Import scanning happens automatically on startup using incremental file checking — files are only re-scanned if their modification time or size has changed since the last import.

To trigger a full re-scan of all files regardless of cached values, use the **Force Full Import** action button in the Import settings category.

### `import.sync-metadata`

✏️

Re-extract metadata (title, author, etc.) whenever a document changes.

```toml
[import]
sync-metadata = true
```

### `import.metadata-kinds`

File extensions of documents whose metadata is extracted during import.

```toml
[import]
metadata-kinds = ["epub", "pdf", "djvu"]
```

### `import.allowed-kinds`

✏️

File extensions of documents considered during the import process.

```toml
[import]
allowed-kinds = ["djvu", "xps", "fb2", "txt", "pdf", "oxps", "cbz", "epub"]
```

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

Cadmus writes JSON logs to disk. When the build enables the `tracing` feature, it
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

✏️ (only when the `tracing` feature is enabled)

Optional OTLP endpoint for exporting logs to an OpenTelemetry collector.

```toml
[logging]
otlp-endpoint = "https://otel.example.com:4318"
```

Environment override:

- `OTEL_EXPORTER_OTLP_ENDPOINT` takes precedence over `logging.otlp-endpoint`.

### `logging.pyroscope-endpoint`

✏️ (only when the `profiling` feature is enabled)

Optional Pyroscope server URL for continuous profiling. When set, Cadmus starts
both a heap profiling agent (via jemalloc) and a CPU profiling agent (via
pprof) that push profiles to this endpoint.

```toml
[logging]
pyroscope-endpoint = "http://localhost:4040"
```

Environment override:

- `PYROSCOPE_SERVER_URL` takes precedence over `logging.pyroscope-endpoint`.

### `logging.enable-kern-log`

🧪 📱 ✏️

Captures kernel logs via `logread -F` and forwards them to structured logging
with the target `cadmus_core::logging:kern`.

```toml
[logging]
enable-kern-log = false
```

### `logging.enable-dbus-log`

🧪 📱 ✏️

Captures D-Bus signals via the built-in zbus-based DbusMonitorTask and forwards
them to structured logging.

```toml
[logging]
enable-dbus-log = false
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

### `db-backup-retention`

Number of database backups to keep. When a new backup is created and the total
would exceed this limit, the oldest backups are deleted automatically.

- Default: `2`
- Set to `0` to disable backups entirely.

See [Database Backup](../database-backup.md) for more details.

```toml
db-backup-retention = 2
```
