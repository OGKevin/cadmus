# Core Crate — Review Checklists

## Documentation

Follow the Documentation freshness section in [`REVIEW.md`](../../REVIEW.md).

Keep related samples and docs (for example settings) consistent with the code,
including app wiring outside this crate.

## User-Facing String Translations

When reviewing code that adds user-facing strings:

- [ ] No `"string literal".to_string()` or `format!("literal")` for user-visible text.
- [ ] New message IDs are in the appropriate `cadmus_core.ftl` comment section,
      alphabetically within sections that are already sorted.
- [ ] Parameterised messages use Fluent variable syntax in `.ftl`.
- [ ] `fl!` is used for literal IDs and `fl_or!` only for runtime IDs.
