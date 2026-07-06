# Dictionaries

Cadmus supports offline word definitions. Dictionaries are stored on your
device and work without an internet connection once downloaded.

Once a dictionary is installed, see [Dictionary](../dictionary.md) for how to
look up words while reading.

Cadmus integrates with [reader-dict](https://github.com/reader-dict/monolingual),
an open-source project that provides high-quality monolingual dictionaries
(where you look up a word and get a definition in the same language) for many
languages.

![dictionaries settings screenshot](../screenshots/settings-editor-dictionaries.png)

## Opening the Dictionaries Tab

Go to **Main Menu → Settings → Dictionaries**.

You will see a list of available languages. Each row shows the language code
and its current status.

## Statuses

| Status           | What it means                            |
| ---------------- | ---------------------------------------- |
| Download         | Not yet on your device — tap to download |
| Downloading      | A download is in progress                |
| Installed        | Ready to use                             |
| Update Available | A newer version is available             |

## Downloading a Dictionary

> [!IMPORTANT]
> Your device must be connected to Wi-Fi before you can download a dictionary.

1. Open **Main Menu → Settings → Dictionaries**.
2. Find the language you want.
3. Tap **Download** next to it.

A progress notification appears at the top of the screen while the file
downloads. Once the download finishes, Cadmus begins
[indexing](#how-indexing-works) the dictionary automatically.

## Updating a Dictionary

When a newer version is available the status shows **Update Available**.

1. Tap the language row.
2. Select **Update** from the menu.

The updated dictionary replaces the old one automatically.

## Re-downloading a Dictionary

If a dictionary is already installed you can re-download it to get a fresh
copy:

1. Tap the language row.
2. Select **Re-download** from the menu.

## Deleting a Dictionary

1. Tap the language row.
2. Select **Delete** from the menu.

The dictionary files are removed from your device.

## How Indexing Works

After you download, update, or re-download a dictionary, Cadmus needs to
**index** it before you can look up words. Indexing reads every word in
the dictionary and stores it in a database on disk so that lookups are
fast without loading the entire dictionary into memory. This is
especially important on devices with limited memory.

A notification with a progress bar appears at the top of the screen
while indexing is in progress.

> [!NOTE]
> You can keep reading while indexing runs in the background. Words that
> have already been indexed are available for lookup right away, so you
> may get partial results until indexing finishes.

### What happens when you restart your Kobo

If your Kobo restarts or shuts down while indexing is still running,
Cadmus picks up where it left off the next time it starts. It does not
start over from the beginning.

### When does re-indexing happen

Cadmus automatically re-indexes a dictionary when you:

- **Update** it to a newer version
- **Re-download** it
- **Delete** it (the old index is removed)

You do not need to trigger indexing yourself — it happens automatically
whenever the dictionary files change.

## Where Dictionaries are Stored

Cadmus stores dictionaries in different locations depending on whether your
device has an SD card and whether you are using a test build:

<!-- i18n:skip-start -->

- **On devices with an SD card**:
  - Production: `/mnt/sd/.cadmus/dictionaries/reader-dict/<lang>/`
  - Test build: `/mnt/sd/.cadmus-tst/dictionaries/reader-dict/<lang>/`
- **On devices without an SD card**:
  - Production: `/mnt/onboard/.adds/cadmus/dictionaries/reader-dict/<lang>/`
  - Test build: `/mnt/onboard/.adds/cadmus-tst/dictionaries/reader-dict/<lang>/`

<!-- i18n:skip-end -->

Each language gets its own subfolder containing a `.dict.dz` (or `.dict`) and a `.index` file.

> [!NOTE]
> If your SD card is already inserted when you upgrade Cadmus, your dictionaries
> are moved to the SD card automatically on the next boot. If you insert an SD
> card after Cadmus has already run, move the `dictionaries/` folder manually —
> see [Moving data to an SD card](../../installation/migration.md#moving-data-to-an-sd-card).
