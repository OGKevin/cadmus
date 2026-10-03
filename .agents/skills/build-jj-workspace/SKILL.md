---
name: build-jj-workspace
description: >
  Build, test, and lint Cadmus from a jj sibling workspace that has no .git
  (non-colocated checkout). Use when jj workspace root is a sibling tmp/agent
  workspace and builds fail with git submodule or .gitmodules errors, empty
  thirdparty/ sources, cache-marker misses, or missing sqlite.
---

# Build Cadmus in a non-colocated jj workspace

## Why a sibling workspace does not build out of the box

A jj sibling workspace (`jj workspace add`) is a **separate working copy** of
the same repository with **no `.git`**. Two consequences break the build:

- jj **ignores git submodules**, so every `thirdparty/<lib>` directory is
  empty (`mupdf`, `libwebp`, `harfbuzz`, `sqlite`, …).
- Cadmus builds through `build-deps`, which needs git:
  - `ensure_submodules` runs `git submodule update` for cold artifacts.
  - `markers` runs `git ls-tree HEAD thirdparty/<lib>` to resolve submodule
    revisions for cache markers.

With no `.git`, both fail (`fatal: not a git repository`). Even a warm
`target/` is re-validated via `git ls-tree`, so it does not help. The fix is to
give the build a git directory to read — without building in the colocated
checkout.

## Detect

```bash
ROOT="$(jj workspace root)"
COLOCATED="$(cd "$(jj workspace root --name default)" && pwd)"
test ! -e "$ROOT/.git" && echo non-colocated
```

Run every build, test, and lint command from `$ROOT`. `jj workspace root` is
the workspace owning the current working copy (the sibling directory), not the
default checkout. `$COLOCATED` is the default workspace — the one carrying
`.git`.

## The fix: point git at the colocated repo

Pass `GIT_DIR` (and `GIT_WORK_TREE` so submodules materialize into `$ROOT`) to
the cargo invocation:

```bash
cd "$ROOT"
devenv shell -- env -u RUSTC_WRAPPER \
  GIT_DIR="$COLOCATED/.git" \
  GIT_WORK_TREE="$ROOT" \
  cargo xtask setup --host
```

`cargo xtask setup --host` initializes the submodules into `$ROOT` and builds
SQLite under `$ROOT/target/cadmus-build-deps/<host-triple>/sqlite/`. Reuse the
same git env for later checks:

```bash
cd "$ROOT"
devenv shell -- env GIT_DIR="$COLOCATED/.git" GIT_WORK_TREE="$ROOT" \
  cargo xtask test --features emulator
```

Scope `GIT_DIR`/`GIT_WORK_TREE` to the build commands; do not export them for
the whole shell, and keep them off jj invocations.

### Caveats

- `git ls-tree HEAD` reads the **colocated** repo's HEAD, not `$ROOT`'s `@`.
  Fine while both revisions record the same submodule gitlinks.
- If a submodule step writes the git index, use a separate `GIT_INDEX_FILE`
  (see `jj-vcs`).
- jj 0.45.x has no `jj workspace add --colocate`; the git-env approach is the
  supported path here.

## Per-workspace `target/`

devenv sets `CARGO_TARGET_DIR` and `PKG_CONFIG_PATH_*` from
`config.devenv.root` (the workspace you entered), so each workspace has its own
`target/`. SQLite and the native deps (MuPDF, libwebp) build per workspace; the
first build in a sibling workspace is a full native build.

## sccache

devenv sets `RUSTC_WRAPPER=sccache`. If setup or a compile panics inside
sccache, retry that command with `env -u RUSTC_WRAPPER` (shown above).

## Checklist (in `$ROOT`)

1. `devenv shell -- cargo xtask docs --mdbook-only` if the EPUB is missing.
2. `cargo xtask setup --host` with `GIT_DIR`/`GIT_WORK_TREE` (above).
3. `devenv shell -- env GIT_DIR=… GIT_WORK_TREE=… cargo xtask test --features emulator`.

A device feature (`emulator`) is required; see `build-cadmus-native` for the
EPUB, feature, and daily-command details.

## Expected noise

- `git-hooks.nix: skipping hook installation: not a git repository` on devenv
  enter — expected.
- No `jj-pre-hook` / git pre-commit here; see `jj-commit`.

## Common mistakes

| Mistake                               | Result                                       | Fix                                              |
| ------------------------------------- | -------------------------------------------- | ------------------------------------------------ |
| Building in `$ROOT` without `GIT_DIR` | `not a git repository`, empty `thirdparty/`  | Pass `GIT_DIR`/`GIT_WORK_TREE` from `$COLOCATED` |
| Reusing colocated `SQLITE3_*` only    | Native deps still fail (empty `thirdparty/`) | Materialize submodules via the git env           |
| Assuming a shared `target/`           | Missing artifacts in the sibling             | Each workspace builds its own `target/`          |
| `cargo test` without a device feature | `compile_error!` in `cadmus-core`            | `--features emulator`                            |
| Leaving `GIT_DIR` exported for jj     | git-based tools read the wrong repo          | Scope it to the build commands                   |

## Related skills

- `build-cadmus-native` — EPUB, xtask commands, device features
- `jj-vcs` — GIT_DIR / GIT_INDEX_FILE caveats for non-colocated workspaces
- `jj-commit` — skip git pre-commit when `.git` is absent
