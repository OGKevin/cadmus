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
the configured `ntp-server` and ipapi.co.

<!-- i18n:skip-start -->

```toml
auto-time = false
```

<!-- i18n:skip-end -->

### `ntp-server`

✏️

Hostname or IP of the NTP server used for automatic and manual time sync. Port 123 is fixed.

<!-- i18n:skip-start -->

```toml
ntp-server = "time.cloudflare.com"
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

### `wifi`

Radio button in the top menu.

WiFi operating mode:

- `off` — radio stays powered down; on-demand features cannot enable it
- `always-on` — radio stays enabled (legacy `wifi = true`)
- `auto` — enable when a feature needs the network, then power down after idle

Legacy boolean values still load: `true` becomes `always-on`, `false` becomes `off`.

<!-- i18n:skip-start -->

```toml
wifi = "off"
```

<!-- i18n:skip-end -->

### `wifi-idle-timeout`

✏️

Number of minutes after the last Auto-mode WiFi use before the radio is powered down.

- Zero means disable as soon as the last WiFi lease is released.
- Only applies when `wifi = "auto"`.
- Minimum non-zero value is `0.5` (30 seconds). The idle poller checks every
  30 seconds, so shorter positive timeouts are raised to `0.5` when settings
  are loaded or edited.

<!-- i18n:skip-start -->

```toml
wifi-idle-timeout = 5.0
```

<!-- i18n:skip-end -->

### `autosleep-mode`

✏️ 📱

Soft-suspend target written to the kernel autosleep interface. See
[Soft Suspend](../soft-suspend.md). The settings editor shows Soft Suspend only
when the device supports it; on unsupported hosts this key has no effect.

- Possible values: `"off"` (default), `"freeze"`, `"mem"`.
- Unsupported values for your device fall back to `"off"`.

<!-- i18n:skip-start -->

```toml
autosleep-mode = "off"
```

<!-- i18n:skip-end -->

### `indicate-autosleep-led`

✏️ 📱

When soft suspend is armed, keep the status LED on while the device is awake.
Hidden in the settings editor when soft suspend is unsupported.

- Default: `false`.

<!-- i18n:skip-start -->

```toml
indicate-autosleep-led = false
```

<!-- i18n:skip-end -->

### `autosleep-grace`

✏️ 📱

Seconds to keep the soft-suspend wake lock after the last Cadmus lease drops.
See [Soft Suspend](../soft-suspend.md). Hidden in the settings editor when soft
suspend is unsupported.

- Default: `5.0`.
- Zero unlocks immediately.

<!-- i18n:skip-start -->

```toml
autosleep-grace = 5.0
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

Settings that control the reading experience. How Cadmus loads
[reflowable](../reader/reflowable.md) and [paged](../reader/paged.md) books is
described in the [Reader](../reader/index.md) section.

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

### `reader.south-east-corner`

Action when you tap the south-east corner of the screen.

Possible values:

- `"go-to-page"`
- `"next-page"`

<!-- i18n:skip-start -->

```toml
[reader]
south-east-corner = "go-to-page"
```

<!-- i18n:skip-end -->

### `reader.bottom-right-gesture`

Action for the bottom-right corner gesture.

Possible values:

- `"toggle-dithered"`
- `"toggle-inverted"`

<!-- i18n:skip-start -->

```toml
[reader]
bottom-right-gesture = "toggle-dithered"
```

<!-- i18n:skip-end -->

### `reader.south-strip`

Action when you tap the south strip.

Possible values:

- `"toggle-bars"`
- `"next-page"`

<!-- i18n:skip-start -->

```toml
[reader]
south-strip = "toggle-bars"
```

<!-- i18n:skip-end -->

### `reader.west-strip`

Action when you tap the west strip.

Possible values:

- `"previous-page"`
- `"next-page"`
- `"none"`

<!-- i18n:skip-start -->

```toml
[reader]
west-strip = "previous-page"
```

<!-- i18n:skip-end -->

### `reader.east-strip`

Action when you tap the east strip.

Possible values:

