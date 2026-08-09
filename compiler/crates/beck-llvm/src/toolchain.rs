//! Finding LLVM, and handing it the module.
//!
//! This crate depends on LLVM the way a build depends on a linker: by name, at run time, through
//! the file system. There is no `llvm-sys`, no `build.rs` probing for a library, and nothing in
//! `Cargo.toml` that changes when the host's LLVM does — which is the point. A machine without a
//! toolchain still builds and still runs every other test in the workspace; what it cannot do is
//! *this*, and [`Toolchain::find`] answers `None` so a caller can say so out loud rather than
//! failing in a way that looks like a bug.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A C compiler driver that can assemble textual LLVM IR and link it.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// The driver — `clang`, or whatever `BECK_CLANG` names.
    pub clang: PathBuf,
    /// The first line of `--version`, for a report that has to say what produced a number.
    pub version: String,
}

impl Toolchain {
    /// The toolchain this machine has, if it has one.
    ///
    /// `BECK_CLANG` first, so a machine with several LLVMs can say which. Then the unsuffixed
    /// name, then the versioned ones newest-first — Debian and Ubuntu install `clang-18` and no
    /// `clang` more often than not.
    pub fn find() -> Option<Toolchain> {
        if let Ok(explicit) = std::env::var("BECK_CLANG") {
            return probe(Path::new(&explicit));
        }
        let mut names = vec!["clang".to_string()];
        names.extend((15..=25).rev().map(|v| format!("clang-{v}")));
        names.iter().find_map(|n| probe(Path::new(n)))
    }

    /// Assemble and link `ir` into an executable at `exe`.
    ///
    /// `-O2` and not `-O3`: `docs/07` §7.2 buys LLVM for "best peak code quality available in open
    /// source", and `-O2` is what that phrase means in every distribution's build of every
    /// language that uses it. A report that quoted `-O3` would be quoting a setting nobody ships.
    pub fn build(&self, ir: &str, ll: &Path, exe: &Path) -> Result<(), String> {
        std::fs::write(ll, ir).map_err(|e| format!("writing {}: {e}", ll.display()))?;
        let out = Command::new(&self.clang)
            .arg("-O2")
            // The module names no target triple, so that the same `.ll` is the artefact on every
            // machine and the driver picks the host — which is exactly the case this warning is
            // about, and it is not news.
            .arg("-Wno-override-module")
            .arg("-o")
            .arg(exe)
            .arg(ll)
            // `llvm.sin.f64` and `llvm.cos.f64` lower to libm calls: LLVM has no instruction for
            // either, and the evaluator's `f64::sin` reaches the same library, which is what makes
            // the two agree bit for bit rather than nearly.
            .arg("-lm")
            .output()
            .map_err(|e| format!("running {}: {e}", self.clang.display()))?;
        if !out.status.success() {
            // The assembler's complaint names a line of the generated module, so the module is
            // left where it was written: a diagnostic about IR nobody can read is not one.
            return Err(format!(
                "{} rejected the generated module ({}):\n{}\nthe module is at {}",
                self.clang.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim(),
                ll.display()
            ));
        }
        Ok(())
    }
}

fn probe(path: &Path) -> Option<Toolchain> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    Some(Toolchain {
        clang: path.to_path_buf(),
        version,
    })
}
