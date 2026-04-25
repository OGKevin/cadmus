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

## Loki base URL

```
http://localhost:3100
```

Override with the `LOKI_URL` environment variable if the instance is elsewhere.

## Step-by-step

### 1. Resolve the time range (optional but recommended)

If you know approximately when the run happened, pass `start` and `end` as
Unix nanoseconds or RFC3339 to narrow the query and avoid timeouts.

Leave them out to default to the last hour.

### 2. Query logs for the run ID

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

### 3. Widen the output if needed

To see all fields (level, target, span, etc.):

```bash
curl -sG "${LOKI_URL}/loki/api/v1/query_range" \
  --data-urlencode 'query={service_name="cadmus"} | json | resources_cadmus_run_id="'"${RUN_ID}"'"' \
  --data-urlencode 'limit=1000' \
  --data-urlencode 'direction=forward' \
  | jq '.data.result[].values[] | .[0], (.[1] | fromjson)'
```

### 4. Filter by log level

Loki attaches a `level` stream label (lowercase) derived from the `severity`
JSON field. Filter by level two ways:

**Stream label (preferred — fast, evaluated before parsing):**

```
{service_name="cadmus", level="error"} | json | resources_cadmus_run_id="<id>"
```

**JSON field filter (use when you need to match severity exactly as logged):**

```
{service_name="cadmus"} | json | resources_cadmus_run_id="<id>" | severity="ERROR"
```

Valid `level` stream label values: `trace`, `debug`, `info`, `warn`, `error`.
Valid `severity` JSON field values: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`.

### 5. Paginate large result sets

Loki returns at most `limit` entries per request. Use the `end` timestamp of
the last entry as the new `start` to fetch the next page:

```bash
# Get the timestamp of the last entry from previous response
LAST_TS=$(echo "$RESPONSE" | jq -r '.data.result[-1].values[-1][0]')

# Next page: start just after the last entry
curl -sG "${LOKI_URL}/loki/api/v1/query_range" \
  --data-urlencode 'query={service_name="cadmus"} | json | resources_cadmus_run_id="'"${RUN_ID}"'"' \
  --data-urlencode "start=$((LAST_TS + 1))" \
  --data-urlencode 'limit=1000' \
  --data-urlencode 'direction=forward'
```

## Common patterns

| Goal | LogQL suffix |
|------|-------------|
| All logs for a run | `| json | resources_cadmus_run_id="<id>"` |
| Errors only | `{service_name="cadmus", level="error"} \| json \| resources_cadmus_run_id="<id>"` |
| Pyroscope push logs | `\| json \| resources_cadmus_run_id="<id>" \| body=~"pyroscope\|profil"` |
| Count entries | Use `/loki/api/v1/query` with `count_over_time(...)` |

## Loki API reference

- `GET /loki/api/v1/query_range` — range query (use for fetching log lines)
- `GET /loki/api/v1/query` — instant query (use for metric expressions)
- `GET /loki/api/v1/labels` — list available label names
- `GET /loki/api/v1/label/{name}/values` — list values for a label

All parameters are passed as query string values and should be URL-encoded.
Timestamps can be Unix epoch nanoseconds (integer) or RFC3339 strings.
