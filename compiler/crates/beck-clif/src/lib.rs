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
//! It compiles the same subset [`beck_llvm`] compiles — scalars, and the records and unions
//! [`beck_llvm::heap`] lays out — to the same semantics, and it is held to that by a differential
//! that runs *both* against the evaluator and against each other. What it does not share is the
//! emitter: [`emit`] is a second implementation, and a second implementation that agrees is the
//! only kind of evidence a backend seam can offer. What it *does* share is the layout, because a
//! layout is a contract with the host as well
//! ([`adr/0026`](../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md)).
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
use beck_llvm::service::{self, Asking};
use beck_llvm::{Refusal, Signature, Trap, Worker};

pub use emit::{module, Module};
pub use toolchain::Linker;

/// A compiled program: the module, the executable, and the process running it.
pub struct Artifact {
    module: Module,
    linker: Linker,
    worker: Worker,
    /// Who answers the four questions compiled code cannot ([`beck_llvm::Upcall`]).
    ///
    /// [`beck_core::host::ProcessAtoms`] unless a caller said otherwise, and the *same* trait the
    /// other backend and the tree-walker ask — which is what keeps a differential over `now()` a
    /// comparison of the program rather than of three clocks.
    atoms: std::sync::Arc<dyn beck_core::host::Atoms>,
    asking: Asking,
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
            atoms: std::sync::Arc::new(beck_core::host::ProcessAtoms),
            asking: Asking::new(),
            dir,
            clif,
            obj,
            exe,
            codegen,
        })
    }

    /// Answer this artefact's host effects with something other than the process.
    pub fn answering(mut self, atoms: std::sync::Arc<dyn beck_core::host::Atoms>) -> Artifact {
        self.atoms = atoms;
        self
    }

    pub fn module(&self) -> &Module {
        &self.module
    }
    /// How many questions the last call asked the host, and how many bytes of arena went with them.
    ///
    /// The number a **shape** gate reads, with no clock in it: a question whose arguments cannot
    /// point into the heap sends none of it however much the program has allocated, and one whose
    /// arguments can sends all of it. Both halves are a decision rather than an accident, and this
    /// is how a test says so (`AGENTS.md`, and `docs/64` §64.1's pattern).
    pub fn questions(&self) -> (u64, u64) {
        self.asking.traffic()
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

    /// The same, and how many bytes of arena the call left behind.
    ///
    /// The second number is what a **shape** gate reads: a program that allocates `n` objects of a
    /// known size has to use a known number of bytes, at every `n`, and a cost per object that
    /// grew with the number of objects would show up here with no clock in the measurement
    /// (`AGENTS.md`, and `docs/64` §64.1's pattern).
    pub fn call_sized(&self, name: &str, args: &[Value]) -> Result<(Value, usize), ExecError> {
        let sig = self.module.signature(name).ok_or_else(|| {
            ExecError::new(format!("`{name}` did not compile natively"), Span::NONE)
        })?;
        self.exchange(sig, args)
    }

    fn invoke(&self, sig: &Signature, args: &[Value]) -> Result<Value, ExecError> {
        self.exchange(sig, args).map(|(v, _)| v)
    }

    fn exchange(&self, sig: &Signature, args: &[Value]) -> Result<(Value, usize), ExecError> {
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
        // The arguments become eight bytes each, plus — when any of them is an object — the flat
        // byte string of the graph they point into. `beck_llvm::heap` is the one description of
        // that shape, shared with the other backend and with the host.
        let (cells, blob) = self
            .module
            .heap
            .encode_args(args, &sig.params)
            .map_err(|why| ExecError::new(format!("`{}` was given {why}", sig.name), Span::NONE))?;

        self.asking.clear();
        let reply = self
            .worker
            .call(sig.index, &cells, &blob, &|q| {
                service::answer(&self.module.heap, &*self.atoms, &self.asking, q)
            })
            .map_err(|e| ExecError::new(e, Span::NONE))?;
        if reply.code != 0 {
            let span = self
                .module
                .spans
                .get(reply.span as usize)
                .copied()
                .unwrap_or(Span::NONE);
            // The one failure that carries a value; see `beck_llvm::Artifact`'s own arm, which this
            // is the second half of one protocol rather than a second opinion about it.
            if Trap::from_code(reply.code) == Some(Trap::Raised) {
                let message = self
                    .module
                    .heap
                    .raised(reply.payload as u64, &reply.heap)
                    .map_or_else(|why| why, |v| format!("raised `{}`", v.display()));
                return Err(ExecError::new(message, span));
            }
            // The sentence a `HostFailed` could not carry, if there is one.
            let message = match (Trap::from_code(reply.code), self.asking.take()) {
                (Some(Trap::HostFailed), Some(why)) => why,
                (Some(trap), _) => trap.message(reply.payload),
                (None, _) => format!("the compiled program reported trap {}", reply.code),
            };
            return Err(ExecError::new(message, span));
        }
        let value = self
            .module
            .heap
            .decode(reply.value, sig.ret, &reply.heap)
            .map_err(|why| ExecError::new(why, Span::NONE))?;
        Ok((value, reply.heap.len()))
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
            let params: Vec<String> = sig
                .params
                .iter()
                .map(|p| self.module.heap.show(*p))
                .collect();
            writeln!(
                f,
                "  {:<28} ({}) -> {}",
                sig.name,
                params.join(", "),
                self.module.heap.show(sig.ret)
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
