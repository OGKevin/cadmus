# Troubleshooting

## Logs

When something isn't working right, logs will help with figuring out what went
wrong.
If you're reporting an issue, sharing your logs makes it much easier to debug.

### Where to find Cadmus logs

Cadmus saves logs in a `logs` folder. Here's where to find it on each platform:

| Platform | Stable build                     | Test build                           |
| -------- | -------------------------------- | ------------------------------------ |
| Kobo     | `/mnt/onboard/.adds/cadmus/logs` | `/mnt/onboard/.adds/cadmus-tst/logs` |

Each time you start Cadmus, it creates a new log file with a unique ID. By
default, only the 3 most recent log files are kept — older ones are deleted
automatically. You can change this with the
[`logging.max-files`](../settings/index.md#loggingmax-files) setting.

The log files look like this:

```txt
cadmus-019cf7e3-ef3a-7752-846f-83b92ac90634.json
```

### Finding your run ID

Every time Cadmus starts, it prints a run ID to help you identify which log
file belongs to that session.
You can find this in:

1. **info.log** - The startup log in the Cadmus folder. Look for the line that
   says `Cadmus run started with ID:` followed by a string of letters and numbers.

   For example:

   ```txt
   Cadmus run started with ID: 019cf7e3-ef3a-7752-846f-83b92ac90634 (version 0.10.0)
   ```

   Copy only the UUID part — the string of letters and numbers between `ID:` and
   the `(version` text.

2. **Console output** - If you're running Cadmus from a terminal, the same run
   ID is printed when it starts.

### Kernel logs

Kernel logs can be useful to debug lower level system issues, for example a
kernel panic, which triggers a device reboot.

Kernel logs are only available in [test builds](../installation/test-builds.md).
If you're using a test build and want to include kernel logs:

1. Open **Main Menu → Settings**
2. Go to `Telemetry`
3. Enable kernel logs
4. Restart Cadmus

Kernel logs will then be saved in the same log file as your Cadmus logs.

> [!NOTE]
> Kernel logs will use more disk space, so don't forget to turn it back off.

## Crashloop recovery

If Cadmus crashes 3 times in a row, it will exit back to Nickel instead of
restarting. This prevents the device from getting stuck in an infinite loop of
crashes.

When this happens:

1. Check `info.log` in the Cadmus folder for the panic error
2. The crash counter resets when you start Cadmus manually (using the restart
   option in the menu or rebooting)
