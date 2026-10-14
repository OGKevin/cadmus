# UI string translations (`i18n/`)

## Source of truth

- Edit **only** [`en-GB/cadmus_core.ftl`](en-GB/cadmus_core.ftl) when adding or
  changing user-visible strings.
- Keep message IDs sorted within each comment section.

## Other locales

- **Never** translate or invent copy in locale files under this tree (for
  example `fr/`). Crowdin owns those translations.
- Do not auto-translate, machine-translate, or hand-port English strings into
  other `.ftl` files.
- Missing keys in a locale fall back to `en-GB` at runtime until Crowdin
  syncs them.

## Slint screens

Static copy in a `.slint` file uses `@tr("English")`; the English text is the
source string. `cadmus-translate` extracts those strings with
`slint-tr-extractor` into `ui/lang/cadmus-core.pot`. Crowdin translates that
template into `ui/lang/<lang>/LC_MESSAGES/cadmus-core.po`, which the build
bundles (`--no-default-translation-context`, so no `msgctxt`).

Do not hand-edit the `.po` files. Runtime values, including setting rows and
category names, stay in Fluent.

## Runtime message IDs

- `fl!("kebab-case-id")` is checked at compile time; unknown IDs fail the build.
- `fl_or!(runtime_id, "English fallback")` skips that check. Missing message IDs
  are logged at `tracing` warning level and the caller-supplied fallback string
  is returned. Use only when the ID cannot be a literal.
