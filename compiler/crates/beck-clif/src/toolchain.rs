//! Finding a linker, and handing it the object.
//!
//! Cranelift produces a relocatable object; turning one into a program that runs is a linker's
//! job, and this crate asks for one by name at run time exactly as [`beck_llvm::Toolchain`] asks
//! for `clang`. The difference is what is being asked *for*: LLVM has to be on the machine to
//! compile anything at all, and here the compiler is a crate — what the machine still has to
//! supply is `cc`, `libc` and `libm`, which is what "links a C program" means.
//!
//! `BECK_LINKER` names one explicitly on a machine with several. `None` from [`Linker::find`] is a
//! fact about the machine rather than a fault in the program, and every caller in this workspace
//! turns it into a printed skip.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A C compiler driver, used only as a linker.
#[derive(Clone, Debug)]
pub struct Linker {
    pub cc: PathBuf,
    /// The first line of `--version`, for a report that has to say what produced a number.
    pub version: String,
}

impl Linker {
    /// The linker this machine has, if it has one.
    pub fn find() -> Option<Linker> {
        if let Ok(explicit) = std::env::var("BECK_LINKER") {
            return probe(Path::new(&explicit));
        }
        ["cc", "clang", "gcc"]
            .iter()
            .find_map(|n| probe(Path::new(n)))
    }

    /// Link `object` into an executable at `exe`.
    ///
    /// `runtime` is the archive `beck_llvm::prim` staged, for a module that calls the runtime
    /// library — `None` for one that does not, which is most of them.
    pub fn link(
        &self,
        object: &[u8],
        obj: &Path,
        exe: &Path,
        runtime: Option<&Path>,
    ) -> Result<(), String> {
        std::fs::write(obj, object).map_err(|e| format!("writing {}: {e}", obj.display()))?;
        let mut cmd = Command::new(&self.cc);
        cmd.arg("-o").arg(exe).arg(obj);
        // After the object, because a static archive answers the symbols of what precedes it on
        // the line and nothing on the line precedes the object.
        if let Some(archive) = runtime {
            cmd.arg(archive);
        }
        // `sin` and `cos` are calls into the C library rather than instructions
        // ([`crate::emit`]), and the evaluator's `f64::sin` reaches the same library — which
        // is what makes the two agree bit for bit rather than nearly.
        cmd.arg("-lm");
        if runtime.is_some() {
            // What Rust's standard library needs from the system. On a glibc since 2.34 both are
            // empty stubs kept for exactly this kind of link line.
            cmd.args(["-lpthread", "-ldl"]);
        }
        let out = cmd
            .output()
            .map_err(|e| format!("running {}: {e}", self.cc.display()))?;
        if !out.status.success() {
            return Err(format!(
                "{} could not link the generated object ({}):\n{}\nthe object is at {}",
                self.cc.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim(),
                obj.display()
            ));
        }
        Ok(())
    }
}

fn probe(path: &Path) -> Option<Linker> {
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
    Some(Linker {
        cc: path.to_path_buf(),
        version,
    })
}
