---
description: "Instructions for managing Cargo.toml dependencies and resolving version conflicts"
applyTo: "**/Cargo.toml"
---

# Cargo Dependency Management Instructions

## Version Specification Best Practices

- Use caret requirements (`"1.2.3"` or `"^1.2.3"`) for most dependencies
- Use exact versions (`"=1.2.3"`) only when absolutely necessary
- For workspace dependencies, define versions in root `Cargo.toml` under `[workspace.dependencies]`
- Keep related crates at compatible versions (e.g., all `opentelemetry-*` crates at same minor version)

## Dependency Organization

1. **Group dependencies logically** with blank lines between groups:
   - Core/standard functionality
   - Serialization (serde, etc.)
   - Async runtime (tokio, etc.)
   - Logging/tracing
   - Optional/feature-gated dependencies

2. **Alphabetize within groups** for easier scanning

3. **Use inline tables for simple features**:

   ```toml
   serde = { version = "1.0", features = ["derive"] }
   ```

4. **Use multi-line for complex configurations**:
   ```toml
   reqwest = { version = "0.13", default-features = false, features = [
       "blocking",
       "json",
       "rustls-no-provider",
   ] }
   ```

## Resolving Version Conflicts

When Renovate/Dependabot updates cause version conflicts:

1. **Identify the conflict chain**: Which package requires which version of what
2. **Check for updates to intermediate packages**: Often the dependent package has a new version
3. **Update related packages together**: Don't leave partial updates
4. **Verify with**: `cargo update && cargo build --all-features`

## Feature Flags

- List features alphabetically within the array
- Remove features that no longer exist in new versions
- Check if features were renamed or merged in changelogs
- Use `default-features = false` when you need fine-grained control

## Workspace Dependencies

For monorepos, prefer workspace dependencies:

```toml
# Root Cargo.toml
[workspace.dependencies]
serde = { version = "1.0", features = ["derive"] }

# Crate Cargo.toml
[dependencies]
serde = { workspace = true }
```

## After Modifying Cargo.toml

Always run:

```bash
cargo update           # Update Cargo.lock
cargo build --all-features  # Verify compilation
cargo test             # Verify tests pass
cargo clippy           # Check for issues
```
