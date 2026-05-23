# Cadmus — Review Checklists

## DeepWiki Configuration (`.devin/wiki.json`)

Review `wiki.json` when a change introduces or removes a **significant system,
subsystem, or architectural concept** (new crate, new hardware target, new
document format, major subsystem rename/split/removal).

Bug fixes, refactors, and incremental feature work generally do not require an
update.

### Constraints

- Max 30 pages, 100 notes, 10 000 chars per note.
- Page titles must be unique and non-empty.

### Checklist

- [ ] Does any existing `purpose` field need updating?
- [ ] Does a new page need to be added (room within the 30-page limit)?
- [ ] Does `repo_notes` need updating?
- [ ] Are all page titles still unique?

## Feature Flag CI Matrix

When a PR adds a new Cargo feature flag, verify:

- [ ] `.github/workflows/cargo.yml` has new matrix entries in **both** `clippy`
      and `test` jobs for the feature alone and in combination with every other
      feature.
- [ ] `--all-features` is **not** the only coverage — `#[cfg(not(feature = "..."))]`
      paths must be tested individually.
