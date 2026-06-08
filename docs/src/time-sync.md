# Automatic Time Syncing

Cadmus can keep your Kobo's clock accurate by syncing it automatically whenever
WiFi connects.

## How it works

When enabled, Cadmus will:

1. Detect your timezone based on your IP address.
2. Fetch the current time from an internet time server.
3. Update the system clock and hardware clock on your Kobo.

After syncing, the clock in the status bar updates immediately.

## Enabling automatic sync

Open **Main Menu → Settings → General** and turn on **Auto Time**.

You can also enable it manually in your settings file:

```toml
auto-time = true
```

## Manual sync

You can trigger a one-time sync without enabling the automatic option:

1. Tap the clock in the top bar.
2. Select **Sync Time**.

If the sync fails (for example, WiFi is not connected), a notification will let
you know.

## Privacy

> [!NOTE]
> Time syncing sends a request to `ipapi.co` to determine your timezone based
> on your IP address. No personal data is sent — only your device's public IP
> address is visible to that service as part of the network request.

The actual time is fetched from `time.cloudflare.com` using the standard NTP
protocol.
