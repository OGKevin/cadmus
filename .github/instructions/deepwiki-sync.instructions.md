---
description: "Keep .devin/wiki.json in sync with significant codebase changes"
applyTo: "**"
---

# DeepWiki Configuration Sync

The DeepWiki steering configuration lives at `.devin/wiki.json`. It defines
the three-section structure of the generated wiki (User Guide, Contributor
Guide, Investigations) and the purpose of each page.

## When to update

Review `.devin/wiki.json` when a change introduces or removes a **significant
system, subsystem, or architectural concept** — i.e. something a new
contributor or user would need to find in the wiki. Examples:

- A new crate is added to the workspace
- A new top-level feature is introduced (e.g. a new document format, a new
  hardware target, a new extension mechanism)
- A major subsystem is renamed, split, or removed
- A new investigation is documented under `docs/src/investigations/`
- The build or release process changes substantially

Not every change requires an update. Bug fixes, refactors, and incremental
feature work within an existing subsystem generally do not.

## What to update

### `repo_notes`

Update the single `repo_notes` entry if the high-level description of the
project changes (e.g. a new supported hardware platform is added).

### `pages`

Each page has a `title`, `purpose`, and optionally `parent` and `page_notes`.

- Update the `purpose` of an existing page if the subsystem it describes has
  changed significantly.
- Add a new page if a new major system warrants its own wiki section and no
  existing page covers it.
- Remove a page if the system it describes no longer exists.

## Constraints to respect

The free-tier DeepWiki limits are:

- Maximum **30 pages** total
- Maximum **100 notes** total (`repo_notes` + all `page_notes` combined)
- Maximum **10,000 characters** per note
- Page titles must be **unique and non-empty**

Adding a page must not push the total over 30. If the limit is reached,
consolidate before adding.

## Review checklist

When reviewing a PR that introduces a significant system change:

- [ ] Does any existing `purpose` field need updating to reflect the change?
- [ ] Does a new page need to be added (and is there room within the 30-page limit)?
- [ ] Does `repo_notes` need updating?
- [ ] Are all page titles still unique?
