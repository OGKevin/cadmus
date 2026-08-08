# WiFi (`device/wifi`)

Named WiFi leases coordinate radio power for Auto mode. Any feature that needs
the network must acquire a lease from `context.wifi_session` for the duration of
the work — do not enable or disable the radio through `WifiManager` directly.

See the contributor guide: [WiFi Leases](../../../../../docs/src/contributing/wifi.md).
