# Automatic Frontlight

Cadmus can automatically adjust your frontlight's warmth and brightness
throughout the day based on the position of the sun at your location.

## How it works

When enabled, Cadmus runs a background task that recalculates the ideal
frontlight levels every **5 minutes** and applies them immediately. The first
adjustment happens as soon as the feature starts, and then repeats on every 5-minute mark.

The adjustment has two dimensions:

- **Warmth** – zero warmth during the day, gradually ramping up to full warmth
  over a 1.5-hour window around sunset, staying at full warmth through the night,
  then ramping back down to zero over the 1.5-hour window before sunrise.
- **Brightness** – unchanged during the day (keeps whatever level you last set).
  After sunset brightness drops to `auto-frontlight-night-brightness` and stays
  there until sunrise.

## Enabling automatic adjustment

Open **Main Menu → Settings → General** and turn on **Auto Frontlight**.

You can also enable it directly in your settings file:

```toml
auto-frontlight = true
```

See [`auto-frontlight`](settings/index.md#auto-frontlight) and related entries
in the Settings reference for all available options.

## Manual adjustments pause automation

If you manually change the frontlight level while automatic adjustment is
running, Cadmus stops the background task so your chosen level is preserved.
Automatic adjustment resumes the next time the app is started.

## Location detection

Sun position is calculated from your geographic coordinates. Cadmus obtains
these automatically during each [time sync](time-sync.md): the same `ipapi.co`
lookup that resolves your timezone also returns a latitude and longitude, which
are saved to `auto-frontlight-last-coordinates` in your settings file.

If no coordinates are available yet (for example, before the first successful
time sync), automatic adjustment is skipped until a location is known.

### Manual coordinates override

If you prefer not to rely on IP-based location, or if the detected location is
inaccurate, you can set your own coordinates:

```toml
auto-frontlight-manual-coordinates = [51.5074, -0.1278]
```

Manual coordinates take priority over auto-detected ones.

You can also edit this in **Main Menu → Settings → General** under **Auto
Frontlight Manual Coordinates**.

## Night brightness

The brightness level used after sunset defaults to `1.0` (1%). You can raise
this if you find the screen too dim for nighttime reading:

```toml
auto-frontlight-night-brightness = 10.0
```

The value is a percentage from `0.0` to `100.0`.

## Privacy

> [!NOTE]
> When [automatic time syncing](time-sync.md) is enabled, a request is sent
> to `ipapi.co` to resolve your timezone. That same response includes an
> approximate latitude and longitude derived from your public IP address, which
> Cadmus stores as `auto-frontlight-last-coordinates`.
