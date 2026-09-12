# Importing Books

Cadmus scans your device's storage and adds books to its database automatically.
This process is called **importing**.

## Automatic import on startup

Cadmus automatically imports books every time it starts. The import is
**incremental**: files whose modification time and file size haven't changed
since the last import are skipped, avoiding unnecessary re-fingerprinting. Only
new or modified files are processed, significantly improving startup performance
for large libraries.

Copy files to your device, restart the app, and they'll appear in your library
right away.

## Force full import

If you suspect the import cache is stale or corrupted, you can force a full
re-import:

1. Open **Settings**.
2. Navigate to the **Import** section.
3. Tap **Force Full Import**.
4. Confirm when prompted.

This bypasses the incremental import cache and re-fingerprints all files in your
library directories. Be aware that this can take time and drain the battery for
large libraries, so keep your device plugged in while it runs.

## If import is interrupted

If Cadmus quits, restarts, or powers off while it is still importing, it shows a
notification that the library import was interrupted. Cadmus does not save that
in-progress scan, and it does not start generating covers.

Start Cadmus again — or tap **Force Full Import** — so a later import can
finish. Cover generation runs only after an import completes successfully.

If a library folder cannot be opened, Cadmus also does not treat the import as
complete. Cover generation still waits for the next successful import.
