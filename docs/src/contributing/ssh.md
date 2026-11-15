<!-- i18n:skip-start -->

# SSH-ing into your Kobo

To enable SSH, mount your kobo normally, and follow the instructions in the file
`.kobo/ssh-disabled`. The `.kobo` file is a hidden file, and if using a GUI may
need to enable viewing of hidden files.

```text
To enable ssh:
- Rename this file to ssh-enabled
- Reboot the device
- Connect via: ssh root@<device_ip>
```

You may find the IP address under 'More > Settings > Device information' in
Nickel, Kobo's factory UI.

To connect to your device must not be sleeping.
If you fail to connect, make sure Nickel is running.

While Nickel runs as root, `adduser` is available and functional.
You may use this to create a new user and as that user instead.
`ssh-copy-id` allows copying your SSH public key to the Kobo,
allowing convenient passwordless login using your key.

As of May 2026, the SSH implementation is
OpenSSH (Currently OpenSSH_8.9p1, OpenSSL 3.0.8 7 Feb 2023).

## Restarting Cadmus over SSH

Cadmus listens for Unix signals while it runs. From a shell on the device:

```sh
kill -USR1 $(pidof cadmus)
```

`SIGUSR1` requests a restart. `SIGTERM` and `SIGINT` request a quit. `SIGHUP`
is ignored, so closing the SSH session does not stop Cadmus.

`pidof cadmus` matches whichever install is running. How those signals map onto
Quit and Restart is in [Event System](event-system.md).

## Deploying and restarting from the host

`cadmuscli` runs the rsync delta algorithm in process and carries it over its
native `russh` connection. The host needs neither `ssh` nor `rsync`; the Kobo
needs a compatible `rsync`.
Only changed blocks of a file are sent, and files the distribution
does not contain are left alone:

```sh
# Cross-compile the test feature set, assemble dist/, and sync it.
cadmuscli kobo deploy --ip 192.168.1.128

# Upload and restart in one step.
cadmuscli kobo deploy --ip 192.168.1.128 --restart

# Restart without deploying.
cadmuscli kobo restart --ip 192.168.1.128
```

`--ip` accepts an address or a `~/.ssh/config` alias. When it is omitted,
`KOBO_IP` is used; inside devenv, set it once in `devenv.local.nix` (see
[Development Environment Setup](devenv-setup.md#local-configuration)). Both
commands also take `--port`, `--user`, `--identity` (repeatable),
`--known-hosts`, `--accept-new`, and `--ssh-config`; when supplied, each
overrides the matching `ssh_config(5)` entry, with defaults of port 22, user
`root`, `~/.ssh/known_hosts`, and `~/.ssh/config`. Pass `--identities-only`
explicitly to skip unrelated agent keys (`IdentitiesOnly` in `ssh_config` is
not read). `deploy --features tracing` adds
Cargo features on top of the pinned `test,kobo` pair. The first connect needs
`--accept-new` (or one manual `ssh`) to record the host key, and
`KOBO_PASSWORD` enables password login.

`restart` sends `SIGUSR1` to the Cadmus process whose working directory is the
test install, then waits up to 30 seconds for a new pid. It reports a clear
error when Cadmus is not running or when the relaunch does not happen.

<!-- i18n:skip-end -->
