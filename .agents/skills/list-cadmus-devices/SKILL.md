---
name: list-cadmus-devices
description: Find Cadmus devices on the local network and print their addresses. Use when asked to discover a Kobo or emulator, check mDNS presence, resolve a `<hostname>.local` address, or debug LAN discovery.
---

# List Cadmus Devices

## Run it

From the repository root, inside the devenv shell:

```bash
cadmuscli list-devices
cadmuscli list-devices --timeout 5
```

`--timeout` is the number of seconds to wait for replies and defaults to 2.

The command browses `_cadmus._udp.local.` from a separate process, so it sees
the same records a peer would. Each result prints the hostname, addresses, and
the presence TXT fields `version`, `kind`, and `platform` (`kobo`, `emulator`,
or `deviceless`).

## Use the address

The presence record has no application port: it tells you which address the
device answers on.

The hostname is `cadmus-<id>.local`. The id is the sanitized device serial, or
a random per-process id for serial-less devices such as the emulator.

## Troubleshooting

- Put both devices on the same L2 network and allow multicast UDP 5353 through
  the firewall.
- Bring the Wi-Fi radio up first. Presence does not take a lease, so it only
  emits when something else already brought the radio up.
- Multiple emulators intentionally get distinct random ids, so two of them
  never clash on one LAN.
