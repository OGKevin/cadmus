<!-- i18n:skip-start -->

# WiFi Leases

Any feature that needs the network must acquire a WiFi lease from
`context.wifi_session` for the duration of the work. Do not enable or disable
the radio directly through
<a href="/api/cadmus_core/device/wifi/trait.WifiManager.html">`WifiManager`</a>.

## Why leases

WiFi modes are `Off`, `AlwaysOn`, and `Auto` (see
<a href="/api/cadmus_core/settings/enum.WifiMode.html">`WifiMode`</a>). In
**Auto**, <a href="/api/cadmus_core/device/wifi/struct.WifiSession.html">`WifiSession`</a>
brings the radio up while at least one named lease is held, then starts the idle
timer when the last lease is released. Lifecycle paths (startup, suspend, USB
share, quit) also go through `wifi_session` rather than calling the manager
directly.

## Acquiring a lease

Hold the returned
<a href="/api/cadmus_core/device/wifi/struct.WifiLease.html">`WifiLease`</a>
until the network work finishes:

```rust
let _wifi = match wifi_session.acquire("my-feature") {
    Ok(lease) => lease,
    Err(error) => {
        // surface failure to the user; do not proceed offline
        return;
    }
};
// network I/O while `_wifi` is in scope
```

Or use the
<a href="/api/cadmus_macros/attr.lease.html">`#[lease]`</a>
attribute from `cadmus_macros` so the lease spans the whole function:

```rust
#[lease(self.wifi_session, "ota-download", try)]
fn download(&self) -> Result<(), Error> {
    // `self.wifi_session.acquire("ota-download")?` held until return
}
```

Use a stable, descriptive lease name. Existing names include:

| Name                  | Feature                                                                                                                               |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `time-sync`           | <a href="/api/cadmus_core/task/time_sync/struct.TimeSyncTask.html">`TimeSyncTask`</a>                                                 |
| `ota-download`        | <a href="/api/cadmus_core/view/ota/struct.OtaView.html">`OtaView`</a>                                                                 |
| `dictionary-download` | <a href="/api/cadmus_core/dictionary/monolingual/service/struct.MonolingualDictionaryService.html">`MonolingualDictionaryService`</a> |

## Gating offline UI

Do not require `context.online` alone before starting work that can bring WiFi
up. Block only when the device is offline **and**
`context.settings.wifi.allows_on_demand()` is false (WiFi is **Off**). That way
**Auto** can start a task that acquires a lease and enables the radio.

## See also

- <a href="/api/cadmus_core/device/wifi/struct.WifiSession.html">`WifiSession`</a>
- <a href="/api/cadmus_core/device/wifi/struct.WifiLease.html">`WifiLease`</a>
- <a href="/api/cadmus_core/settings/enum.WifiMode.html">`WifiMode`</a>
- <a href="/api/cadmus_macros/attr.lease.html">`#[lease]`</a>
- User guide: [WiFi](../wifi.md)

<!-- i18n:skip-end -->
