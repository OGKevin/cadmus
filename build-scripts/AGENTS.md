# Build scripts — Agent Context

Cadmus-specific build inputs for thirdparty libraries live here, one
subdirectory per library under `thirdparty/`.

## Patch tiers

Patched libraries use three standard tier directories:

| Tier       | Role                                                                 |
| ---------- | -------------------------------------------------------------------- |
| `generic/` | Patches shared by every build target (host and Kobo).                |
| `native/`  | Host-only patches. May be empty (use `.gitkeep` so the tier exists). |
| `kobo/`    | Kobo cross-compile patches.                                          |

[`build-deps`](../crates/build-deps/build.rs) scans these folders at compile
time and generates [`PatchTier`](../crates/build-deps/src/patches.rs). At build
time, [`PatchProfile::Native`](../crates/build-deps/src/patches.rs) uses
[`PatchTier::stack`](../crates/build-deps/src/patches.rs) for the `native`
tier (`generic` + `native`); [`PatchProfile::Kobo`](../crates/build-deps/src/patches.rs)
uses [`PatchTier::stack`](../crates/build-deps/src/patches.rs) for the `kobo`
tier (`generic` + `kobo`). Patches from the stack merge and apply in sorted
file-name order (numeric prefixes order across tiers).

Human-oriented overview: [`README.md`](README.md).

Libraries without patches omit the tier directories entirely.
