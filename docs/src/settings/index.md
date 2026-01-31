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
