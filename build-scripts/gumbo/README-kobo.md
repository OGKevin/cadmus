# Gumbo — Kobo Cross-Compile Notes

Upstream recommends Meson and has deprecated Autotools. The Kobo recipe in
`crates/build-deps` therefore matches HarfBuzz: `meson setup` with
`kobo-options.txt`, then `meson compile`.

Configure flags:

- `-Dtests=false` — tests default on and pull in gtest / C++
- `-Ddefault_library=shared` — Cadmus ships `libgumbo.so`
- `--buildtype=release`

Outputs:

| Artifact       | Path relative to the gumbo build tree |
| -------------- | ------------------------------------- |
| Shared library | `build/libgumbo.so`                   |
| SONAME         | `libgumbo.so.4`                       |
| Public headers | `src/gumbo.h`, `src/tag_enum.h`       |

Google 0.10.1 used SONAME `libgumbo.so.1`. `cargo xtask dist` copies the
SONAME filename into `dist/libs/`. MuPDF must be rebuilt against this tree so
`libmupdf.so` `DT_NEEDED`s `libgumbo.so.4`. A `libgumbo.so.1` compatibility
symlink is not valid: 0.14.0 is an ABI break.

MuPDF's `kobo.patch` points `SYS_GUMBO_CFLAGS` at `../gumbo/src` and
`SYS_GUMBO_LIBS` at `../gumbo/build`. The HTML5 walker in `source/fitz/xml.c`
gains a `default` arm so `GUMBO_NODE_PROCESSING_INSTRUCTION` (added in 0.14.0)
does not make `-Wswitch` fail.
