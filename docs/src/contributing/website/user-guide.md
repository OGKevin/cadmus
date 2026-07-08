<!-- i18n:skip-start -->

# User guide

The Cadmus user guide is an [mdBook](https://rust-lang.github.io/mdBook/) project under
[`docs/`](https://github.com/ogkevin/cadmus/tree/master/docs). English sources live in
`docs/src/`; translated builds are produced from `docs/po/*.po` and embedded into the website
at `/{locale}/guide/` (see [Website](index.md)).

## Source layout

- **User-facing pages** — `docs/src/**/*.md` outside `contributing/` (installation, settings,
  UI, library, troubleshooting, …). Listed in [`docs/src/SUMMARY.md`](https://github.com/ogkevin/cadmus/blob/master/docs/src/SUMMARY.md).
- **Contributor pages** — `docs/src/contributing/**`. Wrapped in `<!-- i18n:skip-start/end -->`
  so they are not extracted for translation.

## Building locally

```bash
cargo xtask docs --mdbook-only   # English + translated books → docs/book/
```

English HTML output: `docs/book/html/`. Translated locales: `docs/book/{locale}/html/`.

`--mdbook-only` skips the website build. It is also used by the `docs:build` devenv task to
produce the EPUB embedded in `cadmus-core`.

To preview the guide inside the full site, run `cargo xtask docs` and see [Website](index.md).

## Editing user-facing pages

After changing English user-guide Markdown:

```bash
cadmus-translate   # regenerates docs/po/messages.pot
```

Commit the updated `docs/po/messages.pot` alongside your edits. Other locales are filled in on
[Crowdin](https://crowdin.com/project/cadmus) — do not hand-edit `docs/po/*.po`.

See [Translations — for developers](../translations/developers.md) for `i18n:skip` directives and
the full translation workflow.

## mdBook configuration

- [`docs/book.toml`](https://github.com/ogkevin/cadmus/blob/master/docs/book.toml) — book
  metadata, gettext preprocessor, Mermaid, EPUB output
- [`docs/lang-picker.js`](https://github.com/ogkevin/cadmus/blob/master/docs/lang-picker.js)
  — sidebar language dropdown (reads `website/public/_shared/locales.json` at build time)

<!-- i18n:skip-end -->
