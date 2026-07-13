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
{{ #include ../../../contrib/Settings-sample.toml}}
```

<!-- i18n:skip-end -->

</details>

## General Settings

### `keyboard-layout`

✏️

Keyboard layout to use for text input.

- Possible values: `"English"`, `"Russian"`.

<!-- i18n:skip-start -->

```toml
keyboard-layout = "English"
```

<!-- i18n:skip-end -->

### `sleep-cover`

✏️

Handle the magnetic sleep cover event.

<!-- i18n:skip-start -->

```toml
sleep-cover = true
```

<!-- i18n:skip-end -->

### `auto-share`

✏️

Automatically enter shared mode when connected to a computer, skipping the
"Share storage via USB?" prompt.

> [!TIP]
> Turn this on if you update Cadmus via USB often — you won't have to
> confirm the sharing dialog each time you plug in.

<!-- i18n:skip-start -->

```toml
auto-share = false
```

<!-- i18n:skip-end -->

### `auto-time`

✏️

Automatically synchronize the device time via NTP when WiFi connects. This will also set the correct timezone. Uses
time.cloudflare.com and ipapi.co.

<!-- i18n:skip-start -->

```toml
auto-time = false
```

<!-- i18n:skip-end -->

### `auto-frontlight`

✏️

Automatically adjust the frontlight warmth and brightness based on the sun's position at the device's location.

- During the day warmth is at its minimum.
- Around sunrise and sunset warmth ramps gradually between zero and full.
- After sunset brightness is reduced to `auto-frontlight-night-brightness` and warmth stays at its maximum until
  sunrise.

Coordinates are auto-detected during each time sync (via ipapi.co) and stored in `auto-frontlight-last-coordinates`. Set
`auto-frontlight-manual-coordinates` to override the detected location.

<!-- i18n:skip-start -->

```toml
auto-frontlight = false
```

<!-- i18n:skip-end -->

### `auto-frontlight-night-brightness`

✏️

Frontlight brightness level (0.0–100.0) applied when the sun is below the horizon.

This setting is optional. When not set, a default of `1.0` is used.

<!-- i18n:skip-start -->

```toml
auto-frontlight-night-brightness = 10.0
```

<!-- i18n:skip-end -->

### `auto-frontlight-manual-coordinates`

✏️

GPS coordinates `[latitude, longitude]` to use for sun-position calculations instead of the auto-detected location.
Takes priority over `auto-frontlight-last-coordinates`.

This setting is optional.

<!-- i18n:skip-start -->

```toml
auto-frontlight-manual-coordinates = [51.5074, -0.1278]
```

<!-- i18n:skip-end -->

### `auto-frontlight-last-coordinates`

GPS coordinates `[latitude, longitude]` last detected during a time sync. Written automatically — do not edit this by
hand; set `auto-frontlight-manual-coordinates` to override the location instead.

This setting is optional and managed automatically.

<!-- i18n:skip-start -->

```toml
# auto-frontlight-last-coordinates = [48.8566, 2.3522]
```

<!-- i18n:skip-end -->

### `auto-suspend`

✏️

Number of minutes of inactivity after which the device will automatically go to sleep.

- Zero means never.

<!-- i18n:skip-start -->

```toml
auto-suspend = 30.0
```

<!-- i18n:skip-end -->

### `auto-power-off`

✏️

Delay in days after which a suspended device will power off.

- Zero means never.

<!-- i18n:skip-start -->

```toml
auto-power-off = 3.0
```

<!-- i18n:skip-end -->

### `button-scheme`

✏️

Defines how the back and forward buttons are mapped to page forward and page backward actions.

- Possible values: `"natural"`, `"inverted"`.

<!-- i18n:skip-start -->

```toml
button-scheme = "natural"
```

<!-- i18n:skip-end -->

### `locale`

✏️

The preferred language for the user interface, using BCP 47 format (e.g., `"en-US"`, `"de-DE"`).

This setting is optional. When not set, `en-GB` is used.

<!-- i18n:skip-start -->

```toml
locale = "en-GB"
```

<!-- i18n:skip-end -->

### `startup-mode`

✏️

What to show when Cadmus starts.

- `"home"` — open the home screen (default).
- `"last-file"` — re-open the last book you were reading. If there is no
  unfinished book in the selected library, the home screen is shown instead.

<!-- i18n:skip-start -->

```toml
startup-mode = "home"
```

<!-- i18n:skip-end -->

## Reader

Settings that control the reading experience.

### `reader.finished`

✏️

What to do when you finish reading a book.

Possible values:

- `"notify"` (show a notification)
- `"close"` (close the book and go back)
- `"go-to-next"` (open the next book in the library).

<!-- i18n:skip-start -->

```toml
[reader]
finished = "close"
```

<!-- i18n:skip-end -->

### `reader.dithered-kinds`

✏️

File extensions rendered with dithering by default.

<!-- i18n:skip-start -->

```toml
[reader]
dithered-kinds = ["cbz", "png", "jpg", "jpeg", "webp"]
```

<!-- i18n:skip-end -->

### `reader.font-path`

The directory Cadmus scans for additional reading fonts. Bundled Cadmus fonts
are always available regardless of this setting. See [Fonts](../fonts.md) for
details on installing custom fonts.

<!-- i18n:skip-start -->

```toml
[reader]
font-path = "/mnt/onboard/fonts"
```

<!-- i18n:skip-end -->

### `reader.font-family`

The default reading font family name. New installs default to `Libron`.
Existing configurations using `Libertinus Serif` continue to work because
Libertinus remains bundled.

<!-- i18n:skip-start -->

```toml
[reader]
font-family = "Libron"
```

<!-- i18n:skip-end -->

### `reader.font-size`

The default font size in points.

<!-- i18n:skip-start -->

```toml
[reader]
font-size = 11.0
min-font-size = 5.5
max-font-size = 16.5
```

<!-- i18n:skip-end -->

See [Fonts](../fonts.md) for more info.

## Libraries

✏️

Document library configuration. Each library has a name, path, and mode.

<!-- i18n:skip-start -->

```toml
[[libraries]]
name = "On Board"
path = "/mnt/onboard"
mode = "database"
```

<!-- i18n:skip-end -->

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

<!-- i18n:skip-start -->

```toml
[[libraries]]
name = "KePub"
path = "/mnt/onboard/.kobo/kepub"
finished = "go-to-next"
```

<!-- i18n:skip-end -->

## Intermissions

✏️

Defines the images displayed when entering an intermission state.

<!-- i18n:skip-start -->

```toml
[intermissions]
suspend = "logo:"
power-off = "logo:"
share = "logo:"
```

<!-- i18n:skip-end -->

### `intermissions.suspend`

✏️

Image displayed when the device enters sleep mode.

Setting this to `"calendar:"` also enables the calendar refresh: every 5
minutes, the device wakes, shows the calendar, and then goes back to sleep
automatically.

- Possible values: `"logo:"` (built-in logo), `"cover:"` (current book cover), `"calendar:"` (built-in calendar), or a
  path to a custom image file.

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

Import scanning happens automatically on startup using incremental file checking — files are only re-scanned if their
modification time or size has changed since the last import.

To trigger a full re-scan of all files regardless of cached values, use the **Force Full Import** action button in the
Import settings category.

### `import.sync-metadata`

✏️

Re-extract metadata (title, author, etc.) whenever a document changes.

<!-- i18n:skip-start -->

```toml
[import]
sync-metadata = true
```

<!-- i18n:skip-end -->

### `import.metadata-kinds`

File extensions of documents whose metadata is extracted during import.

<!-- i18n:skip-start -->

```toml
[import]
metadata-kinds = ["epub", "pdf", "djvu"]
```

<!-- i18n:skip-end -->

### `import.allowed-kinds`

✏️

File extensions of documents considered during the import process.

<!-- i18n:skip-start -->

```toml
[import]
allowed-kinds = ["djvu", "xps", "fb2", "txt", "pdf", "oxps", "cbz", "epub"]
```

<!-- i18n:skip-end -->

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

<!-- i18n:skip-start -->

```toml
[logging]
enabled = true
level = "info"
max-files = 3
directory = "logs"
# otlp-endpoint = "https://otel.example.com:4318"
```

<!-- i18n:skip-end -->

### `logging.enabled`

✏️

Enable or disable structured JSON logging.

<!-- i18n:skip-start -->

```toml
[logging]
enabled = true
```

<!-- i18n:skip-end -->

### `logging.level`

✏️

Minimum log level to record.

- Possible values: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`.

<!-- i18n:skip-start -->

```toml
[logging]
level = "info"
```

<!-- i18n:skip-end -->

### `logging.max-files`

Number of log files to keep. Only the most recent N files are kept — older ones
are deleted automatically when Cadmus starts.

- Default: `3`
- Set to `0` to keep all log files.

<!-- i18n:skip-start -->

```toml
[logging]
max-files = 3
```

<!-- i18n:skip-end -->

### `logging.otlp-endpoint`

✏️ (only when the `tracing` feature is enabled)

Optional OTLP endpoint for exporting logs to an OpenTelemetry collector.

<!-- i18n:skip-start -->

```toml
[logging]
otlp-endpoint = "https://otel.example.com:4318"
```

<!-- i18n:skip-end -->

Environment override:

- `OTEL_EXPORTER_OTLP_ENDPOINT` takes precedence over `logging.otlp-endpoint`.

### `logging.pyroscope-endpoint`

✏️ (only when the `profiling` feature is enabled)

Optional Pyroscope server URL for continuous profiling. When set, Cadmus starts
both a heap profiling agent (via jemalloc) and a CPU profiling agent (via
pprof) that push profiles to this endpoint.

<!-- i18n:skip-start -->

```toml
[logging]
pyroscope-endpoint = "http://localhost:4040"
```

<!-- i18n:skip-end -->

Environment override:

- `PYROSCOPE_SERVER_URL` takes precedence over `logging.pyroscope-endpoint`.

### `logging.enable-kern-log`

🧪 📱 ✏️

Captures kernel logs via `logread -F` and forwards them to structured logging
with the target `cadmus_core::logging:kern`.

<!-- i18n:skip-start -->

```toml
[logging]
enable-kern-log = false
```

<!-- i18n:skip-end -->

### `logging.enable-dbus-log`

🧪 📱 ✏️

Captures D-Bus signals via the built-in zbus-based DbusMonitorTask and forwards
them to structured logging.

<!-- i18n:skip-start -->

```toml
[logging]
enable-dbus-log = false
```

<!-- i18n:skip-end -->

## Settings Retention

Cadmus stores each version's settings in a separate file in the `Settings/` directory (for example,
`Settings-v1.2.3.toml`).
This ensures backward and forward compatibility when you upgrade.

### `settings-retention`

Number of recent version settings files to keep. Only the most recent N version files are kept. When a new version is
saved, older versions beyond this limit are deleted automatically.

- Default: `3`
- Set to `0` to keep all version files

<!-- i18n:skip-start -->

```toml
settings-retention = 3
```

<!-- i18n:skip-end -->

### `db-backup-retention`

Number of database backups to keep. When a new backup is created and the total
would exceed this limit, the oldest backups are deleted automatically.

- Default: `2`
- Set to `0` to disable backups entirely.

See [Database Backup](../database-backup.md) for more details.

<!-- i18n:skip-start -->

```toml
db-backup-retention = 2
```

<!-- i18n:skip-end -->
