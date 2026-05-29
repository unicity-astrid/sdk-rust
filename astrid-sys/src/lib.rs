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

// ---------------------------------------------------------------------------
// getrandom custom backend → astrid:sys/host.random-bytes
// ---------------------------------------------------------------------------
//
// Activated when the build is configured with
// `--cfg=getrandom_backend="custom"` (set in each capsule's
// `.cargo/config.toml` for the `wasm32-unknown-unknown` target). On other
// targets the symbol is omitted, so host-tooling builds (build scripts,
// proc-macros, tests on the developer machine) keep using the platform's
// default RNG.
//
// `wasm32-unknown-unknown` is the Astrid-canonical guest target: it has
// zero `wasi:*` imports, so every kernel call shows in
// `astrid.audit.*` and the WIT imports list IS the capsule's literal
// capability list. The price is that `getrandom` (pulled in
// transitively via `uuid` / `astrid-types`) has no platform backend on
// that target — `HashMap` would panic on first insert without this
// shim.
//
// The `#[unsafe(no_mangle)]` symbol below is what `getrandom`'s custom
// backend expects (see getrandom 0.4 docs). Defining it here in
// `astrid-sys` keeps the routing centralised: every capsule depending
// on `astrid-sdk` gets a working RNG with zero per-capsule wiring.
#[cfg(all(target_arch = "wasm32", getrandom_backend = "custom"))]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom::Error> {
    // Pull bytes from the kernel in chunks bounded by the host fn's
    // per-call cap. `astrid:sys/host.random-bytes` is served straight
    // from the host's OS CSPRNG; per the `astrid:sys` WIT contract it is
    // intentionally ungated and not audited (read-only, no side effects),
    // unlike the gated/audited host calls such as fs / net / process.
    const CHUNK: usize = 4096;
    let mut written = 0usize;
    while written < len {
        let want = core::cmp::min(CHUNK, len - written);
        let chunk = generated::astrid::sys::host::random_bytes(want as u64)
            .map_err(|_| getrandom::Error::new_custom(1))?;
        if chunk.is_empty() {
            return Err(getrandom::Error::new_custom(2));
        }
        let take = core::cmp::min(chunk.len(), want);
        // SAFETY: caller guarantees `dest..dest+len` is a valid byte
        // buffer (may be uninitialized — write-only access here).
        unsafe {
            core::ptr::copy_nonoverlapping(chunk.as_ptr(), dest.add(written), take);
        }
        written += take;
    }
    Ok(())
}
