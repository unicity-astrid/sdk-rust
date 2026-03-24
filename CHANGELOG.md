# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changelog tracking starts with 0.2.0. Prior versions were not tracked.

## [Unreleased]

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