- `"previous-page"`
- `"next-page"`
- `"none"`

<!-- i18n:skip-start -->

```toml
[reader]
east-strip = "next-page"
```

<!-- i18n:skip-end -->

### `reader.strip-width`

Width ratio of the side and bottom strip touch regions, relative to
`min(W, H) / 2`.

<!-- i18n:skip-start -->

```toml
[reader]
strip-width = 0.6
```

<!-- i18n:skip-end -->

### `reader.corner-width`

Width ratio of the corner touch regions, relative to `min(W, H) / 2`.

<!-- i18n:skip-start -->

```toml
[reader]
corner-width = 0.4
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

✏️

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

The default font size in points, plus the minimum and maximum sizes available
while reading.

<!-- i18n:skip-start -->

```toml
[reader]
font-size = 11.0
min-font-size = 5.5
max-font-size = 16.5
```

<!-- i18n:skip-end -->

See [Fonts](../fonts.md) for more info.

### `reader.text-align`

Default text alignment for reflowable books.

Possible values:

- `"left"`
- `"right"`
- `"center"`
- `"justify"`

<!-- i18n:skip-start -->

```toml
[reader]
text-align = "left"
```

<!-- i18n:skip-end -->

### `reader.margin-width`

Default page margin width in millimeters, plus the minimum and maximum values
available while reading.

<!-- i18n:skip-start -->

```toml
[reader]
margin-width = 8
min-margin-width = 0
max-margin-width = 10
```

<!-- i18n:skip-end -->

### `reader.line-height`

Default line height for reflowable books, in ems.

<!-- i18n:skip-start -->

```toml
[reader]
line-height = 1.2
```

<!-- i18n:skip-end -->

### `reader.continuous-fit-to-width`

Scroll mode used for fit-to-width zoom when a document is first opened.

<!-- i18n:skip-start -->

```toml
[reader]
continuous-fit-to-width = true
```

<!-- i18n:skip-end -->

### `reader.ignore-document-css`

When `true`, Cadmus ignores stylesheets embedded in reflowable documents and
uses Cadmus viewer styles only.

<!-- i18n:skip-start -->

```toml
[reader]
ignore-document-css = false
```

<!-- i18n:skip-end -->

### `reader.dithered-kinds`

✏️

File extensions rendered with dithering by default when opened for the first
time.

<!-- i18n:skip-start -->

```toml
[reader]
dithered-kinds = ["cbz", "png", "jpg", "jpeg", "webp"]
```

<!-- i18n:skip-end -->

### `reader.paragraph-breaker`

Controls how Cadmus breaks paragraphs in reflowable text.

#### `hyphen-penalty`

Penalty for hyphenated lines. The maximum value is `10000`.

#### `stretch-tolerance`

How much inter-word spacing may stretch or shrink when justifying lines.

<!-- i18n:skip-start -->

```toml
[reader.paragraph-breaker]
hyphen-penalty = 50
stretch-tolerance = 1.26
```

<!-- i18n:skip-end -->

### `reader.refresh-rate`

✏️

How often Cadmus fully refreshes the screen while you turn pages. `regular`
applies when colors are not inverted; `inverted` applies when they are. Use
`0` to never force a full refresh on that schedule.

Optional `by-kind` entries override the global pair for specific file
extensions.

<!-- i18n:skip-start -->

```toml
[reader.refresh-rate]
regular = 8
inverted = 2

# [reader.refresh-rate.by-kind]
# cbz = { regular = 1, inverted = 1 }
```

<!-- i18n:skip-end -->

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
fill-color = { gray = 255 }
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

### `intermissions.fill-color`

✏️

Background color for unused screen area when the suspend, power-off, or share
screen uses the built-in logo, the current book cover, or a custom image. This
does not affect blank, inverted-blank, or calendar intermission screens.

- Default: `{ gray = 255 }` (white)
- In **Settings → Intermissions → Fill Color**, choose **White** or **Black**
- Advanced: any valid color value accepted elsewhere in settings, for example
  `{ rgb = [128, 64, 32] }` (no color picker in the UI)

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
