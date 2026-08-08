# Documentation — Review Checklists

## Translations POT Sync

When any English doc source (`docs/src/**/*.md`) is modified — including **new**
user-facing pages — the POT file must be regenerated. New user-facing pages must
also appear in `docs/src/SUMMARY.md`.

### Checklist

- [ ] New user-facing pages are listed in `docs/src/SUMMARY.md`.
- [ ] `docs/po/messages.pot` is updated in the same commit or PR.
- [ ] New or changed English strings appear in `messages.pot`.
- [ ] Removed strings are no longer present in `messages.pot`.

## devenv.nix Sync

When `devenv.nix` changes, update `docs/src/contributing/devenv-setup.md`:

- **Available Commands** table — if scripts in `scripts = { ... }` change.
- **Platform Support** — if `isLinux`/`isDarwin` conditionals change.
- **Observability Stack** — if services/ports change.
- **Troubleshooting** — for known platform-specific issues.

### Checklist

- [ ] New scripts documented in "Available Commands" table.
- [ ] Platform limitations documented in "Platform Support" section.
- [ ] New services/ports documented in "Observability Stack" section.
- [ ] Breaking changes noted in "Troubleshooting" section.

## Formatting

- [ ] Markdown formatted and linted with rumdl via treefmt (`.rumdl.toml`).
- [ ] Docs markdown stays excluded from Prettier (`.prettierignore`) so i18n
      list nesting is preserved.
