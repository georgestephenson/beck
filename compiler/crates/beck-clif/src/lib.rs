//! Beck's development code generator: `Core` compiled to machine code through Cranelift.
//!
//! # What this is
//!
//! [`docs/07-dependencies.md`](../../../../docs/07-dependencies.md) §7.3 chooses two code
//! generators and says what each is for: **LLVM** for release, "best peak code quality available in
//! open source", and **Cranelift** for development, "~40% faster whole-compile and ~10× faster
//! codegen step than LLVM; makes `beck dev` hot reload feel instant".
//! [`93`](../../../../docs/93-llvm-backend-report.md) built the first and listed the second as the
//! half that did not exist. This is the second half.
//!
//! It compiles the same **scalar subset** [`beck_llvm`] compiles, to the same semantics, and it is
//! held to that by a differential that runs *both* against the evaluator and against each other.
//! What it does not share is the emitter: [`emit`] is a second implementation, and a second
//! implementation that agrees is the only kind of evidence a backend seam can offer.
//!
//! # What it costs, and what it saves
//!
//! Cranelift is a **crate**, so this needs no LLVM on the machine — only a linker, because an
//! object file is not a program. Execution is still a separate process reading a pipe, for
//! [`adr/0021`](../../../../docs/adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)'s
//! reason: turning a pointer into a function is `unsafe`, and
//! [`docs/43`](../../../../docs/43-threat-model.md) §43.2 claims `forbid(unsafe_code)` structurally
//! about first-party code. [`adr/0024`](../../../../docs/adr/0024-cranelift-emits-an-object-and-a-linker-makes-it-a-program.md)
//! records the shape.
//!
//! # Using it
//!
//! ```no_run
//! # fn main() -> Result<(), String> {
//! let (placed, diags, _) = beck_core::compile_str("t.beck", "");
//! # let _ = diags;
//! let placed = placed.expect("compiles");
//! let program = std::sync::Arc::new(placed.program.clone());
//! // `None` when the machine has no linker: every other backend still works, and the caller says so.
//! if let Some(artifact) = beck_clif::Artifact::build(&program).transpose() {
//!     let artifact = artifact?;
//!     println!("{}", artifact.report());
//! }
//! # Ok(())
//! # }
//! ```

pub mod emit;
pub mod toolchain;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use beck_core::backend::{Backend, Callable, ExecError};
use beck_core::check::Program;
use beck_core::core::CoreKind;
use beck_core::{Core, Value};
use beck_diag::Span;
use beck_llvm::{Refusal, Scalar, Signature, Trap, Worker};

pub use emit::{module, Module};
pub use toolchain::Linker;

/// A compiled program: the module, the executable, and the process running it.
pub struct Artifact {
    module: Module,
    linker: Linker,
    worker: Worker,
    dir: Workspace,
    clif: PathBuf,
    obj: PathBuf,
    exe: PathBuf,
    /// How long Cranelift itself took, which is the number §7.3's choice is about.
    codegen: std::time::Duration,
}

impl Artifact {
    /// Compile `program`, or answer `None` if this machine has no linker.
    pub fn build(program: &Program) -> Result<Option<Artifact>, String> {
        let Some(linker) = Linker::find() else {
            return Ok(None);
        };
        Artifact::build_bounded(program, linker, None, None).map(Some)
    }

    /// The same, killing the worker if any one call takes longer than `limit`.
    pub fn build_within(
        program: &Program,
        limit: std::time::Duration,
    ) -> Result<Option<Artifact>, String> {
        let Some(linker) = Linker::find() else {
            return Ok(None);
        };
        Artifact::build_bounded(program, linker, None, Some(limit)).map(Some)
    }

    /// The same, with a linker the caller chose and somewhere to leave the output.
    pub fn build_with(
        program: &Program,
        linker: Linker,
        keep: Option<&Path>,
    ) -> Result<Artifact, String> {
        Artifact::build_bounded(program, linker, keep, None)
    }

