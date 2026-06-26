<!-- i18n:skip-start -->

# DHCP IP Address Changes on WiFi Toggle

After identifying that killing the original `dhcpcd` and replacing it with
`udhcpc` caused the issue reported in [#51](https://github.com/OGKevin/cadmus/issues/51),
I confirmed the fix works in testing but wanted to understand the root cause
before finalising PR [#299](https://github.com/OGKevin/cadmus/pull/299).

---

## Summary

Investigated the DHCP behaviour by inspecting the running device alongside the
KOReader source tree and the original Plato shell scripts.

## What is actually running on the device

```sh
1074  /libexec/dhcpcd-dbus
1110  wpa_supplicant -D nl80211 -s -i wlan0 -c /etc/wpa_supplicant/wpa_supplicant.conf ...
1120  dhcpcd -d -z wlan0
```

Nickel uses **`dhcpcd`**, not `udhcpc`. There is no `udhcpc` running under normal operation.

## Why the old script caused a new IP every toggle

The original [`scripts/wifi-enable.sh`](https://github.com/OGKevin/cadmus/blob/253edbe8958a44d108676d57b85942f21bb7c899/scripts/wifi-enable.sh#L92-L93) (inherited from Plato) always spawned a fresh `udhcpc`:

```sh
[root@monza root]# udhcpc --help
BusyBox v1.35.99.139-g15f7d618e (2021-11-14 22:54:11 CET) multi-call binary.

Usage: udhcpc [-fbqvRB] [-a[MSEC]] [-t N] [-T SEC] [-A SEC|-n]
 [-i IFACE] [-P PORT] [-s PROG] [-p PIDFILE]
 [-oC] [-r IP] [-V VENDOR] [-F NAME] [-x OPT:VAL]... [-O OPT]...

 -i IFACE Interface to use (default eth0)
 -P PORT  Use PORT (default 68)
 -s PROG  Run PROG at DHCP events (default /usr/share/udhcpc/default.script)
 -p FILE  Create pidfile
 -B  Request broadcast replies
 -t N  Send up to N discover packets (default 3)
 -T SEC  Pause between packets (default 3)
 -A SEC  Wait if lease is not obtained (default 20)
 -b  Background if lease is not obtained
 -n  Exit if lease is not obtained
 -q  Exit after obtaining lease
 -R  Release IP on exit
 -f  Run in foreground
 -S  Log to syslog too
 -a[MSEC] Validate offered address with ARP ping
 -r IP  Request this IP address
 -o  Don't request any options (unless -O is given)
 -O OPT  Request option OPT from server (cumulative)
 -x OPT:VAL Include option OPT in sent packets (cumulative)
   Examples of string, numeric, and hex byte opts:
   -x hostname:bbox - option 12
   -x lease:3600 - option 51 (lease time)
   -x 0x3d:0100BEEFC0FFEE - option 61 (client id)
   -x 14:'"dumpfile"' - option 14 (shell-quoted)
 -F NAME  Ask server to update DNS mapping for NAME
 -V VENDOR Vendor identifier (default 'udhcp VERSION')
 -C  Don't send MAC as client identifier
 -v  Verbose
Signals:
 USR1 Renew lease
 USR2 Release lease
```

```sh
udhcpc -S -i "$INTERFACE" -s /etc/udhcpc.d/default.script -t15 -T10 -A3 -b -q > /dev/null &
```

The `-q` flag is the core issue. It tells `udhcpc` to **quit immediately after obtaining a lease**. The full
lifecycle on every WiFi toggle was:

1. `udhcpc` spawned → sends `DISCOVER` → gets `OFFER` → sends `REQUEST` → gets `ACK` → runs
   `default.script bound` (sets IP, rewrites `resolv.conf`) → **process exits**
2. WiFi disabled → `killall udhcpc default.script` → nothing to kill anyway, already exited
3. WiFi enabled again → repeat from step 1 with **zero memory of the previous lease**

Because the process exits after getting the lease, there is no daemon to renew it and no lease file written
anywhere. Busybox's `udhcpc` has no lease persistence mechanism. On the next cycle it sends a bare
`DISCOVER` with no preferred-IP hint (no DHCP Option 50), so the DHCP server is free to hand out any
address from its pool.

Additionally, [`cadmus.sh`](https://github.com/OGKevin/cadmus/blob/253edbe8958a44d108676d57b85942f21bb7c899/contrib/cadmus.sh#L18-L20) (previously `plato.sh`) was killing Nickel's already-running `dhcpcd` at startup:

```sh
killall -TERM nickel hindenburg sickel fickel adobehost foxitpdf iink dhcpcd-dbus dhcpcd fmon
```

So even the stateful daemon that Nickel had set up (which _would_ have requested the same IP again) was
torn down before Cadmus replaced it with a stateless `udhcpc -q`.

## What `default.script` does

`/etc/udhcpc.d/default.script` is a minimal Busybox hook called by `udhcpc` on lease events. The relevant
part:

```sh
case "$1" in
    renew|bound|probe)
        /sbin/ifconfig $interface $ip $BROADCAST $NETMASK
        # ... deletes all default routes, adds new ones from $router ...
        echo -n > $RESOLV_CONF          # ← truncates resolv.conf to zero
        for i in $dns ; do
            echo nameserver $i >> $RESOLV_CONF
        done
        ;;
esac
echo network $1 ip="$ip" ... > /tmp/nickel-hardware-status &
```

Notable behaviours:

- **Wipes `resolv.conf` on every `bound` event.** This is why KOReader's [`disable-wifi.sh`](https://github.com/koreader/koreader/blob/d98dd9f244c5697c08a3bb9ac068f381d70b42c4/platform/kobo/disable-wifi.sh#L3-L6) saves and
  restores `resolv.conf` with an md5 check. This is a safety net against `udhcpc`'s script wiping DNS on lease
  release.
- **Writes to `/tmp/nickel-hardware-status`**, which is a FIFO Nickel listens on for network events.
  [KOReader explicitly removes this FIFO](https://github.com/koreader/koreader/blob/d98dd9f244c5697c08a3bb9ac068f381d70b42c4/platform/kobo/koreader.sh#L232-L233) (`rm -f /tmp/nickel-hardware-status`) to prevent scripts hanging
  when Nickel is not running.

## Why `dhcpcd` produces stable IPs

`dhcpcd` writes a per-SSID lease file to **`/var/db/`**:

```sh
/var/db/dhcpcd-wlan0-1.lease
/var/db/dhcpcd-wlan0-2.lease
/var/db/dhcpcd-wlan0-3.lease
/var/db/dhcpcd-wlan0-4.lease
```

`/var/db` lives on the root eMMC partition (`/dev/mmcblk0p10`), not a tmpfs. It survives reboots.

When reconnecting to a known SSID, `dhcpcd` reads the matching `.lease` file, parses the previously-held
IP, and sends a DHCP `REQUEST` directly for that IP (DHCP Option 50: Requested IP Address), skipping
`DISCOVER` entirely if the lease has not expired. The DHCP server sees a familiar MAC + familiar IP request
and simply ACKs it.

The lease file for the current network encodes `192.x.x.x` in the `yiaddr` field of the saved
BOOTREPLY, which is exactly the IP the device is using right now.

## Filesystem layout: what is ephemeral vs persistent

| Path       | Type                | Persistent? |
| ---------- | ------------------- | ----------- |
| `/tmp`     | `tmpfs`             | No          |
| `/var/lib` | `tmpfs` (16 KiB)    | No          |
| `/var/run` | `tmpfs` (128 KiB)   | No          |
| `/var/log` | `tmpfs` (16 KiB)    | No          |
| `/var/db`  | eMMC (`mmcblk0p10`) | **Yes**     |
| `/etc`     | eMMC                | **Yes**     |

`dhcpcd`'s runtime state (`/var/run/dhcpcd.pid`, `/var/run/dhcpcd.sock`) is ephemeral as expected, but the
lease database in `/var/db/` persists. That is the architectural key.

## Why KOReader kills `dhcpcd` in its startup script but still gets stable IPs

From [`koreader.sh`](https://github.com/koreader/koreader/blob/d98dd9f244c5697c08a3bb9ac068f381d70b42c4/platform/kobo/koreader.sh#L216-L220):

```sh
# NOTE: We kill Nickel's master dhcpcd daemon on purpose,
#       as we want to be able to use our own per-if processes w/ custom args later on.
#       A SIGTERM does not break anything, it'll just prevent automatic lease renewal
#       until the time KOReader actually sets the if up itself (i.e., it'll do)...
killall -q -TERM nickel ... dhcpcd-dbus dhcpcd ...
```

KOReader kills Nickel's `dhcpcd` because it wants to start its own instance with custom arguments later via
[`obtain-ip.sh`](https://github.com/koreader/koreader/blob/d98dd9f244c5697c08a3bb9ac068f381d70b42c4/platform/kobo/obtain-ip.sh#L54-L60). Crucially, `obtain-ip.sh` prefers `dhcpcd` over `udhcpc`:

```sh
# NOTE: Prefer dhcpcd over udhcpc if available. That's what Nickel uses,
#       and udhcpc appears to trip some insanely wonky corner cases on current FW (#6421)
if [ -x "/sbin/dhcpcd" ]; then
    dhcpcd -d -t 30 -w "${INTERFACE}"
else
    udhcpc -S -i "${INTERFACE}" -s /etc/udhcpc.d/default.script -b -q
fi
```

The new `dhcpcd` instance KOReader starts reads the same `/var/db/*.lease` files and requests the same IP.
Stable address despite the kill/restart cycle.

## Why the fix in PR [#299](https://github.com/OGKevin/cadmus/pull/299) works

The fix is twofold:

1. **Remove `dhcpcd` from the kill list in [`cadmus.sh`](https://github.com/OGKevin/cadmus/blob/253edbe8958a44d108676d57b85942f21bb7c899/contrib/cadmus.sh#L18-L20)**. Nickel's running `dhcpcd -d -z wlan0` instance
   survives into the Cadmus session, continuously managing the lease.

2. **Do not start `udhcpc` at all in the native Rust WiFi implementation**. No new DHCP client is spawned
   on toggle, so the already-running `dhcpcd` is never displaced.

The result: one long-lived `dhcpcd` daemon manages the lease for the entire session, renews it in the
background, and requests the same IP on every reconnect using the persisted `/var/db/*.lease` file.

<!-- i18n:skip-end -->
