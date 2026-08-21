---
name: translations-sync
description: Regenerate the translations POT file after modifying English user-facing documentation. Use when docs/src/**/*.md files outside contributing/ are changed. Skip when only docs/src/contributing/** changes.
---

# Regenerate Translations POT File

Run after modifying English **user-facing** documentation
(`docs/src/**/*.md` outside `docs/src/contributing/`).

Contributor docs under `docs/src/contributing/**` are wrapped in
`<!-- i18n:skip-start -->` / `<!-- i18n:skip-end -->` and are not
extracted, so they do not require a POT update.

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
