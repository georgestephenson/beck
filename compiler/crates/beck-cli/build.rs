//! What `beck --version` says beyond the number.
//!
//! A version alone identifies a *release*; a bug report about a downloaded binary has to identify
//! the *artefact* — which commit, built for which platform. `docs/28-releases-and-deployment.md`
//! §28.2 publishes one tarball per target from one tagged commit, so those two facts are exactly
//! what tells four artefacts of the same release apart.
//!
//! Both are best-effort and neither can fail a build: a source tarball has no `.git`, and the
//! answer there is the string `unknown` rather than a broken build. `BECK_COMMIT` overrides the
//! git lookup, which is how a packager who has the commit but not the repository supplies it.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=BECK_COMMIT");

    // Cargo hands every build script the triple it is building for, so this needs no detection.
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=BECK_TARGET={target}");

    let commit = std::env::var("BECK_COMMIT")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(git_commit)
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BECK_COMMIT={commit}");

    // Only when there is one: `rerun-if-changed` on a path that does not exist asks cargo to
    // rebuild this crate on every invocation.
    if let Some(head) = git_head() {
        println!("cargo:rerun-if-changed={}", head.display());
    }
}

fn repo_root() -> Option<std::path::PathBuf> {
    // .../compiler/crates/beck-cli → the repository
    Path::new(&std::env::var("CARGO_MANIFEST_DIR").ok()?)
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
}

fn git_head() -> Option<std::path::PathBuf> {
    let head = repo_root()?.join(".git/HEAD");
    head.exists().then_some(head)
}

fn git_commit() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(repo_root()?)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|c| !c.is_empty())
}
