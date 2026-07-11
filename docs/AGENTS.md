# Documentation

## User-Facing Docs (`docs/src/**/*.md` except `contributing/`)

Audience: end users with no technical background, using Cadmus.

### Tone

- Conversational and friendly.
- Active voice ("Copy the file", not "The file should be copied").
- No jargon:
  - "artifact" → "file" or "package"
  - "deploy" → "install" or "download"
  - "configure" → "set up"
  - "bundle" → "package"
  - "on-device" → "on your Kobo" or "wirelessly"

### Admonitions

```markdown
> [!NOTE]
> General information or additional context.

> [!TIP]
> A helpful suggestion or best practice.

> [!IMPORTANT]
> Key information that shouldn't be missed.

> [!WARNING]
> Critical information that highlights a potential risk.

> [!CAUTION]
> Information about potential issues that require caution.
```

Source: <https://rust-lang.github.io/mdBook/format/markdown.html?highlight=note#admonitions>

## Contributor Docs (`docs/src/contributing/**/*.md`)

Audience: developers and contributors. Technical terminology is appropriate.

- Clear and direct — get to the point.
- Include code examples where helpful.
- Document setup steps precisely.

## devenv.nix Documentation Sync

When modifying `devenv.nix`, update `docs/src/contributing/devenv-setup.md`:

- **Available Commands** table — if scripts in `scripts = { ... }` change.
- **Platform Support** — if `isLinux`/`isDarwin` conditionals change.
- **Observability Stack** — if services/ports change.
- **Troubleshooting** — for known platform-specific issues.

## Formatting

- Format and lint Markdown with **rumdl** (via `treefmt`); see `.rumdl.toml`.
- Docs markdown is excluded from Prettier (`.prettierignore`) to preserve i18n
  list nesting.
- Use code blocks with language tags.

## API doc links

Cargo-generated API pages are not present in the source tree; they are produced
during the docs portal build and symlinked into the deployed site at
`/{locale}/api/cadmus_core/`.

When linking to rustdoc pages from markdown under `docs/src/`, use absolute
HTML anchors — not relative markdown links:

```html
<a href="/api/cadmus_core/settings/struct.Settings.html">`Settings`</a>
```

Do **not** use relative paths such as
`[Settings](../../api/cadmus_core/settings/struct.Settings.html)`. rumdl rule
MD057 validates relative links against the filesystem and will fail in CI and
`treefmt` because `docs/api/` does not exist until after `cargo xtask docs`.

Absolute `/api/...` paths are site routes; rumdl skips them by default. Inline
HTML is allowed — MD033 is disabled globally in `.rumdl.toml`.

Do **not** embed a locale prefix (e.g. `en/`) in markdown links. Two mechanisms
route them at deploy/runtime:

1. **Direct URL access** — `cargo xtask docs` symlinks cargo-doc to
   `website/public/api/` (GitHub Pages) and `_redirects` splat rules redirect
   deep unprefixed paths to `/en/api/...` (Cloudflare Pages).
2. **In-guide clicks** — [`docs/lang-picker.js`](lang-picker.js) rewrites
   `a[href^="/api/"]` to `/{locale}/api/...` (and `/{basePath}/{locale}/api/...`
   on GitHub Pages) based on the current guide URL.

Examples in contributor docs:

- `docs/src/contributing/runtime-migrations.md`
- `docs/src/contributing/sqlite-sqlx.md`
- `docs/src/contributing/library-database.md`
