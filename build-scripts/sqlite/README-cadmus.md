# SQLite — Cadmus Build Notes

## Why we build from source

Cadmus uses `DELETE … LIMIT` to batch-delete dictionary index entries
efficiently. This SQL syntax requires the SQLite parser to have been
generated with `SQLITE_ENABLE_UPDATE_DELETE_LIMIT`, which bakes in
`SQLITE_UDL_CAPABLE_PARSER`. The standard amalgamation shipped by
`libsqlite3-sys` and system SQLite packages do **not** include this
flag, so the syntax is always rejected at parse time.

Building from the [canonical source tree][sqlite-gh] with
`./configure --enable-update-limit` regenerates the parser via the
Lemon tool (requires TCL) and produces a UDL-enabled amalgamation that
supports the extended syntax.

[sqlite-gh]: https://github.com/sqlite/sqlite

## Submodule

The submodule lives at `thirdparty/sqlite` and is pinned to a release
tag (e.g. `version-3.49.2`) in `.gitmodules`. Renovate tracks the
submodule automatically via its `git-submodules` manager.

## Build flow

```text
cargo xtask setup [--host | --kobo | --all | --target <triple>]
  └─ build_deps::build::sqlite::ensure_sqlite()
       ├─ cp_r thirdparty/sqlite → target/cadmus-build-deps/<TARGET>/sqlite/
       ├─ ./configure --enable-update-limit
       ├─ make sqlite3.c sqlite3.h         (UDL-enabled amalgamation)
       └─ cc -c sqlite3.c → ar rcs libsqlite3.a
            output → target/cadmus-build-deps/<TARGET>/sqlite/{lib,include}/
```

The Kobo cross-build (`cargo xtask build-kobo`) calls `ensure_sqlite`
automatically for the `arm-unknown-linux-gnueabihf` target.

## Compile-time defines

| Flag                                | Purpose                                         |
| ----------------------------------- | ----------------------------------------------- |
| `SQLITE_ENABLE_UPDATE_DELETE_LIMIT` | Enables `DELETE … LIMIT` / `UPDATE … LIMIT`     |
| `SQLITE_DEFAULT_WAL_SYNCHRONOUS=1`  | WAL mode uses NORMAL sync (faster writes)       |
| `SQLITE_OMIT_DEPRECATED`            | Removes deprecated API symbols                  |
| `SQLITE_DQS=0`                      | Disallows double-quoted string literals         |
| `SQLITE_DEFAULT_MEMSTATUS=0`        | Disables memory usage tracking (lower overhead) |
| `SQLITE_LIKE_DOESNT_MATCH_BLOBS`    | `LIKE` skips BLOBs (marginal speedup)           |
| `SQLITE_OMIT_SHARED_CACHE`          | Removes shared-cache mode (unused by Cadmus)    |

## Environment variables

`libsqlite3-sys` discovers the custom build via:

| Variable              | Value                                              |
| --------------------- | -------------------------------------------------- |
| `SQLITE3_LIB_DIR`     | `target/cadmus-build-deps/<TARGET>/sqlite/lib`     |
| `SQLITE3_INCLUDE_DIR` | `target/cadmus-build-deps/<TARGET>/sqlite/include` |
| `SQLITE3_STATIC`      | `1`                                                |

For native development these are set automatically by `devenv.nix`.
For Kobo cross-compilation they are injected via `CROSS_ENV` in
`crates/build-deps/src/versions.rs` and the `build-kobo` GitHub Action.
