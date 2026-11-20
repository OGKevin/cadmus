# Rust Crates — Agent Coding Conventions

## Rust Conventions

- Prefer `?` over `unwrap()` / `expect()` in library and app code.
- In async code paths, use the async API when one exists: `tokio::fs`,
  `tokio::process::Command`, `tokio::net`, `tokio::time::sleep`. Reach for
  `spawn_blocking` only for work with no async equivalent — subprocess I/O,
  ioctls, hashing, large parses.
- Never call a blocking function (`std::fs`, `Path::exists`, `std::thread::sleep`,
  a sync `Mutex` guard held across an `.await`) from an `async fn` on the Tokio
  worker pool, and never block the main thread. Details of the runtime's cost
  model live in [`core/AGENTS.md`](core/AGENTS.md).
- Use a [`Job`](core/src/runtime.rs) for work whose lifetime and cancellation
  are owned by the subsystem that started it. Detached `runtime::spawn` is
  allowed only when ownership already lives elsewhere. See
  [`core/AGENTS.md`](core/AGENTS.md) for the ownership rule and
  [`core/src/device/AGENTS.md`](core/src/device/AGENTS.md) for the
  device-handler test harness.
- Async methods on a **public** trait must state their future's auto-trait
  bounds; prefer the stable RPITIT form. The decision tree lives in
  [`core/AGENTS.md`](core/AGENTS.md).
- Use `thiserror` for custom error types and `anyhow` for ad-hoc errors.
- Use iterators over index-based loops.
- Use `&str` over `String` in function parameters when ownership is not needed.
- Prefer borrowing over cloning.
- Inline expressions directly into struct fields — avoid intermediate bindings
  solely to pass them into a struct literal.
- Avoid `unsafe` unless required and documented.
- Avoid premature `collect()` — keep iterators lazy.
- Ensure code compiles without warnings.

## Visibility

Use the most restrictive visibility that works:

- Prefer private `mod` over `pub mod`, and `pub(crate)` over `pub`.
- Use `pub` only when non-doc code outside the parent module needs the item.
  Check external references before widening visibility.
- Do not widen visibility for rustdoc links. Feature-gated `docsrs` modules can
  stay private; `cadmus-core` allows `rustdoc::private_intra_doc_links`.

## Newtype Wrappers

Wrap primitives that represent domain concepts: `Fingerprint(String)` is safer
and clearer than a bare `String`, and enum variants are safer than string
comparisons.

- Implement the parsing, formatting, and conversion traits used by call sites.
- Match on enum variants so the compiler catches missing arms.

## Code Comments

Comment **why**, not **what**. Most code needs no comments — use good naming.

- **No inline comments** — if an inline comment feels necessary, extract the
  code into its own well-named, documented function instead.
- No dead-code comments (commented-out code).
- No changelog comments (`Modified by X on date`).
- No divider comments (`//=====`).
- Annotations (`TODO`, `FIXME`, `HACK`, `NOTE`) are fine with context.

## Structured Logging

Put log data in structured `tracing` fields, not format strings or `[Module]`
prefixes; instrumentation scope supplies module context. Do not combine
structured fields with format args.

```rust
tracing::debug!(pr_number, count, "Found artifacts");
tracing::debug!(url = %url, headers = ?headers, "Fetched artifacts");
```

Use direct fields for primitives, `%` for `Display`, and `?` for `Debug`.

- `debug` — development and troubleshooting detail
- `info` — important runtime events
- `warn` — recoverable issues such as retries and fallbacks
- `error` — failures requiring attention

On Tokio worker tasks, do not hold a `tracing::Span::enter()` guard across an
`.await`: the guard is not `Send` and the span can leak into unrelated work.
Use `.instrument(span)` or `#[tracing::instrument]` so the span is active only
while that future polls.

## SQLite / sqlx Conventions

Return domain types directly from typed query macros instead of parsing
primitives at call sites. Implement `sqlx::Type`, `Encode`, and `Decode` for
newtypes, and use column overrides such as:

```rust
sqlx::query!(r#"SELECT fingerprint AS "fingerprint: Fp" FROM books"#)
```

When the orphan rule prevents implementations for types such as `PathBuf` or
`SystemTime`, convert at the database function boundary. Extract a newtype if
the same conversion recurs. Existing implementations live in
`core/src/db/types.rs` and `core/src/helpers.rs`.

Regenerate `.sqlx/` with the `sqlx` skill after changing query macros or
migrations.

## Cargo Dependencies

- Define versions in root `Cargo.toml` under `[workspace.dependencies]`.
- Use caret requirements for most dependencies.
- Alphabetize dependencies within logical groups.
- Use `default-features = false` when fine-grained control is needed.
- Keep related crate families at compatible versions (e.g. all
  `opentelemetry-*` crates).
- After modifying `Cargo.toml`, run the [root verification
  steps](../AGENTS.md#testing) (see the `build-cadmus-native` skill).

## Feature Flags and CI Matrix

`../xtask/src/tasks/util/matrix.rs` scans workspace manifests (skipping
`xtask/`, `thirdparty/`, and `target/`) and generates valid feature
combinations, each prefixed with one device feature. `cadmuscli ci matrix`
emits the dynamic matrices consumed by both `clippy` and `test` in
`../.github/workflows/cargo.yml`; do not add hand-written matrix entries to
that workflow.

When adding a feature flag:

1. Add it to the relevant `Cargo.toml` under `crates/`; the generator includes
   non-excluded features automatically (`default`, `bench`, `docs`,
   `telemetry`, `device`, and the device-list features are excluded).
2. Run `cadmuscli ci matrix` and confirm each compatible device has an entry
   where the new feature is the only non-device feature. Update the expected
   entry count in `scan_workspace_includes_runtime_features_on_ubuntu`
   (`matrix.rs`), which asserts the total; also update the generator and its
   tests when changing exclusions or validity rules.
3. Test the affected combination locally with a device feature. `--all-features`
   does not cover `#[cfg(not(feature = "..."))]` paths.

## User-Facing String Tests

- When testing strings rendered through `fl!`, build expected strings with
  `fl!` too. Fluent may add Unicode isolation marks around variables, so raw
  string literals can fail even when visible text matches.
