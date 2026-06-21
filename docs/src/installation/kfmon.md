# Using Cadmus with KFMon

[KFMon](https://github.com/NiLuJe/kfmon) is a launcher that starts apps
when you open certain icon files on your Kobo. If you already use KFMon with
readers like Plato or KOReader, you may run into a conflict when you install
Cadmus.

## The problem

KFMon watches PNG files in the library. When Cadmus opens one of those watched
PNG files — for example, while showing a cover or importing a book — KFMon
thinks you tapped the icon and launches the matching reader. This can leave you
with two readers running at the same time, such as both Plato and Cadmus.

## The fix: add a Cadmus KFMon watch

Add a KFMon watch that points to Cadmus. This tells KFMon about Cadmus, so it
blocks other launches while Cadmus is already running.

1. Place the Cadmus icon somewhere on your Kobo, for example:

   ```text
   /mnt/onboard/icons/cadmus.png
   ```

   If you don't have an icon, create or copy a PNG image to use as the
   launcher icon.

2. Create a new file on your Kobo:

   ```text
   /mnt/onboard/.adds/kfmon/config/cadmus.ini
   ```

3. Paste this into the file, changing the `filename` value to match your icon
   path:

   ```ini
   [watch]
   ; Absolute path of the icon to watch for
   ; At the time of writing, Cadmus does not ship a custom icon, so you
   ; need to create or copy a PNG file to use as the icon.
   filename = /mnt/onboard/icons/cadmus.png
   ; Absolute path of the command to launch when the icon is opened
   ; If you are using a test build, point this to /mnt/onboard/.adds/cadmus-tst/cadmus.sh
   action = /mnt/onboard/.adds/cadmus/cadmus.sh
   ; Label shown in a GUI frontend
   label = Cadmus
   ; Show this entry in GUI frontends
   hidden = 0
   ; Prevent KFMon from launching another app while Cadmus is running
   block_spawns = 1
   ; Do not update Nickel's database for this icon
   do_db_update = 0
   ```

4. Reboot your Kobo.

> [!IMPORTANT]
> The `block_spawns = 1` line is the key setting. It stops KFMon from launching
> another reader while Cadmus is already open.

## Pick one launcher method

Cadmus can be launched by either NickelMenu or KFMon, but try to avoid both at
once. If you use KFMon, install a non-NickelMenu package from the
[Installation page](./index.md) (`KoboRoot.tgz` or `KoboRoot-test.tgz`)
instead of the `KoboRoot-nm.tgz` or `KoboRoot-nm-test.tgz` packages.
Using both can create duplicate icons and make conflicts harder to diagnose.

## Temporarily disable KFMon

If you need to stop KFMon from launching anything for a short time — for
example, while troubleshooting — you can create a blank `BLOCK` file:

```sh
touch /mnt/onboard/.adds/kfmon/config/BLOCK
```

Remove the file when you want KFMon to work again:

```sh
rm /mnt/onboard/.adds/kfmon/config/BLOCK
```
