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

<!-- i18n:skip-end -->
