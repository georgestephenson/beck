//! Building the runtime library, and putting it where a link step can reach it.
//!
//! `beck-prim` is a `staticlib`: an archive a compiled program links against
//! ([`crate::prim`](src/prim.rs)). Two things have to be true of it that an ordinary dependency
//! does not have to satisfy.
//!
//! * It is built **for the program**, not for the compiler, so it is a second `cargo` invocation
//!   with its own target directory rather than a line in `Cargo.toml`.
//! * It has to be *inside* the `beck` binary. A release is one executable per platform
//!   (`docs/28` §28.2), and a compiler that looked for an archive beside itself would be a
//!   compiler that works from `target/debug` and not after `install.sh`.
//!
//! So the archive is built here, into `OUT_DIR`, and `include_bytes!` puts it in the binary.
//! Always in release: this is the program's runtime, and a debug build of it would make every
//! compiled program's digests slow for no reason a user of the compiler could see.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let prim = manifest
        .parent()
        .expect("crates/")
        .join("beck-prim")
        .canonicalize()
        .expect("the runtime library is a sibling crate");
    println!("cargo:rerun-if-changed={}", prim.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        prim.join("Cargo.toml").display()
    );

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets this"));
    let target = std::env::var("TARGET").expect("cargo sets this");
    let into = out.join("prim");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut build = Command::new(cargo);
    build
        .arg("build")
        .args(["--package", "beck-prim"])
        .args(["--features", "abi"])
        .arg("--release")
        // The same lock file the compiler is built from, and refusing to update it: a release
        // build is `cargo build --release --locked` (`docs/28` §28.2), and an inner build that
        // could resolve a different graph would put a dependency in the artefact that is in no
        // lock file anybody checked.
        .arg("--locked")
        .args(["--target", &target])
        .arg("--target-dir")
        .arg(&into)
        .current_dir(&prim);
    // The outer build's settings are for the *compiler*: a `RUSTFLAGS` chosen for it, and a
    // wrapper that would recurse. What is deliberately kept is `CARGO_HOME` and the network
    // settings, so this resolves from the same cache and works offline exactly as the outer build
    // does.
    for leaked in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOCFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_RUSTFLAGS",
    ] {
        build.env_remove(leaked);
    }

    let built = build
        .output()
        .expect("running cargo for the runtime library");
    if !built.status.success() {
        panic!(
            "the runtime library did not build ({}):\n{}",
            built.status,
            String::from_utf8_lossy(&built.stderr)
        );
    }

    let archive = into.join(&target).join("release").join("libbeck_prim.a");
    assert!(
        archive.is_file(),
        "the runtime library built but left no archive at {}",
        archive.display()
    );

    // Compressed on the way in, because a `staticlib` carries the whole standard library whether a
    // primitive reaches it or not: 21 MiB of archive would be 21 MiB of `beck`. The level is the
    // default rather than the best — this runs on every source change, and the last two per cent
    // cost more time than the binary saves.
    let raw = std::fs::read(&archive).expect("reading the archive");
    let mut squeezed =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    squeezed.write_all(&raw).expect("compressing the archive");
    let squeezed = squeezed.finish().expect("compressing the archive");
    let packed = out.join("libbeck_prim.a.z");
    std::fs::write(&packed, &squeezed).expect("writing the compressed archive");

    println!(
        "cargo:rustc-env=BECK_PRIM_ARCHIVE={}",
        canonical(&packed).display()
    );
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
