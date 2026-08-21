# Cadmus — Review Checklists

## Documentation freshness

When a PR changes or introduces user-facing or contributor-facing behavior,
docs under `docs/src/` must stay accurate. Docs-area detail (POT, devenv,
formatting) lives in [`docs/REVIEW.md`](docs/REVIEW.md). Crate-local review
pointers live in nested `REVIEW.md` files.

### Update existing docs

When a change alters already-documented behavior, update the matching pages
under `docs/src/`.

### Add new docs

When a PR introduces a new user-facing or contributor-facing surface (feature,
workflow, setting category, install path, CLI/dev command, subsystem):

- **User-facing** — add or extend page(s) under `docs/src/**` outside
  `contributing/`, list them in `docs/src/SUMMARY.md`, and run POT sync per
  [`docs/REVIEW.md`](docs/REVIEW.md).
- **Contributor/dev** — add or extend page(s) under `docs/src/contributing/**`
  (and `SUMMARY.md`); follow tone in [`docs/AGENTS.md`](docs/AGENTS.md).
  Wrap pages in `<!-- i18n:skip-start -->` / `<!-- i18n:skip-end -->`.
  POT sync is not required for contributor-only changes.
- Prefer extending an existing page when the change is a small addition to an
  existing topic. Add a new page when it is a distinct workflow users or
  contributors must discover.

### Skip only with justification

Internal-only refactors, invisible bug fixes, or pure test/CI churn with no
user or contributor surface may skip docs updates. Silence is not a
justification — say why in the PR.

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
