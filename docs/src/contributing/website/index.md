<!-- i18n:skip-start -->

# Website

Cadmus ships a unified static site that combines the Next.js landing page, translated mdBook
user guide, Rust API docs, and Storybook component gallery. The site is built by
`cargo xtask docs` and written to `website/out/`.

For editing the mdBook user guide itself, see [User guide](user-guide.md).

## Site structure

All paths are locale-first (see [`website/i18n/routing.ts`](https://github.com/ogkevin/cadmus/blob/master/website/i18n/routing.ts)
and [`website/lib/doc-hrefs.ts`](https://github.com/ogkevin/cadmus/blob/master/website/lib/doc-hrefs.ts)):

- **Landing**: `/{locale}/` (e.g. `/en/`, `/fr/`)
- **User guide**: `/{locale}/guide/` (mdBook HTML symlinked into `website/public/`)
- **API reference**: `/{locale}/api/cadmus_core/` (cargo doc)
- **Storybook**: `/{locale}/storybook/`

### Deployed URLs

- **Cloudflare Pages**: <https://cadmus-dt6.pages.dev/>
- **PR previews**: `https://pr-{NUMBER}.cadmus-dt6.pages.dev/` (branch `pr-{NUMBER}` in CI)
- **GitHub Pages (mirror)**: <https://ogkevin.github.io/cadmus/> (built with
  `NEXT_PUBLIC_BASE_PATH=/cadmus`)

Legacy paths `/guide/`, `/api/`, and `/storybook/` redirect to `/en/...` via
[`website/public/_redirects`](https://github.com/ogkevin/cadmus/blob/master/website/public/_redirects)
on Cloudflare Pages and generated HTML redirects on GitHub Pages.

## Local development

```bash
devenv shell
cargo xtask docs        # full static site → website/out/
cadmus-docs-serve       # Next.js dev server → http://localhost:3000
```

**Full-site preview** (matches CI):

```bash
cargo xtask docs
npx wrangler pages dev website/out
```

**Fast iteration**:

- Website UI: `cd website && npm run dev`
- Storybook: `cd website && npm run storybook` (port 6006)
- `cadmus-docs-serve` reminds you to run `cargo xtask docs --mdbook-only` first if guide
  content is missing

Prerequisites: devenv provides `mdbook`, `mdbook-mermaid`, and `npm`; the `website:install`
devenv task handles `npm install`.

## Editing website content

### Components and Storybook

Build the website from **small, reusable components** — one concern per component under
`website/components/`. Each component should have a matching Storybook story
(`index.stories.tsx` next to `index.tsx`) so it can be developed and reviewed in isolation.

Pages (`website/components/pages/`) and larger components compose these building blocks.
For example, the landing page assembles `Cadmus`, action buttons, `DocGrid`, and `SiteFooter`
from their own components rather than inlining markup.

Preview components locally:

```bash
cd website && npm run storybook   # http://localhost:6006
```

Run `cd website && npm run lint` before opening a PR.

### Translatable UI text

**Every user-visible string** in a website component must be translatable. Do not hard-code
copy in JSX.

1. Add the English string to
   [`website/messages/en.json`](https://github.com/ogkevin/cadmus/blob/master/website/messages/en.json)
   (group keys by component or section).
2. Read it in the component with [next-intl](https://next-intl.dev):
   `useTranslations("section")` and `t("key")`.
3. Other locales are filled in on Crowdin — do not hand-edit `website/messages/<lang>.json`.

See [Translations](../translations/index.md) for the Crowdin workflow and
[Translations — for developers](../translations/developers.md) for adding website strings.

| What you change  | Where                         | Follow-up                                |
| ---------------- | ----------------------------- | ---------------------------------------- |
| UI copy          | `website/messages/en.json`    | [Translations](../translations/index.md) |
| Component markup | `website/components/**`       | `index.stories.tsx`, `npm run lint`      |
| Page layout      | `website/components/pages/**` | Compose existing components              |

## Build pipeline

`cargo xtask docs` (see [`xtask/src/tasks/docs.rs`](https://github.com/ogkevin/cadmus/blob/master/xtask/src/tasks/docs.rs))
runs:

1. Install mdbook-mermaid assets
2. Build English mdBook (`docs/book/html/`)
3. Build translated mdBooks per `docs/po/*.po`
4. `cargo doc` → `target/doc/`
5. Inject git version into API HTML
6. Write `website/public/_shared/locales.json`
7. Run `generate-version.mjs` + `generate-locales.mjs`
8. Build Storybook → `website/storybook-static/`
9. Symlink guide/api/storybook into `website/public/{locale}/`
10. `next build` (static export) → `website/out/`
11. Deduplicate shared api/storybook trees via symlinks in `website/out/_shared/`

`--mdbook-only` stops after step 3 (see [User guide](user-guide.md)).

## CI and deployment

[`.github/workflows/cadmus-docs.yml`](https://github.com/ogkevin/cadmus/blob/master/.github/workflows/cadmus-docs.yml)
builds and deploys the site:

- Triggers on changes to `docs/**`, `website/**`, `crates/**/*.rs`, the doc-tools action, and
  the workflow file
- `cargo xtask docs` → artifact `website/out`
- Separate GitHub Pages rebuild with `NEXT_PUBLIC_BASE_PATH=/cadmus`
- PR previews on Cloudflare Pages for non-fork PRs (fork PRs still build; no preview deploy)

## Reviewing changes

When you open a pull request that modifies website or documentation files, a preview deployment
is created automatically for non-fork PRs. The PR checks panel shows a link to the preview URL
(`https://pr-{NUMBER}.cadmus-dt6.pages.dev/`).

<!-- i18n:skip-end -->
