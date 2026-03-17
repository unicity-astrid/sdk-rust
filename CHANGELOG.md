# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Changelog tracking starts with 0.2.0. Prior versions were not tracked.

## [Unreleased]

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

[Unreleased]: https://github.com/unicity-astrid/sdk-rust/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/unicity-astrid/sdk-rust/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/unicity-astrid/sdk-rust/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/unicity-astrid/sdk-rust/releases/tag/v0.2.0
