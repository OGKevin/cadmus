---
description: "OTA test build update feature overview"
applyTo: "crates/core/src/view/ota.rs"
---

# OTA test build updates

The OTA view lets users download and install builds directly on device. It
checks for WiFi before allowing updates. A GitHub token is required only for
main branch and PR builds, not for stable releases.

## Keep this instruction current

- Update this file when `crates/core/src/view/ota.rs` changes.
- If `ota.rs` changes, review user-facing docs to confirm they still match the
  implementation.
