# SSH-ing into your Kobo

To enable SSH, mount your kobo normally, and follow the instructions in the file
`.kobo/ssh-disabled`. The `.kobo` file is a hidden file, and if using a GUI may
need to enable viewing of hidden files.

    To enable ssh:
    - Rename this file to ssh-enabled
    - Reboot the device
    - Connect via: ssh root@<device_ip>

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
