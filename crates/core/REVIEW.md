# Core Crate — Review Checklists

## Event System Documentation

When `crates/cadmus/src/app.rs` changes, check that
`docs/src/contributing/event-system.md` still matches.

### Checklist

- [ ] New event types in the main loop match statement are documented.
- [ ] `Event::Close(id)` handling still matches docs (uses `locate_by_id()`?
      top-level children only?).
- [ ] Hub (`tx.send()`) vs Bus (`bus.push_back()`) patterns are accurate.
- [ ] View lifecycle changes (creation, removal, transitions) are reflected.
- [ ] Mermaid diagrams match the current flow (use `<br/>` for line breaks in
      node labels).
- [ ] Code examples in docs match actual implementation.

## Settings Documentation

All locations must stay in sync:

- `contrib/Settings-sample.toml`
- `crates/core/src/settings/*`
- `docs/src/settings/*`

- [ ] If settings structures, fields, or defaults changed, do the docs match?

## OTA Documentation

The OTA view (`crates/core/src/view/ota.rs`) lets users download and install
builds on device. Main/PR builds require GitHub device-flow authentication;
stable releases are public.

- [ ] If `ota.rs` changed, do the user-facing docs still match?
