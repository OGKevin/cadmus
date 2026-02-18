# Settings

Cadmus reads settings from `Settings/Settings-*.toml`.
Settings can be changed on your Kobo through **Main Menu → Settings**, which opens the built-in settings editor.

**Legend:**

- ✏️ Editable in the settings editor
- 🔑 Required for feature to work

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

Automatically enter shared mode when connected to a computer.

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
