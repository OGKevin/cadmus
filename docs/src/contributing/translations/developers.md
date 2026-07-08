<!-- i18n:skip-start -->

# Translations — for developers

This page is for contributors who change English source strings or
documentation. Translators should use [Crowdin](https://crowdin.com/project/cadmus)
or see the main [Translations](index.md) page.

## Adding UI strings

1. Add the message to
   [`crates/core/i18n/en-GB/cadmus_core.ftl`](https://github.com/ogkevin/cadmus/blob/master/crates/core/i18n/en-GB/cadmus_core.ftl)
   (kebab-case IDs, sorted within sections — see
   [`crates/core/AGENTS.md`](https://github.com/ogkevin/cadmus/blob/master/crates/core/AGENTS.md)).
2. Use `fl!("message-id")` or `fl!("id", var = value)` in Rust.
3. Other languages are filled in on Crowdin — do not hand-edit locale FTL files
   unless you have a specific reason.
4. Run `cargo check -p cadmus-core` to validate message IDs at compile time.

```rust
let label = crate::fl!("my-new-message");
let label = crate::fl!("books-loaded", count = book_count);
```

## Updating documentation translation sources

After changing **user-facing** English docs (`docs/src/**/*.md` outside
`contributing/`):

```bash
cadmus-translate   # devenv
# or: MDBOOK_OUTPUT='{"xgettext": {}}' mdbook build -d docs/po docs
```

Commit the updated `docs/po/messages.pot` alongside your doc edits. Crowdin
picks up the new template; translators update locale `.po` files there.

When changing website strings, update
[`website/messages/en.json`](https://github.com/ogkevin/cadmus/blob/master/website/messages/en.json)
only — Crowdin handles `website/messages/<lang>.json`.

## You do not need to maintain translated files

After changing English source strings, do **not** hand-edit `docs/po/*.po`,
`crates/core/i18n/*/*.ftl`, or `website/messages/*.json` for other locales.

Outdated or fuzzy entries in those files are reconciled on Crowdin and synced
back via the `crowdin` CI workflow (`skip_untranslated_strings: true` on
export). Missing translations fall back to English at runtime or build time
until translators catch up.

## Excluding documentation from extraction

These directives apply to the mdBook user guide only. They tell
`cadmus-translate` which Markdown blocks to omit from `docs/po/messages.pot`.

Use `<!-- i18n:skip -->` before a single block (paragraph, code block, table):

<!-- i18n:skip -->

```markdown
<!-- i18n:skip -->

This paragraph will not appear in the POT file.
```

Use `<!-- i18n:skip-start -->` / `<!-- i18n:skip-end -->` to exclude multiple
consecutive blocks:

<!-- i18n:skip -->

```markdown
<!-- i18n:skip-start -->

This paragraph is excluded.

And so is this one.

<!-- i18n:skip-end -->
```

When skip directives appear inside a list, indent them to match the list
continuation level:

<!-- i18n:skip -->

```markdown
1. Step one.

   <!-- i18n:skip-start -->

   | Path           | Value  |
   | -------------- | ------ |
   | `/mnt/onboard` | stable |

   <!-- i18n:skip-end -->

2. Step two.
```

## Previewing translated docs locally

```bash
cargo xtask docs
cadmus-docs-serve
```

Use this only to verify builds — translation work happens on Crowdin.

<!-- i18n:skip-end -->
