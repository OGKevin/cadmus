# Core Crate — Review Checklists

## Documentation

Follow the Documentation freshness section in [`REVIEW.md`](../../REVIEW.md).

When reviewing changes in this crate (and related app wiring), check that
`docs/src/` still describes the user-facing and contributor-facing behavior
accurately — update existing pages, add pages for new surfaces, or justify
skipping. Keep related samples and docs (for example settings) consistent with
the code.

## User-Facing String Translations

When reviewing code that adds user-facing strings:

- [ ] No `"string literal".to_string()` or `format!("literal")` for user-visible text.
- [ ] New message IDs added to `cadmus_core.ftl` in the correct sorted section.
- [ ] Parameterised messages use Fluent variable syntax in `.ftl`.
- [ ] `fl!` macro is used at every call site.
