//! Component Model bindings for the Astrid OS System API (The Airlocks).
//!
//! This crate generates typed guest bindings from the per-domain WIT
//! packages in `contracts/host/` (the `unicity-astrid/wit` submodule).
//! `build.rs` stages those files into `wit-staging/deps/astrid-<pkg>/`;
//! a single [`wit_bindgen::generate!`] invocation then emits one Rust
//! module per package under a synthetic `capsule` world that imports
//! every host package and includes every guest export world.
//!
//! What this crate provides:
//!
//! - **Host import functions** — typed calls to the kernel under
//!   `astrid::{fs, ipc, net, http, kv, sys, ...}::host`.
//! - **Guest export trait** — `Guest` trait combining all four guest
//!   worlds (interceptor, background, installable, upgradable). Each
//!   method maps to an `astrid-hook-trigger` / `run` / `astrid-install`
//!   / `astrid-upgrade` export. Capsules that don't implement an
//!   export emit a stub the kernel detects.
//! - **`export!` macro** — wires a `Guest` implementation as
//!   component exports.
//! - **WIT types** — generated Rust structs for every WIT record /
//!   variant / enum across all imported / exported packages, plus
//!   the foundation `astrid:io/{error, poll, streams}` resources.
//!
//! Capsule authors typically use `astrid-sdk` (the ergonomic wrapper)
//! rather than this crate directly. The `#[capsule]` proc macro
//! generates the `impl Guest` and `export!()` call automatically.
//!
//! ## ABI evolution
//!
//! Every package imported here is pinned at `@1.0.0`. When a new
//! frozen version ships (e.g. `host/ipc@1.1.0.wit`), add it to the
//! inline world as an additional `import` — the Component Model
//! linker enforces exact `(package, version)` matches, so capsules
//! pinned at the older version continue to resolve their old
//! interface unchanged.

#![deny(clippy::all)]
#![deny(unreachable_pub)]

// wit-bindgen generates code with patterns that trip clippy (e.g. Vec::from_raw_parts
// with same length and capacity). Suppress only for the generated module.
#[allow(clippy::all, clippy::pedantic, unreachable_pub, unsafe_code)]
mod generated {
    wit_bindgen::generate!({
        inline: "
            package astrid-sdk:capsule;

            /// Synthetic SDK world.
            ///
            /// Imports every frozen host package so capsules using
            /// the SDK can call any host fn through the ergonomic
            /// `astrid-sdk` wrappers.
            ///
            /// Includes every guest export world. Capsules that only
            /// implement one (say, interceptor) will see the unused
            /// exports stubbed by the toolchain; the kernel detects
            /// those stubs at load time (see
            /// `astrid-capsule::engine::wasm::mod::wasm_exports_contain`)
            /// and only dispatches to real implementations.
            ///
            /// `astrid:io/{error, poll, streams}` are the Astrid-owned
            /// foundation I/O primitives — no `wasi:*` dependency. The
            /// shape mirrors `wasi:io@0.2.0` but every operation is
            /// gated, audited, principal-scoped, and cancellable on
            /// the kernel side.
            world capsule {
                import astrid:io/error@1.0.0;
                import astrid:io/poll@1.0.0;
                import astrid:io/streams@1.0.0;

                import astrid:fs/host@1.0.0;
                import astrid:ipc/host@1.0.0;
                import astrid:kv/host@1.0.0;
                import astrid:net/host@1.0.0;
                import astrid:http/host@1.0.0;
                import astrid:sys/host@1.0.0;
                import astrid:process/host@1.0.0;
                import astrid:uplink/host@1.0.0;
                import astrid:elicit/host@1.0.0;
                import astrid:approval/host@1.0.0;
                import astrid:identity/host@1.0.0;

                include astrid:guest/interceptor@1.0.0;
                include astrid:guest/background@1.0.0;
                include astrid:guest/installable@1.0.0;
                include astrid:guest/upgradable@1.0.0;
            }
        ",
        path: "wit-staging",
        pub_export_macro: true,
        generate_unused_types: true,
        // Tell wit-bindgen to emit guest-side Rust wrappers for every
        // resource the host owns. We don't need to provide a host
        // type — wit-bindgen synthesizes a typed handle that calls
        // the resource's drop fn when it goes out of scope.
        generate_all,
        // No `additional_derives` — pre-v1 we blanket-derived
        // serde::Serialize / Deserialize on every generated type, but
        // wit-bindgen now emits resource types whose handles cannot be
        // round-tripped through serde (the handle owns a kernel-side
        // resource via Drop). The `astrid-sdk` wrappers convert
        // records to/from serde-friendly shapes at the boundary; raw
        // wit types stay non-serializable.
    });
}

pub use generated::*;
