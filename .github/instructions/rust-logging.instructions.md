---
description: "Rust structured logging guidelines using tracing crate"
applyTo: "**/*.rs"
---

# Rust Structured Logging

Use structured fields with the `tracing` crate. Never use string formatting for
log data.

## Rules

- **Structured fields only** - data goes in fields, not format strings
- **No prefixes** - no `[Module]` tags; instrumentation scope provides context
- **No mixing** - don't combine structured fields with format args

## Field Formatters

```rust
// Direct value - primitives (integers, floats, booleans)
tracing::debug!(count = 42, "Items processed");

// Display (%) - types implementing Display
tracing::debug!(url = %url, "Fetching resource");

// Debug (?) - types implementing Debug
tracing::debug!(headers = ?response.headers(), "Response received");
```

## Examples

```rust
// ❌ Bad
tracing::debug!("[OTA] Found {} artifacts for PR #{}", count, pr_number);

// ✅ Good
tracing::debug!(pr_number, count, "Found artifacts");
```

```rust
// ❌ Bad
tracing::error!("[API] Request to {} failed with status {}: {}", url, status, e);

// ✅ Good
tracing::error!(url = %url, status = ?status, error = %e, "Request failed");
```

## Log Levels

- `debug` - development and troubleshooting detail
- `info` - important runtime events
- `warn` - recoverable issues (e.g. retries)
- `error` - failures requiring attention
