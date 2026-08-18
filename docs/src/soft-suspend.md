# Soft Suspend

Soft suspend lets Cadmus save battery between interactions by asking the
system to sleep when nothing important is running. It is optional and off by
default.

This is separate from Auto Suspend (the timed “put the device to sleep”
setting) and from pressing the power button or closing a sleep cover. Auto
Suspend still applies when soft suspend is on: after the idle timeout Cadmus
prepares the touchscreen and other peripherals, then enters a deeper sleep
than the light nap used between taps.

## How it works

When soft suspend is enabled, Cadmus tells the system which sleep target to
use (**Freeze** or **Memory**). While Cadmus is busy — handling a tap, using
the network, or running background work — it holds a wake lock so the device
stays awake. When that work finishes and nothing else needs attention, the
system can sleep.

## Enabling soft suspend

Soft Suspend, the LED indicator, and release grace appear under **Settings →
Power** only when your device supports soft suspend. If those rows are missing,
Cadmus cannot soft-suspend on this hardware (for example the desktop emulator).

When the Soft Suspend setting is available, open **Main Menu → Settings →
Power** and set it to one of:

| Option | Meaning                                            |
| ------ | -------------------------------------------------- |
| Off    | Soft suspend disabled (default)                    |
| Freeze | Lighter sleep (when your device supports it)       |
| Memory | Deeper sleep to RAM (when your device supports it) |

Only sleep targets your device supports appear in the list. You can also set
this in your settings file:

<!-- i18n:skip-start -->

```toml
autosleep-mode = "off"     # or "freeze" or "mem"
```

<!-- i18n:skip-end -->

## Status LED indicator

When soft suspend is available, under **Settings → Power** turn on **Use LED
to Indicate Soft Suspend** if you want the status LED on while soft suspend is
armed and the device is awake. The LED turns off when the device sleeps.

<!-- i18n:skip-start -->

```toml
indicate-autosleep-led = true
```

<!-- i18n:skip-end -->

## Release grace

When soft suspend is available, **Soft Suspend Release Grace** under
**Settings → Power** is how many seconds Cadmus keeps the wake lock after the
last busy activity ends. That avoids rapid sleep/wake when short gaps fall
between taps or background work. Set it to `0` to unlock immediately. The
default is five seconds.

<!-- i18n:skip-start -->

```toml
autosleep-grace = 5.0
```

<!-- i18n:skip-end -->

## Soft suspend vs Auto Suspend

| Feature        | Soft Suspend                         | Auto Suspend                          |
| -------------- | ------------------------------------ | ------------------------------------- |
| Purpose        | Opportunistic battery saving         | Timed sleep after inactivity          |
| Default        | Off                                  | On by default (30 minutes)            |
| Settings tab   | Power → Soft Suspend                 | Power → Auto Suspend                  |
| User action    | Opt-in                               | Happens after idle timeout            |

You can use both together. Soft suspend naps between events; after the idle
timeout, Auto Suspend still puts the device into a deeper sleep.
