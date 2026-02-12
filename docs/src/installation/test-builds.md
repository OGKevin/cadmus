# Test builds

## First-time install

1. Open the [Cadmus GitHub Actions page](https://github.com/OGKevin/cadmus/actions/workflows/cargo.yml).
2. Select the run for the change you want to test.
3. Download the `cadmus-kobo-test-<suffix>` file.
   ![Download from GitHub Actions](./screenshots/artifacts.png)
4. Extract it and pick the [package](./index.md) that matches your setup.
5. Copy the selected KoboRoot file to:
   `/mnt/onboard/.kobo/KoboRoot.tgz`
6. Eject the device and reboot.

## Updating an existing test build

Use the OTA feature to download updates from a PR number directly on your
device. This lets you test changes without connecting to a computer.
