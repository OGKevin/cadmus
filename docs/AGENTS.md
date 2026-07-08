# Documentation — Agent Writing Conventions

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
