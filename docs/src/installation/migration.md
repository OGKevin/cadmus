# Migrating from Plato

Cadmus is a fork of Plato and uses the same `Settings.toml` format, so
migrating is mostly a matter of copying your settings file across.

## Copy your settings

| Build  | Plato settings                           | Cadmus settings                               |
| ------ | ---------------------------------------- | --------------------------------------------- |
| Stable | `/mnt/onboard/.adds/plato/Settings.toml` | `/mnt/onboard/.adds/cadmus/Settings.toml`     |
| Test   | `/mnt/onboard/.adds/plato/Settings.toml` | `/mnt/onboard/.adds/cadmus-tst/Settings.toml` |

Copy the file as-is into the Cadmus folder so it is named `Settings.toml` (for example, `/mnt/onboard/.adds/cadmus/Settings.toml` or `/mnt/onboard/.adds/cadmus-tst/Settings.toml`).
The `[[libraries]]` section is the most important part, it tells Cadmus where your books live and drives the reading-progress import on
first launch. On first launch, Cadmus will move this file into its `Settings/` folder automatically.

```toml
[[libraries]]
name = "On Board"
path = "/mnt/onboard"
mode = "database"
```

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

   | Build  | Database path                                 |
   | ------ | --------------------------------------------- |
   | Stable | `/mnt/onboard/.adds/cadmus/cadmus.sqlite`     |
   | Test   | `/mnt/onboard/.adds/cadmus-tst/cadmus.sqlite` |

2. Restart Cadmus — the import will run again from scratch.

If something still looks wrong after re-running, check the logs for details.
See [Troubleshooting](../troubleshooting/index.md) for where to find them.
