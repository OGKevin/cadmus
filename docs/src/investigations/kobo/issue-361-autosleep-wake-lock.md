<!-- i18n:skip-start -->

# Soft suspend via autosleep and wake_lock

Research for [#361](https://github.com/OGKevin/cadmus/issues/361): can Kobo’s
kernel drive opportunistic soft suspend without a Cadmus-owned sleep loop?

How Cadmus wires this up:

- Generic leases / session: [Soft Suspend (contributor)](../../contributing/soft-suspend.md)
- Kobo deep idle + Auto Suspend: [Kobo suspend](../../contributing/kobo/suspend.md)

---

## Summary

On KLC, `/sys/power/autosleep` and `/sys/power/wake_lock` /
`wake_unlock` are present. Writing `freeze` or `mem` to autosleep arms the
kernel’s opportunistic suspend workqueue; userspace stays awake by holding
wake locks. That is enough for soft suspend — Cadmus does not need to poll
and write `/sys/power/state` itself for light naps.

**Deep idle** (Auto Suspend timeout, power button, sleep cover once soft
suspend is armed) also uses autosleep, forced to **`mem`**, plus Kobo
`state-extended=1` for touch prep, then drops the `cadmus` (and cycle) wake
lock so the kernel sleeps. There is no userspace `state=mem` write on that
path. A named `deep-idle` cycle lease covers PrepareSuspend / Suspend / RTC
post-wake work so autosleep cannot freeze mid-cycle; the classic 3s / 15s
PrepareSuspend delays are skipped because the lease is sufficient.

When soft suspend mode is **Off**, hard suspend remains the explicit
[`KoboPowerManager`](../../../../crates/core/src/device/kobo/power.rs)
`state-extended` + `state=mem` path (with those delays).

## Device probes

```sh
$ cat /sys/power/state
freeze mem

$ ls /sys/power/
… autosleep … state state-extended wake_lock wake_unlock …
```

Smoke test that worked on device:

```sh
$ echo test > /sys/power/wake_lock
$ echo freeze > /sys/power/autosleep
# stays awake

$ echo test > /sys/power/wake_unlock
# enters freeze when nothing else holds a wakeup source

$ echo off > /sys/power/autosleep
```

### freeze vs mem

With `freeze`, Wi‑Fi could stay associated across soft sleep. With `mem`,
the link dropped and needed reconnect — consistent with deeper device
suspend. Issue notes also observed charging LED / radio side effects under
soft sleep. Deep idle therefore always targets `mem`.

### Status LED

```sh
$ echo 1 > /sys/class/leds/LED/brightness
$ echo 0 > /sys/class/leds/LED/brightness
```

Useful as an awake indicator while soft suspend is armed: the kernel clears
the LED on sleep, and on wake it is turned back on automatically.

### `state-extended` is not upstream

`/sys/power/state-extended` is a **Kobo/NiX vendor** sysfs node, not part of
mainline [sysfs-power](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power).
Writing `1` prepares peripherals (e.g. touch); it does **not** suspend by
itself. Cadmus still needs either autosleep `mem` + clear wake locks, or an
explicit `state=mem` write (classic path).

## Design takeaway from the research

Do **not** enable/disable autosleep as a stand-in for “busy”. Leave
autosleep armed at the chosen target and use wake locks (refcount in
userspace → one kernel lock) so the wake→handle→sleep race stays
kernel-correct. Missing autosleep / wake_lock nodes → no-op.

## Upstream references

- [sysfs-power ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power)
  — `/sys/power/autosleep`, `wake_lock`, `wake_unlock`, `state`
- [Autosleep and wake locks (LWN)](https://lwn.net/Articles/479841/) —
  opportunistic suspend + userspace wakeup sources
- [`kernel/power/autosleep.c`](https://github.com/torvalds/linux/blob/master/kernel/power/autosleep.c)
- [sleep-states admin guide](https://www.kernel.org/doc/html/latest/admin-guide/pm/sleep-states.html)

<!-- i18n:skip-end -->
