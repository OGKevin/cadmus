# Build scripts layout

Each third-party library under `thirdparty/` may carry Cadmus-specific files
under `build-scripts/<lib>/`.

## Patch tiers

Patched libraries use three standard tier directories:

| Tier       | Role                                                                 |
| ---------- | -------------------------------------------------------------------- |
| `generic/` | Patches shared by every build target (host and Kobo).                |
| `native/`  | Host-only patches. May be empty (use `.gitkeep` so the tier exists). |
| `kobo/`    | Kobo cross-compile patches.                                          |

[`build-deps`](../crates/build-deps/build.rs) scans these folders at compile
time and generates a [`PatchTier`](../crates/build-deps/src/patches.rs) enum.
At build time, [`PatchProfile::Native`](../crates/build-deps/src/patches.rs)
applies the `native` stack (`generic` + `native`); [`PatchProfile::Kobo`](../crates/build-deps/src/patches.rs)
applies the `kobo` stack (`generic` + `kobo`). Patches from the stack are
merged and applied in sorted file-name order (use numeric prefixes to order
across tiers).

Libraries without patches omit the tier directories entirely.
