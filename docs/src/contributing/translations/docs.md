# Translating Cadmus Documentation

This guide explains how to translate the Cadmus documentation into other
languages. Translations live in `docs/po/` as standard GNU gettext PO files.

## Prerequisites

If you are using the `devenv` environment, all required tools are already
available:

- `mdbook-xgettext` / `mdbook-gettext` — string extraction and preprocessing
- `msginit` / `msgmerge` / `msgfmt` — gettext utilities (from `gettext`)
- `poedit` — graphical PO editor

Install them outside devenv from the vendored fork with
`cargo install --path thirdparty/mdbook-i18n-helpers/i18n-helpers --locked` and
your system's `gettext` package.

## Adding a new language

### 1. Extract the POT template

Run the `cadmus-translate` script (devenv) or the equivalent command:

```bash
cadmus-translate
```

This writes `docs/po/messages.pot` — the source template every translation
derives from. Commit this file whenever the English source changes so
translators have an up-to-date starting point.

### 2. Create a PO file for your locale

```bash
# Replace 'fr' with the BCP 47 language tag you are adding.
msginit --input=docs/po/messages.pot \
        --output-file=docs/po/fr.po \
        --locale=fr
```

Open `docs/po/fr.po` and set the `Language-Name` header so the language picker
displays a readable label:

```po
"Language-Name: Français\n"
```

The xtask reads this header when generating `locales.json`; without it the
locale code (e.g. `fr`) is shown instead.

### 3. Translate the strings

Open the PO file in Poedit or any text editor and fill in each `msgstr`:

```po
msgid "Welcome to Cadmus!"
msgstr "Bienvenue dans Cadmus !"
```

Preserve Markdown formatting — bold, code spans, links — exactly as in the
`msgid`. Untranslated or _fuzzy_ entries fall back to the English source.

### 4. Build and preview

```bash
# Build everything including translated books
cargo xtask docs --base-url http://localhost

# Serve
cd docs-portal
zola serve
```

Navigate to `http://localhost:1111/guide/` and use the language picker in the sidebar
to switch to your locale.

## Keeping translations up to date

When English source files change, regenerate the template and merge new strings
into existing PO files:

```bash
cadmus-translate                    # regenerate docs/po/messages.pot
msgmerge --update docs/po/fr.po docs/po/messages.pot
```

## Excluding content from extraction

Wrap any block you want to keep in English with `<!-- i18n:skip -->` comments:

<!-- i18n:skip -->

```markdown
<!-- i18n:skip -->

This paragraph will not appear in the POT file.
```

## How the build works

1. `cargo xtask docs` calls `mdbook build -d book/<lang>` for each `.po` file
   found in `docs/po/`, passing `MDBOOK_BOOK__LANGUAGE=<lang>`.
2. The `[preprocessor.gettext]` in `docs/book.toml` substitutes translated
   strings at build time.
3. `locales.json` is written to `docs/book/html/` with the available locales;
   `lang-picker.js` fetches it at runtime to populate the language dropdown.
4. Symlinks under `docs-portal/static/guide/<lang>/` expose each locale build
   to Zola so it is served at `/guide/<lang>/`.
