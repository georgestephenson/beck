//! Beck's native backend: `Core` compiled to machine code through LLVM.
//!
//! # What this is
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.2 names LLVM as the
//! release codegen and [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md)
//! §4.8 names a differential test *between backends* as the thing that keeps two of them honest.
//! Neither could exist while `beck-eval` was the only implementation of
//! [`beck_core::backend::Backend`]. This crate is the second one.
//!
//! It compiles the **scalar subset** of `Core` — see [`emit`] for exactly what that is and what it
//! refuses — through textual LLVM IR and the host's `clang`, and runs the result as a separate
//! process ([`worker`]). Everything outside the subset falls back to the evaluator, and
//! [`Report`] says which definitions went which way, by name, with a reason for each refusal.
//!
//! # What this is not
//!
//! Not a general backend. There is no heap here, so a fold over a record, a view that builds
//! `Html` and every effect in the language still run on the tree-walker; `beck run` and `beck up`
//! are unchanged. Not Cranelift either: §5.2's dual-codegen story has one half built, and the
//! `beck dev` half is not this.
//!
//! # Using it
//!
//! ```no_run
//! # fn main() -> Result<(), String> {
//! let (placed, diags, _) = beck_core::compile_str("t.beck", "");
//! # let _ = diags;
//! let placed = placed.expect("compiles");
//! let program = std::sync::Arc::new(placed.program.clone());
//! // `None` when the machine has no LLVM: every other backend still works, and the caller says so.
//! if let Some(artifact) = beck_llvm::Artifact::build(&program).transpose() {
//!     let artifact = artifact?;
//!     println!("{}", artifact.report());
//! }
//! # Ok(())
//! # }
//! ```

pub mod emit;
pub mod toolchain;
pub mod worker;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use beck_core::backend::{Backend, Callable, ExecError};
use beck_core::check::Program;
use beck_core::core::CoreKind;
use beck_core::{Core, Value};
use beck_diag::Span;

pub use emit::{module, Module, Refusal, Scalar, Signature, Trap, MAX_PARAMS};
pub use toolchain::Toolchain;
pub use worker::Worker;

/// A compiled program: the module, the executable, and the process running it.
pub struct Artifact {
    module: Module,
    toolchain: Toolchain,
    worker: Worker,
    dir: Workspace,
    ll: PathBuf,
    exe: PathBuf,
}

impl Artifact {
    /// Compile `program`, or answer `None` if this machine has no LLVM.
    ///
    /// `None` and not an error, because "there is no toolchain here" is a fact about the machine
    /// and not a fault in the program. Every caller in this workspace turns it into a printed skip
    /// — `BECK_REQUIRE_LLVM=1` is what forbids the skip in a run that must not be silent about it.
    pub fn build(program: &Program) -> Result<Option<Artifact>, String> {
        let Some(toolchain) = Toolchain::find() else {
            return Ok(None);
        };
        Artifact::build_bounded(program, toolchain, None, None).map(Some)
    }

    /// The same, killing the worker if any one call takes longer than `limit`.
    pub fn build_within(
        program: &Program,
        limit: std::time::Duration,
    ) -> Result<Option<Artifact>, String> {
        let Some(toolchain) = Toolchain::find() else {
            return Ok(None);
        };
        Artifact::build_bounded(program, toolchain, None, Some(limit)).map(Some)
    }

    /// The same, with a toolchain the caller chose and somewhere to leave the output.
    ///
    /// `keep` is what `beck native --out` passes: a directory that outlives the process, so the
    /// `.ll` and the executable can be read after the compiler has exited. Without one, both live
    /// in a temporary directory that is removed when the [`Artifact`] is dropped.
    pub fn build_with(
        program: &Program,
        toolchain: Toolchain,
        keep: Option<&Path>,
    ) -> Result<Artifact, String> {
        Artifact::build_bounded(program, toolchain, keep, None)
    }

    /// The same, killing the worker if any one call takes longer than `limit`.
    ///
    /// There is no fuel in compiled code — see [`worker`] — so this is the whole of what bounds a
    /// program that will not stop. Every harness in this workspace sets one; nothing else does,
    /// because a native binary computing for a long time is a native binary doing its job.
    pub fn build_bounded(
        program: &Program,
        toolchain: Toolchain,
        keep: Option<&Path>,
        limit: Option<std::time::Duration>,
    ) -> Result<Artifact, String> {
        let module = emit::module(program);
        let dir = match keep {
            Some(path) => {
                std::fs::create_dir_all(path)
                    .map_err(|e| format!("creating {}: {e}", path.display()))?;
                Workspace::borrowed(path.to_path_buf())
            }
            None => Workspace::temporary()?,
        };
        let stem = sanitise(&program.name);
        let ll = dir.path.join(format!("{stem}.ll"));
        let exe = dir.path.join(&stem);
        toolchain.build(&module.ir, &ll, &exe)?;
        let worker = Worker::start_with(&exe, limit)?;
        Ok(Artifact {
            module,
            toolchain,
            worker,
            dir,
            ll,
            exe,
        })
    }

    pub fn module(&self) -> &Module {
        &self.module
    }

