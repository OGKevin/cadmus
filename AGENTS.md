# Cadmus — Agent Coding Conventions

## Error Handling Philosophy

The emulator code path should **panic on errors** to catch issues early during
development. The app code path should **handle errors gracefully** for a smooth
user experience.

## Rust Conventions

- Prefer `?` over `unwrap()` / `expect()` in library and app code.
- Use `thiserror` for custom error types and `anyhow` for ad-hoc errors.
- Use iterators over index-based loops.
- Use `&str` over `String` in function parameters when ownership is not needed.
- Prefer borrowing over cloning.
- Inline expressions directly into struct fields — avoid intermediate bindings
  solely to pass them into a struct literal.
- Avoid `unsafe` unless required and documented.
- Avoid premature `collect()` — keep iterators lazy.
- Ensure code compiles without warnings.

## Newtype Wrappers

Prefer newtype wrappers over raw primitives for domain concepts. A
`Fingerprint(String)` is safer and more descriptive than a bare `String`.

```rust
// ✅ Good — the type documents intent and prevents misuse
struct Fingerprint(String);
struct FileExtension(/* variant enum */);

fn lookup(fp: &Fingerprint) { /* … */ }

// ❌ Bad — raw primitives are interchangeable by accident
fn lookup(fp: &str) { /* … */ }
```

### Guidelines

- Wrap `String`, `i64`, and similar primitives when the value represents a
  specific domain concept (fingerprints, file kinds, identifiers, …).
- Implement `Display`, `FromStr`, and other standard traits on the newtype so
  it stays ergonomic.
- Match on enum variants instead of string comparisons — the compiler catches
  missing arms.

## Code Comments

Comment **why**, not **what**. Most code needs no comments — use good naming.

- **No inline comments** — if an inline comment feels necessary, extract the
  code into its own well-named, documented function instead.
- No dead-code comments (commented-out code).
- No changelog comments (`Modified by X on date`).
- No divider comments (`//=====`).
- Annotations (`TODO`, `FIXME`, `HACK`, `NOTE`) are fine with context.

## Structured Logging

Use structured fields with the `tracing` crate. Never use string formatting for
log data.

```rust
// ✅ Good
tracing::debug!(pr_number, count, "Found artifacts");

// ❌ Bad
tracing::debug!("[OTA] Found {} artifacts for PR #{}", count, pr_number);
```

### Rules

- **Structured fields only** — data goes in fields, not format strings.
- **No prefixes** — no `[Module]` tags; instrumentation scope provides context.
- **No mixing** — don't combine structured fields with format args.

### Field formatters

| Formatter   | Use for                       | Example                                 |
| ----------- | ----------------------------- | --------------------------------------- |
| Direct      | Primitives (int, float, bool) | `tracing::debug!(count = 42, "msg");`   |
| Display `%` | Types implementing `Display`  | `tracing::debug!(url = %url, "msg");`   |
| Debug `?`   | Types implementing `Debug`    | `tracing::debug!(headers = ?h, "msg");` |

### Log levels

- `debug` — development and troubleshooting detail
- `info` — important runtime events
- `warn` — recoverable issues (retries, fallbacks)
- `error` — failures requiring attention

## SQLite / sqlx Conventions

Return domain types directly from queries instead of parsing primitives after
the fact.

### Implementing sqlx traits for a custom type

For a newtype to be used in `sqlx::query!()` results, implement three traits
delegating to the inner primitive:

```rust
impl sqlx::Type<sqlx::Sqlite> for MyType {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <String as sqlx::Type<sqlx::Sqlite>>::type_info()
    }

    fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for MyType {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        self.inner_string().encode_by_ref(buf)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for MyType {
    fn decode(
        value: sqlx::sqlite::SqliteValueRef<'r>,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<'r, sqlx::Sqlite>>::decode(value)?;
        s.parse().map_err(|e| /* convert to BoxDynError */)
    }
}
```

### Column type annotations

Use sqlx type-override syntax so the macro deserializes into the domain type
directly:

```rust
// ✅ Good — result field is already `Fp`
sqlx::query!(r#"SELECT fingerprint AS "fingerprint: Fp" FROM books"#)

// ❌ Bad — caller must parse the String manually
sqlx::query!(r#"SELECT fingerprint FROM books"#)
```

### Orphan rule — types you don't own

When the orphan rule prevents implementing sqlx traits (e.g. `PathBuf`,
`SystemTime`), convert at the database function boundary so callers never see
the primitive:

```rust
// The query returns a String, but the public function returns PathBuf.
pub fn get_book_path(&self, fp: &Fp) -> Result<PathBuf, Error> {
    let row = sqlx::query!(
        r#"SELECT file_path AS "file_path!: String" FROM library_books WHERE book_fingerprint = ?"#,
        fp,
    )
    .fetch_one(&self.pool)
    .await?;

    Ok(PathBuf::from(row.file_path))
}
```

If the same conversion appears in many places, create a newtype wrapper instead
(e.g. `UnixTimestamp(i64)`) and implement the sqlx traits on it.

## Cargo Dependencies

- Define versions in root `Cargo.toml` under `[workspace.dependencies]`.
- Use caret requirements for most dependencies.
- Alphabetize dependencies within logical groups.
- Use `default-features = false` when fine-grained control is needed.
- Keep related crate families at compatible versions (e.g. all
  `opentelemetry-*` crates).
- After modifying `Cargo.toml`, run:

  ```bash
  cargo xtask clippy
  cargo xtask test --features default
  ```

## Feature Flags and CI Matrix

The `clippy` and `test` jobs in `.github/workflows/cargo.yml` use a matrix to
check every feature flag individually and in combination. `--all-features` alone
misses `#[cfg(not(feature = "..."))]` paths.

### When adding a new feature flag

1. Add the feature to the relevant `Cargo.toml`.
2. Open `.github/workflows/cargo.yml` and add matrix entries to **both** the
   `clippy` and `test` jobs for:
   - The feature on its own
   - The feature combined with every other feature
3. Workspace-wide features:

   ```yaml
   - features: <name>
     cargo_args: "--workspace --all-targets --features <name>"
   ```

4. Crate-specific features:

   ```yaml
   - features: <name>
     cargo_args: "-p <crate> --all-targets --features <name>"
   ```

5. All combinations run on `ubuntu-latest`.
6. Only `default` and `test` features produce build artifacts.

## OTA and Asset Build Order

- OTA updates delete only Cadmus-owned bundled files before reboot. Do not
  assume a whole asset directory can be removed because users may add their own
  fonts, icons, or other files.
- `libs/` is special: all Cadmus-shipped shared libraries may be cleaned before
  reboot because the next `KoboRoot.tgz` repopulates that directory.
- If code generates compile-time metadata from bundled asset directories, make
  sure `bin/`, `resources/`, and `hyphenation-patterns/` are present before the
  Kobo build starts. In CI, `cargo xtask download-assets` must run before
  `cargo xtask build-kobo` for the generated asset list to stay accurate.
