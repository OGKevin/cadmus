<!-- i18n:skip-start -->

# API

Cadmus serves two cleartext
[Connect](https://connectrpc.com/) listeners. Both serve the shared
`cadmus.api.shared.v1` RPCs; only the loopback listener serves
`cadmus.api.internal.v1`.

| Listener | Bind                                      | Starts                | Token                                                     | Site files |
| -------- | ----------------------------------------- | --------------------- | --------------------------------------------------------- | ---------- |
| Public   | `[::]:80`, dual-stack (emulator IPv4 `0`) | Companion menu switch | 4-digit `Authorization: Bearer` on settings/file RPCs     | Yes        |
| Private  | `127.0.0.1:8474` (tests: port 0)          | Process startup       | No                                                        | No         |

The public listener accepts IPv4 and IPv6 on one socket and falls back to IPv4
where the host has no IPv6. The private listener is loopback-only and is not
advertised over mDNS, which cannot scope a record to loopback.

Every build authenticates. The token is the four-digit code the user reads off
the device and types into the companion client; it comes from the OS CSPRNG,
lives in memory only, and survives suspend so a resume keeps the same token.
Three rejected tokens in one session shut the public listener down; the switch
has to be turned on again.

The debug emulator never binds the LAN: its startup companion session binds
`127.0.0.1:0`, which also skips mDNS. Opening Companion from the emulator menu
still binds `0.0.0.0:0` and requires the token from the logs.

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
does not run before the main loop answers. The listener takes the same lease per
RPC request, so an idle keep-alive socket holds none. `ListDirectory` and
`GetFile` use `tokio::fs` on the RPC task; `GetFile` streams through a
`take(32 MiB + 1)` bound and at most four reads buffer concurrently.

Kobo confines file RPCs to `/mnt/onboard`; every other build confines them to
the device install directory, so a LAN client can never browse the host.

The listener accepts at most 16 connections, requires request headers within
30 s, and pings idle HTTP/2 connections every 60 s with a 20 s timeout.

The public listener takes the Wi-Fi lease and binds the port in a job, not on
the main loop: on Kobo the lease only completes once the `NetUp` handler runs
the radio bring-up, so awaiting it inline would stall the loop for the acquire
timeout. The job reports back with a command and `Event::Api`, and a stop or
suspend cancels it before it can publish a superseded listener.

## Protobuf

Shared services live in
[`proto/cadmus/api/shared/v1/`](../../../proto/cadmus/api/shared/v1/).
Internal-only RPCs go in
[`proto/cadmus/api/internal/v1/`](../../../proto/cadmus/api/internal/v1/)
when those callers exist. `crates/api-proto/build.rs` compiles every Cadmus
`.proto` plus generated enums.

`Extension`, `CategoryId`, `SettingIdentityKind`, and `TextMode` are defined
once in [`crates/build-deps/src/api_enums.json`](../../../crates/build-deps/src/api_enums.json),
validated by `api_enums.schema.json`. That manifest carries the Rust shape
(derives, attributes, payload types, `cfg`) and the proto number of every
variant. The `cadmus-core` build script generates the Rust enums from it and
the `api-proto` build script generates the `.proto` enums and mapping code, so
adding a variant is one manifest entry with the next free number.

`Locale` is not in the manifest: `api-proto` generates it from `docs/po/*.po`
plus English, the same source as `website/scripts/generate-locales.mjs`.
Numbers follow the sorted code order; `Locale` is read by name and never
stored as a number.

Library rows use dedicated operations: `Settings.AddLibrary` takes an
`AddLibraryRequest { LibraryValue library }` and appends the row,
`Settings.RemoveLibrary` takes a `LIBRARY_INFO` `SettingId` and removes it, and
`Settings.Update` only edits an existing row. `LibraryValue` has no `remove`
flag; the field number is reserved.
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
