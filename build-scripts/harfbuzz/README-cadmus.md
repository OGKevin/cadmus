# HarfBuzz — Cadmus Build Notes

Kobo cross-build only. Upstream Meson normally resolves FreeType, libpng, and
zlib through `dependency()`; on Kobo those libraries are built as sibling trees
under `target/cadmus-build-deps/<target>/` with no pkg-config layout.

## Kobo patch

`kobo/000-kobo.patch` wires Meson to the vendored `.libs` outputs and include
paths for FreeType, libpng, bzip2, and zlib. See [Patch tiers](../README.md#patch-tiers)
for tier layout and apply order.

## Generic patch

`generic/010-graph-result-move.patch` is
[harfbuzz/harfbuzz#6272](https://github.com/harfbuzz/harfbuzz/pull/6272)
(`8de2e6e2`, Change-Id `7a4bec5b5418bd0921b273d8d4a1e79b`) carried
byte-for-byte. It makes the implicit `graph_t` to
`graph_result_t<graph_t>` conversion in `src/graph/graph.hh` explicit as
`return Ok (std::move (g));`, which HarfBuzz 14.5.0 needs to compile with the
Kobo toolchain `gcc-linaro-4.9.4-2017.01`.

**Drop this patch when `thirdparty/harfbuzz` reaches 14.5.1**, which carries
the fix upstream.
