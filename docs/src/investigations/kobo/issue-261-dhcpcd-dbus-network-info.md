<!-- i18n:skip-start -->

# Reading IP and ESSID via dhcpcd-dbus

As part of [#261](https://github.com/OGKevin/cadmus/issues/261) the scripts `scripts/ip.sh` and
`scripts/essid.sh`, used only to fill the NetUp notification and the System Info
page, needed to migrated to pure rust. Cadmus already talks to Nickel’s **dhcpcd-dbus** for Wi-Fi status
(`WpaStatus` → `DeviceEvent::NetUp`), so I SSH’d into a device to see whether
the same bus already exposes the connected IP and ESSID without shelling out to
`ip` / `iwgetid`.

Examples below use documentation-range addresses and fake SSIDs; real device
values are omitted.

---

## Summary

On current Kobo firmware, `name.marples.roy.dhcpcd` (dhcpcd-dbus **0.6.0**) is
the only network-related D-Bus service. There is no
`fi.w1.wpa_supplicant1`. Pollable answers:

| Need  | Method          | How                                       |
| ----- | --------------- | ----------------------------------------- |
| IP    | `GetInterfaces` | `IPAddress` property as host-endian `u32` |
| ESSID | `ListNetworks`  | row whose flags contain `[CURRENT]`       |

There is **no** dedicated “get current SSID” method. Signals such as
`WpaStatus` / `SignalStrength` can carry `ssid`, but they are not queryable.

Outcome in tree: `WifiManager::network_info()` assembles both fields in
[`crates/core/src/device/kobo/wifi/dhcpcd.rs`](https://github.com/OGKevin/cadmus/blob/master/crates/core/src/device/kobo/wifi/dhcpcd.rs).

## Device environment

Tools on device:

- `dbus-send` present
- `busctl` / `gdbus` **not** present

Ground truth from the old scripts / wpa:

```sh
# was scripts/ip.sh
$ ip route get 1
1.0.0.0 via 203.0.113.1 dev wlan0  src 203.0.113.10

# was scripts/essid.sh
$ iwgetid -r
ExampleNet
```

```sh
$ wpa_cli -i wlan0 status
bssid=aa:bb:cc:dd:ee:ff
ssid=ExampleNet
wpa_state=COMPLETED
ip_address=203.0.113.10
…
```

System bus names (trimmed): only `org.freedesktop.DBus` and
`name.marples.roy.dhcpcd` matter for networking.

`GetVersion` → `0.6.0`, `GetDhcpcdVersion` → firmware’s `dhcpcd` version string.

## Introspection (what exists)

Object path: `/name/marples/roy/dhcpcd`, interface `name.marples.roy.dhcpcd`.

Notable **methods**: `GetStatus`, `ListInterfaces`, `GetInterfaces`,
`ListNetworks`, `GetNetwork`, `Scan` / `ScanResults`, config helpers, …

Notable **signals**: `Event`, `StatusChanged`, `WpaStatus`, `WpaFailureEvent`,
`SignalStrength`, `ScanResults`, …

**Not** present: `org.freedesktop.DBus.Properties`, `GetCurrentNetwork`,
`GetWpaStatus` (as a method).

Full Introspect XML is long; probe with:

```sh
dbus-send --system --print-reply \
  --dest=name.marples.roy.dhcpcd \
  /name/marples/roy/dhcpcd \
  org.freedesktop.DBus.Introspectable.Introspect
```

## IP address: `GetInterfaces`

```sh
$ dbus-send --system --print-reply \
    --dest=name.marples.roy.dhcpcd \
    /name/marples/roy/dhcpcd \
    name.marples.roy.dhcpcd.GetStatus
method return …
   string "connected"
```

```sh
$ dbus-send --system --print-reply \
    --dest=name.marples.roy.dhcpcd \
    /name/marples/roy/dhcpcd \
    name.marples.roy.dhcpcd.GetInterfaces
method return …
   array [
      dict entry(
         string "wlan0"
         array [
            dict entry( string "Interface"  variant string "wlan0" )
            dict entry( string "Reason"     variant string "BOUND" )
            dict entry( string "Wireless"   variant boolean true )
            dict entry( string "Up"         variant boolean true )
            # 203.0.113.10 as host-endian u32 on little-endian ARM ≈ 175177931
            dict entry( string "IPAddress"  variant uint32 175177931 )
            dict entry( string "SubnetCIDR" variant byte 24 )
            # further lease / DNS / router fields omitted
            dict entry( string "Type"       variant string "ipv4" )
         ]
      )
   ]
```

Decode in Rust with the same layout Cadmus uses:
`Ipv4Addr::from(host_u32.to_ne_bytes())`.

`GetStatus` alone is only a coarse string (`"connected"`); it does not carry
the address.

## ESSID: `ListNetworks` + `[CURRENT]`

`GetInterfaces` has **no** SSID field.

```sh
$ dbus-send --system --print-reply \
    --dest=name.marples.roy.dhcpcd \
    /name/marples/roy/dhcpcd \
    name.marples.roy.dhcpcd.ListNetworks \
    string:wlan0
method return …
   array [
      struct { int32 0  string "ExampleNet"  string "any"  string "[CURRENT]" }
      struct { int32 1  string "GuestNet"    string "any"  string "[DISABLED]" }
      struct { int32 2  string "OtherNet"    string "any"  string "[DISABLED]" }
   ]
```

Take the first row whose flags contain `[CURRENT]`.

Once the id is known, `GetNetwork` can fetch the `ssid` parameter (wpa may
return extra quotes):

```sh
$ dbus-send --system --print-reply \
    --dest=name.marples.roy.dhcpcd \
    /name/marples/roy/dhcpcd \
    name.marples.roy.dhcpcd.GetNetwork \
    string:wlan0 int32:0 string:ssid
method return …
   string "\"ExampleNet\""
```

Prefer the SSID from `ListNetworks` for display.

<!-- i18n:skip-end -->
