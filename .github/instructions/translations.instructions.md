---
description: "All user-facing strings must use Fluent translations"
applyTo: "**/*.rs,crates/core/i18n/**/*.ftl"
---

# User-Facing String Translations

All user-facing strings in Rust code must use the `fl!` macro from the
`crate::fl` re-export. Never use hardcoded string literals for labels, button
text, input placeholders, or any other text visible to the user.

## Rules

1. **No hardcoded strings** — every user-visible string must have a Fluent
   message ID in `crates/core/i18n/en-GB/cadmus_core.ftl`.
2. **Use `fl!` macro** — import via `use crate::fl;` and call `fl!("message-id")`.
3. **Parameterised strings** — use Fluent variables (`{ $var }`) in the `.ftl`
   file and pass them as keyword arguments to `fl!`. Example:
   `fl!("settings-reader-refresh-rate-by-kind-regular-input", ext = self.0.as_str())`

4. **Keep `.ftl` keys sorted** — message IDs in
   `crates/core/i18n/en-GB/cadmus_core.ftl` must remain in alphabetical order
   within each comment section.
5. **Naming convention** — use kebab-case, prefixed by the feature area:
   - `settings-<category>-<description>` for settings labels
   - `settings-<category>-<description>-input` for input field placeholder/labels
   - `notification-<description>` for notifications

## Where this applies

- `label()` implementations on `SettingKind` traits
- Input field `label` strings passed to `Event::OpenNamedInput`
- Menu entry text in `EntryKind::Command`, `EntryKind::RadioButton`, etc.
- Button labels, notification text, and any other user-visible string

## Adding a new string

1. Add the Fluent message to `crates/core/i18n/en-GB/cadmus_core.ftl` in the
   appropriate section, keeping IDs sorted.
2. Use `fl!("your-message-id")` at the call site.
3. If the message has variables, declare them in the `.ftl` file with
   `{ $varname }` syntax and pass values via `fl!("id", varname = value)`.

## Example

```rust
// crates/core/i18n/en-GB/cadmus_core.ftl
// settings-reader-refresh-rate = Refresh Rate
// settings-reader-refresh-rate-by-kind-regular-input = { $ext } regular refresh rate (0 = never)

// Rust usage
fn label(&self, _settings: &Settings) -> String {
    fl!("settings-reader-refresh-rate")
}

fn fetch(&self, settings: &Settings) -> SettingData {
    SettingData {
        widget: WidgetKind::ActionLabel(Event::OpenNamedInput {
            label: fl!(
                "settings-reader-refresh-rate-by-kind-regular-input",
                ext = self.0.as_str()
            ),
            // ...
        }),
        // ...
    }
}
```

## Review checklist

When reviewing code that adds user-facing strings:

- [ ] No `"string literal".to_string()` or `format!("literal")` for user-visible text
- [ ] New message IDs added to `cadmus_core.ftl` in the correct sorted section
- [ ] Parameterised messages use Fluent variable syntax in `.ftl`
- [ ] `fl!` macro is used at every call site
