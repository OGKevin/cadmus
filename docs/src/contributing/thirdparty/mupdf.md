<!-- i18n:skip-start -->

# MuPDF

MuPDF backs Cadmus rendering for PDF, comic archives (CBZ/CBR), XPS/OXPS,
FictionBook, Mobipocket, plain text, and image formats opened via
`PdfOpener`. EPUB and HTML use Cadmus parsers; DjVu uses DjVuLibre. Routing is
in
<a href="/api/cadmus_core/document/fn.open.html">`cadmus_core::document::open`</a>.
Kobo builds link a patched `libmupdf.so`; host
builds use the native patch profile. Patch tables and WebP provenance are
maintained in the repository README below.

{{#include ../../../../build-scripts/mupdf/README.md:2:}}

<!-- i18n:skip-end -->
