# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changelog tracking starts with 0.2.0. Prior versions were not tracked.

## [Unreleased]

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
