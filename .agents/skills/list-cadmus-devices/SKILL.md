---
name: list-cadmus-devices
description: Find Cadmus devices on the local network and print their addresses. Use when asked to discover a Kobo or emulator, check mDNS presence, resolve a `<hostname>.local` address, or debug LAN discovery.
---

# List Cadmus Devices

## Run it

The `cadmus.kobo.devices` OpenCode tool runs the same browse and returns one
address per device with its metadata. Prefer it when the address is all you
need.

From the repository root, inside the devenv shell:

```bash
cadmuscli list-devices
cadmuscli list-devices --timeout 5
cadmuscli list-devices --json
```

`--timeout` is the number of seconds to wait for replies and defaults to 2.
`--json` prints the same records as an array of objects.

The command browses `_cadmus._udp.local.` from a separate process, so it sees
the same records a peer would. Each result prints the hostname, addresses, and
the presence TXT fields `version`, `kind`, and `platform` (`kobo`, `emulator`,
or `deviceless`).

## Learn about a device from its record

The presence record is more than an address. Its TXT fields identify what
answered, so a lookup alone tells you:

- `platform`: `kobo` for a real device, `emulator` for a local run, or
  `deviceless` for a tool build. Use it to tell a Kobo from an emulator when
  both are on the LAN.
- `version`: the Cadmus version that device runs, for example to confirm an
  OTA landed.
- `kind`: `test` for a build made with the `test` feature, otherwise
  `standard`.

No login or Cadmus API call is needed for these. They are the same fields the
`cadmus.kobo.devices` tool returns.

## Use the address

The presence record has no application port: it tells you which address the
device answers on.

The hostname is `cadmus.local`. `mdns-sd` renames it to `cadmus-2.local` and so
on when another host already claims it, so two devices on one LAN stay
distinct.

## Troubleshooting

- Put both devices on the same L2 network and allow multicast UDP 5353 through
  the firewall.
- Bring the Wi-Fi radio up first. Presence does not take a lease, so it only
  emits when something else already brought the radio up.
- Every emulator starts as `cadmus.local`. When another host already claims
  it, `mdns-sd` renames the later one to `cadmus-2.local` (and so on), so the
  suffix depends on start order.
