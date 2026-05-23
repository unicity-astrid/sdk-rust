//! Build script for `astrid-sys`.
//!
//! Stages the WIT submodule into a layout `wit_bindgen::generate!` can
//! resolve. The canonical WIT lives at `sdk-rust/contracts/` (a submodule
//! of `unicity-astrid/wit`) with per-domain packages under `host/` and
//! the guest-side lifecycle worlds under `host/guest@1.0.0.wit`.
//!
//! wit-bindgen expects a single root directory with one package per
//! `deps/<name>/` subdir, so we copy each `host/<pkg>@<ver>.wit` into
//! `wit-staging/deps/astrid-<pkg>/<pkg>@<ver>.wit`. The synthetic
//! `capsule` world that imports every host package and includes every
//! guest export world is supplied via the `inline:` option in
//! `src/lib.rs`.
//!
//! No external WIT packages are vendored — the contract is fully
//! Astrid-owned (`astrid:*` only, no `wasi:*` dependency).

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Tell rustc the `getrandom_backend="custom"` cfg flag is known —
    // capsule builds set it via `.cargo/config.toml` rustflags. The
    // check-cfg declaration suppresses the `unexpected_cfgs` lint when
    // the flag isn't set (host-tooling builds, wasip2 builds).
    println!("cargo::rustc-check-cfg=cfg(getrandom_backend, values(\"custom\"))");

    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let contracts_root = crate_root
        .parent() // sdk-rust/
        .expect("astrid-sys must live under the sdk-rust workspace root")
        .join("contracts");

    let staging = crate_root.join("wit-staging");
    let deps = staging.join("deps");

    if staging.exists() {
        fs::remove_dir_all(&staging).expect("clean wit-staging");
    }
    fs::create_dir_all(&deps).expect("create wit-staging/deps");

    // Placeholder root package so wit-bindgen has a starting point.
    // The real synthetic `capsule` world is supplied inline from
    // `src/lib.rs`.
    fs::write(
        staging.join("root.wit"),
        "package astrid-root:placeholder;\n",
    )
    .expect("write root.wit");

    let host_src = contracts_root.join("host");
    for entry in fs::read_dir(&host_src).expect("read contracts/host") {
        let entry = entry.unwrap();
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".wit") {
            continue;
        }
        let stem = file_name.trim_end_matches(".wit");
        let pkg_name = stem.split('@').next().unwrap();
        let dst_dir = deps.join(format!("astrid-{pkg_name}"));
        fs::create_dir_all(&dst_dir).expect("mkdir deps/astrid-<pkg>");
        let dst = dst_dir.join(file_name);
        fs::copy(&path, &dst).expect("copy host wit");
        println!("cargo:rerun-if-changed={}", path.display());
    }

    rerun_if_dir_changed(&host_src);
    println!("cargo:rerun-if-changed=build.rs");
    // CI environments may run `git submodule update` lazily; the
    // .gitmodules pointer changing without the working tree yet
    // checked out should still invalidate the staging dir.
    println!(
        "cargo:rerun-if-changed={}",
        crate_root.parent().unwrap().join(".gitmodules").display()
    );
}

fn rerun_if_dir_changed(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
}
