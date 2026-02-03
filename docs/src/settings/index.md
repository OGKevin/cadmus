# Settings

Cadmus reads settings from `Settings.toml`.

Settings can be modified on-device through **Main Menu → Settings**, which opens the built-in settings editor.

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

The OTA feature downloads builds from GitHub CI artifacts.

### `ota.github-token`

🔑

GitHub personal access token used to access CI artifacts.

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
