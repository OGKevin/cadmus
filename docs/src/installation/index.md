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

There are two ways to update Cadmus once it's installed.

### Wirelessly (OTA)

The easiest way — no computer needed, just WiFi. Open
**Main Menu → Check for Updates** and follow the prompts. See
[OTA updates](./ota.md) for details.

### Via USB

You can also update by copying a new package over USB, the same way you did the
first-time install.

1. Connect your Kobo to your computer via USB.
2. When Cadmus asks "Share storage via USB?", tap **Share**.
3. Download the package you want from the [latest release](https://github.com/OGKevin/cadmus/releases/latest).
4. Copy it to `/mnt/onboard/.kobo/KoboRoot.tgz` on your Kobo.
5. Eject and disconnect the USB cable.

> [!NOTE]
> Always name the file `KoboRoot.tgz` on the device, regardless of which
> package you downloaded (e.g. `KoboRoot-nm.tgz` must be renamed).

Cadmus detects the file automatically and reboots your Kobo to install the
update. You don't need to do anything else.
