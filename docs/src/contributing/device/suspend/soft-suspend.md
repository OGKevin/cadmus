<!-- i18n:skip-start -->

# Soft Suspend

Cadmus soft suspend is an opt-in opportunistic sleep path driven by the
kernel autosleep workqueue and a single userspace wake lock. User-facing
behaviour is documented in [Soft Suspend](../../../soft-suspend.md). Research that
led to this design is in
[investigation #361](../../../investigations/kobo/issue-361-autosleep-wake-lock.md).

Kobo Auto Suspend / deep-idle integration (RTC deadline, `state-extended`,
cycle lease, PrepareSuspend delays) is documented separately in
[Kobo suspend](kobo/suspend.md). Shared cycle orchestration is in
[Suspend orchestrator](orchestrator.md).

## Architecture

```mermaid
flowchart TD
  settings["AutosleepMode<br/>Off / Freeze / Mem"] --> apply["write autosleep sysfs"]
  ledSetting["indicate autosleep LED"] --> led["LED brightness"]
  graceSetting["autosleep grace seconds"] --> armer["WakeLockArmer"]
  holders["Named soft-suspend leases"] --> tracker["LeaseTracker"]
  tracker -->|"0 to 1"| lock["wake_lock cadmus"]
  tracker -->|"0 to 1"| cancel["cancel pending unlock"]
  tracker -->|"0 to 1"| ledOn["LED on if indicator<br/>enabled"]
  tracker -->|"1 to 0"| armer
  armer -->|"after grace or<br/>immediate if 0"| unlock["wake_unlock cadmus"]
  apply --> kernel["kernel autosleep"]
  lock --> kernel
  unlock --> kernel
```

`SoftSuspendSession` owns:

- Reading `/sys/power/state` to discover available targets
- Writing `/sys/power/autosleep` (`off` / `freeze` / `mem`)
- A lease tracker whose observer maps 0→1 to the single kernel lock name
  `cadmus`, and 1→0 to a deferred `wake_unlock` after autosleep grace
  seconds (zero = unlock immediately). A new lease during the grace cancels
  the pending unlock.
- Optional LED indicator via `DeviceLeds`

Missing autosleep / wake_lock sysfs is a no-op (emulator and hosts without
those nodes).

Upstream references:
[sysfs-power ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power)
(`autosleep`, `wake_lock`, `wake_unlock`, `state`).

## Lease holders

Named leases only adjust the Cadmus refcount; the kernel still sees one lock
(`cadmus`). Holders include the main loop, input, Wi‑Fi, and background
tasks while they run. Dropping the last lease (after grace) lets the kernel
enter the armed autosleep target.

<!-- i18n:skip-end -->