    pub fn build_bounded(
        program: &Program,
        linker: Linker,
        keep: Option<&Path>,
        limit: Option<std::time::Duration>,
    ) -> Result<Artifact, String> {
        let started = std::time::Instant::now();
        let module = emit::module(program)?;
        let codegen = started.elapsed();
        let dir = match keep {
            Some(path) => {
                std::fs::create_dir_all(path)
                    .map_err(|e| format!("creating {}: {e}", path.display()))?;
                Workspace::borrowed(path.to_path_buf())
            }
            None => Workspace::temporary()?,
        };
        let stem = sanitise(&program.name);
        let clif = dir.path.join(format!("{stem}.clif"));
        let obj = dir.path.join(format!("{stem}.o"));
        let exe = dir.path.join(&stem);
        // Written whether or not anybody reads it, for the reason the other backend writes its
        // `.ll`: a code generator whose output cannot be looked at is one nobody can argue with.
        std::fs::write(&clif, &module.clif)
            .map_err(|e| format!("writing {}: {e}", clif.display()))?;
        linker.link(&module.object, &obj, &exe)?;
        let worker = Worker::start_with(&exe, limit)?;
        Ok(Artifact {
            module,
            linker,
            worker,
            dir,
            clif,
            obj,
            exe,
            codegen,
        })
    }

    pub fn module(&self) -> &Module {
        &self.module
    }

    pub fn linker(&self) -> &Linker {
        &self.linker
    }

    /// How long Cranelift took to turn the whole program into an object.
    ///
    /// Exported because §7.3's reason for having two code generators is a *compile time*, and a
    /// claim about one that the compiler cannot report is a claim nobody can check.
    pub fn codegen_time(&self) -> std::time::Duration {
        self.codegen
    }

    pub fn directory(&self) -> &Path {
        &self.dir.path
    }

    /// Where the textual IR was written.
    pub fn ir_path(&self) -> &Path {
        &self.clif
    }

    /// Where the object was written.
    pub fn object_path(&self) -> &Path {
        &self.obj
    }

    pub fn executable(&self) -> &Path {
        &self.exe
    }

    pub fn report(&self) -> Report<'_> {
        Report {
            module: &self.module,
            linker: &self.linker,
            codegen: self.codegen,
        }
    }

    /// Call a compiled definition by name.
    ///
    /// Errors rather than falls back, for [`beck_llvm::Artifact::call`]'s reason: a harness that
    /// means to measure or compare *this* backend and silently ran the evaluator instead would be
    /// comparing a backend with itself.
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
    linker: &'a Linker,
    codegen: std::time::Duration,
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
        writeln!(
            f,
            "cranelift {} — codegen {:.1} ms, linked by {}",
            env!("CARGO_PKG_VERSION"),
            self.codegen.as_secs_f64() * 1000.0,
            self.linker.version
        )?;
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

/// The Cranelift backend, with somewhere for everything it cannot compile to go.
///
/// The same pair [`beck_llvm::Native`] is, for the same reason: a [`Backend`] answers for the
/// *whole* program, and this one compiles a subset of it. [`Dev::compiled`] is how a harness asks
/// whether a particular definition went native rather than assuming it did.
pub struct Dev {
    artifact: Arc<Artifact>,
    fallback: Arc<dyn Backend>,
    /// The span of each compiled definition's body, against the index that runs it.
    by_span: Vec<(Span, usize, u32)>,
}

impl Dev {
    /// A backend over `program`, or `None` when this machine has no linker.
    pub fn build(
        program: &Arc<Program>,
        fallback: Arc<dyn Backend>,
        limit: Option<std::time::Duration>,
    ) -> Result<Option<Dev>, String> {
        let artifact = match limit {
            Some(limit) => Artifact::build_within(program, limit)?,
            None => Artifact::build(program)?,
        };
        let Some(artifact) = artifact else {
            return Ok(None);
        };
        Ok(Some(Dev::over(Arc::new(artifact), program, fallback)))
    }

    pub fn over(artifact: Arc<Artifact>, program: &Program, fallback: Arc<dyn Backend>) -> Dev {
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
        Dev {
            artifact,
            fallback,
            by_span,
        }
    }

    pub fn artifact(&self) -> &Arc<Artifact> {
        &self.artifact
    }

    /// Whether this expression is one the compiled half will answer.
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

impl Backend for Dev {
    fn name(&self) -> &'static str {
        "cranelift"
    }

    /// The fallback's, because the fallback is what runs everything this cannot compile.
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

    /// Interception is a property of *how* a backend executes a call, and compiled code consults
    /// nothing per call. Rather than pretend, this hands back the fallback's intercepting backend.
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

struct Workspace {
    path: PathBuf,
    owned: bool,
}

impl Workspace {
    fn borrowed(path: PathBuf) -> Workspace {
        Workspace { path, owned: false }
    }

    fn temporary() -> Result<Workspace, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "beck-clif-{}-{}",
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

/// A module name as a file name: everything that is not a letter or a digit becomes a dash.
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
