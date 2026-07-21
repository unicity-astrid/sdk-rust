//! Build script for `astrid-sys`.
//!
//! Stages the WIT submodule into a layout `wit_bindgen::generate!` can
//! resolve. The canonical WIT lives at `sdk-rust/contracts/` (a submodule
//! of `astrid-runtime/wit`) with per-domain packages under `host/` and
//! the guest-side lifecycle worlds under `host/guest@1.0.0.wit`.
//!
//! wit-bindgen expects a single root directory with one package per
//! `deps/<name>/` subdir, so we copy each `host/<pkg>@<ver>.wit` into
//! `wit-staging/deps/astrid-<pkg>@<ver>/<pkg>@<ver>.wit` — keyed by the
//! full `<pkg>@<ver>` stem so a package can ship multiple frozen versions
//! (e.g. `http@1.0.0` + `http@1.1.0`) side by side. The synthetic
//! `capsule` world that imports every host package and includes every
//! guest export world is supplied via the `inline:` option in
//! `src/lib.rs`.
//!
//! No external WIT packages are vendored — the contract is fully
//! Astrid-owned (`astrid:*` only, no `wasi:*` dependency).
//!
//! Two execution modes:
//!
//! - **Workspace builds**: the `contracts/` submodule is present at
//!   `sdk-rust/contracts/host/`. Clean and re-stage `wit-staging/` from
//!   the submodule so the committed copy stays in lockstep with the
//!   canonical source.
//! - **Published builds** (`cargo install`, `cargo publish` verifier):
//!   the submodule isn't part of the `.crate` tarball. Skip staging —
//!   the committed `wit-staging/` ships with the crate and is what
//!   `wit_bindgen::generate!` consumes.

use std::fs;
use std::path::PathBuf;

fn main() {
    // Tell rustc the `getrandom_backend="custom"` cfg flag is known —
    // capsule builds set it via `.cargo/config.toml` rustflags. The
    // check-cfg declaration suppresses the `unexpected_cfgs` lint when
    // the flag isn't set (host-tooling builds, wasip2 builds).
    println!("cargo::rustc-check-cfg=cfg(getrandom_backend, values(\"custom\"))");

    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let host_src = crate_root
        .parent() // sdk-rust/
        .expect("astrid-sys must live under the sdk-rust workspace root")
        .join("contracts")
        .join("host");

    let staging = crate_root.join("wit-staging");
    let deps = staging.join("deps");

    // Published-crate path: the `astrid-runtime/wit` submodule isn't
    // available on a consumer's machine. The committed `wit-staging/`
    // ships with the crate; `src/lib.rs`'s `wit_bindgen::generate!`
    // reads it directly. Skip the stage step.
    //
    // Empty-submodule path: a fresh clone without `git submodule
    // update --init` leaves `host_src/` non-existent or empty. Treat
    // identically to the published-crate path so we don't wipe the
    // committed wit-staging.
    let has_wit_files = fs::read_dir(&host_src)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wit"))
            })
        })
        .unwrap_or(false);
    if !has_wit_files {
        // Watch the same surface we'd watch in the staging path. Without
        // these, Cargo won't rerun build.rs after a developer runs
        // `git submodule update --init` against a fresh clone, so the
        // committed wit-staging would stay stale relative to the now-
        // checked-out submodule.
        println!("cargo:rerun-if-changed=wit-staging");
        println!("cargo:rerun-if-changed={}", host_src.display());
        println!("cargo:rerun-if-changed=build.rs");
        println!(
            "cargo:rerun-if-changed={}",
            crate_root.parent().unwrap().join(".gitmodules").display()
        );
        return;
    }

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

    for entry in fs::read_dir(&host_src).expect("read contracts/host") {
        let entry = entry.unwrap();
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !std::path::Path::new(file_name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wit"))
        {
            continue;
        }
        // Key each deps dir by the FULL `<pkg>@<version>` stem, not just
        // `<pkg>`. A package may now ship multiple frozen versions side by
        // side (e.g. `http@1.0.0.wit` + `http@1.1.0.wit`). wit-bindgen
        // resolves one package per `deps/<dir>/`, and rejects two files
        // declaring different `(package, version)` identifiers in the same
        // dir ("package identifier `astrid:http@1.1.0` does not match
        // previous package name of `astrid:http@1.0.0`"). Staging each
        // version in its own `deps/astrid-<pkg>@<version>/` keeps every
        // frozen version independently resolvable, so a capsule pinned at
        // the old version keeps its old interface while the inline `world`
        // imports whichever version it names.
        let stem = file_name.trim_end_matches(".wit");
        let dst_dir = deps.join(format!("astrid-{stem}"));
        fs::create_dir_all(&dst_dir).expect("mkdir deps/astrid-<pkg>@<ver>");
        let dst = dst_dir.join(file_name);
        fs::copy(&path, &dst).expect("copy host wit");
        println!("cargo:rerun-if-changed={}", path.display());
    }

    // Before 1.0, contract PRs intentionally remain unmerged in the canonical
    // WIT repository. Keep those explicitly draft surfaces in an SDK-owned
    // overlay so workspace builds can exercise them without changing the
    // submodule pin. They use the same one-package-per-directory staging shape
    // and may not replace a canonical file of the same name.
    let experimental = crate_root.join("wit-experimental");
    if let Ok(entries) = fs::read_dir(&experimental) {
        for entry in entries {
            let path = entry.expect("read experimental WIT entry").path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("wit"))
            {
                continue;
            }
            let stem = file_name.trim_end_matches(".wit");
            let dst_dir = deps.join(format!("astrid-{stem}"));
            fs::create_dir_all(&dst_dir).expect("mkdir experimental WIT package");
            let destination = dst_dir.join(file_name);
            assert!(
                !destination.exists(),
                "experimental WIT must not replace canonical package {}",
                destination.display()
            );
            fs::copy(&path, &destination).expect("stage experimental WIT package");
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    println!("cargo:rerun-if-changed={}", host_src.display());
    println!("cargo:rerun-if-changed={}", experimental.display());
    println!("cargo:rerun-if-changed=build.rs");
    // CI environments may run `git submodule update` lazily; the
    // .gitmodules pointer changing without the working tree yet
    // checked out should still invalidate the staging dir.
    println!(
        "cargo:rerun-if-changed={}",
        crate_root.parent().unwrap().join(".gitmodules").display()
    );
}