    pub fn toolchain(&self) -> &Toolchain {
        &self.toolchain
    }

    /// The directory holding the module and the executable.
    ///
    /// Temporary and removed on drop unless [`Artifact::build_with`] was given somewhere to keep
    /// the output.
    pub fn directory(&self) -> &Path {
        &self.dir.path
    }

    /// Where the generated IR was written.
    pub fn ir_path(&self) -> &Path {
        &self.ll
    }

    /// Where the executable was written.
    pub fn executable(&self) -> &Path {
        &self.exe
    }

    /// What compiled and what did not.
    pub fn report(&self) -> Report<'_> {
        Report {
            module: &self.module,
            toolchain: &self.toolchain,
        }
    }

    /// Call a compiled definition by name.
    ///
    /// Errors rather than falls back: this is the entry point for a harness that means to measure
    /// or compare *the native backend*, and one that silently ran the evaluator instead would
    /// measure the wrong thing and compare a backend with itself.
    pub fn call(&self, name: &str, args: &[Value]) -> Result<Value, ExecError> {
        let sig = self.module.signature(name).ok_or_else(|| {
            ExecError::new(format!("`{name}` did not compile natively"), Span::NONE)
        })?;
        self.invoke(sig, args)
    }

    fn invoke(&self, sig: &Signature, args: &[Value]) -> Result<Value, ExecError> {
        if args.len() != sig.params.len() {
            return Err(ExecError::new(
                format!(
                    "`{}` takes {} arguments, got {}",
                    sig.name,
                    sig.params.len(),
                    args.len()
                ),
                Span::NONE,
            ));
        }
        let mut cells = Vec::with_capacity(args.len());
        for (arg, want) in args.iter().zip(&sig.params) {
            cells.push(widen(arg, *want).ok_or_else(|| {
                ExecError::new(
                    format!(
                        "`{}` expects a {} where it was given `{}`",
                        sig.name,
                        match want {
                            Scalar::Int => "Int",
                            Scalar::Float => "Float",
                            Scalar::Bool => "Bool",
                        },
                        arg.display()
                    ),
                    Span::NONE,
                )
            })?);
        }

        let reply = self
            .worker
            .call(sig.index, &cells)
            .map_err(|e| ExecError::new(e, Span::NONE))?;
        if reply.code != 0 {
            let span = self
                .module
                .spans
                .get(reply.span as usize)
                .copied()
                .unwrap_or(Span::NONE);
            let message = match Trap::from_code(reply.code) {
                Some(trap) => trap.message(reply.payload),
                None => format!("the compiled program reported trap {}", reply.code),
            };
            return Err(ExecError::new(message, span));
        }
        Ok(narrow(reply.value, sig.ret))
    }
}

/// A `Value` as the eight bytes the protocol carries, if it is of the type the signature wants.
fn widen(v: &Value, want: Scalar) -> Option<u64> {
    match (v, want) {
        (Value::Int(i), Scalar::Int) => Some(*i as u64),
        (Value::Bool(b), Scalar::Bool) => Some(u64::from(*b)),
        // Through `as_f64` rather than off the discriminant: `Value::Float` holds the *order key*,
        // and the compiled code works in ordinary IEEE bits.
        (Value::Float(_), Scalar::Float) => Some(v.as_f64()?.to_bits()),
        _ => None,
    }
}

fn narrow(bits: u64, ty: Scalar) -> Value {
    match ty {
        Scalar::Int => Value::Int(bits as i64),
        Scalar::Bool => Value::Bool(bits != 0),
        // `Value::float` and not `Value::Float`: the constructor applies the order-key transform
        // and the canonicalisation, which is what makes this equal to what the evaluator built.
        Scalar::Float => Value::float(f64::from_bits(bits)),
    }
}

/// What compiled, what did not, and what compiled it — rendered for a person.
pub struct Report<'a> {
    module: &'a Module,
    toolchain: &'a Toolchain,
}

impl Report<'_> {
    pub fn compiled(&self) -> &[Signature] {
        &self.module.functions
    }

    pub fn refusals(&self) -> &[Refusal] {
        &self.module.refusals
    }
}

impl fmt::Display for Report<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.toolchain.version)?;
        writeln!(
            f,
            "\n{} compiled to native code:",
            self.module.functions.len()
        )?;
        for sig in &self.module.functions {
            let params: Vec<&str> = sig.params.iter().map(|p| p.llvm()).collect();
            writeln!(
                f,
                "  {:<28} ({}) -> {}",
                sig.name,
                params.join(", "),
                sig.ret.llvm()
            )?;
        }
        if !self.module.refusals.is_empty() {
            writeln!(f, "\n{} left to the evaluator:", self.module.refusals.len())?;
            for r in &self.module.refusals {
                writeln!(f, "  {:<28} {}", r.name, r.reason)?;
            }
        }
        Ok(())
    }
}

// -------------------------------------------------------------------------------------------
// The seam
// -------------------------------------------------------------------------------------------

