# Installation

Cadmus comes in different packages. Pick the one that matches your needs.

## Available packages

| Package                | What's included         | Installs to                     |
| ---------------------- | ----------------------- | ------------------------------- |
| `KoboRoot.tgz`         | Cadmus only             | `/mnt/onboard/.adds/cadmus`     |
| `KoboRoot-nm.tgz`      | Cadmus + NickelMenu     | `/mnt/onboard/.adds/cadmus`     |
| `KoboRoot-test.tgz`    | Test build only         | `/mnt/onboard/.adds/cadmus-tst` |
| `KoboRoot-nm-test.tgz` | Test build + NickelMenu | `/mnt/onboard/.adds/cadmus-tst` |

## Which one should I pick?

- **Normal installs**: Use `KoboRoot.tgz` or `KoboRoot-nm.tgz`
- **If you use NickelMenu**: Pick a package that includes it (`-nm` versions)
- **Testing a new feature**: Use test packages (`-test` versions) for trying
  out changes that haven't been released yet

## First-time setup

1. Go to the [latest release](https://github.com/OGKevin/cadmus/releases/latest).
2. Download the package you want from the table above.
3. Connect your Kobo to your computer via USB.
4. Copy the downloaded file to `/mnt/onboard/.kobo/KoboRoot.tgz` on the device.
5. Eject the device and reboot.

## Updating

Once installed, you can update Cadmus directly through its built-in OTA feature

- no computer required, just WiFi. See [OTA updates](./ota.md) for details.
