<!-- i18n:skip-start -->

# Third-party libraries

Git submodules under `thirdparty/` vend upstream dependencies (C/C++ libraries,
SQLite, fonts, and other project assets). Libraries in the Kobo cross-build
chain are built through the <a href="/api/build_deps/">`build-deps`</a> crate:
host builds for development, ordered cross-builds for Kobo.

Per-library patches, Meson cross files, and maintainer notes live in
`build-scripts/<lib>/`. Shared rules for patch tiers and apply order are in
[Patch tiers](patches.md).

## Kobo build order

Cross-compilation builds libraries in dependency order. The list is fixed in
<a href="/api/build_deps/versions/constant.LIBRARY_NAMES.html">`LIBRARY_NAMES`</a>:
each library may
link against earlier entries on the include and library search path. Do not
reorder without checking link dependencies.

## Submodule inventory

| Library   | Upstream                                                                  | Patched (tiers)           | Contributor notes         |
| --------- | ------------------------------------------------------------------------- | ------------------------- | ------------------------- |
| zlib      | [madler/zlib](https://github.com/madler/zlib)                             | No                        | —                         |
| bzip2     | [bzip2/bzip2](https://gitlab.com/bzip2/bzip2)                             | No                        | —                         |
| libpng    | [pnggroup/libpng](https://github.com/pnggroup/libpng)                     | No                        | —                         |
| libjpeg   | [libjpeg-turbo](https://github.com/libjpeg-turbo/libjpeg-turbo)           | No                        | —                         |
| openjpeg  | [uclouvain/openjpeg](https://github.com/uclouvain/openjpeg)               | No                        | —                         |
| jbig2dec  | [ArtifexSoftware/jbig2dec](https://github.com/ArtifexSoftware/jbig2dec)   | No                        | —                         |
| libwebp   | [webmproject/libwebp](https://github.com/webmproject/libwebp)             | No                        | —                         |
| freetype2 | [freetype/freetype](https://github.com/freetype/freetype)                 | No                        | —                         |
| harfbuzz  | [harfbuzz/harfbuzz](https://github.com/harfbuzz/harfbuzz)                 | Yes (`kobo/`)             | [HarfBuzz](harfbuzz.md)   |
| gumbo     | [gumbo-parser/gumbo-parser](https://github.com/gumbo-parser/gumbo-parser) | Meson cross file only     | [Gumbo](gumbo.md)         |
| djvulibre | [barak/djvulibre](https://github.com/barak/djvulibre)                     | Yes (`kobo/`)             | [DjVuLibre](djvulibre.md) |
| mupdf     | [ArtifexSoftware/mupdf](https://github.com/ArtifexSoftware/mupdf)         | Yes (`generic/`, `kobo/`) | [MuPDF](mupdf.md)         |

Other submodules (for example `thirdparty/sqlite`, fonts, mdbook helpers) are
not in `LIBRARY_NAMES` but follow the same submodule workflow. Custom SQLite
build notes: [SQLite](sqlite.md).

## Submodule upgrades

Version pins are the submodule commits recorded in the repo (often via Renovate).
After bumping a submodule, rebuild affected targets and refresh patches if
upstream changed touched files. See [`thirdparty/AGENTS.md`](../../../../thirdparty/AGENTS.md).

<!-- i18n:skip-end -->
