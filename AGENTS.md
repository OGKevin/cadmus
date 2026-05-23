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
