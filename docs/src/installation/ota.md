# OTA updates

Once Cadmus is installed, you can update it wirelessly without connecting to a
computer. The OTA (Over-The-Air) feature downloads updates directly from GitHub.

## What you need

- A WiFi connection
- A GitHub personal access token in your `Settings.toml` file:

```toml
[ota]
github-token = "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
```

See the [OTA settings](../settings/index.md#otagithub-token) reference for
details on getting a token.

## How to update

Open **Main Menu → Check for Updates**. You'll see options for where to get the
update from:

| Source             | Description                                    |
| ------------------ | ---------------------------------------------- |
| **Stable Release** | Latest official release from GitHub            |
| **Main Branch**    | Latest development build (most recent changes) |
| **PR Build**       | Test a specific pull request                   |

> **Note:** The _Stable Release_ option is not shown in test builds.

## Updating from the main branch

Select **Main Branch** to get the most recent development build. This includes
changes that have been merged but not yet released officially.

The update downloads from GitHub, installs automatically, and prompts you to
reboot to finish.

## Testing a pull request

Select **PR Build** to try out a specific change before it's released. Enter the
PR number when prompted.

This downloads the update from that pull request, installs it, and asks you to
reboot.

> **Tip:** Find the PR number in the GitHub URL. For example, in
> `github.com/OGKevin/cadmus/pull/42` the PR number is **42**.

## Normal vs test builds

OTA works for both types of builds. The type you're currently using determines
what gets downloaded:

- **Normal builds** update to `KoboRoot.tgz` in `/mnt/onboard/.adds/cadmus`
- **Test builds** update to `KoboRoot-test.tgz` in `/mnt/onboard/.adds/cadmus-tst`

See the [available packages](./index.md#available-packages) table for all
options.

## First-time setup

OTA only works for updating an existing installation. To install Cadmus for the
first time, follow the [installation guide](./index.md) or the
[test builds guide](./test-builds.md) to copy a KoboRoot file via USB.
