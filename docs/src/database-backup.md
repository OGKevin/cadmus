# Database Backup

Cadmus automatically backs up its SQLite database every time it starts.

## How it works

When Cadmus starts up it runs through these steps in order:

1. **Integrity check** — Cadmus checks the database file for corruption.
2. **Version check** — Cadmus compares the version stamp stored in the database
   against the app version and database layout used by the version that is
   currently running.
3. **Restore (if needed)** — If the database is corrupted, or if it was last
   written by a _newer_ version of Cadmus than the one running now, Cadmus
   restores the best available backup before continuing.
4. **Migrations** — Schema and data migrations run against the (possibly
   restored) database.
5. **Backup** — A fresh backup of the now-migrated database is saved to disk.

## Where backups are stored

Backups live in a `backups/` folder inside the directory that contains
`cadmus.sqlite`. A small index file called `.cadmus-db-index.toml` in that same
folder tracks every backup.

```tree
<data dir>/
├── cadmus.sqlite
└── backups/
    ├── .cadmus-db-index.toml
    ├── cadmus-v1.2.0.sqlite
    └── cadmus-v1.3.0.sqlite
```

Each backup file is named after the Cadmus version that created it, for example
`cadmus-v1.2.3.sqlite`.

## Downgrade protection

When you install an older version of Cadmus on top of a newer one, the database
already on disk was written by the newer version.

Cadmus checks whether the older version uses the same database layout as the
newer version. If it does, Cadmus keeps using the database normally.

If the database layout changed, Cadmus automatically restores the most recent
backup that is compatible with the older version. Data such as reading progress
that was only written on the newer version will be lost.

Before the restore happens, the current database file is renamed to
`cadmus-<newer-version>-demoted.sqlite` in the `backups/` folder. This demoted
file is kept on disk indefinitely as a safety net — Cadmus never deletes it
automatically, so you can recover it manually if you ever need to.

## Corruption recovery

If the database file fails its integrity check, Cadmus restores the most recent
backup and re-runs migrations. The corrupt file is replaced and Cadmus continues
normally.

> [!WARNING]
> If the database is corrupted _and_ no backup exists (for example on a fresh
> install), Cadmus cannot start. You would have to manually delete the database
> file to allow Cadmus to start.

## Controlling how many backups to keep

Use the `db-backup-retention` setting to control how many backup files are kept
on disk. When a new backup is created and the total number of backups would
exceed this limit, the oldest backups are deleted automatically.

- Default: `2`
- Set to `0` to disable backups entirely.

```toml
db-backup-retention = 2
```
