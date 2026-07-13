# Fonts

Cadmus ships with reading fonts optimized for e-ink displays, plus
additional fonts for the app interface and in-book HTML fallbacks. You can switch
fonts while reading or install your own.

## Pre-installed reading fonts

Cadmus's reading fonts come from two open-source projects:

- [ebook-fonts](https://github.com/nicoverbruggen/ebook-fonts) by Nico Verbruggen —
  reading fonts tuned for e-ink displays. See the [interactive showcase](https://ebook-fonts.nicoverbruggen.be/)
  to preview them.
- [Libertinus](https://github.com/alerque/libertinus) — classic serif, sans, and mono families,
  plus Nico's metric-tweaked **NV Libertinus** from the ebook-fonts extra collection.

### ebook-fonts (Core Collection)

| Font            | Style                |
| --------------- | -------------------- |
| Libron          | Serif (default)      |
| Sourcerer       | Serif                |
| Cartisse        | Serif                |
| NV Charis       | Serif                |
| NV Garamond     | Serif                |
| NV Bitter       | Slab serif           |
| NV Palatium     | Serif                |
| NV Jost         | Sans                 |
| NV Legible Next | Sans (accessibility) |

### Libertinus

| Font             | Style     |
| ---------------- | --------- |
| NV Libertinus    | Serif     |
| Libertinus Serif | Serif     |
| Libertinus Sans  | Sans      |
| Libertinus Mono  | Monospace |

## Other packaged fonts

Cadmus also ships fonts from [Noto](https://github.com/notofonts/noto-fonts),
[Source Code Pro](https://github.com/adobe-fonts/source-code-pro), and
[Google Fonts](https://github.com/googlefonts/google-fonts) for the app interface
and HTML rendering fallbacks:

| Font            | Used for                                      |
| --------------- | --------------------------------------------- |
| Noto Sans       | Menus, settings, and other app text           |
| Noto Serif      | Serif text in the app interface               |
| Source Code Pro | Monospace in the app and code blocks in books |
| Varela Round    | On-screen keyboard                            |
| Cormorant       | Startup and intermission screens              |
| Parisienne      | Decorative cursive text in EPUB/HTML content  |
| Delius          | Decorative fantasy text in EPUB/HTML content  |

## Changing the font while reading

1. Open a book in the reader.
2. Tap the **font family** button in the reader toolbar.
3. Select a font from the list.

The menu shows fonts from Cadmus's packaged collection and any fonts in your
custom font directory (see below). When you select a font, Cadmus resolves the
family name using the order described in [How Cadmus resolves fonts](#how-cadmus-resolves-fonts).

## How Cadmus resolves fonts

When you pick a font in the reader or reopen a book with a saved
[`font-family`](settings/index.md#readerfont-family), Cadmus looks for that family
name in two places, **in this order**:

1. **Packaged fonts** — shipped with Cadmus and updated via [OTA](installation/ota.md)
2. **Your custom font directory** — [`reader.font-path`](settings/index.md#readerfont-path)
   (default `/mnt/onboard/fonts/`)

If the same family name exists in both places, the **packaged** copy is used.

Your chosen family replaces Cadmus's default **serif** stack for reflowable
books. Paragraphs the book styles as sans-serif, monospace, cursive, or fantasy
keep those respective packaged fallbacks. If Cadmus cannot find the family name,
the previous font is kept.

## Installing additional fonts

To add your own fonts, copy `.ttf` or `.otf` files into your custom font
directory (by default at the root of your Kobo device):

<!-- i18n:skip-start -->

```text
/mnt/onboard/fonts/
```

<!-- i18n:skip-end -->

Cadmus scans that directory recursively, so fonts can live in subfolders.

Point Cadmus at your custom directory with [`reader.font-path`](settings/index.md#readerfont-path)
if you use a different location.

## Settings

Font options can also be set in your settings file. See the
[Reader settings](settings/index.md#reader) section for `font-family`,
`font-path`, `font-size`, and related entries.

## OTA updates

[OTA updates](installation/ota.md) replace Cadmus-packaged fonts before
installing the new release. Cadmus removes **each shipped font file
individually** — it does not delete the whole `fonts/` directory — so any
extra files you placed in Cadmus's install `fonts/` folder survive the update
if they are not part of the release.

Under normal use, your custom fonts live in a **separate directory**
([`reader.font-path`](settings/index.md#readerfont-path), default
`/mnt/onboard/fonts/`) and are never touched by OTA. Packaged fonts stay in
Cadmus's install directory.
