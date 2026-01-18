<div align="center">

# Cadmus

</div>

<p align="center">
    <a href="https://github.com/ogkevin/cadmus/releases" alt="GitHub release">
        <img src="https://img.shields.io/github/release/ogkevin/cadmus.svg?style=for-the-badge" /></a>
    <img src="https://img.shields.io/github/actions/workflow/status/ogkevin/cadmus/cargo.yml?style=for-the-badge" />
    <a href="https://discord.gg/3AJHp6rV5a" alt="Discord">
        <img src="https://img.shields.io/discord/1459138935203565741?style=for-the-badge" /></a>
</p>

<div align="center">

*Cadmus* is a document reader for *Kobo*'s e-readers.

</div>

---

Documentation: [GUIDE](doc/GUIDE.md), [MANUAL](doc/MANUAL.md) and [BUILD](doc/BUILD.md).

## Supported firmwares

Any 4.*X*.*Y* firmware, with *X* ≥ 6, will do.

## Supported devices

- *Libra Colour*.
- *Clara Colour*.
- *Clara BW*.
- *Elipsa 2E*.
- *Clara 2E*.
- *Libra 2*.
- *Sage*.
- *Elipsa*.
- *Nia*.
- *Libra H₂O*.
- *Forma*.
- *Clara HD*.
- *Aura H₂O Edition 2*.
- *Aura Edition 2*.
- *Aura ONE*.
- *Glo HD*.
- *Aura H₂O*.
- *Aura*.
- *Glo*.
- *Touch C*.
- *Touch B*.

## Supported formats

- PDF, CBZ, FB2, MOBI, XPS and TXT via [MuPDF](https://mupdf.com/index.html).
- ePUB through a built-in renderer.
- DJVU via [DjVuLibre](http://djvu.sourceforge.net/index.html).

## Features

- Crop the margins.
- Continuous fit-to-width zoom mode with line preserving cuts.
- Rotate the screen (portrait ↔ landscape).
- Adjust the contrast.
- Define words using *dictd* dictionaries.
- Annotations, highlights and bookmarks.
- Retrieve articles from online sources through [hooks](doc/HOOKS.md) (an example *wallabag* [article fetcher](doc/ARTICLE_FETCHER.md) is provided).

[![Tn01](artworks/thumbnail01.png)](artworks/screenshot01.png) [![Tn02](artworks/thumbnail02.png)](artworks/screenshot02.png) [![Tn03](artworks/thumbnail03.png)](artworks/screenshot03.png) [![Tn04](artworks/thumbnail04.png)](artworks/screenshot04.png)

## Acknowledgments

Cadmus is a fork of [Plato](https://github.com/baskerville/plato), a document reader created by Bastien Dejean.
