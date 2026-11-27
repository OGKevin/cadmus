<!-- i18n:skip-start -->

# mDNS

Cadmus runs one [`mdns-sd`](https://crates.io/crates/mdns-sd) daemon per
process, so other devices and programs on the device can find it without
knowing its IP address. Nothing else should create a daemon: two in one
process fight over port 5353.

## What is advertised

- **Presence**: `_cadmus._udp.local.` with no port, and TXT `version`, `kind`
  and `platform`. The hostname is `cadmus.local`, or `cadmus-2.local` and so on
  when another host already uses the name.
- **Subsystem services**: a subsystem registers its own
  `_cadmus-<name>._tcp.local.` (or `_udp`) through `MdnsSession::register`.
  The record is withdrawn when the returned `Registration` is dropped.

Everything is also advertised on loopback so programs on the device can find
it. LAN answers never carry a loopback address.

Presence never takes a [Wi-Fi lease](wifi.md): packets only leave while the
radio is already up.

## Good to know

- The `lo` interface is down on a Kobo at boot. The Kobo lifecycle raises it,
  because the private API listens on `127.0.0.1` and loopback announcements
  need it.
- Kernels without `SO_REUSEPORT` (older Kobos) work: the daemon logs the failed
  attempt at debug and carries on.
- `mdns announced` (info) is logged only when records actually left an
  interface. Registration alone logs `mdns registered` at debug. If
  `mdns announced` is missing from the logs, mDNS is not working.
- `cadmuscli list-devices` browses Presence from another process. Next to the
  emulator it also lists `127.0.0.1`.

<!-- i18n:skip-end -->
