# Cadmus — Agent Coding Conventions

## Error Handling Philosophy

The emulator code path should **panic on errors** to catch issues early during
development. The app code path should **handle errors gracefully** for a smooth
user experience.

## Scoped Instructions

Nested `AGENTS.md` files add scope-specific rules. Read the file covering each
directory you modify, including `.github/workflows/` and `.opencode/`. Rust code
outside `crates/`, including `xtask/`, follows [`crates/AGENTS.md`](crates/AGENTS.md).

## Skills

For workflows, load the matching skill from [`.agents/skills/`](.agents/skills/):

| Skill                 | Use when                                                    |
| --------------------- | ----------------------------------------------------------- |
| `build-cadmus-native` | Compile, test, lint, or run the emulator on a native host   |
| `build-kobo`          | Cross-compile for Kobo (ARM); required after code changes   |
| `fmt`                 | Check or apply rustfmt                                      |
| `clippy-diff-report`  | Lint only the current diff (matches CI reviewdog)           |
| `dependency-updater`  | Fix failed Renovate/Dependabot dependency PRs               |
| `sqlx`                | Regenerate `.sqlx/` after query macro changes               |
| `docs`                | Build or preview the documentation site                     |
| `translations-sync`   | Regenerate the translations POT after user-facing doc edits |
| `fetch-cadmus-logs`   | Look up Cadmus logs and traces by run ID                    |
| `list-cadmus-devices` | Discover Cadmus devices on the LAN via mDNS                 |

## Testing

After code changes, complete every step before considering work done:

1. Formatting — `fmt` skill
2. Lint — `clippy-diff-report` or `build-cadmus-native` skill
3. Tests — `build-cadmus-native` skill (`--features emulator` locally; a
   device feature — `emulator`, `kobo`, or `deviceless` — is **required** to
   compile `cadmus-core`)
4. Kobo ARM build — `build-kobo` skill (**required**; host builds can pass while
   ARM fails)
5. Documentation freshness — if the change updates or introduces
   user/contributor-facing behavior, follow [`REVIEW.md`](REVIEW.md) (update or
   add `docs/src` pages, or justify why not)

After modifying `Cargo.toml`, run the full verification sequence above.

Local clippy may be newer than CI; extra style warnings are non-fatal (CI filters
to the diff via reviewdog).

## OTA and Asset Build Order

- OTA updates delete only Cadmus-owned bundled files before reboot. Do not
  assume a whole asset directory can be removed because users may add their own
  fonts, icons, or other files.
- `libs/` and `website/` are special: they are entirely Cadmus-owned and may be
  cleaned wholesale before reboot because the next `KoboRoot.tgz` repopulates
  them. `website/` is shipped by `cadmuscli dist` (build it with
  `cadmuscli docs` first).
- If code generates compile-time metadata from bundled asset directories, make
  sure `bin/`, `resources/`, and `hyphenation-patterns/` are present before the
  Kobo build starts. In CI, `cadmuscli download-assets` must run before
  `cadmuscli build-kobo` for the generated asset list to stay accurate.
