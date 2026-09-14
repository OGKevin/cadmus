# Gumbo — Cadmus Build Notes

Google archived [`google/gumbo-parser`](https://github.com/google/gumbo-parser)
in 2016. Cadmus builds Gumbo for Kobo and links it into `libmupdf.so`
(`libgumbo.so` → `libmupdf.so`); it is not interchangeable with another HTML
parser.

The submodule at `thirdparty/gumbo` tracks the maintained fork:

- Canonical: [codeberg.org/gumbo-parser/gumbo-parser](https://codeberg.org/gumbo-parser/gumbo-parser)
- GitHub mirror (what `.gitmodules` uses, so Renovate can watch tags):
  [github.com/gumbo-parser/gumbo-parser](https://github.com/gumbo-parser/gumbo-parser)

Pinned to tag `0.14.0`. Native host builds still use the distro `libgumbo-dev`
/ `gumbo-parser` package; this tree is the Kobo cross-compile source.
