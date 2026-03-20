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
> After a successful import the original files are renamed:
>
> - `.metadata.json` → `.metadata.json.imported`
> - `.reading-states/` → `.reading-states.imported`
>
> These renamed files are just a safety backup. Once you've confirmed
> everything looks right you can delete them.

> [!NOTE]
> Cadmus also removes the `.thumbnail-previews/` folder and regenerates
> thumbnails itself.

## Re-running the import

If the import went wrong (for example, the library path was incorrect in
settings), you can start it fresh:

1. Rename `.metadata.json.imported` back to `.metadata.json` and
   `.reading-states.imported` back to `.reading-states/` in each library
   directory.
2. Delete the Cadmus SQLite database:

   | Build  | Database path                                 |
   | ------ | --------------------------------------------- |
   | Stable | `/mnt/onboard/.adds/cadmus/cadmus.sqlite`     |
   | Test   | `/mnt/onboard/.adds/cadmus-tst/cadmus.sqlite` |

3. Restart Cadmus — the import will run again from scratch.

If something still looks wrong after re-running, check the logs for details.
See [Troubleshooting](../troubleshooting/index.md) for where to find them.
