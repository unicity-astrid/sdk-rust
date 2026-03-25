//! Component Model bindings for the Astrid OS System API (The Airlocks).
//!
//! This crate generates typed guest bindings from the `astrid-capsule.wit`
//! interface using `wit-bindgen`. It provides:
//!
//! - **Host import functions** — typed calls to the kernel (fs, ipc, kv, etc.)
//! - **Guest export trait** — `Guest` trait that capsules implement
//! - **WIT types** — generated Rust structs for all WIT records
//!
//! Capsule authors typically use `astrid-sdk` (the ergonomic wrapper) rather
//! than this crate directly. The `#[capsule]` proc macro generates the
//! `impl Guest` and `export!()` call automatically.

#![deny(clippy::all)]
#![deny(unreachable_pub)]

// wit-bindgen generates code with patterns that trip clippy (e.g. Vec::from_raw_parts
// with same length and capacity). Suppress only for the generated module.
#[allow(clippy::all, clippy::pedantic, unreachable_pub, unsafe_code)]
mod generated {
    wit_bindgen::generate!({
        world: "capsule",
        path: "../wit",
    });
}

pub use generated::*;
