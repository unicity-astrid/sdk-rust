# astrid-sdk

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV: 1.94](https://img.shields.io/badge/MSRV-1.94-blue)](https://www.rust-lang.org)

**The Rust SDK for building [Astrid](https://github.com/unicity-astrid/astrid) capsules.**

In the OS model, this is the standard library for user-space processes. It gives capsule authors safe, typed access to every kernel service: filesystem, IPC, networking, storage, approval, scheduling, and more. Capsule authors depend on `astrid-sdk` and `serde`. Everything else is handled.

## Crates

| Crate | Role |
|---|---|
| `astrid-sdk` | Safe Rust SDK for capsule authors. Mirrors `std` module layout: `fs`, `net`, `process`, `env`, `time`, `log`, plus Astrid-specific modules: `ipc`, `kv`, `http`, `hooks`, `cron`, `uplink`, `identity`, `approval`, `runtime`. |
| `astrid-sdk-macros` | `#[capsule]` proc macro. Generates WASM ABI exports from annotated impl blocks: tool dispatch, command routing, hook handlers, cron handlers, install/upgrade entry points. |
| `astrid-sys` | Raw WASM-to-host FFI bindings. The syscall table. Every parameter crosses as `Vec<u8>`. You should not use this directly. |

## Quick start

```toml
[dependencies]
astrid-sdk = "0.2"
serde = { version = "1.0", features = ["derive"] }
```

```rust
use astrid_sdk::prelude::*;

#[derive(Default)]
pub struct MyTools;

#[capsule]
impl MyTools {
    #[astrid::tool]
    fn search_issues(&self, args: SearchArgs) -> Result<SearchResult, SysError> {
        let token = env::var("GITHUB_TOKEN")?;
        let resp = http::get(&format!(
            "https://api.github.com/search/issues?q={}", args.query
        ))?;
        // ...
    }
}
```

The `#[capsule]` macro generates all WASM ABI boilerplate: `extern "C"` exports, JSON serialization across the boundary, tool schema generation, and dispatch routing.

## Building capsules

Capsules compile to `wasm32-wasip2`:

```bash
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

## Development

```bash
cargo build --workspace
cargo test --workspace -- --quiet
cargo clippy --workspace --all-features -- -D warnings
```

## License

Dual-licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).

Copyright (c) 2025-2026 Joshua J. Bouw and Unicity Labs.
