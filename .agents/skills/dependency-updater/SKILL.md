---
name: dependency-updater
description: >
  Resolve failed Renovate or Dependabot dependency update PRs by fixing version
  constraints, API breaking changes, and verifying build/test/format compliance.
  Use when a dependency bump PR fails CI, Cargo cannot select versions, or a
  crate update requires code migration.
---

# Updating Dependencies

Resolve failed dependency update PRs from Renovate or Dependabot by diagnosing
version constraints and API breaks, then verifying with the project skills.

## Core mission

1. **Diagnose** — Understand what Renovate/Dependabot bumped and why it failed
2. **Reproduce** — Recreate the failure locally
3. **Resolve** — Fix version constraints and API changes methodically
4. **Verify** — Follow the [AGENTS.md](../../../AGENTS.md) testing sequence

## Workflow

### Phase 1: Analysis

1. **Read the PR description** to understand:
   - Which package was updated (e.g. `rand_core 0.9.3 -> 0.10.0`)
   - The changelog/release notes for breaking changes
   - Any artifact update errors from Renovate

2. **Check CI logs** for:
   - Version constraint conflicts (`failed to select a version for the requirement`)
   - Compilation errors (API changes, removed types/traits/methods)
   - Test failures

3. **Examine the commit diff** to see exactly what changed in `Cargo.toml`

### Phase 2: Reproduce locally

Confirm you are on the PR branch, then:

```bash
cargo update
cargo xtask test --features emulator
```

If `cargo update` fails with version constraints, note which packages conflict
before changing manifests.

### Phase 3: Resolve version constraints

When you see errors like:

```text
error: failed to select a version for the requirement `rand_core = "^0.9.0"`
candidate versions found which didn't match: 0.10.0
required by package `rand_xoshiro v0.7.0`
```

**Resolution strategy:**

1. **Identify the dependency chain**: `rand_xoshiro` requires `rand_core ^0.9.0`
2. **Check if dependent packages have updates**: Look for versions compatible
   with the new requirement
3. **Update related dependencies together**:

   ```bash
   cargo search rand_xoshiro
   ```

4. **Edit `Cargo.toml`** (and workspace deps in the root manifest when
   applicable) so related packages move to compatible versions together

### Phase 4: Resolve API breaking changes

After constraints resolve, build/test to surface API changes:

```bash
cargo xtask test --features emulator
```

To understand new APIs:

```bash
cargo doc -p <crate_name>
```

Docs land under `target/doc/{crate_name}/`. Prefer the crate's CHANGELOG or
migration guide when one exists.

### Phase 5: Fix compilation errors

For each error:

1. Read the compiler message carefully — it usually suggests the fix
2. Check the crate's migration guide if available
3. Make minimal changes — do not refactor unrelated code

### Phase 6: Verification

After `Cargo.toml` or dependency-related code changes, complete every step in
[AGENTS.md Testing](../../../AGENTS.md):

1. Formatting — `fmt` skill
2. Lint — `clippy-diff-report` or `build-cadmus-native` skill
3. Tests — `build-cadmus-native` skill (`--features emulator`; a device feature
   is required)
4. Kobo ARM build — `build-kobo` skill (**required**)

## Commit message format

```text
chore(deps): resolve {package} {old_version} -> {new_version} update

- Update {related_package} to {version} for compatibility
- Migrate from {old_api} to {new_api}
- {other changes}

Resolves version constraint conflict with {explanation}
```

## Known Renovate bugs

### Cargo workspace packages with `+metadata` version strings

**Affects:** Packages that use build metadata in their version (e.g.
`toml@1.1.0+spec-1.1.0`), when those packages are declared in **multiple**
`Cargo.toml` files within the same workspace.

**Symptom:** Renovate runs
`cargo update --manifest-path <crate>/Cargo.toml --package pkg@old+meta --precise new`
once per manifest file. The first run succeeds and rewrites `Cargo.lock`. The
second run then fails:

```text
error: package ID specification `pkg@old+meta` did not match any packages
```

because `old+meta` no longer exists after the first run.

See: <https://github.com/renovatebot/renovate/discussions/42208>

**Fix:** Run `cargo update` from the workspace root, targeting the _new_
version already written into `Cargo.lock` by the first run:

```bash
cargo update -p pkg@<new-version>+<meta> --precise <new-version>
```

Also bump the minimum version constraint in every affected `Cargo.toml` to the
new version so the intent is explicit, e.g.:

```toml
toml = "1.1.2"
```

## Important guidelines

1. **Never downgrade security updates** — find forward-compatible solutions
2. **Update related packages together** — do not leave partial updates
3. **Preserve existing functionality** — API changes should not change behavior
4. **Document non-obvious migrations** in commit messages or PR text (not inline
   code comments)
5. **Test thoroughly** — run the full AGENTS.md verification sequence
6. **Keep changes minimal** — only modify what the update requires
