# HarfBuzz — Cadmus Build Notes

Kobo cross-build only. Upstream Meson normally resolves FreeType, libpng, and
zlib through `dependency()`; on Kobo those libraries are built as sibling trees
under `target/cadmus-build-deps/<target>/` with no pkg-config layout.

## Kobo patch

`kobo/000-kobo.patch` wires Meson to the vendored `.libs` outputs and include
paths for FreeType, libpng, bzip2, and zlib. See [Patch tiers](../README.md#patch-tiers)
for tier layout and apply order.
