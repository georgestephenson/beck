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

    // Only paths that exist: `rerun-if-changed` on a missing one asks cargo to rebuild this crate
    // on every invocation.
    for path in git_watch() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn repo_root() -> Option<std::path::PathBuf> {
    // .../compiler/crates/beck-cli → the repository
    Path::new(&std::env::var("CARGO_MANIFEST_DIR").ok()?)
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
}

/// What has to change for the stamped commit to be stale.
///
/// `.git/HEAD` is **not** enough on its own, and the difference is the whole point of the stamp:
/// on a branch, `HEAD` holds `ref: refs/heads/<branch>` and does not change when a commit is made
/// — the ref file does. Watching only `HEAD` gives a binary that keeps printing whichever commit
/// it was first built at, which is a wrong answer rather than a missing one. Both are watched, so
/// a commit, a checkout and a detached HEAD all invalidate it.
fn git_watch() -> Vec<std::path::PathBuf> {
    let Some(git) = repo_root().map(|r| r.join(".git")) else {
        return Vec::new();
    };
    let head = git.join("HEAD");
    if !head.exists() {
        return Vec::new();
    }
    let mut out = vec![head.clone()];
    // A symbolic ref names the file the commit actually lives in. A packed ref has no file, and
    // then `HEAD` alone is all there is to watch.
    if let Some(target) = std::fs::read_to_string(&head)
        .ok()
        .and_then(|t| t.strip_prefix("ref: ").map(|r| r.trim().to_string()))
    {
        let path = git.join(target);
        if path.exists() {
            out.push(path);
        }
    }
    out
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
