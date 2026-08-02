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
