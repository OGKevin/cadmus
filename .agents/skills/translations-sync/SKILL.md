---
name: translations-sync
description: Regenerate the translations POT file after modifying English documentation sources. Use when docs/src/**/*.md files are changed.
---

# Regenerate Translations POT File

Run after modifying any English documentation source (`docs/src/**/*.md`).

## Command

```bash
cadmus-translate
```

Or equivalently:

```bash
MDBOOK_OUTPUT='{"xgettext": {}}' mdbook build -d docs/po docs
```

## Verify

Check that `docs/po/messages.pot` reflects the changes and commit it alongside
the documentation edits.