/// The native backend, with somewhere for everything it cannot compile to go.
///
/// A [`Backend`] has to answer for the *whole* program — the runtime prepares `validate`, the fold
/// and the view without asking whether they are compilable — so this is a pair rather than a
/// single implementation. What that costs in honesty is paid by [`Native::compiled`]: a harness
/// can ask whether a particular role went native instead of assuming it did.
pub struct Native {
    artifact: Arc<Artifact>,
    fallback: Arc<dyn Backend>,
    /// The span of each compiled definition's body, against the index that runs it.
    ///
    /// A definition's body is one lambda at one source location, so its span names it and nothing
    /// else. Matching on the span rather than on the tree is what lets [`Backend::function`] —
    /// which is handed an *expression*, not a name — recognise the definitions it compiled;
    /// `the_seam_recognises_a_compiled_definition` is the gate that says it does.
    by_span: Vec<(Span, usize, u32)>,
}

impl Native {
    /// A backend over `program`, or `None` when this machine has no LLVM.
    ///
    /// `limit` bounds one call, and is how a harness keeps a compiled loop that will not stop from
    /// stopping the harness instead. `None` is the right answer outside a harness.
    pub fn build(
        program: &Arc<Program>,
        fallback: Arc<dyn Backend>,
        limit: Option<std::time::Duration>,
    ) -> Result<Option<Native>, String> {
        let artifact = match limit {
            Some(limit) => Artifact::build_within(program, limit)?,
            None => Artifact::build(program)?,
        };
        let Some(artifact) = artifact else {
            return Ok(None);
        };
        Ok(Some(Native::over(Arc::new(artifact), program, fallback)))
    }

    pub fn over(artifact: Arc<Artifact>, program: &Program, fallback: Arc<dyn Backend>) -> Native {
        let mut by_span = Vec::new();
        for sig in &artifact.module.functions {
            let Some(def) = program.defs.get(&sig.name) else {
                continue;
            };
            let CoreKind::Lam { params, .. } = &def.body.kind else {
                continue;
            };
            // A synthesized body carries no span, and a span that names nothing cannot identify
            // one definition among several. Such a definition is compiled but not *recognised*,
            // which costs a fallback and never a wrong answer.
            if def.body.span == Span::NONE {
                continue;
            }
            by_span.push((def.body.span, params.len(), sig.index));
        }
        Native {
            artifact,
            fallback,
            by_span,
        }
    }

    pub fn artifact(&self) -> &Arc<Artifact> {
        &self.artifact
    }

    /// Whether this expression is one the native half will answer.
    pub fn compiled(&self, code: &Core) -> bool {
        self.index_of(code).is_some()
    }

    fn index_of(&self, code: &Core) -> Option<u32> {
        let CoreKind::Lam { params, .. } = &code.kind else {
            return None;
        };
        self.by_span
            .iter()
            .find(|(span, arity, _)| *span == code.span && *arity == params.len())
            .map(|(_, _, index)| *index)
    }
}

impl Backend for Native {
    fn name(&self) -> &'static str {
        "native"
    }

    /// The fallback's, because the fallback is what runs everything this cannot compile.
    ///
    /// Compiled code needs no host stack of its own — it recurses on the thread's stack and LLVM
    /// turns a tail call into a jump — but a backend that answered `0` here would under-provision
    /// the tree-walker sitting behind it, which is `docs/31` §31.3's abort with an extra step.
    fn stack_bytes(&self) -> usize {
        self.fallback.stack_bytes()
    }

    fn constant(&self, code: &Core) -> Result<Value, ExecError> {
        self.fallback.constant(code)
    }

    fn function(&self, code: &Core) -> Result<Callable, ExecError> {
        let Some(index) = self.index_of(code) else {
            return self.fallback.function(code);
        };
        let artifact = self.artifact.clone();
        let sig = artifact.module.functions[index as usize].clone();
        Ok(Arc::new(move |args: Vec<Value>| {
            artifact.invoke(&sig, &args)
        }))
    }

    /// Interception is a property of *how* a backend executes a call, and the compiled half does
    /// not consult anything per call. Rather than pretend, this hands back the fallback's
    /// intercepting backend: `beck test` gets stubs, and gets them from the tree-walker.
    fn intercepting(
        &self,
        by: Arc<dyn beck_core::backend::Interceptor>,
    ) -> Option<Arc<dyn Backend>> {
        self.fallback.intercepting(by)
    }
}

// -------------------------------------------------------------------------------------------
// Somewhere to put the output
// -------------------------------------------------------------------------------------------

/// A directory for the `.ll` and the executable.
struct Workspace {
    path: PathBuf,
    /// Whether dropping this removes the directory. A caller who asked for the output to be kept
    /// wants it after the process exits.
    owned: bool,
}

impl Workspace {
    fn borrowed(path: PathBuf) -> Workspace {
        Workspace { path, owned: false }
    }

    fn temporary() -> Result<Workspace, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "beck-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).map_err(|e| format!("creating {}: {e}", path.display()))?;
        Ok(Workspace { path, owned: true })
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// A module name as a file name: everything that is not a letter, a digit or a dash becomes one.
fn sanitise(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        "module".into()
    } else {
        trimmed.to_string()
    }
}
