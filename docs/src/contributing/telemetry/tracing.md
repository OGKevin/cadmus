<!-- i18n:skip-start -->

# Tracing

Cadmus uses `tracing` instrumentation to capture execution flow through the
app and export spans to an OTLP backend.

## What it does

- Adds spans around key operations in the app and core crates
- Captures timing, fields, and parent-child relationships between spans
- Exports traces to an OpenTelemetry collector when configured

## Feature flag

- `tracing` enables tracing support in the codebase

## Instrumentation

Most view and runtime chokepoints are instrumented with conditional
compilation so tracing can be compiled out when the feature is disabled.

Each instrumented function can capture:

- function and module name
- selected input fields
- execution duration
- return values at `TRACE` level
- parent-child span relationships

View code is instrumented around two high-value paths:

- `handle_event` methods for event flow through the UI tree
- `render` methods for rendering and layout timing

For detailed instrumentation conventions, see
`.github/instructions/rust-instrumentation.instructions.md`.

### Example: instrument a function

Use `#[cfg_attr(feature = "tracing", tracing::instrument(...))]` on functions
that should create a span for each call.

```rust
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(self, data), fields(book_id, size = data.len()))
)]
fn save_cover(&self, book_id: i64, data: &[u8]) -> Result<(), Error> {
    tracing::debug!(book_id, "Saving cover image");

    self.storage.save(book_id, data)?;

    Ok(())
}
```

Use `skip(...)` for large values, borrowed buffers, or types with noisy `Debug`
output. Add `fields(...)` for the identifiers you will actually query in Tempo
or logs.

### Example: instrument a closure

For a short synchronous closure, create a span and run the closure inside it
with `in_scope()`.

```rust
let sorted = tracing::info_span!("sorting", entry_count = entries.len()).in_scope(|| {
    let mut entries = entries;
    entries.sort_unstable();
    entries
});
```

This is the usual pattern when you want to time a single closure body without
extracting it into a separate function.

## OTLP export

When `tracing` is enabled, Cadmus initializes a tracer provider that exports to
`<endpoint>/v1/traces` using batch span processors.

This gives you distributed traces in backends like Tempo, Jaeger, or any other
OTLP-compatible collector.

## Resource attributes

Each exported span includes shared process metadata:

- `service.name = cadmus`
- `service.version = <git describe output>`
- `cadmus.run_id = <uuid-v7>`
- `hostname = <system hostname>`

## Configuration

See the [settings reference](../../settings/index.md#logging) for the full
logging and OTLP configuration. The main option is `logging.otlp-endpoint`.

`OTEL_EXPORTER_OTLP_ENDPOINT` overrides `logging.otlp-endpoint` when both are
set.

To build Cadmus with tracing support:

```bash
cargo build --features tracing
```

## Related docs

- [Logging](logging.md)
- [Profiling](profiling.md)

<!-- i18n:skip-end -->
