---
description: "Cargo feature flag CI matrix maintenance"
applyTo: "**/Cargo.toml,.github/workflows/cargo.yml"
---

# Cargo Feature Flags and CI Matrix

## Keep the CI matrix in sync with feature flags

The `clippy` and `test` jobs in `.github/workflows/cargo.yml` use a matrix to
check every feature flag **individually and in combination**. When a new feature
is added to any `Cargo.toml`, the matrix must be updated to include it.

### Why per-feature and combination checks matter

`--all-features` only tests the "everything enabled" combination. Code behind
`#[cfg(not(feature = "..."))]` gates is never compiled under `--all-features`,
so breakage in those paths goes undetected.

Feature combinations also matter because features can interact. For example,
`#[cfg(all(feature = "emulator", not(test)))]` behaves differently when
`emulator` is combined with `test` versus `emulator` alone.

### When adding a new feature flag

1. Add the feature to the relevant `Cargo.toml`.
2. Open `.github/workflows/cargo.yml` and add matrix entries to **both** the
   `clippy` and `test` jobs for:
   - The new feature **on its own**
   - The new feature **combined with every other feature**
3. If the feature is workspace-wide (exposed by the `cadmus` binary crate), add:
   ```yaml
   - features: <name>
     cargo_args: "--workspace --all-targets --features <name>"
   - features: <name> + <other>
     cargo_args: "--workspace --all-targets --features <name>,<other>"
   ```
4. If the feature is crate-specific (e.g. `emulator` on the `emulator` crate),
   scope the entry to that package. Cargo will propagate the feature to
   dependencies via `Cargo.toml` forwarding (e.g. `emulator = ["cadmus-core/emulator"]`):
   ```yaml
   - features: <name>
     cargo_args: "-p <crate> --all-targets --features <name>"
   - features: <name> + <other>
     cargo_args: "-p <crate> --all-targets --features <name>,<other>"
   ```
5. Only `default` and `test` features produce build artifacts. Other features
   are checked for compilation and lint correctness only.
