---
description: "Development environment documentation sync policy"
applyTo: "devenv.nix"
---

# Development environment documentation

The devenv setup guide lives at `docs/src/contributing/devenv-setup.md`.

When modifying `devenv.nix`, ensure the documentation stays in sync.

## Key sections to update

### Available Commands

Update the commands table if you add, remove, or rename scripts in `scripts = { ... }`:

- `cadmus-setup-native` - Native development setup
- `cadmus-build-kobo` - Kobo cross-compilation (Linux only)
- `cadmus-dev-otel` - OTEL-instrumented emulator

### Platform Support

Update platform-specific notes if you modify:

- `packages` with `pkgs.lib.optionals isLinux/isDarwin` conditionals
- `env` with `pkgs.lib.optionalAttrs isLinux` conditionals
- `enterShell` with `pkgs.lib.optionalString isLinux/isDarwin` conditionals
- `git-hooks.hooks` enable flags
- `languages.c.debugger` or other platform-conditional settings

### Observability Stack

Update the services table if you modify ports or add/remove services:

- `services.opentelemetry-collector`
- `services.prometheus`
- `processes.tempo`
- `processes.loki`
- `processes.grafana`

### Troubleshooting

Add troubleshooting entries for known issues, especially platform-specific ones.
Reference nixpkgs issues with full URLs when documenting workarounds.

## Sync checklist

When reviewing changes to `devenv.nix`:

- [ ] New scripts documented in "Available Commands" table
- [ ] Platform limitations documented in "Platform Support" section
- [ ] New services/ports documented in "Observability Stack" section
- [ ] Breaking changes noted in "Troubleshooting" section
- [ ] Environment variables mentioned if user-configurable
