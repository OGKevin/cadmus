# Documentation — Review Checklists

## Translations POT Sync

When any English doc source (`docs/src/**/*.md`) is modified, the POT file must
be regenerated.

### Checklist

- [ ] `docs/po/messages.pot` is updated in the same commit or PR.
- [ ] New or changed English strings appear in `messages.pot`.
- [ ] Removed strings are no longer present in `messages.pot`.

## devenv.nix Sync

When `devenv.nix` changes, verify `docs/src/contributing/devenv-setup.md`:

- [ ] New scripts documented in "Available Commands" table.
- [ ] Platform limitations documented in "Platform Support" section.
- [ ] New services/ports documented in "Observability Stack" section.
- [ ] Breaking changes noted in "Troubleshooting" section.

## Formatting

- [ ] Markdown formatted with Prettier.
- [ ] Markdown passes rumdl (`.rumdl.toml`).
