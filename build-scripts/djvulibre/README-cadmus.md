# DjVuLibre — Cadmus Build Notes

Kobo cross-build only. The full upstream tree builds tools, share data, and
optional xmltools; Cadmus only needs `libdjvu` for the Kobo reader.

## Kobo patch

`kobo/000-kobo.patch` sets `SUBDIRS = libdjvu` so the cross Makefile does not
recurse into host-oriented targets. See [Patch tiers](../README.md#patch-tiers) for
tier layout and apply order.
