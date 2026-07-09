---
name: fetch-cadmus-logs
description: Fetch Cadmus application logs from Loki by run ID. Use when asked to look up logs for a specific run, debug a device session, or find log output for a given run ID.
compatibility: Requires curl and a running Loki instance at http://localhost:3100
---

# Fetch Cadmus Logs by Run ID

## Context

Cadmus logs are shipped to Loki under the stream label `service_name="cadmus"`.
The run ID is stored as a JSON field named `resources_cadmus_run_id` — it is
**not** a Loki stream label, so it cannot be used in the stream selector `{}`.
It must be filtered with `| json | resources_cadmus_run_id="<id>"` in the
LogQL pipeline.

### Two separate observability sources

Cadmus uses two distinct pipelines:

| Source                              | What it contains                                                                                                 | How to query                         |
| ----------------------------------- | ---------------------------------------------------------------------------------------------------------------- | ------------------------------------ |
| **Loki** (`http://localhost:3100`)  | `tracing`-bridge log records (third-party crate logs like `reqwest`, `i18n_embed`)                               | LogQL via `/loki/api/v1/query_range` |
| **Tempo** (`http://localhost:3200`) | Spans and span events from `#[tracing::instrument]` — this is where cadmus-core importer, library, db spans live | `/api/traces/<traceID>`              |

**Important:** Cadmus application logic (`cadmus_core::*`) emits structured
spans via `tracing::instrument`, not log records. These appear in **Tempo**, not
Loki. Loki for a cadmus run will mostly contain noise from `reqwest::retry`,
`i18n_embed`, etc. — not importer or library logic.

To debug application behaviour (import scan, db inserts, errors), query Tempo.
To find third-party log noise or connectivity errors, query Loki.

## Loki

### Base URL

```text
http://localhost:3100
```

Override with the `LOKI_URL` environment variable if the instance is elsewhere.

### Query logs for a run ID

```bash
LOKI_URL="${LOKI_URL:-http://localhost:3100}"
RUN_ID="<run-id>"

curl -sG "${LOKI_URL}/loki/api/v1/query_range" \
  --data-urlencode 'query={service_name="cadmus"} | json | resources_cadmus_run_id="'"${RUN_ID}"'"' \
  --data-urlencode 'limit=1000' \
  --data-urlencode 'direction=forward' \
  | jq -r '.data.result[].values[] | .[1] | fromjson | .body // .msg // .'
```

This prints the `body` field of each JSON log line in chronological order.

### Filter to cadmus-core log records only

Add `| instrumentation_scope_name="log"` to isolate records that went through
the log bridge (as opposed to span events). Even then, most records will be
from third-party crates. To further narrow to cadmus-core targets:

```bash
curl -sG "${LOKI_URL}/loki/api/v1/query_range" \
  --data-urlencode 'query={service_name="cadmus"} | json | resources_cadmus_run_id="'"${RUN_ID}"'" | instrumentation_scope_name="log"' \
  --data-urlencode 'limit=1000' \
  --data-urlencode 'direction=forward' \
  | jq -r '.data.result[].values[] | .[1] | fromjson | select(.attributes["log.target"] | startswith("cadmus")) | .body'
```

### Filter by log level

Loki attaches a `level` stream label (lowercase) derived from the `severity`
JSON field. Filter by level two ways:

**Stream label (preferred — fast, evaluated before parsing):**

```text
{service_name="cadmus", level="error"} | json | resources_cadmus_run_id="<id>"
```

**JSON field filter:**

```text
{service_name="cadmus"} | json | resources_cadmus_run_id="<id>" | severity="ERROR"
```

Valid `level` stream label values: `trace`, `debug`, `info`, `warn`, `error`.
Valid `severity` JSON field values: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`.

### Paginate large result sets

Loki returns at most `limit` entries per request. Keep limit ≤ 1000 —
larger values may return empty results. Paginate using the last timestamp:

```bash
LAST_TS=$(echo "$RESPONSE" | jq -r '.data.result[-1].values[-1][0]')

curl -sG "${LOKI_URL}/loki/api/v1/query_range" \
  --data-urlencode 'query={service_name="cadmus"} | json | resources_cadmus_run_id="'"${RUN_ID}"'"' \
  --data-urlencode "start=$((LAST_TS + 1))" \
  --data-urlencode 'limit=1000' \
  --data-urlencode 'direction=forward'
```

### Common Loki patterns

| Goal                     | LogQL                                                                                   |
| ------------------------ | --------------------------------------------------------------------------------------- |
| All logs for a run       | `{service_name="cadmus"} \| json \| resources_cadmus_run_id="<id>"`                     |
| Errors only              | `{service_name="cadmus", level="error"} \| json \| resources_cadmus_run_id="<id>"`      |
| cadmus-core targets only | add `\| instrumentation_scope_name="log"` then filter `.attributes["log.target"]` in jq |
| Count entries            | Use `/loki/api/v1/query` with `count_over_time(...)`                                    |

## Tempo (spans and span events)

### Tempo Base URL

```text
http://localhost:3200
```

### Find a trace by run ID

```bash
curl -sG "http://localhost:3200/api/search" \
  --data-urlencode "tags=cadmus.run_id=<run-id>" \
  --data-urlencode "limit=5" \
  | jq -r '.traces[] | "\(.traceID) \(.rootName)"'
```

### Fetch a full trace

```bash
TRACE_ID="<traceID>"
curl -s "http://localhost:3200/api/traces/${TRACE_ID}" \
  | jq -r '.batches[].scopeSpans[].spans[] | "\(.name) | \(.attributes[] | "\(.key)=\(.value.stringValue // .value.intValue)")"'
```

### Find error spans

```bash
curl -s "http://localhost:3200/api/traces/${TRACE_ID}" | jq -r '
  .batches[].scopeSpans[].spans[]
  | select(.status.code == "STATUS_CODE_ERROR")
  | {name, status, events: .events}
'
```

Span events carry the actual error message. Check `.events[].attributes` for
fields like `error` which contain the database error string.

### Filter spans by name

```bash
curl -s "http://localhost:3200/api/traces/${TRACE_ID}" | jq -r '
  .batches[].scopeSpans[].spans[]
  | select(.name | test("scan_entries|flush_to_db|batch_insert|run"))
  | "\(.name) | \(.attributes[] | select(.key | test("count|library_id|home")) | "\(.key)=\(.value.stringValue // .value.intValue)")"
'
```

### Convert nanosecond timestamps

Tempo span timestamps are Unix nanoseconds:

```bash
python3 -c "import datetime; print(datetime.datetime.fromtimestamp(<ns>/1e9, tz=datetime.timezone.utc))"
```

## Loki API reference

- `GET /loki/api/v1/query_range` — range query (fetching log lines)
- `GET /loki/api/v1/query` — instant query (metric expressions)
- `GET /loki/api/v1/labels` — list available label names
- `GET /loki/api/v1/label/{name}/values` — list values for a label

All parameters are passed as query string values and should be URL-encoded.
Timestamps can be Unix epoch nanoseconds (integer) or RFC3339 strings.
