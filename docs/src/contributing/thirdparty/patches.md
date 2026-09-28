<!-- i18n:skip-start -->

# Third-party patch tiers

Cadmus builds vendored C/C++ libraries from git submodules under `thirdparty/`.
When upstream sources need changes, patches and build extras live under
`build-scripts/<lib>/`. The <a href="/api/build_deps/">`build-deps`</a> crate
applies them before configure or compile.

## Tier directories

Each patched library uses the same layout:

| Tier       | Role                                                                 |
| ---------- | -------------------------------------------------------------------- |
| `generic/` | Patches shared by host and Kobo builds.                              |
| `native/`  | Host-only patches.                                                   |
| `kobo/`    | Kobo ARM cross-compile patches.                                      |

Libraries with no Cadmus patches omit these folders entirely.

Any tier directory that has no `.patch` files yet should still contain a
`.gitkeep` so `build-deps` discovers the tier at compile time.

The `build-deps` build script scans `build-scripts/` at compile time and
generates a
<a href="/api/build_deps/patches/enum.PatchTier.html">`PatchTier`</a> enum from
the tier directory names it finds.

## Profiles and stacking

<a href="/api/build_deps/patches/enum.PatchProfile.html">`PatchProfile`</a>
selects which tiers merge for a build:

| Profile       | Stack applied        |
| ------------- | -------------------- |
| Native (host) | `generic` + `native` |
| Kobo (cross)  | `generic` + `kobo`   |

Implementation:
<a href="/api/build_deps/patches/enum.PatchTier.html#method.stack">`PatchTier::stack`</a>.

## Sorted apply order

Patches from the stacked tiers are collected, then sorted **globally by file
name** (not tier-by-tier). Use numeric prefixes (`000-`, `020-`, `100-`) so
ordering stays explicit across tiers. Example: `kobo/020-bar.patch` runs
before `generic/100-foo.patch` because `020` sorts before `100` by basename.

Logic:
<a href="/api/build_deps/patches/fn.sorted_patch_files.html">`sorted_patch_files`</a>.

## Already-applied detection

`patch` runs with `--forward`. If apply fails, `build-deps` checks whether the
full patch reverse-applies (`patch --reverse --dry-run --force`). Only then it
treats the patch as already present. Partial application still fails the build.

## `.patches-applied` marker

After a successful patch pass (for example MuPDF), `build-deps` writes
`.patches-applied` in the library build tree so later incremental builds skip
re-applying. When you change patch files, remove the marker or delete the
library output under `target/cadmus-build-deps/` and rebuild.

Constant:
<a href="/api/build_deps/markers/constant.PATCHES_APPLIED_MARKER.html">`PATCHES_APPLIED_MARKER`</a>.

## Kobo source copy and Meson cross files

Kobo builds copy each submodule into
`target/cadmus-build-deps/<target>/<lib>/`, apply the Kobo profile patches
there, then compile. **No** overlay copy of `build-scripts/` into the submodule
tree.

Meson needs a cross file (for example `build-scripts/mupdf/kobo-options.txt`).
Recipes pass the **absolute path** from the repo into Meson (`--cross-file=…`).
See the <a href="/api/build_deps/build/kobo/">`build_deps::build::kobo`</a>
module (source copy and recipes).

## Checklist: add or change a patch

1. Place the `.patch` under the correct tier (`generic/`, `native/`, or
   `kobo/`) with a sortable file name.
2. Regenerate or extend the patch against the **current** submodule commit.
3. If the library uses a patches marker, bump or clear it so CI and local
   builds re-apply.
4. Document provenance in `build-scripts/<lib>/README.md` (or
   `README-kobo.md` / `README-cadmus.md`) and link from
   [Third-party libraries](index.md) when the page exists.
5. Run `cargo xtask build-kobo` (or the relevant native build) before opening
   a PR.

## Further reading

- [`build-scripts/README.md`](../../../../build-scripts/README.md) — layout
  summary for maintainers.
- [`build-scripts/AGENTS.md`](../../../../build-scripts/AGENTS.md) — agent
  context and conventions.
- [`thirdparty/AGENTS.md`](../../../../thirdparty/AGENTS.md) — submodule
  upgrade notes and build order.

<!-- i18n:skip-end -->
