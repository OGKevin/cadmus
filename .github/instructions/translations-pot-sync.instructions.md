---
description: "Ensure POT file is regenerated when English docs source changes"
applyTo: "docs/src/**/*.md"
---

# Translation POT File Sync

When any English documentation source file (`docs/src/**/*.md`) is modified,
the contributor must also regenerate `docs/po/messages.pot` and commit the
result.

## How to regenerate

```bash
cadmus-translate
```

Or the equivalent command:

```bash
MDBOOK_OUTPUT='{"xgettext": {}}' mdbook build -d docs/po docs
```

## Review checklist

When reviewing a PR that modifies `docs/src/**/*.md`:

- [ ] `docs/po/messages.pot` is updated in the same commit or PR
- [ ] New or changed English strings appear in `messages.pot`
- [ ] Removed strings are no longer present in `messages.pot`
