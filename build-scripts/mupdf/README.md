# MuPDF Patches for Cadmus

This directory contains the upstream MuPDF source tree (currently 1.27.0) plus
Cadmus-specific patches. The patches are applied by `cargo xtask setup-native`
and by the Kobo build scripts.

## Patch overview

| Patch                                  | Origin            | What it does                                                                                                                                                                                                               |
| -------------------------------------- | ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `kobo.patch`                           | Custom            | Build-system tweaks for the Kobo ARM cross-compile target.                                                                                                                                                                 |
| `webp-upstream-697749-kobo.patch`      | KOReader verbatim | Complete upstream WebP support patch from [koreader/koreader-base](https://github.com/koreader/koreader-base/blob/master/thirdparty/mupdf/webp-upstream-697749.patch). See provenance below.                               |
| `webp-image-h-kobo.patch`              | Custom            | Adds `fz_load_webp` / `fz_load_webp_info` declarations to `include/mupdf/fitz/image.h`. Our C wrapper code includes this header directly, whereas the upstream patch only adds declarations to the internal `image-imp.h`. |
| `webp-load-webp-deviations-kobo.patch` | Cadmus            | All Cadmus-specific deviations from the upstream `source/fitz/load-webp.c`. See details below.                                                                                                                             |

## Provenance of the WebP patches

The WebP support originates from three sources:

1. **Ghostscript Bugzilla** – [bug #697749](https://bugs.ghostscript.com/show_bug.cgi?id=697749) contained an early patch proposal adding WebP support to MuPDF.

2. **KOReader** – The KOReader project maintained a cleaned-up version of that
   proposal in their `koreader-base` repository:
   `thirdparty/mupdf/webp-upstream-697749.patch`
   (see [koreader/koreader-base](https://github.com/koreader/koreader-base)).

3. **Cadmus** – Applied additional fixes on top of the KOReader patch.

### The `webp-upstream-697749-kobo.patch` is included verbatim

This file is byte-for-byte identical to KOReader's upstream patch so that the
provenance chain is unambiguous and easy to verify:

```bash
curl -L https://raw.githubusercontent.com/koreader/koreader-base/master/thirdparty/mupdf/webp-upstream-697749.patch | diff - thirdparty/mupdf/webp-upstream-697749-kobo.patch
```

The upstream patch touches the following files:

| File                                     | Change                                                                               |
| ---------------------------------------- | ------------------------------------------------------------------------------------ |
| `include/mupdf/fitz/compressed-buffer.h` | Adds `FZ_IMAGE_WEBP` to enum; extends `fz_recognize_image_format` from 8 to 12 bytes |
| `scripts/wrap/make_cppyy.py`             | Comment update for 12-byte recognition                                               |
| `source/cbz/mucbz.c`                     | Adds `.webp` to CBZ extension list                                                   |
| `source/cbz/muimg.c`                     | Extends `img_recognize_content` from 8 to 12 bytes                                   |
| `source/fitz/image-imp.h`                | Adds `fz_load_webp` / `fz_load_webp_info` declarations                               |
| `source/fitz/image.c`                    | WEBP switch cases, type-name mapping, RIFF+WEBP sniff, 12-byte buffer check          |
| `source/fitz/load-webp.c`                | **New file** — WebP decoder implementation (see deviations below)                    |
| `source/html/mobi.c`                     | Extends MOBI image detection from 8 to 12 bytes                                      |

### Cadmus deviations in `webp-load-webp-deviations-kobo.patch`

This single patch contains all changes we made to the upstream `load-webp.c`:

| Change                                                        | Why                                                                                                                                                                     |
| ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Remove `#ifdef HAVE_WEBP` guards                              | Cadmus always builds with libwebp, so the unconditional code is fine.                                                                                                   |
| Remove `#include <webp/types.h>`                              | Included transitively by `decode.h`/`demux.h`.                                                                                                                          |
| Remove unused `pages` field from `struct info`                | Not used in our reader.                                                                                                                                                 |
| Tighten `float_can_be_int` epsilon from `< 1` to `< 0.001f`   | A tolerance of `< 1` treated non-integers like `72.5` as integer-like and truncated them.                                                                               |
| Fix ICC warning message: `JPEG` → `WebP`                      | Incorrect format name in the warning string.                                                                                                                            |
| Fix demux memory leak (`fz_var(demux)` + `fz_always` cleanup) | The upstream code created `WebPDemuxer` but only deleted it on the normal success path. Any `fz_throw` (decode failure, pixmap allocation failure, etc.) would leak it. |
| Add animated WebP first-frame extraction                      | Extracts the first frame of animated WebP files using `WebPDemuxGetFrame`. The demux is kept alive during decode for this purpose.                                      |
| Fix `yres` copy-paste bug in `fz_load_webp_info`              | Upstream had `*yresp = info.xres` instead of `*yresp = info.yres`.                                                                                                      |
| Add `fz_always` colorspace cleanup in `fz_load_webp_info`     | Properly drops the colorspace on both success and failure paths.                                                                                                        |

### `webp-image-h-kobo.patch`

The upstream patch adds `fz_load_webp` / `fz_load_webp_info` declarations to the
internal header `source/fitz/image-imp.h`. Cadmus' C wrapper code (`mupdf_wrapper`)
includes `include/mupdf/fitz/image.h` directly, so the declarations must also be
present there. This patch adds them without conflicting with the upstream changes.
