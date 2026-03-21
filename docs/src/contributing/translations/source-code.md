# Translating Source Strings

The Cadmus UI uses [Fluent] for all user-visible strings. Translations are
embedded directly into the binary at compile time — no external files are
needed on the device.

## How it works

- FTL files live under `crates/core/i18n/<lang-tag>/cadmus_core.ftl`.
- The fallback language is **en-GB**; any string missing from a translation
  falls back to the English text automatically.
- `crates/core/src/i18n.rs` loads the correct language at startup based on
  `settings.locale`.
- The `fl!("message-id")` macro resolves message IDs at **compile time**.

## Adding a new language

### 1. Create the FTL file

Create a new file at the path matching the BCP 47 tag for your language:

```text
crates/core/i18n/<lang-tag>/cadmus_core.ftl
```

For example, for French:

```text
crates/core/i18n/fr/cadmus_core.ftl
```

### 2. Copy and translate the English strings

Use the English fallback file as your starting point:

```text
crates/core/i18n/en-GB/cadmus_core.ftl
```

Translate each message value. The message ID (left of `=`) must stay
unchanged — only the value (right of `=`) changes:

```fluent
# en-GB
startup-loading = Cadmus starting up…

# fr
startup-loading = Chargement de Cadmus…
```

### 3. Set the locale in Settings

To activate the new language locally during development, add a `locale` key to
your `Settings.toml`:

```toml
locale = "fr"
```

### 4. Build and verify

```bash
cargo check -p cadmus-core
```

The `fl!()` macro validates all message IDs at compile time. A successful build
confirms the FTL file is well-formed and all IDs referenced in code are present.

## Adding a string to the source code

When you add a new UI string:

1. Add the message to `en-GB`, and any other languages you know how to translate
   it into.
2. Use the `fl!()` macro in the Rust source:

   ```rust
   let label = crate::fl!("my-new-message");
   ```

3. For messages with variables, use named arguments:

   ```fluent
   # In the FTL file
   books-loaded = Loaded { $count } books
   ```

   ```rust
   let label = crate::fl!("books-loaded", count = book_count);
   ```

## FTL file format

Fluent uses a straightforward syntax. A few rules to keep in mind:

- Message IDs use `kebab-case`.
- Values can span multiple lines by indenting continuation lines.
- Use Unicode characters directly — no escaping needed.
- Comments start with `#`.

For the full syntax reference see the [Fluent syntax guide].

[Fluent]: https://projectfluent.org
[Fluent syntax guide]: https://projectfluent.org/fluent/guide/
