<!-- i18n:skip-start -->

# Website

Cadmus ships a unified static site that combines the Next.js landing page, translated mdBook
user guide, Rust API docs, and Storybook component gallery. The site is built by
`cadmuscli docs` and written to `website/out/`.

For editing the mdBook user guide itself, see [User guide](user-guide.md).

## Site structure

Entry points are locale-first (see [`website/i18n/routing.ts`](https://github.com/ogkevin/cadmus/blob/master/website/i18n/routing.ts)
and [`website/lib/doc-hrefs.ts`](https://github.com/ogkevin/cadmus/blob/master/website/lib/doc-hrefs.ts)):

- **Landing**: `/{locale}/` (e.g. `/en/`, `/fr/`)
- **User guide**: `/{locale}/guide/` (mdBook HTML symlinked into `website/public/`)
- **API reference**: `/{locale}/api/cadmus_core/` (cargo doc)
- **Storybook**: `/{locale}/storybook/`

The API reference and Storybook are deployed once, under `/_shared/`, and served at
`/{locale}/api/*` and `/{locale}/storybook/*` through `200` proxy rules in the generated
`_redirects`. Directory URLs keep the locale prefix. Page URLs ending in `.html` (rustdoc
pages, Storybook's `iframe.html`) are redirected by Cloudflare to extensionless
`/_shared/...` URLs, so deep links end up there rather than under `/{locale}/`. Bare
`/{locale}/api` and `/{locale}/api/` redirect to the `cadmus_core` crate index, because
rustdoc has no index page at its root.

### Deployed URLs

- **Production (Cloudflare Workers)**: <https://cadmus.ogkevin.nl/>
  (`cadmus.5f27h4ytfw.workers.dev` is the same Worker)
- **PR previews**: `wrangler versions upload --preview-alias pr-{NUMBER}`
  (pull requests whose head is this repository; fork PRs still build with no preview deploy)
- **GitHub Pages**: <https://ogkevin.github.io/cadmus/> redirects to
  `https://cadmus.ogkevin.nl/` (path after `/cadmus` is preserved)

Legacy paths `/guide`, `/guide/`, `/api/`, and `/storybook` (with or without a
trailing slash) redirect to `/en/...` via
[`website/public/_redirects`](https://github.com/ogkevin/cadmus/blob/master/website/public/_redirects)
(including splat rules for deep `/api/*` and `/storybook/*` paths). In-guide API
links are rewritten client-side by
[`docs/lang-picker.js`](https://github.com/ogkevin/cadmus/blob/master/docs/lang-picker.js)
to preserve the reader's locale.

## Local development

```bash
devenv shell
cadmuscli docs          # full static site → website/out/
cadmus-docs-serve       # Next.js dev server through `devenv up website`
cadmus-storybook        # Storybook through `devenv up storybook`
cadmus-docs-mdbook      # mdBook preview through `devenv up mdbook`
```

The dev servers are supervised devenv processes with allocated ports: Next
starts at 3001, mdBook at 3002, and Storybook at 6006. Read the resolved URLs
from `$WEBSITE_URL`, `$MDBOOK_URL`, and `$STORYBOOK_URL` (also printed by the
shell banner and `cadmus-process-status`) — a taken port moves the server.

`cadmus-docs-mdbook` installs the mdbook-mermaid assets itself and serves the
preview under `$DEVENV_STATE/mdbook-preview`, outside `docs/book/html` so it
does not conflict with the full `cadmuscli docs` build.

**Full-site preview** (matches CI):

```bash
cadmuscli docs
wrangler dev              # Workers Static Assets, same as production
# or: cadmus-docs-preview
```

**Supervised** (inside devenv): `devenv up` starts the website and Storybook on
allocated ports. Read the resolved URLs from `$WEBSITE_URL` and `$STORYBOOK_URL`.

**Fast iteration** (without devenv supervision; each command prints the URL it binds):

- Website UI: `cd website && npm run dev`
- Storybook: `cd website && npm run storybook`
- mdBook user guide: `cd docs && mdbook serve`

Inside devenv, prefer `cadmus-docs-serve`, `cadmus-storybook`, and
`cadmus-docs-mdbook`. The supervised processes wait for `website:install`,
start on allocated ports (so they can run alongside Grafana on 3000), and
generate the website locales first — as does a bare `npm run dev`, through
its `predev` hook.

Prerequisites: devenv provides `mdbook`, `mdbook-mermaid`, and `npm`; the `website:install`
devenv task handles `npm install`.

### Companion dev proxy

`npm run dev` starts `scripts/companion-dev-proxy.mjs` on a random `127.0.0.1` port and
passes its origin to the Next dev server as `COMPANION_PROXY_ORIGIN`; `next.config.ts`
rewrites to that origin and refuses to start without it. Next itself binds `127.0.0.1`
unless you pass `-H` or `--hostname`. Start the dev server through `npm run dev`, not a
bare `next dev`, or the rewrites have no origin. The proxy forwards only the RPCs the Companion page uses and refuses anything
else loudly: an unknown procedure logs the expected list and answers 404, and a request
whose `content-type` does not match the procedure (JSON for unary calls, `application/connect+json`
for `FileOps/GetFile`) logs the expected type and answers 415. A mismatch here means the
website and the proto contract drifted apart, not a transient failure.

The proxy finds Companion by probing `cadmus` process listeners on Linux, and by `lsof`
listeners on macOS. A wildcard bind wins (the emulator with Companion opened from the
menu), otherwise it probes each loopback listener with an unauthenticated
`cadmus.api.internal.v1.Placeholder/Ping`. Only the internal listener registers that RPC,
so the listener that does not answer it is Companion and is never the open internal API.
Auth answers cannot do this job: debug builds accept any bearer, so both listeners would
look identical. With no matching listener the proxy reports `{"companion":"down"}` instead
of forwarding to the wrong surface.

Set `COMPANION_API_PORT` to skip discovery and use that port directly. Other platforms
fail at startup and name this variable.

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
cadmus-storybook                  # devenv process, port 6006 base ($STORYBOOK_URL)
cd website && npm run storybook   # without devenv: http://localhost:6006
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

`cadmuscli docs` (see [`xtask/src/tasks/docs.rs`](https://github.com/ogkevin/cadmus/blob/master/xtask/src/tasks/docs.rs))
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
11. Deduplicate shared api/storybook trees: keep one copy in `website/out/_shared/`,
    remove the locale and top-level copies (the output has no symlinks, because
    artifact upload and the Cloudflare deploy would turn them back into copies), and
    append `200` proxy rules for `/{locale}/api/*` and `/{locale}/storybook/*` to
    `website/out/_redirects`

`--mdbook-only` stops after step 3 (see [User guide](user-guide.md)).

## CI and deployment

[`.github/workflows/cadmus-docs.yml`](https://github.com/ogkevin/cadmus/blob/master/.github/workflows/cadmus-docs.yml)
builds and deploys the site:

- Triggers on changes to `docs/**`, `website/**`, `crates/**/*.rs`, the doc-tools action,
  `wrangler.toml`, and the workflow file
- `cadmuscli docs` → artifact `website/out`
- Production: `wrangler deploy` (Workers Static Assets from `wrangler.toml`,
  with Workers Logs and traces enabled)
- PR previews: `wrangler versions upload --preview-alias pr-{NUMBER}` when the
  pull request head is this repository (fork PRs still build; no preview deploy)
- GitHub Pages: [`.github/workflows/cadmus-docs-gh-pages-redirect.yml`](https://github.com/ogkevin/cadmus/blob/master/.github/workflows/cadmus-docs-gh-pages-redirect.yml)
  deploys `.github/gh-pages-redirect/` so `ogkevin.github.io/cadmus` bookmarks
  reach `https://cadmus.ogkevin.nl/`. Fork repositories skip that deploy.

`CLOUDFLARE_API_TOKEN` must allow **Workers Scripts:Edit** (Cloudflare token template
**Edit Cloudflare Workers**). A Pages-only token will fail `wrangler deploy`.
`CLOUDFLARE_ACCOUNT_ID` is unchanged.

## Reviewing changes

When you open a pull request that modifies website or documentation files, a preview
deployment is created automatically when the pull request head is this repository.
The PR environment
`cloudflare-workers-preview` shows the preview URL from `wrangler versions upload`.

<!-- i18n:skip-end -->
