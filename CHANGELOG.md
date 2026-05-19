# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changelog tracking starts with 0.2.0. Prior versions were not tracked.

## [Unreleased]

### Removed

- **`astrid_sdk::net::{recv, try_recv, send}` + `RecvError` / `TryRecvError` / `SendError`.** Thin convenience wrappers around the host-side framed `net-read` / `net-write` fns, which themselves baked length-prefix framing into the host ABI. Length-prefixed framing is application-layer (`std::net::TcpStream` doesn't ship it, the OS doesn't ship it), so the host fns are being removed too. Capsules that need length-prefix framing build a small state machine on top of `net::read_bytes` / `net::write_bytes`. **Breaking**: the only known consumer is the Astrid CLI proxy capsule, which gets its own user-space framing module in [`unicity-astrid/capsule-cli`](https://github.com/unicity-astrid/capsule-cli).

### Added

- **`astrid_sdk::net::connect(host, port)` + `astrid_sdk::net::TcpStream`** — outbound TCP. `connect` is the low-level call returning a `StreamHandle`, parallel to the existing `accept`. `TcpStream` is the `std::net::TcpStream`-shaped facade with `std::io::Read + Write` impls and RAII close-on-drop; user code generally writes `TcpStream::connect("host:port")`. Underlying ABI is the new `astrid:capsule/net.net-connect-tcp` host fn ([wit#5](https://github.com/unicity-astrid/wit/pull/5)); capability-gated against a per-capsule `net_connect` allowlist in `Capsule.toml` (kernel-side, separate PR). SSRF airlock runs on the resolved IP, matching the gate on `http-request`. Unblocks WebSocket, MQTT, Discord/Telegram, postgres/redis, etc. Tracking issue: [astrid#745](https://github.com/unicity-astrid/astrid/issues/745). RFC: [rfcs#27](https://github.com/unicity-astrid/rfcs/pull/27).

## [0.6.1] - 2026-05-19

### Added

- **`ipc::publish_as(topic, payload, principal)` and `ipc::publish_json_as`.** SDK helpers for uplink principal propagation. Without these, an uplink that fans inbound socket messages onto the bus stamps every published event with its own (capsule-owner) principal — the kernel's Layer 5/6 enforcement preamble would then see `caller = default` regardless of which agent the operator was impersonating via the local CLI, and `astrid agent switch alice; astrid agent disable bob` would silently succeed as default. Uplink capsules (CLI proxy, future Telegram/Discord bridges) can now claim a principal other than their own at the IPC boundary. Gated host-side by an `uplink: bool` capability — non-uplink capsules calling these get a clear error. (#37)

### Changed

- **`astrid-sdk/wit/astrid-contracts.wit` is now sourced from `unicity-astrid/wit`.** The contract types (`astrid_sdk::contracts::*`) were generated from a hand-maintained bundled WIT that drifted from the canonical `unicity-astrid/wit` repo — 9 of 17 interfaces present, 33 of 71 records. With sdk-js coming online, having two SDKs generate types from independent WIT copies would have meant cross-SDK contract drift the moment the canonical repo added or changed a record. Now:
  - `astrid-sdk/wit/astrid-contracts.wit` becomes a sync artifact, written by `scripts/sync-contracts-wit.sh` from `contracts/interfaces/*.wit`. Same `cargo package` rationale as `astrid-capsule.wit`: file has to physically live in the crate dir, but the submodule is authoritative.
  - The sync transforms the per-package canonical layout (`package astrid:context@1.0.0;` etc.) into the single-package bundled layout the `wit_events!` macro consumes (`package astrid:contracts@1.0.0;`). Cross-package `use astrid:types/types.{…};` references become same-package `use types.{…};`.
  - New CI job step runs `scripts/sync-contracts-wit.sh --check`. Drift fails CI.
  - `wit_events!` now emits one `pub mod <interface> { … }` per WIT interface instead of flat top-level types. Required because the canonical interface set has same-named records across packages (e.g. `agent::response`, `approval::response`, `elicit::response`); without per-interface namespacing they collided. Type references across interfaces are emitted as fully-qualified `super::<iface>::<Name>` paths.
  - **Breaking** (technical): if anything was using flat `astrid_sdk::contracts::CompactRequest`, it's now `astrid_sdk::contracts::context::CompactRequest`. No known consumer today — capsule code hand-rolls equivalent structs and is being migrated separately. (#39)

- **`astrid-sys/wit/astrid-capsule.wit` is now sourced from `unicity-astrid/wit`.** The host ABI lived as an unsynced copy in three repos (kernel, sdk-rust, sdk-js); PR `unicity-astrid/wit#3` made `unicity-astrid/wit` the canonical home (new `host/astrid-capsule.wit` path).
  - This repo's `contracts/` submodule pointer is bumped to that commit.
  - `astrid-sys/wit/astrid-capsule.wit` becomes a sync artifact maintained by `scripts/sync-host-wit.sh`. Can't go away entirely because `astrid-sys` publishes to crates.io and `cargo package` only bundles files inside the crate dir — but the submodule is authoritative.
  - New CI job `wit-sync` runs `scripts/sync-host-wit.sh --check` on every push/PR. Drift between `contracts/host/` and `astrid-sys/wit/` fails CI. (#38)

### Fixed

- **`cargo publish -p astrid-sys` now works.** The WIT file lived outside the crate directory (`../wit/astrid-capsule.wit`), which `cargo package` strips and `cargo publish` rejects. WIT files moved into their respective crate dirs (`astrid-sys/wit/`, `astrid-sdk/wit/`), root `wit/` directory removed, `include` directive added to `astrid-sys/Cargo.toml`. (#36)

## [0.6.0] - 2026-04-10

### Breaking

- **WASM engine migrated from Extism to wasmtime Component Model.** Capsules must be rebuilt targeting `wasm32-wasip2`. The `#[capsule]` macro now generates `impl Guest` + `export!()` instead of `extern "C"` exports. The ABI is completely different — existing `.wasm` binaries will not load. (#27)
- **All `&[u8]` parameters changed to `&str`.** IPC topics, KV keys, filesystem paths, and uplink params now require UTF-8 at compile time (was runtime validation). (#31)
- **Approval API simplified to OS permission model.** `approval::request(action, resource) -> Result<bool>` replaces the 3-param version with `RiskLevel`. Removed `RiskLevel`, `ApprovalDecision`, `ApprovalResult`. Capsules declare what they want; the kernel classifies risk. (#31, unicity-astrid/astrid#641)
- **Interceptor errors halt the chain.** The `#[capsule]` macro returns `"deny"` on error (was `"error"` which the kernel silently treated as `"continue"`). (#27)
- **`CallerContext` fields corrected.** `session_id` → `source_id` (capsule UUID), added `timestamp`. (#27)
- **`identity::link` returns `Result<()>`.** Was `Result<Link>` with empty `linked_at` field. (#27)

### Removed

- `ipc::publish_bytes`, `ipc::publish_msgpack`, `ipc::poll_bytes`, `ipc::recv_bytes` — use typed `publish`/`publish_json`/`poll`/`recv`. (#31)
- `uplink::send_bytes` — use `uplink::send`. (#31)
- `net::bind_unix(path)` — use `net::bind_unix()` (no path arg). (#31)
- `hooks::trigger(&[u8])` — now `trigger(&str) -> String`. (#31)
- `process::ProcessRequest`, `SubscriptionHandle::as_bytes()`, `host_result.rs`, `cron` module. (#27, #31)
- `extism-pdk` and `rmp-serde` dependencies. (#27, #31)

### Added

- **`wit_events!` proc macro.** Reads a `.wit` file and generates Rust `pub struct` / `pub enum` definitions for every named WIT record and enum, with `Serialize + Deserialize + PartialEq + Clone + Debug` derives and `///` doc comments preserved. Capsule authors write WIT once — the same file feeds `wit_events!()` for Rust types and the core's `wit-parser` for JSON Schema extraction. Zero type duplication. (#32)
- **Serde derives on all WIT-generated types.** `astrid-sys` now passes `generate_unused_types: true` and `additional_derives: [serde::Serialize, serde::Deserialize, PartialEq]` to `wit_bindgen::generate!`. (#32)
- **Typed IPC poll/recv.** `ipc::poll()` / `ipc::recv()` return `PollResult { messages: Vec<Message>, dropped, lagged }`. (#31)
- **HTTP Response exposes status + headers.** `Response::status()`, `Response::headers()`, `Response::is_success()`. (#31)
- **`log::trace()`.** All log functions now return `()` (was `Result`). (#31)
- **Component Model test capsule.** `examples/test-capsule/` validates the full SDK→macro→WIT pipeline, including `wit_events!` generated types used in `ipc::publish_json()`. (#27, #32)
- **Mandatory WIT exports.** `#[capsule]` macro generates all 4 guest exports with no-op stubs for unused ones. Solves unicity-astrid/astrid#638. (#27)

### Changed

- `astrid-sys` uses `wit_bindgen::generate!` instead of `extism_pdk` FFI. (#27)
- `SysError::HostError` wraps `String` (was `extism_pdk::Error`). (#27)
- Handle types internally wrap `u64` with consistent `id()` accessor. (#31)
- `http::Response` fields private with accessor methods. (#31)
- `interceptors::poll` handler receives typed `&ipc::PollResult`. (#31)
- State not persisted on tool error. Install/upgrade/run errors logged. (#27)
- `#![forbid(unsafe_code)]` on `astrid-sdk`. (#27)

## [0.5.3] - 2026-03-24

### Added

- `host_result` module — canonical decoder for the `HostResult` wire format (`0x00` Ok + payload / `0x01` Err + message). All host functions that return data use this encoding instead of WASM traps for recoverable errors. (#25)

### Changed

- `fs::read`, `fs::metadata`, `fs::exists`, `fs::read_dir` — decode `HostResult` from kernel. File-not-found, permission denied, and VFS errors returned as `Err(SysError)` instead of crashing the capsule. (#25)
- `kv::get_bytes`, `kv::list_keys`, `kv::clear_prefix` — decode `HostResult` from kernel. (#25)

### Fixed

- Interceptor chain payload corruption — interceptors returning `Result<(), SysError>` serialized `()` as `b"null"` (4 bytes), overwriting the chain payload for subsequent interceptors. Now returns empty bytes, preserving the original payload. (#25)

## [0.5.2] - 2026-03-23

### Fixed

- `tool_describe` interceptor return value not wrapped in `Ok()` for `FnResult` compatibility — caused type mismatch at the Extism boundary. (#21)
- `tool_describe` returned `BTreeMap` object (`{"tools": {"name": schema}}`) instead of array format (`{"tools": [{"name", "description", "input_schema"}]}`). Prompt-builder expected array, so 0 tool schemas were collected. (#23)
- `EmptyArgs` tools missing `properties: {}` in input schema — OpenAI API requires this field. (#23)

## [0.5.1] - 2026-03-23

### Fixed

- `tool_describe` interceptor parsed `response_topic` from payload and published schemas via IPC — incompatible with `hooks::trigger` which expects return bytes, not a publish. Now returns JSON bytes directly as the interceptor response. (#19)

## [0.5.0] - 2026-03-23

### Changed

- **Tools are now IPC convention.** `#[astrid::tool]` macro rewired to generate interceptor arms in `astrid_hook_trigger` instead of a separate `astrid_tool_call` WASM export. Each tool generates a `tool_execute_<name>` action (deserializes `ToolExecuteRequest`, calls handler, publishes result to `tool.v1.execute.<name>.result` via IPC) and a shared `tool_describe` action (returns all tool schemas as JSON). Capsule code using `#[astrid::tool]` compiles unchanged — only the generated glue changes.

### Added

- WIT interface definitions for all standard contracts: llm, session, spark, context, prompt, tool, hook, registry, types (`wit/` directory)

### Removed

- `astrid_tool_call` WASM export — replaced by `tool_execute_<name>` interceptor arms in `astrid_hook_trigger`
- `astrid_export_schemas` WASM export — replaced by `tool_describe` interceptor arm
- `astrid_cron_trigger` WASM export — dead code, cron was never implemented

## [0.4.0] - 2026-03-19

### Added

- `fs` module: `Metadata`, `DirEntry`, `ReadDir`, `FileType` types mirroring `std::fs`. `read_dir()` returns an iterator, `metadata()` returns a typed struct with `.len()`, `.is_dir()`, `.is_file()`, `.modified()`. (`astrid-sdk`)
- `http` module: typed `Request` builder (`get`/`post`/`put`/`delete`/`header`/`body`/`json`) and `Response` with `.bytes()`/`.text()`/`.json()`. `send()` and `stream_start()` take `&Request`. (`astrid-sdk`)
- `net` module: `recv`/`try_recv`/`send`/`try_accept` with `RecvError`/`TryRecvError`/`SendError` mirroring `std::sync::mpsc`. `NetReadStatus` wire format with status-byte prefix replaces sentinel hack. (`astrid-sdk`)
- `impl std::error::Error` for `RecvError`, `TryRecvError`, `SendError`. (`astrid-sdk`)
- `#[capsule(state)]` attribute for explicit stateful opt-in alongside `&mut self` auto-detection. (`astrid-sdk-macros`)

### Changed

- `time::now_ms() -> Result<u64>` replaced by `time::now() -> Result<SystemTime>` using `std::time::SystemTime` directly. (`astrid-sdk`)
- `log` functions take `impl Display` instead of `impl AsRef<[u8]>` for messages, `&str` for level. (`astrid-sdk`)
- `fs` module extracted to its own file (`fs.rs`). (`astrid-sdk`)
- Handle types (`ListenerHandle`, `StreamHandle`, `BackgroundProcessHandle`) inner fields are now private. (`astrid-sdk`)

### Removed

- `read()`, `write()`, `poll_accept()` from `net` module — replaced by `recv`/`send`/`try_accept`. (`astrid-sdk`)
- `request_bytes()` from `http` module — replaced by `send(&Request)`. (`astrid-sdk`)
- `now_ms()` from `time` module — replaced by `now()`. (`astrid-sdk`)

### Fixed

- `SysError` conversion in macro-generated dispatch code — `?` on method calls now maps `SysError` explicitly instead of relying on unimplemented `From<SysError> for WithReturnCode<Error>`. (`astrid-sdk-macros`)
- `net::read` no longer traps on peer disconnect — uses `NetReadStatus` wire format instead of WASM trap. (`astrid-sdk`)

## [0.3.0] - 2026-03-17

### Added

- Doc comments as tool/capsule descriptions — `///` on `#[astrid::tool]` methods become `metadata.description` in the generated JSON schema. Doc comments on the `#[capsule]` impl block become the capsule-level description. Full doc text (all paragraphs) preserved for LLM context. (`astrid-sdk-macros`)
- Inline mutable flag — `#[astrid::tool("name", mutable)]` or `#[astrid::tool(mutable)]` (name inferred from method). Standalone `#[astrid::mutable]` still works for backward compatibility. (`astrid-sdk-macros`)

### Changed

- Schema export format now returns `{ "tools": {...}, "description": "capsule doc" }` with backward compatibility when no capsule-level doc comment is present. (`astrid-sdk-macros`)

## [0.2.2] - 2026-03-17

### Added

- Streaming HTTP API: `HttpStreamHandle` type and `http::stream_start`/`stream_read`/`stream_close` functions for consuming HTTP responses chunk-by-chunk (`astrid-sdk`)
- FFI declarations for `astrid_http_stream_start`, `astrid_http_stream_read`, `astrid_http_stream_close` (`astrid-sys`)

## [0.2.1] - 2026-03-17

### Added

- `astrid_sdk::types` module — re-exports `astrid-types` 0.3.0 (IPC payloads, LLM schemas, kernel API types). Capsule authors no longer need a separate `astrid-events` dependency.
- CI workflow: check, fmt, clippy, test (Linux + macOS), MSRV verification, security audit.

### Changed

- `CallerContext` moved from standalone `types.rs` file into the `astrid_sdk::types` module alongside the `astrid-types` re-exports.

## [0.2.0] - 2026-03-15

Initial tracked release. See the [repository history](https://github.com/unicity-astrid/sdk-rust/commits/v0.2.0)
for changes included in this version.

[Unreleased]: https://github.com/unicity-astrid/sdk-rust/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/unicity-astrid/sdk-rust/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/unicity-astrid/sdk-rust/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/unicity-astrid/sdk-rust/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/unicity-astrid/sdk-rust/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/unicity-astrid/sdk-rust/releases/tag/v0.2.0
