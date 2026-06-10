# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changelog tracking starts with 0.2.0. Prior versions were not tracked.

## [Unreleased]

## [0.7.1] - 2026-06-10

This release restores capsule tool discovery and ships the first two post-0.7 additions to the
host surface. The headline is the `tool_describe` macro fix: every tool capsule built on 0.7.0
answered the kernel's describe fan-out with nothing, so every collector — prompt-builder, the
registry, the `astrid mcp serve` MCP bridge — saw zero tools. Rebuilding a capsule against 0.7.1
is the entire fix; `astrid-sdk = "0.7"` consumers resolve it automatically with no `Cargo.toml`
change. Riding along, two additive surfaces: the `astrid:process` persistent-process tier
(background children that outlive the pooled capsule instance, reattachable by id from a later
invocation) and `capabilities::enumerate` (a capsule can ground its behaviour in the capabilities
it actually holds instead of hard-coding assumptions). No breaking changes.

### Fixed

- **`#[capsule]`-generated `tool_describe` now publishes its descriptor for the describe fan-out.** The macro built the `{tools, description}` descriptor and *returned* it as `CapsuleResult{action:"continue"}` — but under the current ABI an interceptor's return value is not fanned out to the caller, so no SDK-macro tool capsule ever answered `tool.v1.request.describe` and every collector (prompt-builder, registry, the sage-mcp bridge behind `astrid mcp serve`) collected 0 tools. The arm now publishes the descriptor on `tool.v1.response.describe.self` before returning — the `.self` suffix is routing-only (satisfies the route layer's ≥5-segment trailing-`*` arity); the responder's real `source_id` rides in kernel-stamped message metadata. Verified end-to-end: a tool capsule rebuilt against this SDK took the MCP bridge's `tools/list` from 0 tools to its full set. Regression guards: `tool_describe_publishes_response_for_fanout`, `capsule_without_tools_has_no_describe_publish`. (#56)

### Added

- **`capabilities::enumerate` — list the calling capsule's own held capability names.** The list dual of `capabilities::check`: returns the capability categories declared in this capsule's `[capabilities]` manifest block (`host_process`, `net_connect`, `fs_read`, …) — the names, not the scoped arguments within them (allowlists, `host:port`, paths). Argument-free (the kernel already knows the caller) and infallible — an empty list is the valid "no capabilities" answer — so a reusable capsule can ground its behaviour in what it can actually do instead of hard-coding it, avoiding code-vs-manifest drift. Backed by unicity-astrid/wit#13's `astrid:sys/host.enumerate-capabilities`; contracts submodule bumped accordingly. (#53)
- **`process` persistent-process tier — `Command::spawn_persistent`, `PersistentProcess`, and `process::{attach, list, status_many}`.** Mirrors the host `astrid:process@1.0.0` persistent tier: a background child that **outlives the pooled, stateless instance** that started it (unlike `Process`, whose kernel resource is reaped on instance reset). `Command` gains the persistent-only builder knobs — `label`, `keep_stdin_open`, `overflow`, `log_ring_bytes`, `max_lifetime`, `idle_timeout`, `exit_retention`, `limits` — and a `spawn_persistent()` terminal returning a `PersistentProcess` keyed by an opaque `ProcessId`. `PersistentProcess` exposes `status` / `read_logs` (drain) / `read_since` (non-draining cursor, byte-faithful `LogChunk`) / `write_stdin` / `close_stdin` / `signal` / `wait` (bounded) / `stop` (consumes the handle; SIGTERM→grace→SIGKILL, frees the slot) / `release` (consumes). Persist the `ProcessId` (e.g. in KV) and `process::attach(id)` from a later invocation to reattach — the SDK `attach` is a thin id-wrapper, so it works without the host's deferred `attach` resource fn; the first id-keyed call validates ownership. `process::list` / `status_many` enumerate the capsule+principal's persistent processes. New types: `ProcessId`, `ProcessInfo`, `ProcessPhase`, `LogStream`, `LogCursor`, `LogChunk`, `OverflowPolicy`, `ResourceLimits`; `Signal` gains `Stop` / `Cont`. The host's `watch` / `unwatch` lifecycle-event channel and resource-limit enforcement are not yet wired (poll via `status` + bounded `wait`). Backed by unicity-astrid/wit#12; contracts submodule bumped to the merged `astrid:process@1.0.0` persistent tier. (#53)

## [0.7.0] - 2026-05-26

### Breaking

- **Per-domain WIT host ABI.** The single `astrid:capsule@0.1.0` world has been split into per-domain frozen packages at `@1.0.0` (`astrid:fs`, `astrid:ipc`, `astrid:kv`, `astrid:net`, `astrid:http`, `astrid:sys`, `astrid:process`, `astrid:uplink`, `astrid:elicit`, `astrid:approval`, `astrid:identity`) plus Astrid-owned foundation I/O (`astrid:io/{error, poll, streams}`). Capsule authors must recompile against this SDK; the contract surface is wire-level incompatible with pre-0.7 capsules. Guest exports moved to per-export worlds in `astrid:guest@1.0.0` (`interceptor`, `background`, `installable`, `upgradable`).
- **Resource-backed handles.** `ipc::Subscription`, `fs::File`, `process::Process`, `net::TcpStream`, `net::TcpListener`, `net::UnixListener`, `net::UdpSocket`, and `http::HttpStream` are now component-model resources with RAII `Drop` that release the underlying kernel handle. Pre-migration code that carried opaque `u64` handles (`SubscriptionHandle`, `StreamHandle`, `ListenerHandle`, `BackgroundProcessHandle`, `HttpStreamHandle`) and called free fns like `ipc::unsubscribe`, `net::close`, `http::stream_close`, `process::kill(id)` is gone. The new pattern: hold the typed handle in scope, call methods on it, let scope-end clean up. Explicit `close` methods remain where the spec defines them (e.g. `HttpStream::close`).
- **Typed `ErrorCode` enums per domain.** Every host fn previously returning `Result<T, String>` now returns a domain-specific typed `ErrorCode` (`astrid:fs/host.error-code`, `astrid:net/host.error-code`, …). The SDK preserves `SysError::HostError(String)` as the unified public error type — typed kernel errors are converted via `Debug` formatting at the boundary (see the new `host_err` helper) so capsule code that branches on `SysError::HostError(msg).contains("Timeout")` continues to work without change. Pattern-matching on a typed variant requires calling the host fn directly through `astrid_sys::astrid::<domain>::host::*`.
- **`identity::resolve` returns `Ok(None)` for unlinked.** The new contract surfaces "no link" as a typed `ErrorCode::LinkNotFound` rather than the pre-migration `found: bool` flag. The SDK wrapper translates this to `Ok(None)` so callers use idiomatic `Option` semantics. `identity::unlink` similarly returns `Ok(false)` when no link existed.
- **`fs::Metadata` reshaped.** `Metadata.mtime` (raw u64 seconds) is replaced by typed `modified() -> Result<SystemTime>` / `created()` / `accessed()` reading optional `Datetime` records. `is_dir` is replaced by a `FileType` enum with `Regular`, `Directory`, `Symlink`, `Other` variants. New: `symlink_metadata`, `Metadata::mode`.
- **`fs::FileHandle` (now `fs::File`).** Returned by `fs::File::open` / `fs::File::create` / `fs::File::open_mode`. Wraps the resource; provides `read_at` / `write_at` / `sync_data` / `sync_all` / `set_len` / `metadata` methods. Replaces ad-hoc `u64` file handles.
- **`process::ProcessResult` renamed to `process::Output`; `process::ExitInfo` added.** The new contract carries structured `(exit_code, signal)` so capsules can distinguish SIGTERM-shutdown from SIGKILL-oom from normal-exit. `Output::exit_code() -> i32` is kept as a backward-compat accessor (returns `-1` for signal-killed). `BackgroundProcessHandle` is replaced by `Process` (Drop-reaped) with methods `read_logs`, `write_stdin`, `close_stdin`, `signal`, `kill`, `wait`, `wait_with_output`, `os_pid`. New `process::Command` builder surfaces the `cwd`, `env`, and `stdin` fields the new ABI adds.
- **`approval::request_decision` returns typed `Decision`.** `approval::request` still returns `bool` (any approval variant = `true`). The new `request_decision` exposes the full `Denied / Approved / ApprovedSession / ApprovedAlways / Allowance` ladder.
- **`uplink::register` takes a string profile or `uplink::Profile`.** New `Profile` enum mirrors the WIT enum; `register` validates the string against canonical names (`chat` / `interactive` / `notify` / `bridge`) and `register_profile` takes the typed enum directly.
- **`net::TcpStream::connect` accepts both Unix-domain accepted streams and outbound TCP.** The pre-migration distinction between `StreamHandle` (accepted) and `TcpStream` (outbound) collapses — the new contract uses a single `astrid:net.tcp-stream` resource for both, and TCP-only options return a `NotTcp` host error when called on a Unix-domain stream. `net::recv` / `net::send` / `net::try_recv` / `net::accept` / `net::close` / `net::bind_unix` (free fns) are now methods on `TcpStream` / `UnixListener`. New `net::bind_tcp` exposes inbound-TCP listeners; new `net::udp_bind` exposes `UdpSocket`.
- **`http::HttpStream` reshaped.** `stream_start` returns a single `HttpStream` (resource-backed) carrying status, headers, and `read_chunk` / `close` methods. The pre-migration `(handle, status, headers)` triple is gone. `HttpStream::read_chunk()` returns `Ok(None)` at EOF.
- **`ipc::Message` carries typed `PrincipalAttribution`.** Subscribers see the principal as `Verified(...)`, `Claimed(...)`, or `System` so cross-uplink trust decisions branch on the variant rather than parsing a string. `Subscription::poll` / `Subscription::recv` are methods; the free-fn `ipc::poll` / `ipc::recv` are gone.
- **`hooks::trigger` removed.** The pre-migration `wit_sys::trigger_hook` host fn is no longer part of the host ABI surface. User-middleware triggering is now an internal capsule-to-capsule concern.
- **`interceptors::poll` removed.** Interceptor events are delivered through `astrid-hook-trigger` (the existing guest export), not run-loop subscriptions. `interceptors::bindings()` remains for enumeration / debugging.
- **`env::var` returns `Ok("")` for missing keys; `env::var_opt` is the new disambiguator.** The host fn now returns `Result<Option<String>>`; the SDK keeps `var` returning `String` for source compatibility (empty for missing) and adds `var_opt` for the option semantics.

### Added

- **`host_err` helper.** Single point of conversion from any per-domain `ErrorCode` (or other `Debug` host failure) into `SysError::HostError(String)`.
- **`kv::list_keys_page` + `kv::cas`.** Wrappers for the new paginated key listing and atomic compare-and-swap host fns.
- **`kv::get_bytes_opt` / `kv::get_json_opt`.** Disambiguate "missing key" from "empty value"; the existing `get_bytes` / `get_json` continue to collapse both into empty.
- **`time::sleep` / `time::monotonic` / `runtime::random_bytes`.** Wrap the new `astrid:sys` primitives (sleep-ns, clock-monotonic-ns, random-bytes).
- **`process::Command` builder.** Mirrors `std::process::Command` for the new `env` / `cwd` / `stdin` fields on `spawn-request`.
- **`net::lookup_host`.** Wraps the new DNS-with-airlock host fn.
- **`net::TcpListener` + `net::bind_tcp`.** Inbound TCP server posture for self-hosted webhook receivers, gRPC endpoints, etc.
- **`net::UdpSocket` + `net::udp_bind`.** Unconnected and connected-mode UDP.
- **`net::TcpStream::peek` / `set_keepalive` / `set_linger` / `set_reuseaddr` / `set_hop_limit`.** Full surface of the new `tcp-stream` resource.
- **`net::TcpStream::shutdown(Shutdown::Read | Write | Both)`.** Mirrors `std::net::Shutdown`.
- **`fs::append` / `fs::create_dir_all` / `fs::copy` / `fs::rename` / `fs::canonicalize` / `fs::read_link` / `fs::hard_link` / `fs::symlink_metadata` / `fs::remove_dir_all`.** Bring the SDK to feature parity with `std::fs` for the operations the new VFS surfaces.

### Notes

- The `request_response` helper is unchanged in semantics; the parallel TS SDK is migrating to match.

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
