# Migrating from Plato

Cadmus is a fork of Plato and uses the same `Settings.toml` format, so
migrating is mostly a matter of copying your settings file across.

## Copy your settings

<!-- i18n:skip-start -->

| Build  | Plato settings                           | Cadmus settings                               |
| ------ | ---------------------------------------- | --------------------------------------------- |
| Stable | `/mnt/onboard/.adds/plato/Settings.toml` | `/mnt/onboard/.adds/cadmus/Settings.toml`     |
| Test   | `/mnt/onboard/.adds/plato/Settings.toml` | `/mnt/onboard/.adds/cadmus-tst/Settings.toml` |

<!-- i18n:skip-end -->

Copy the file as-is into the Cadmus folder so it is named `Settings.toml` (for example, `/mnt/onboard/.adds/cadmus/Settings.toml` or `/mnt/onboard/.adds/cadmus-tst/Settings.toml`).
The `[[libraries]]` section is the most important part, it tells Cadmus where your books live and drives the reading-progress import on
first launch. On first launch, Cadmus will move this file into its `Settings/` folder automatically.

> [!NOTE]
> If your SD card is already inserted when you upgrade, Cadmus automatically
> moves your settings, logs, and dictionaries to `/mnt/sd/.cadmus/`
> (or `/mnt/sd/.cadmus-tst/` for test builds) on the next boot.

> [!NOTE]
> If you insert an SD card **after** Cadmus has already run, the automatic
> migration will not run. You will need to copy your data manually — see
> [Moving data to an SD card](#moving-data-to-an-sd-card) below.

<!-- i18n:skip-start -->

```toml
[[libraries]]
name = "On Board"
path = "/mnt/onboard"
mode = "database"
```

<!-- i18n:skip-end -->

> [!IMPORTANT]
> Make sure each `[[libraries]]` entry has the correct `path` and `name`.
> If a path doesn't match what's on disk, Cadmus skips that library's import.

## What happens on first launch

When Cadmus starts for the first time it automatically imports your data from
each library listed in settings:

| Source                      | What's imported                                             |
| --------------------------- | ----------------------------------------------------------- |
| `.metadata.json`            | Book metadata (title, author, …) and reading progress       |
| `.reading-states/<fp>.json` | Reading progress for books not already covered by the above |

Both database mode and filesystem mode libraries are handled. Cadmus reads
`.reading-states/` in all cases, so current page, bookmarks, and annotations
carry over regardless of which mode you used in Plato.

> [!NOTE]
> The original `.metadata.json` and `.reading-states/` files are not modified
> or deleted during import. Once you have confirmed everything looks right in
> Cadmus, you can safely delete them. Keeping these files means your Plato
> progress remains intact. If you decide to go back to Plato, you can do so
> without losing your original Plato reading states. Though keep in
> mind that any progress you make in Cadmus will not sync back to Plato.

> [!NOTE]
> Cadmus also removes the `.thumbnail-previews/` folder and regenerates
> thumbnails itself.

## Re-running the import

If the import went wrong (for example, the library path was incorrect in
settings), you can start it fresh:

1. Delete the Cadmus SQLite database:

   <!-- i18n:skip-start -->

   | Build  | Database path (with SD card)        | Database path (without SD card)               |
   | ------ | ----------------------------------- | --------------------------------------------- |
   | Stable | `/mnt/sd/.cadmus/cadmus.sqlite`     | `/mnt/onboard/.adds/cadmus/cadmus.sqlite`     |
   | Test   | `/mnt/sd/.cadmus-tst/cadmus.sqlite` | `/mnt/onboard/.adds/cadmus-tst/cadmus.sqlite` |

   <!-- i18n:skip-end -->

2. Restart Cadmus — the import will run again from scratch.

If something still looks wrong after re-running, check the logs for details.
See [Troubleshooting](../troubleshooting/index.md) for where to find them.

## Moving data to an SD card

If you insert an SD card after Cadmus has already run, you need to move your
data manually. Cadmus will use the SD card for new data once it detects the
card, but your existing files stay in internal storage until you move them.

> [!CAUTION]
> Do these operations while Cadmus is not running!
>
> In other words, do this while the Nickel/KOReader/etc is running.

1. Connect your Kobo to your computer
2. Copy the following from internal storage to the SD card:

   | What to move       | From (internal)                           | To (SD card)                    |
   | ------------------ | ----------------------------------------- | ------------------------------- |
   | Settings directory | `/mnt/onboard/.adds/cadmus/Settings/`     | `/mnt/sd/.cadmus/Settings/`     |
   | Settings file      | `/mnt/onboard/.adds/cadmus/Settings.toml` | `/mnt/sd/.cadmus/Settings.toml` |
   | Logs               | `/mnt/onboard/.adds/cadmus/logs/`         | `/mnt/sd/.cadmus/logs/`         |
   | Dictionaries       | `/mnt/onboard/.adds/cadmus/dictionaries/` | `/mnt/sd/.cadmus/dictionaries/` |
   | Database           | `/mnt/onboard/.adds/cadmus/cadmus.sqlite` | `/mnt/sd/.cadmus/cadmus.sqlite` |

   > [!NOTE]
   > Use the test paths (`cadmus-tst` / `.cadmus-tst`) if you are on a test build.

3. Restart Cadmus. It will pick up the files from the SD card automatically.
