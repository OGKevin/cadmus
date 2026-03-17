---
description: "OTA test build update feature overview"
applyTo: "crates/core/src/view/ota.rs"
---

# OTA test build updates

The OTA view lets users download and install builds directly on device. It
checks for WiFi before allowing updates. Main branch and PR builds require
GitHub authentication, which is handled via device flow. When authentication is needed, a
`DeviceAuthView` child is pushed that displays the user code and polls GitHub
in a background thread. On success the token is saved to disk and the pending
download resumes automatically. Stable releases are public and require no
authentication.

## Ensure user documentation remains current

- If `ota.rs` changes, review user-facing docs to confirm they still match the
  implementation.
