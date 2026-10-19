<!-- i18n:skip-start -->

# mDNS

Cadmus runs one [`mdns-sd`](https://crates.io/crates/mdns-sd) daemon for the
process. The app calls `context.mdns.start()` after device `on_startup`.
On exit only `app.rs` calls `context.mdns.shutdown_and_wait()`, before device
`on_shutdown`, so the goodbye goes out while the interfaces are still up.
`mdns-sd` owns a dedicated `mDNS_daemon` thread and binds multicast UDP 5353;
Cadmus does not wrap that thread in a
<a href="/api/cadmus_core/runtime/struct.Job.html">`Job`</a>.

The session lives on
<a href="/api/cadmus_core/context/struct.Context.html">`AppContext`</a>
as `mdns`. Starting the daemon is best-effort: if 5353 cannot bind, the session
stays inert and logs a warning. HTTP listeners still work.

## Presence

At start the session announces `_cadmus._udp.local.` on port 0, instance
`cadmus`. That record has no TCP listener. TXT `version` is the Cadmus git
version, `kind` is `standard` or `test` (`BuildKind`), and `platform` is `kobo`,
`emulator`, or `deviceless`. The hostname is `cadmus.local`.

The hostname is fixed, so it carries no device identity. When another host on
the LAN already answers to `cadmus.local`, `mdns-sd` resolves the conflict by
renaming the local host to `cadmus-2.local` (and so on) and reports the change
on its monitor channel. `MdnsSession::hostname` drains those events, so
`GetStatus` and later registrations use the label the LAN actually sees. A
second device therefore shows up as `cadmus-2` rather than colliding.
Addresses come from `enable_addr_auto`. Loopback and unspecified addresses are
omitted, so an emulator Wi-Fi stub that reports `127.0.0.1` does not become the
advertised address.

Presence does **not** take a Wi-Fi lease. Auto mode can still idle the radio.
Packets only leave when the radio is already up.

Kobo boots with `lo` down, and `mdns-sd` binds its daemon signal socket to
`127.0.0.1`. The Kobo lifecycle raises `lo` in `on_startup`, the app starts the
session after device startup. The app stops the session before device
`on_shutdown` lowers `lo`. Without `lo` at start, `ServiceDaemon::new` fails with
`EADDRNOTAVAIL` and the session stays inert.

## Registering a service

Subsystems call `MdnsSession::register` with a `ServiceType`, port, optional
TXT, and optional IP. `Subsystem::parse` validates the name and rejects
anything longer than 23 bytes. `mdns-sd` measures `cadmus-<name>` and refuses
a service name over 30 bytes, the highest limit that crate accepts. The
session raises the daemon to that ceiling at start; the crate default of 15
is too small for `companion`. `ServiceType::tcp` announces
`_cadmus-<name>._tcp.local.`; `udp` follows the same template. The instance
is the session hostname unless a caller passes an
explicit `Instance::Label`. The returned `Registration` unregisters on drop.
Presence stays advertised.

Re-registering the same full name updates the record. Two
`ServiceDaemon::new()` calls in one process fight over 5353, so nothing else
should create a daemon.

## Listing devices

`cadmuscli list-devices` browses `_cadmus._udp.local.` from a separate
process. It waits two seconds by default (`--timeout`) and prints each
hostname, addresses, TXT `version`, `kind`, and `platform`; `--json` prints the
same records as objects. A `ServiceRemoved` during that wait drops the device.

## See also

- [WiFi Leases](wifi.md)

<!-- i18n:skip-end -->
