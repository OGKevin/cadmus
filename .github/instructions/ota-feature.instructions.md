---
description: "OTA test build update feature overview"
applyTo: "crates/core/src/view/ota.rs"
---

# OTA test build updates

The OTA view lets users download and install test builds from GitHub pull
requests directly on device. It checks for WiFi and a configured GitHub token
before allowing a PR number submission.

## Keep this instruction current

- Update this file when `crates/core/src/view/ota.rs` changes.
- If `ota.rs` changes, review user-facing docs to confirm they still match the
  implementation.
