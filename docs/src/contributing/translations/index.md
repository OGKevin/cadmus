<!-- i18n:skip-start -->

# Translations

Cadmus has three separate translation systems, each covering a different part of
the project.

| What                         | System           | Files                       |
| ---------------------------- | ---------------- | --------------------------- |
| [Documentation](docs.md)     | GNU gettext / PO | `docs/po/*.po`              |
| Website landing page         | i18next JSON     | `website/messages/*.json`   |
| [UI strings](source-code.md) | Fluent (FTL)     | `crates/core/i18n/**/*.ftl` |

Locale codes for the website UI are derived from `docs/po/*.po` (same set as
the user guide). Add `website/messages/{locale}.json` when translating UI
strings; an empty `{}` file enables the locale route with English fallback
until strings are translated.

Pick the guide that matches what you want to translate.

## Website URL layout

All website content uses **locale-first URLs**. The locale is always the first
path segment:

| Content       | Example URL                |
| ------------- | -------------------------- |
| Website home  | `/en/`, `/fr/`             |
| User guide    | `/en/guide/`, `/fr/guide/` |
| API reference | `/en/api/cadmus_core/`     |
| Storybook     | `/en/storybook/`           |

Bare `/` redirects to `/en/` (or a saved or browser-matched locale). API docs
and Storybook serve the same English content under every locale prefix so
language switching only swaps the first URL segment.

<!-- i18n:skip-end -->
