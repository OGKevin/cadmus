---
description: "Use shared Context test helper"
applyTo: "crates/core/src/**/*.rs"
---

# Test Context Helpers

Tests must not re-define `create_test_context` (or similar context builders).
Always use the shared helper from `crate::context::test_helpers`.

## Required Helper

- `crate::context::test_helpers::create_test_context`

## Allowed Extensions

- Tests may wrap the shared helper to add additional setup (e.g. loading
  keyboard layouts), but must not reimplement the base `Context` construction.
