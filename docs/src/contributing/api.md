<!-- i18n:skip-start -->

# API

Cadmus serves two cleartext
[Connect](https://connectrpc.com/) listeners. Both serve the shared
`cadmus.api.shared.v1` RPCs; only the loopback listener serves
`cadmus.api.internal.v1`.

| Listener | Bind                                      | Starts                | Token                                                              | Site files |
| -------- | ----------------------------------------- | --------------------- | ------------------------------------------------------------------ | ---------- |
| Public   | `[::]:80`, dual-stack (emulator IPv4 `0`) | Companion menu switch | Release: 4-digit `Authorization: Bearer` on settings and file RPCs | Yes        |
| Private  | `127.0.0.1:8474` (tests: port 0)          | Process startup       | No                                                                 | No         |

The public listener accepts IPv4 and IPv6 on one socket and falls back to IPv4
where the host has no IPv6. The private listener is loopback-only and is not
advertised over mDNS, which cannot scope a record to loopback.

`GetStatus` and static `GET`s are open on the public listener. There is no
`Access-Control-Allow-Origin` header.

`cadmus.api.shared.v1` is the whole public surface. `cadmus.api.internal.v1`
holds loopback-only RPCs, starting with the no-op `Placeholder` service,
and is registered only when the listener scope is private, so a public request
to a private method is an unknown route. The OpenAPI document still lists those
methods, because plugins and other on-device callers use it as their reference.

Both listeners log every RPC at `trace` level through the `Auth` layer, with a
`router` field set to `public` or `internal`, plus method, path, status, and
duration. Raise `RUST_LOG` to `trace` for `cadmus_core::api::serve` to see them.

The binary event loop in `crates/cadmus/src/app.rs` owns the lifecycle:
<a href="/api/cadmus_core/api/fn.start_private.html">`start_private`</a> after
device `on_startup`,
<a href="/api/cadmus_core/api/fn.handle_event.html">`handle_event`</a> before
`DeviceLifecycle::handle_event`, and
<a href="/api/cadmus_core/api/fn.shutdown.html">`shutdown`</a> before mDNS
teardown.

Handlers do not touch `AppContext`. Settings and status queue a command, send
<a href="/api/cadmus_core/view/enum.Event.html">`Event::Api`</a>, and wait on a
oneshot. Each wait holds a
<a href="/api/cadmus_core/device/inhibitor/soft_suspend/enum.SoftSuspendName.html#variant.Api">`SoftSuspendName::Api`</a>
lease on the worker and attaches the same lease to the hub message so autosleep
does not run before the main loop answers. Active HTTP connections hold the same
lease for their lifetime. `ListDirectory` and `GetFile` use `tokio::fs` on the
RPC task.

## Protobuf

Shared services live in
[`proto/cadmus/api/shared/v1/`](../../../proto/cadmus/api/shared/v1/).
Internal-only RPCs go in
[`proto/cadmus/api/internal/v1/`](../../../proto/cadmus/api/internal/v1/)
when those callers exist. `crates/api-proto/build.rs` compiles every Cadmus
`.proto` plus a generated `Locale` enum from `docs/po/*.po` (same source as
`website/scripts/generate-locales.mjs`), a generated `Extension` enum from
<a href="/api/cadmus_core/document/file_extension/enum.FileExtension.html">`FileExtension`</a>,
and generated `CategoryId`, `SettingIdentityKind`, and `TextMode` enums from the
on-device settings editor types.
`google.protobuf.Timestamp` comes from `buffa-types`
(`protos/google/protobuf/timestamp.proto`).

Generated Rust is `cadmus-api-proto`. Message and service docs are on the
<a href="/api/cadmus_api_proto/cadmus/api/shared/v1/index.html">`cadmus.api.shared.v1`</a>
rustdoc page, not copied here.

## Exploring the API

`GET /openapi.json` returns an OpenAPI document generated at build time by
`protoc-gen-connect-openapi` from `proto/` plus the
[gnostic](https://github.com/google/gnostic) OpenAPI annotations in
[`proto/gnostic`](../../../proto/gnostic) (submodule).
`crates/api-proto/build.rs` runs the plugin after the Rust codegen and stamps
`info.version` with the same git-describe string as `GetStatus`. Both listeners
serve it, and the public listener serves it without a bearer token: the protos
are public documentation. Paths use the Connect form
`/cadmus.api.shared.v1.<Service>/<Method>`.

The document-level default is `bearerAuth`, declared in
[`proto/cadmus/api/shared/v1/openapi.proto`](../../../proto/cadmus/api/shared/v1/openapi.proto).
An unauthenticated method overrides that in its proto definition:

```protobuf
rpc GetStatus(GetStatusRequest) returns (DeviceStatus) {
  option (openapi.v3.operation) = { security: [{}] };
}
```

An empty security requirement is the OpenAPI way to say "no auth"; a
`security: []` list cannot be expressed in a proto3 repeated field. Every path
without that override inherits `bearerAuth`.

Clients pick the protocol they prefer.
[Connect](https://connectrpc.com/) works over HTTP/1.1 with JSON
or binary payloads, gRPC-Web is available to browsers, and cleartext gRPC runs
over HTTP/2 with prior knowledge.

## See also

- <a href="/api/cadmus_core/api/index.html">`cadmus_core::api`</a>
- [mDNS](mdns.md)
- [WiFi Leases](wifi.md)

<!-- i18n:skip-end -->
