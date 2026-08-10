//! `Core` → LLVM IR, as text.
//!
//! # Why text, and not `inkwell`
//!
//! LLVM's Rust bindings are `unsafe` from the first call, and `docs/43-threat-model.md` claims
//! "no memory-unsafety in first-party code" **structurally** — `unsafe_code = "forbid"`, inherited
//! by every crate in the workspace. A backend that took that claim away would be trading a
//! property the project tests for one it only asserts. LLVM's textual IR is a stable, documented
//! interface to exactly the same compiler, so this crate writes `.ll` and hands it to `clang`.
//! [`docs/adr/0021`](../../../../../docs/adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)
//! is where that decision is recorded, with what it costs.
//!
//! # What is compiled, and what is refused
//!
//! The **scalar subset**: a definition whose parameters and result are all `Int`, `Float` or
//! `Bool`, and whose body is built from constants, variables, `let`, `if`, `match` on scalar
//! constants, direct calls to other compiled definitions, and the arithmetic, comparison and
//! logical primitives. There is no heap here, so a list, a string, a map, a record, a closure or
//! an effect is refused — by name, with the reason, in [`crate::Report`]. Nothing is silently
//! approximated: a definition either compiles to machine code that agrees with the evaluator on
//! every input, or it does not compile.
//!
//! # Agreeing with the evaluator exactly
//!
//! The differential in `beck-cli/tests/native.rs` is the point of this crate, so the semantics are
//! matched rather than approximated:
//!
//! * **Integer arithmetic is checked.** `beck-eval` uses `i64::checked_*`, so `+`, `-`, `*` go
//!   through `llvm.s*.with.overflow.i64` and `/`, `%` carry an explicit zero-and-`INT_MIN/-1`
//!   guard. An overflow is a *value* — a trap code and a span the host turns back into the
//!   evaluator's own message — not a wrapped result and not a `SIGFPE`.
//! * **Reals are compared by `beck_core`'s order key, not by `fcmp`.** `Value::Float` stores a
//!   monotone transform of the bits so the derived `Ord` is the numeric one (`docs/32` §32.2), and
//!   under it `-0.0 < 0.0` and NaN is the maximum. `fcmp` says something else for both, so a
//!   comparison here bitcasts and compares the keys — four integer instructions, and the same
//!   answer as the tree-walker.
//! * **A real is normalised where a signed zero is *observable*.** `Value::float` maps `-0.0` to
//!   `0.0` on every real it makes; doing the same after every operation here costs 3× on
//!   float-heavy code and is not needed, because every float operation maps zeros to zeros. The
//!   three places it is needed — a comparison, a division's divisor, and a trap's payload — and
//!   the invariant that says the rest is safe are documented on `Function::normalise` in this
//!   module's source. Without the divisor
//!   case, `1.0 / (0.0 * -1.0)` is `-inf` here and `+inf` there.
//! * **`trunc` saturates.** The evaluator's `f as i64` is Rust's saturating cast, which is
//!   `llvm.fptosi.sat.i64.f64` and not `fptosi` — plain `fptosi` is poison out of range.
//!
//! **A NaN is canonicalised at those same three places**, and for the same reason: `Value::float`
//! maps every NaN to `f64::NAN`. The tempting argument that the platform's default NaN already *is*
//! that one is false on x86-64, where `0.0 * inf` yields the indefinite QNaN with its sign bit set
//! — which sorts below every number under the order key where `f64::NAN` sorts above every one.
//! `Function::normalise` has the whole story, and `docs/93` §93.2 is where it is written down.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use beck_core::check::{Def, Program};
use beck_core::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use beck_core::ty::Ty;
use beck_diag::Span;

/// The most parameters a compiled function may have.
///
/// The worker's argument buffer is a fixed-size `alloca`, so this is the buffer rather than a
/// judgement about signatures. Nothing in the corpus comes close.
pub const MAX_PARAMS: usize = 16;

/// One of the three types this backend has a machine representation for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Int,
    Float,
    Bool,
}

impl Scalar {
    /// The LLVM type.
    pub fn llvm(self) -> &'static str {
        match self {
            Scalar::Int => "i64",
            Scalar::Float => "double",
            Scalar::Bool => "i1",
        }
    }

    /// The zero of that type, written as LLVM expects it.
    ///
    /// Returned from a function whose computation trapped. The value is never read — the host
    /// looks at the trap code first — but a `ret` needs an operand.
    fn zero(self) -> &'static str {
        match self {
            Scalar::Int => "0",
            Scalar::Float => "0x0000000000000000",
            Scalar::Bool => "false",
        }
    }

    /// The scalar a Beck type denotes, if it denotes one.
    ///
    /// By name and exactly: a `newtype` over `Int` is a `Value::Data` at run time, so it is not an
    /// `i64` here however zero-cost it is in the type system.
    pub fn of(ty: &Ty) -> Option<Scalar> {
        match ty {
            Ty::Con(name, args) if args.is_empty() => match &**name {
                Ty::INT => Some(Scalar::Int),
                Ty::FLOAT => Some(Scalar::Float),
                Ty::BOOL => Some(Scalar::Bool),
                _ => None,
            },
            _ => None,
        }
    }
}

/// What a compiled function looks like from the outside.
#[derive(Clone, Debug)]
pub struct Signature {
    pub name: Arc<str>,
    pub params: Vec<Scalar>,
    pub ret: Scalar,
    /// Which entry of the worker's dispatch table calls it.
    pub index: u32,
}

/// A definition that did not compile, and why.
#[derive(Clone, Debug)]
pub struct Refusal {
    pub name: Arc<str>,
    pub reason: String,
}

/// A run of a computation that could not produce a value.
///
/// The compiled code stores one of these and returns; the host turns it back into the message the
/// evaluator would have produced, so a differential can compare failures and not only successes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    AddOverflow,
    SubOverflow,
    MulOverflow,
    DivOverflow,
    RemOverflow,
    AbsOverflow,
    NegOverflow,
    NoMatchInt,
    NoMatchFloat,
    NoMatchBool,
}

impl Trap {
    /// The number the compiled program stores in the error cell.
    ///
    /// Public because it is a **protocol** rather than an implementation detail: a second emitter
    /// stores these (`beck-clif`), the host decodes them through [`Trap::from_code`], and two
    /// spellings of one wire would be the drift this workspace spends its gates on.
    pub fn code(self) -> u32 {
        match self {
            Trap::AddOverflow => 1,
            Trap::SubOverflow => 2,
            Trap::MulOverflow => 3,
            Trap::DivOverflow => 4,
            Trap::RemOverflow => 5,
            Trap::AbsOverflow => 6,
            Trap::NegOverflow => 7,
            Trap::NoMatchInt => 8,
            Trap::NoMatchFloat => 9,
            Trap::NoMatchBool => 10,
        }
    }

    pub fn from_code(code: u32) -> Option<Trap> {
        const ALL: [Trap; 10] = [
            Trap::AddOverflow,
            Trap::SubOverflow,
            Trap::MulOverflow,
            Trap::DivOverflow,
            Trap::RemOverflow,
            Trap::AbsOverflow,
            Trap::NegOverflow,
            Trap::NoMatchInt,
            Trap::NoMatchFloat,
            Trap::NoMatchBool,
        ];
        ALL.into_iter().find(|t| t.code() == code)
    }

    /// The message `beck-eval` produces for the same failure, word for word.
    ///
    /// Word for word on purpose: the differential compares the error and not merely the fact of
    /// one, so a backend that failed for a *different* reason than the evaluator is a divergence
    /// rather than an agreement.
    pub fn message(self, payload: i64) -> String {
        match self {
            Trap::AddOverflow => "`+` overflowed".into(),
            Trap::SubOverflow => "`-` overflowed or divided by zero".into(),
            Trap::MulOverflow => "`*` overflowed or divided by zero".into(),
            Trap::DivOverflow => "`/` overflowed or divided by zero".into(),
            Trap::RemOverflow => "`%` overflowed or divided by zero".into(),
            Trap::AbsOverflow => "`abs` overflowed".into(),
            Trap::NegOverflow => "`negate` overflowed".into(),
            Trap::NoMatchInt => format!("no match arm applies to {payload}"),
            Trap::NoMatchFloat => format!(
                "no match arm applies to {}",
                beck_core::Value::Float(payload as u64)
                    .as_f64()
                    .unwrap_or(0.0)
            ),
            Trap::NoMatchBool => {
                format!("no match arm applies to {}", payload != 0)
            }
        }
    }
}

/// A whole module of compiled definitions, as LLVM IR text plus the tables the host reads.
pub struct Module {
    /// The `.ll` source.
    pub ir: String,
    /// One per compiled definition, in dispatch-index order.
    pub functions: Vec<Signature>,
    /// The spans a trap can name, indexed by what the compiled code stores.
    pub spans: Vec<Span>,
    /// Definitions this backend declined, and why.
    pub refusals: Vec<Refusal>,
}

impl Module {
    pub fn signature(&self, name: &str) -> Option<&Signature> {
        self.functions.iter().find(|f| &*f.name == name)
    }
}

/// Compile every definition of `program` that this backend can compile.
///
/// Never fails: a program with nothing scalar in it yields a module with no functions and a
/// refusal per definition. Whether that is worth running is the caller's decision, and
/// [`Module::functions`] is how it takes it.
pub fn module(program: &Program) -> Module {
    let mut refusals: Vec<Refusal> = Vec::new();
    let mut sigs: BTreeMap<Arc<str>, Signature> = BTreeMap::new();

    // Round one: the signature alone. A definition whose parameters or result are not scalars
    // cannot be called through the worker's protocol however simple its body is.
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        match signature_of(def) {
            Ok(sig) => {
                sigs.insert(name.clone(), sig);
            }
            Err(reason) => refusals.push(Refusal {
                name: name.clone(),
                reason,
            }),
        }
    }

    // Round two, to a fixed point: emit each body, and drop whichever ones will not emit. A body
    // that calls a definition dropped in an earlier round fails in a later one, which is what
    // makes mutual recursion work — the pair survives together or is refused together.
    let mut eligible: BTreeSet<Arc<str>> = sigs.keys().cloned().collect();
    loop {
        let mut removed = false;
        for name in eligible.clone() {
            let def = &program.defs[&name];
            let mut fun = Function::new(&sigs, &eligible);
            if let Err(reason) = fun.emit(def) {
                eligible.remove(&name);
                refusals.push(Refusal { name, reason });
                removed = true;
            }
        }
        if !removed {
            break;
        }
    }

    // The order the survivors are emitted in is the order they were declared in, so a module is a
    // function of the program and not of a hash seed: `beck native --emit-ir` twice gives the same
    // bytes, which is the property a build wants and a diff needs.
    let order: Vec<Arc<str>> = program
        .def_order
        .iter()
        .filter(|n| eligible.contains(*n))
        .cloned()
        .collect();

    let mut indexed: BTreeMap<Arc<str>, Signature> = BTreeMap::new();
    for (i, name) in order.iter().enumerate() {
        let mut sig = sigs[name].clone();
        sig.index = i as u32;
        indexed.insert(name.clone(), sig);
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut bodies = String::new();
    for name in &order {
        let def = &program.defs[name];
        let mut fun = Function::new(&indexed, &eligible);
        fun.spans = std::mem::take(&mut spans);
        let text = fun
            .emit(def)
            .expect("the fixed point already proved this emits");
        spans = std::mem::take(&mut fun.spans);
        bodies.push_str(&text);
        bodies.push('\n');
    }

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    let ir = assemble(&bodies, &functions);
    refusals.sort_by(|a, b| a.name.cmp(&b.name));
    Module {
        ir,
        functions,
        spans,
        refusals,
    }
}

/// The signature, or the reason there is not one.
fn signature_of(def: &Def) -> Result<Signature, String> {
    if !def.typarams.is_empty() {
        return Err(format!(
            "generic over {} — a type parameter has no machine representation here",
            def.typarams.join(", ")
        ));
    }
    if !def.bounds.is_empty() {
        return Err("bounded: a dictionary parameter is a function value".into());
    }
    if def.params.len() > MAX_PARAMS {
        return Err(format!(
            "{} parameters, and the worker's argument buffer holds {MAX_PARAMS}",
            def.params.len()
        ));
    }
    let mut params = Vec::with_capacity(def.params.len());
    for (_, name, ty) in &def.params {
        match Scalar::of(ty) {
            Some(s) => params.push(s),
            None => {
                return Err(format!(
                    "parameter `{name}` is `{}`, and only Int, Float and Bool have a machine \
                     representation here",
                    ty
                ))
            }
        }
    }
    let ret = Scalar::of(&def.ret).ok_or_else(|| {
        format!(
            "returns `{}`, and only Int, Float and Bool have a machine representation here",
            def.ret
        )
    })?;
    Ok(Signature {
        name: def.name.clone(),
        params,
        ret,
        // Assigned once the survivors are known — an index into a table that does not exist yet
        // would be a number that means nothing.
        index: u32::MAX,
    })
}

/// An SSA value: the LLVM operand, and what it is.
#[derive(Clone, Debug)]
struct Val {
    text: String,
    ty: Scalar,
}

/// Where the value an expression produces has to go.
///
/// The whole reason this exists is [`Dest::Return`]: an expression in tail position is compiled to
/// a jump rather than a call, and "in tail position" is a fact about the *context* an expression
/// sits in rather than about the expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dest {
    /// Into a register, for whatever is around it to use.
    Value,
    /// Out of the function.
    Return,
}

/// One function under construction.
struct Function<'a> {
    sigs: &'a BTreeMap<Arc<str>, Signature>,
    eligible: &'a BTreeSet<Arc<str>>,
    /// Emitted blocks, complete with their terminators.
    out: String,
    /// The block being written.
    body: String,
    label: String,
    next: u32,
    env: BTreeMap<VarId, Val>,
    spans: Vec<Span>,
    /// What this function returns, which is what its trap exit has to return too.
    ret: Scalar,
    /// Whether anything branched to the trap exit, so an unused block is not emitted.
    trapped: bool,
}

impl<'a> Function<'a> {
    fn new(
        sigs: &'a BTreeMap<Arc<str>, Signature>,
        eligible: &'a BTreeSet<Arc<str>>,
    ) -> Function<'a> {
        Function {
            sigs,
            eligible,
            out: String::new(),
            body: String::new(),
            label: String::new(),
            next: 0,
            env: BTreeMap::new(),
            spans: Vec::new(),
            ret: Scalar::Int,
            trapped: false,
        }
    }

    fn emit(&mut self, def: &Def) -> Result<String, String> {
        let sig = self
            .sigs
            .get(&def.name)
            .ok_or_else(|| "no signature".to_string())?
            .clone();
        self.ret = sig.ret;

        // A definition is stored as the lambda that is its whole body (`Def::body`), so the
        // parameters are the lambda's and the types are the signature's.
        let CoreKind::Lam { params, body } = &def.body.kind else {
            return Err("the body is not a lambda".into());
        };
        if params.len() != sig.params.len() {
            return Err("the lambda's parameters do not match the signature".into());
        }

        let mut head = format!(
            "define internal tailcc {} @{}(ptr noalias %err",
            sig.ret.llvm(),
            mangle(&def.name)
        );
        for (i, (var, ty)) in params.iter().zip(&sig.params).enumerate() {
            let _ = write!(head, ", {} %a{i}", ty.llvm());
            self.env.insert(
                *var,
                Val {
                    text: format!("%a{i}"),
                    ty: *ty,
                },
            );
        }
        head.push_str(") {\n");

        self.label = "entry".into();
        self.expr(body, Dest::Return)?;

        let mut text = head;
        text.push_str(&self.out);
        if self.trapped {
            let _ = write!(text, "trap:\n  ret {} {}\n", sig.ret.llvm(), sig.ret.zero());
        }
        text.push_str("}\n");
        Ok(text)
    }

    // -- block plumbing ---------------------------------------------------------------------

    fn fresh(&mut self) -> String {
        self.next += 1;
        format!("%v{}", self.next)
    }

    fn label(&mut self, hint: &str) -> String {
        self.next += 1;
        format!("{hint}{}", self.next)
    }

    fn line(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.body, "  {}", text.as_ref());
    }

    /// Close the current block with `term`, and stop having one.
    fn terminate(&mut self, term: impl AsRef<str>) {
        let _ = writeln!(self.out, "{}:", self.label);
        self.out.push_str(&self.body);
        let _ = writeln!(self.out, "  {}", term.as_ref());
        self.body.clear();
        self.label.clear();
    }

    fn start(&mut self, label: String) {
        debug_assert!(self.label.is_empty(), "a block was left open");
        self.label = label;
    }

    /// Record a span and answer the index the compiled code should store for it.
    fn span(&mut self, span: Span) -> usize {
        self.spans.push(span);
        self.spans.len() - 1
    }

    /// Store the trap and leave for the exit block. The caller carries on in `cont`.
    fn trap(&mut self, trap: Trap, span: Span, payload: &str, cond: &str) {
        let idx = self.span(span);
        let set = self.label("trap.set");
        let cont = self.label("trap.no");
        self.terminate(format!("br i1 {cond}, label %{set}, label %{cont}"));

        self.start(set);
        self.line(format!("store i32 {}, ptr %err", trap.code()));
        let sp = self.fresh();
        self.line(format!("{sp} = getelementptr inbounds i8, ptr %err, i64 4"));
        self.line(format!("store i32 {idx}, ptr {sp}"));
        let pl = self.fresh();
        self.line(format!("{pl} = getelementptr inbounds i8, ptr %err, i64 8"));
        self.line(format!("store i64 {payload}, ptr {pl}"));
        self.terminate("br label %trap");
        self.trapped = true;

        self.start(cont);
    }

    /// Leave for the exit block if a callee trapped.
    fn check_call(&mut self) {
        let code = self.fresh();
        self.line(format!("{code} = load i32, ptr %err"));
        let ok = self.fresh();
        self.line(format!("{ok} = icmp eq i32 {code}, 0"));
        let cont = self.label("call.ok");
        self.terminate(format!("br i1 {ok}, label %{cont}, label %trap"));
        self.trapped = true;
        self.start(cont);
    }

    // -- expressions ------------------------------------------------------------------------

    /// Emit `c`, and hand back a value — the mode everything that is not in tail position uses.
    fn value(&mut self, c: &Core) -> Result<Val, String> {
        Ok(self
            .expr(c, Dest::Value)?
            .expect("value mode always produces a value"))
    }

    /// Emit `c` for `dest`.
    ///
    /// `Dest::Return` is not an optimisation: `docs/31-tail-calls-report.md` makes "a call in tail
    /// position is free" a property of the *language*, so a backend that spent a frame on one
    /// would be a backend on which a Beck loop overflows the stack. It is threaded through `if`,
    /// `let` and `match` rather than pattern-matched at the top of a body because tail position
    /// travels through all three: the interesting call is almost never the outermost node.
    fn expr(&mut self, c: &Core, dest: Dest) -> Result<Option<Val>, String> {
        let value = match &c.kind {
            CoreKind::Const(k) => constant(k)?,
            CoreKind::Var(v) => self
                .env
                .get(v)
                .cloned()
                .ok_or_else(|| format!("variable {v} is not in scope here"))?,
            CoreKind::Let { var, value, body } => {
                let v = self.value(value)?;
                let shadowed = self.env.insert(*var, v);
                let r = self.expr(body, dest);
                match shadowed {
                    Some(old) => {
                        self.env.insert(*var, old);
                    }
                    None => {
                        self.env.remove(var);
                    }
                }
                return r;
            }
            CoreKind::If { cond, then, alt } => return self.if_(cond, then, alt, dest),
            CoreKind::Match { scrutinee, arms } => {
                return self.match_(scrutinee, arms, c.span, dest)
            }
            CoreKind::App { func, args } => return self.call(func, args, dest),
            CoreKind::Prim { op, args } => self.prim(*op, args, c.span)?,
            CoreKind::Lam { .. } => {
                return Err("a nested function is a closure, and there is no heap here".into())
            }
            CoreKind::Global(name) => {
                return Err(format!(
                    "`{name}` is used as a value rather than called, and a function value is a \
                     closure"
                ))
            }
            CoreKind::Make { ty, .. } => {
                return Err(format!("builds a `{ty}`, and there is no heap here"))
            }
            CoreKind::Field { name, .. } => {
                return Err(format!("reads the field `{name}` of a record"))
            }
            CoreKind::With { .. } => return Err("updates a record".into()),
            CoreKind::ListLit(_) => return Err("builds a list, and there is no heap here".into()),
            CoreKind::MapLit(_) => return Err("builds a map, and there is no heap here".into()),
        };
        self.finish(value, dest)
    }

    /// Hand a computed value to its destination: keep it, or return it.
    fn finish(&mut self, v: Val, dest: Dest) -> Result<Option<Val>, String> {
        match dest {
            Dest::Value => Ok(Some(v)),
            Dest::Return => {
                if v.ty != self.ret {
                    return Err(format!(
                        "returns {:?} where the signature says {:?}",
                        v.ty, self.ret
                    ));
                }
                self.terminate(format!("ret {} {}", v.ty.llvm(), v.text));
                Ok(None)
            }
        }
    }

    fn if_(
        &mut self,
        cond: &Core,
        then: &Core,
        alt: &Core,
        dest: Dest,
    ) -> Result<Option<Val>, String> {
        let c = self.value(cond)?;
        if c.ty != Scalar::Bool {
            return Err("the condition of an `if` is not a Bool".into());
        }
        let lt = self.label("if.then");
        let lf = self.label("if.else");
        let lm = self.label("if.join");
        self.terminate(format!("br i1 {}, label %{lt}, label %{lf}", c.text));

        self.start(lt);
        let tv = self.expr(then, dest)?;
        let from_t = self.label.clone();
        if dest == Dest::Value {
            self.terminate(format!("br label %{lm}"));
        }

        self.start(lf);
        let fv = self.expr(alt, dest)?;
        let from_f = self.label.clone();
        if dest == Dest::Value {
            self.terminate(format!("br label %{lm}"));
        }

        let (Some(tv), Some(fv)) = (tv, fv) else {
            // Both branches returned. There is nothing to join and nothing to name.
            return Ok(None);
        };
        if tv.ty != fv.ty {
            return Err("the two branches of an `if` have different types".into());
        }
        self.start(lm);
        let r = self.fresh();
        self.line(format!(
            "{r} = phi {} [ {}, %{from_t} ], [ {}, %{from_f} ]",
            tv.ty.llvm(),
            tv.text,
            fv.text
        ));
        Ok(Some(Val { text: r, ty: tv.ty }))
    }

    /// A `match` over a scalar: a chain of tests, each falling through to the next.
    ///
    /// Falling through is what makes a guard a guard — an arm whose pattern matched but whose
    /// guard was false has to reach the arm after it, which is the evaluator's `continue`.
    fn match_(
        &mut self,
        scrutinee: &Core,
        arms: &[Arm],
        span: Span,
        dest: Dest,
    ) -> Result<Option<Val>, String> {
        let v = self.value(scrutinee)?;
        let join = self.label("match.join");
        let mut incoming: Vec<(String, String)> = Vec::new();
        let mut ty: Option<Scalar> = None;

        for arm in arms {
            let next = self.label("match.next");
            let taken = self.label("match.arm");
            let cond = self.test(&arm.pattern, &v)?;
            self.terminate(format!("br i1 {cond}, label %{taken}, label %{next}"));

            self.start(taken);
            let bound = self.bind(&arm.pattern, &v);
            if let Some(guard) = &arm.guard {
                let g = self.value(guard)?;
                if g.ty != Scalar::Bool {
                    return Err("a match guard is not a Bool".into());
                }
                let run = self.label("match.guarded");
                self.terminate(format!("br i1 {}, label %{run}, label %{next}", g.text));
                self.start(run);
            }
            if let Some(av) = self.expr(&arm.body, dest)? {
                match ty {
                    Some(t) if t != av.ty => return Err("match arms have different types".into()),
                    _ => ty = Some(av.ty),
                }
                incoming.push((av.text.clone(), self.label.clone()));
                self.terminate(format!("br label %{join}"));
            }
            self.unbind(bound);

            self.start(next);
        }

        // Nothing matched. The checker proves a `match` exhaustive, so this is unreachable for a
        // program that compiled — but "unreachable" in LLVM means the optimiser may do anything at
        // all with the path that reaches it, and a wrong exhaustiveness check would then be
        // undefined behaviour rather than a message. It traps instead.
        let trap = match v.ty {
            Scalar::Int => Trap::NoMatchInt,
            Scalar::Float => Trap::NoMatchFloat,
            Scalar::Bool => Trap::NoMatchBool,
        };
        let payload = self.widen(&v);
        self.trap(trap, span, &payload, "true");
        self.terminate("br label %trap");
        self.trapped = true;

        if dest == Dest::Return {
            return Ok(None);
        }
        let Some(ty) = ty else {
            return Err("a `match` with no arms".into());
        };
        self.start(join);
        let r = self.fresh();
        let arms: Vec<String> = incoming
            .iter()
            .map(|(v, b)| format!("[ {v}, %{b} ]"))
            .collect();
        self.line(format!("{r} = phi {} {}", ty.llvm(), arms.join(", ")));
        Ok(Some(Val { text: r, ty }))
    }

    /// Whether `pat` matches `v`, as an `i1`.
    fn test(&mut self, pat: &Pattern, v: &Val) -> Result<String, String> {
        match pat {
            Pattern::Wildcard | Pattern::Bind(_) => Ok("true".into()),
            Pattern::At { inner, .. } => self.test(inner, v),
            Pattern::Const(k) => {
                let want = constant(k)?;
                if want.ty != v.ty {
                    return Err("a match arm compares against a constant of another type".into());
                }
                Ok(self.equals(v, &want))
            }
            Pattern::Or(alts) => {
                let mut acc: Option<String> = None;
                for alt in alts {
                    let t = self.test(alt, v)?;
                    acc = Some(match acc {
                        None => t,
                        Some(prev) => {
                            let r = self.fresh();
                            self.line(format!("{r} = or i1 {prev}, {t}"));
                            r
                        }
                    });
                }
                acc.ok_or_else(|| "an or-pattern with no alternatives".into())
            }
            Pattern::Ctor { variant, .. } => Err(format!(
                "matches the constructor `{variant}`, and a union value lives on the heap"
            )),
            Pattern::List { .. } => Err("matches a list pattern".into()),
        }
    }

    /// Bind what an irrefutable part of `pat` names, and answer what to undo afterwards.
    fn bind(&mut self, pat: &Pattern, v: &Val) -> Vec<(VarId, Option<Val>)> {
        let mut undo = Vec::new();
        self.bind_into(pat, v, &mut undo);
        undo
    }

    fn bind_into(&mut self, pat: &Pattern, v: &Val, undo: &mut Vec<(VarId, Option<Val>)>) {
        match pat {
            Pattern::Bind(var) => undo.push((*var, self.env.insert(*var, v.clone()))),
            Pattern::At { var, inner } => {
                undo.push((*var, self.env.insert(*var, v.clone())));
                self.bind_into(inner, v, undo);
            }
            // Every alternative of an or-pattern binds the same names to the same scrutinee here,
            // because a scalar pattern takes nothing apart: binding through the first is binding
            // through all of them.
            Pattern::Or(alts) => {
                if let Some(first) = alts.first() {
                    self.bind_into(first, v, undo);
                }
            }
            Pattern::Wildcard | Pattern::Const(_) | Pattern::Ctor { .. } | Pattern::List { .. } => {
            }
        }
    }

    fn unbind(&mut self, undo: Vec<(VarId, Option<Val>)>) {
        for (var, old) in undo.into_iter().rev() {
            match old {
                Some(v) => self.env.insert(var, v),
                None => self.env.remove(&var),
            };
        }
    }

    /// A direct call of a named definition — and in tail position, a jump.
    ///
    /// The tail case is `musttail`, which LLVM *guarantees* rather than attempts: if it could not
    /// discard the frame it refuses the module, so a build that succeeds is a build in which every
    /// tail call is a jump. That is stronger than the usual `-O2` sibling-call heuristic and it is
    /// the point — `docs/31` §31.2 says 1,500 and 60,000 tail calls spend the same host stack, and
    /// an optimisation that "usually" fires cannot be what a language guarantee rests on.
    ///
    /// It is `tailcc` rather than the C convention because the C convention's `musttail` demands
    /// that the caller and callee prototypes match, which is a rule about arity and not about
    /// tails: `def loop(n, acc)` calling `def done(acc)` is an ordinary tail call in the language
    /// and would have been a stack frame here.
    fn call(&mut self, func: &Core, args: &[Core], dest: Dest) -> Result<Option<Val>, String> {
        let CoreKind::Global(name) = &func.kind else {
            return Err("calls something other than a named definition".into());
        };
        if !self.eligible.contains(&**name) {
            return Err(format!("calls `{name}`, which does not compile"));
        }
        let sig = self
            .sigs
            .get(&**name)
            .ok_or_else(|| format!("calls `{name}`, which has no signature"))?
            .clone();
        if args.len() != sig.params.len() {
            return Err(format!(
                "calls `{name}` with {} arguments where it takes {}",
                args.len(),
                sig.params.len()
            ));
        }
        let mut vals = Vec::with_capacity(args.len());
        for (a, want) in args.iter().zip(&sig.params) {
            let v = self.value(a)?;
            if v.ty != *want {
                return Err(format!("an argument to `{name}` is the wrong type"));
            }
            vals.push(v);
        }
        let mut operands = String::from("ptr %err");
        for v in &vals {
            let _ = write!(operands, ", {} {}", v.ty.llvm(), v.text);
        }
        let r = self.fresh();

        if dest == Dest::Return {
            if sig.ret != self.ret {
                return Err(format!(
                    "`{name}` returns {:?} in tail position of a {:?}",
                    sig.ret, self.ret
                ));
            }
            self.line(format!(
                "{r} = musttail call tailcc {} @{}({operands})",
                sig.ret.llvm(),
                mangle(name)
            ));
            // No trap check: there is no frame left to check in. A callee that trapped stored the
            // reason before returning, and whoever eventually reads `%err` — the worker's loop —
            // still sees it, because the cell outlives every frame that shared it.
            self.terminate(format!("ret {} {r}", sig.ret.llvm()));
            return Ok(None);
        }

        self.line(format!(
            "{r} = call tailcc {} @{}({operands})",
            sig.ret.llvm(),
            mangle(name)
        ));
        self.check_call();
        Ok(Some(Val {
            text: r,
            ty: sig.ret,
        }))
    }

    // -- primitives -------------------------------------------------------------------------

    fn prim(&mut self, op: Prim, args: &[Core], span: Span) -> Result<Val, String> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.value(a)?);
        }
        let arity = |n: usize| -> Result<(), String> {
            if vals.len() == n {
                Ok(())
            } else {
                Err(format!(
                    "`{}` is applied to {} arguments here",
                    op.name(),
                    vals.len()
                ))
            }
        };
        let same = |vals: &[Val]| -> Result<Scalar, String> {
            if vals[0].ty == vals[1].ty {
                Ok(vals[0].ty)
            } else {
                Err(format!("`{}` mixes two scalar types", op.name()))
            }
        };

        match op {
            Prim::Add | Prim::Sub | Prim::Mul => {
                arity(2)?;
                let ty = same(&vals)?;
                match ty {
                    Scalar::Int => {
                        let (intrinsic, trap) = match op {
                            Prim::Add => ("sadd", Trap::AddOverflow),
                            Prim::Sub => ("ssub", Trap::SubOverflow),
                            _ => ("smul", Trap::MulOverflow),
                        };
                        Ok(self.checked_int(intrinsic, trap, &vals[0], &vals[1], span))
                    }
                    Scalar::Float => {
                        let opcode = match op {
                            Prim::Add => "fadd",
                            Prim::Sub => "fsub",
                            _ => "fmul",
                        };
                        let r = self.fresh();
                        self.line(format!(
                            "{r} = {opcode} double {}, {}",
                            vals[0].text, vals[1].text
                        ));
                        Ok(Val {
                            text: r,
                            ty: Scalar::Float,
                        })
                    }
                    Scalar::Bool => Err(format!("`{}` on two Bools", op.name())),
                }
            }
            Prim::Div | Prim::Rem => {
                arity(2)?;
                let ty = same(&vals)?;
                match ty {
                    Scalar::Int => {
                        let (opcode, trap) = match op {
                            Prim::Div => ("sdiv", Trap::DivOverflow),
                            _ => ("srem", Trap::RemOverflow),
                        };
                        Ok(self.checked_divide(opcode, trap, &vals[0], &vals[1], span))
                    }
                    // `%` on reals is not in the language: `Prim::Rem`'s evaluator arm answers
                    // only for two Ints, so a Float here would be a type error there.
                    // The one arithmetic operation that turns a signed zero into something a
                    // signed zero is not: `1.0 / -0.0` is `-inf` and `1.0 / 0.0` is `+inf`. So the
                    // *divisor* is normalised — see `Function::normalise` for why nothing else
                    // needs to be.
                    Scalar::Float if op == Prim::Div => {
                        let divisor = self.normalise(&vals[1].text);
                        let r = self.fresh();
                        self.line(format!(
                            "{r} = fdiv double {}, {}",
                            vals[0].text, divisor.text
                        ));
                        Ok(Val {
                            text: r,
                            ty: Scalar::Float,
                        })
                    }
                    _ => Err(format!("`{}` on this type", op.name())),
                }
            }
            Prim::Neg => {
                arity(1)?;
                match vals[0].ty {
                    Scalar::Int => {
                        // `i64::checked_neg`, which is what the evaluator does: the one input
                        // without an answer is `i64::MIN`.
                        let bad = self.fresh();
                        self.line(format!(
                            "{bad} = icmp eq i64 {}, -9223372036854775808",
                            vals[0].text
                        ));
                        let payload = self.widen(&vals[0]);
                        self.trap(Trap::NegOverflow, span, &payload, &bad);
                        let r = self.fresh();
                        self.line(format!("{r} = sub i64 0, {}", vals[0].text));
                        Ok(Val {
                            text: r,
                            ty: Scalar::Int,
                        })
                    }
                    Scalar::Float => {
                        let r = self.fresh();
                        self.line(format!("{r} = fneg double {}", vals[0].text));
                        Ok(Val {
                            text: r,
                            ty: Scalar::Float,
                        })
                    }
                    Scalar::Bool => Err("`negate` on a Bool".into()),
                }
            }
            Prim::Abs => {
                arity(1)?;
                match vals[0].ty {
                    Scalar::Int => {
                        let bad = self.fresh();
                        self.line(format!(
                            "{bad} = icmp eq i64 {}, -9223372036854775808",
                            vals[0].text
                        ));
                        let payload = self.widen(&vals[0]);
                        self.trap(Trap::AbsOverflow, span, &payload, &bad);
                        let r = self.fresh();
                        self.line(format!(
                            "{r} = call i64 @llvm.abs.i64(i64 {}, i1 true)",
                            vals[0].text
                        ));
                        Ok(Val {
                            text: r,
                            ty: Scalar::Int,
                        })
                    }
                    Scalar::Float => {
                        let r = self.intrinsic_f64("llvm.fabs.f64", &vals[0])?;
                        Ok(Val {
                            text: r,
                            ty: Scalar::Float,
                        })
                    }
                    Scalar::Bool => Err("`abs` on a Bool".into()),
                }
            }
            Prim::Sqrt | Prim::Sin | Prim::Cos => {
                arity(1)?;
                let name = match op {
                    Prim::Sqrt => "llvm.sqrt.f64",
                    Prim::Sin => "llvm.sin.f64",
                    _ => "llvm.cos.f64",
                };
                let r = self.intrinsic_f64(name, &vals[0])?;
                Ok(Val {
                    text: r,
                    ty: Scalar::Float,
                })
            }
            Prim::Trunc => {
                arity(1)?;
                if vals[0].ty != Scalar::Float {
                    return Err("`trunc` of something that is not a Float".into());
                }
                // Saturating, because the evaluator's `f as i64` is: plain `fptosi` is poison out
                // of range, and NaN would be whatever the target felt like rather than zero.
                let r = self.fresh();
                self.line(format!(
                    "{r} = call i64 @llvm.fptosi.sat.i64.f64(double {})",
                    vals[0].text
                ));
                Ok(Val {
                    text: r,
                    ty: Scalar::Int,
                })
            }
            Prim::ToFloat => {
                arity(1)?;
                if vals[0].ty != Scalar::Int {
                    return Err("`float` of something that is not an Int".into());
                }
                let r = self.fresh();
                self.line(format!("{r} = sitofp i64 {} to double", vals[0].text));
                // No normalisation: an integer converts to neither a negative zero nor a NaN.
                Ok(Val {
                    text: r,
                    ty: Scalar::Float,
                })
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                arity(2)?;
                same(&vals)?;
                Ok(self.compare(op, &vals[0], &vals[1]))
            }
            Prim::And | Prim::Or | Prim::Not => {
                let want = if op == Prim::Not { 1 } else { 2 };
                arity(want)?;
                if vals.iter().any(|v| v.ty != Scalar::Bool) {
                    return Err(format!("`{}` on something that is not a Bool", op.name()));
                }
                let r = self.fresh();
                match op {
                    Prim::Not => self.line(format!("{r} = xor i1 {}, true", vals[0].text)),
                    Prim::And => {
                        self.line(format!("{r} = and i1 {}, {}", vals[0].text, vals[1].text))
                    }
                    _ => self.line(format!("{r} = or i1 {}, {}", vals[0].text, vals[1].text)),
                }
                Ok(Val {
                    text: r,
                    ty: Scalar::Bool,
                })
            }
            other => Err(format!(
                "`{}` is not one of the scalar primitives",
                other.name()
            )),
        }
    }

    fn intrinsic_f64(&mut self, name: &str, v: &Val) -> Result<String, String> {
        if v.ty != Scalar::Float {
            return Err(format!("`{name}` of something that is not a Float"));
        }
        let r = self.fresh();
        self.line(format!("{r} = call double @{name}(double {})", v.text));
        Ok(r)
    }

    /// `llvm.s{add,sub,mul}.with.overflow`, with the overflow bit turned into a trap.
    fn checked_int(&mut self, which: &str, trap: Trap, a: &Val, b: &Val, span: Span) -> Val {
        let pair = self.fresh();
        self.line(format!(
            "{pair} = call {{ i64, i1 }} @llvm.{which}.with.overflow.i64(i64 {}, i64 {})",
            a.text, b.text
        ));
        let over = self.fresh();
        self.line(format!("{over} = extractvalue {{ i64, i1 }} {pair}, 1"));
        self.trap(trap, span, "0", &over);
        let r = self.fresh();
        self.line(format!("{r} = extractvalue {{ i64, i1 }} {pair}, 0"));
        Val {
            text: r,
            ty: Scalar::Int,
        }
    }

    /// `sdiv`/`srem`, guarded exactly where `i64::checked_div`/`checked_rem` answer `None`.
    ///
    /// Two cases, and both have to be caught before the instruction runs: a zero divisor, and
    /// `i64::MIN / -1` whose quotient is not representable. In LLVM either is immediate undefined
    /// behaviour rather than a fault, so a missing guard here would not be a wrong answer — it
    /// would be the optimiser deleting the surrounding code.
    fn checked_divide(&mut self, opcode: &str, trap: Trap, a: &Val, b: &Val, span: Span) -> Val {
        let zero = self.fresh();
        self.line(format!("{zero} = icmp eq i64 {}, 0", b.text));
        let min = self.fresh();
        self.line(format!(
            "{min} = icmp eq i64 {}, -9223372036854775808",
            a.text
        ));
        let neg1 = self.fresh();
        self.line(format!("{neg1} = icmp eq i64 {}, -1", b.text));
        let both = self.fresh();
        self.line(format!("{both} = and i1 {min}, {neg1}"));
        let bad = self.fresh();
        self.line(format!("{bad} = or i1 {zero}, {both}"));
        self.trap(trap, span, "0", &bad);
        let r = self.fresh();
        self.line(format!("{r} = {opcode} i64 {}, {}", a.text, b.text));
        Val {
            text: r,
            ty: Scalar::Int,
        }
    }

    /// `-0.0` becomes `0.0`, because [`beck_core::Value::float`] does it on every real it makes.
    ///
    /// # Where this is called, and the invariant that says the rest is safe
    ///
    /// It was once applied to the result of *every* float operation, which is the obvious way to
    /// match a host that normalises on every construction — and it cost **3×** on float-heavy code
    /// (`docs/93` §93.5). It is now applied in three places, on this invariant:
    ///
    /// > A value in a register here differs from the one the evaluator holds **at most in the sign
    /// > of a zero, or in which NaN it is**.
    ///
    /// Every float operation preserves that. `fadd`, `fsub`, `fmul`, `fdiv`, `fneg`, `fabs`,
    /// `sqrt`, `sin` and `cos` map zeros to zeros and NaNs to NaNs, and everything else to the same
    /// thing either way; `trunc` of either zero is `0` and of any NaN is `0`. So normalising is
    /// needed only where the difference becomes observable:
    ///
    /// * **a comparison** — `-0.0` and `0.0` have different order keys, and so do two NaNs; the
    ///   language says each pair is one value;
    /// * **a division's divisor** — `1.0 / -0.0` is `-inf` where `1.0 / 0.0` is `+inf`, which is a
    ///   difference a zero's sign has escaped into;
    /// * **a trap's payload**, which is rendered into a message a person reads.
    ///
    /// Returning is not on the list because it is already handled: the host narrows through
    /// `Value::float`, which normalises.
    ///
    /// The NaN half is not theoretical and was not free. It was originally argued away — "the
    /// operations here produce the platform's default quiet NaN, which is the one the
    /// canonicalisation picks" — and that is **false on x86-64**: `0.0 * inf` yields the
    /// *indefinite* QNaN `0xFFF8…`, whose sign bit is set, where `f64::NAN` is `0x7FF8…`. Under the
    /// order key one sorts below every number and the other above every number, so
    /// `(0.0 * inf) > 0.0` answered `true` in the evaluator and `false` here (`docs/93` §93.2).
    ///
    /// `product_order` in the differential is what found it, and it found it only because that test
    /// compares a NaN a *computation produced* rather than one the host handed in already
    /// canonicalised. `product_is_zero` and `reciprocal_of_product` are the same shape for the two
    /// zero cases — each is a gate that goes red if its line here is removed, which is the property
    /// `AGENTS.md` asks for and the reason these three exist rather than one.
    fn normalise(&mut self, raw: &str) -> Val {
        let is_zero = self.fresh();
        self.line(format!(
            "{is_zero} = fcmp oeq double {raw}, 0x0000000000000000"
        ));
        let zeroed = self.fresh();
        self.line(format!(
            "{zeroed} = select i1 {is_zero}, double 0x0000000000000000, double {raw}"
        ));
        // …and every NaN becomes one NaN, for the same reason and with the same rule: `Value::float`
        // maps them all to `f64::NAN`. `fcmp uno x, x` is the NaN test.
        let is_nan = self.fresh();
        self.line(format!("{is_nan} = fcmp uno double {raw}, {raw}"));
        let r = self.fresh();
        self.line(format!(
            "{r} = select i1 {is_nan}, double 0x7FF8000000000000, double {zeroed}"
        ));
        Val {
            text: r,
            ty: Scalar::Float,
        }
    }

    /// `beck_core`'s order key: the transform that makes the derived `Ord` on the bits the numeric
    /// order. `bits ^ ((bits >> 63) | sign)` — arithmetic shift, so a negative becomes `!bits`.
    ///
    /// Normalises first, because the two zeros have different keys and the language has one zero.
    fn order_key(&mut self, v: &Val) -> String {
        let v = self.normalise(&v.text);
        let bits = self.fresh();
        self.line(format!("{bits} = bitcast double {} to i64", v.text));
        let sign = self.fresh();
        self.line(format!("{sign} = ashr i64 {bits}, 63"));
        let mask = self.fresh();
        self.line(format!("{mask} = or i64 {sign}, -9223372036854775808"));
        let key = self.fresh();
        self.line(format!("{key} = xor i64 {bits}, {mask}"));
        key
    }

    fn equals(&mut self, a: &Val, b: &Val) -> String {
        let cmp = self.compare(Prim::Eq, a, b);
        cmp.text
    }

    fn compare(&mut self, op: Prim, a: &Val, b: &Val) -> Val {
        // Reals compare through the order key and Bools compare unsigned, so `false < true`. Both
        // are the ordering `Value`'s derived `Ord` gives, which is the one the evaluator uses.
        let (lhs, rhs, signed) = match a.ty {
            Scalar::Float => (self.order_key(a), self.order_key(b), false),
            Scalar::Int => (a.text.clone(), b.text.clone(), true),
            Scalar::Bool => (a.text.clone(), b.text.clone(), false),
        };
        let width = if a.ty == Scalar::Bool { "i1" } else { "i64" };
        let pred = match op {
            Prim::Eq => "eq",
            Prim::Ne => "ne",
            Prim::Lt if signed => "slt",
            Prim::Le if signed => "sle",
            Prim::Gt if signed => "sgt",
            Prim::Ge if signed => "sge",
            Prim::Lt => "ult",
            Prim::Le => "ule",
            Prim::Gt => "ugt",
            _ => "uge",
        };
        let r = self.fresh();
        self.line(format!("{r} = icmp {pred} {width} {lhs}, {rhs}"));
        Val {
            text: r,
            ty: Scalar::Bool,
        }
    }

    /// The value as an `i64`, which is how it crosses the worker's protocol and how a trap carries
    /// the scrutinee that matched nothing.
    fn widen(&mut self, v: &Val) -> String {
        match v.ty {
            Scalar::Int => v.text.clone(),
            // Normalised, because the one thing that reads this is a message: `Trap::message`
            // renders it, and a scrutinee printed as `-0` where the evaluator prints `0` is a
            // divergence in the differential.
            Scalar::Float => {
                let v = self.normalise(&v.text);
                let r = self.fresh();
                self.line(format!("{r} = bitcast double {} to i64", v.text));
                r
            }
            Scalar::Bool => {
                let r = self.fresh();
                self.line(format!("{r} = zext i1 {} to i64", v.text));
                r
            }
        }
    }
}

fn constant(k: &Const) -> Result<Val, String> {
    match k {
        Const::Int(i) => Ok(Val {
            text: i.to_string(),
            ty: Scalar::Int,
        }),
        Const::Bool(b) => Ok(Val {
            text: b.to_string(),
            ty: Scalar::Bool,
        }),
        // Written as the bit pattern, so what the assembler reads back is the double the compiler
        // held rather than whatever a decimal rendering happened to round to.
        Const::Float(f) => Ok(Val {
            text: format!("0x{:016X}", f.to_bits()),
            ty: Scalar::Float,
        }),
        Const::Str(_) => Err("a string constant, and there is no heap here".into()),
        Const::Unit => Err("the unit value, which has no machine representation here".into()),
    }
}

/// A Beck name as an LLVM symbol.
///
/// Quoted and escaped rather than transliterated: identifiers are Unicode (`docs/44` §44.4), and a
/// scheme that dropped or folded characters could give two definitions one symbol.
fn mangle(name: &str) -> String {
    let mut out = String::from("\"beck.");
    for b in name.bytes() {
        match b {
            b'"' | b'\\' => {
                let _ = write!(out, "\\{b:02X}");
            }
            0x20..=0x7e => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{b:02X}");
            }
        }
    }
    out.push('"');
    out
}

// -------------------------------------------------------------------------------------------
// The module around the functions
// -------------------------------------------------------------------------------------------

/// The declarations, the compiled bodies, the dispatch table and the worker loop.
fn assemble(bodies: &str, functions: &[Signature]) -> String {
    let mut m = String::new();
    m.push_str(HEADER);
    m.push_str(bodies);

    // One thunk per function: the protocol carries every argument as eight bytes, so this is where
    // an `i64` becomes a `double` or an `i1` and the result becomes eight bytes again.
    for sig in functions {
        let _ = writeln!(
            m,
            "define internal i64 @\"beck.thunk.{}\"(ptr noalias %err, ptr %args) {{\nentry:",
            sig.index
        );
        let mut operands = String::from("ptr %err");
        for (i, ty) in sig.params.iter().enumerate() {
            let slot = if i == 0 {
                "%args".to_string()
            } else {
                let _ = writeln!(
                    m,
                    "  %g{i} = getelementptr inbounds i8, ptr %args, i64 {}",
                    i * 8
                );
                format!("%g{i}")
            };
            let _ = writeln!(m, "  %r{i} = load i64, ptr {slot}");
            match ty {
                Scalar::Int => {
                    let _ = writeln!(m, "  %p{i} = add i64 %r{i}, 0");
                }
                Scalar::Float => {
                    let _ = writeln!(m, "  %p{i} = bitcast i64 %r{i} to double");
                }
                Scalar::Bool => {
                    let _ = writeln!(m, "  %p{i} = icmp ne i64 %r{i}, 0");
                }
            }
            let _ = write!(operands, ", {} %p{i}", ty.llvm());
        }
        let _ = writeln!(
            m,
            "  %out = call tailcc {} @{}({operands})",
            sig.ret.llvm(),
            mangle(&sig.name)
        );
        match sig.ret {
            Scalar::Int => {
                let _ = writeln!(m, "  %wide = add i64 %out, 0");
            }
            Scalar::Float => {
                let _ = writeln!(m, "  %wide = bitcast double %out to i64");
            }
            Scalar::Bool => {
                let _ = writeln!(m, "  %wide = zext i1 %out to i64");
            }
        }
        m.push_str("  ret i64 %wide\n}\n\n");
    }

    // The dispatch table. An index the host never sends is still an index the worker has to
    // answer, so the default arm returns rather than falling off the end of the function.
    m.push_str(
        "define internal i64 @\"beck.dispatch\"(i32 %idx, ptr noalias %err, ptr %args) {\nentry:\n",
    );
    if functions.is_empty() {
        m.push_str("  ret i64 0\n}\n\n");
    } else {
        m.push_str("  switch i32 %idx, label %unknown [\n");
        for sig in functions {
            let _ = writeln!(m, "    i32 {}, label %c{}", sig.index, sig.index);
        }
        m.push_str("  ]\n");
        for sig in functions {
            let _ = writeln!(
                m,
                "c{0}:\n  %v{0} = call i64 @\"beck.thunk.{0}\"(ptr %err, ptr %args)\n  ret i64 %v{0}",
                sig.index
            );
        }
        m.push_str("unknown:\n  store i32 255, ptr %err\n  ret i64 0\n}\n\n");
    }

    m.push_str(WORKER);
    m
}

/// Declarations, and nothing that depends on the program.
const HEADER: &str = r#"; Generated by `beck native`. Do not edit.
;
; Every function here takes a `ptr %err` first: a 24-byte cell holding a trap code, the index of
; the span that trapped, and a payload. A computation that cannot produce a value stores into it
; and returns; its caller checks and returns in turn. That is the whole error mechanism, and it is
; a mechanism rather than a signal because the host is a different process and a `SIGFPE` would
; tell it nothing about which span was at fault.

declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64)
declare { i64, i1 } @llvm.ssub.with.overflow.i64(i64, i64)
declare { i64, i1 } @llvm.smul.with.overflow.i64(i64, i64)
declare i64 @llvm.abs.i64(i64, i1)
declare i64 @llvm.fptosi.sat.i64.f64(double)
declare double @llvm.sqrt.f64(double)
declare double @llvm.sin.f64(double)
declare double @llvm.cos.f64(double)
declare double @llvm.fabs.f64(double)
declare i64 @read(i32, ptr, i64)
declare i64 @write(i32, ptr, i64)

"#;

/// The worker: read a call, answer it, repeat until the host closes the pipe.
///
/// A request is eight bytes of header — a function index and an argument count — followed by eight
/// bytes per argument. A reply is always 24 bytes: the trap cell, then the result. Fixed widths,
/// so neither side has to parse anything.
const WORKER: &str = r#"define internal i64 @"beck.read_exact"(ptr %p, i64 %n) {
entry:
  br label %loop
loop:
  %done = phi i64 [ 0, %entry ], [ %next, %cont ]
  %left = sub i64 %n, %done
  %full = icmp eq i64 %left, 0
  br i1 %full, label %out, label %again
again:
  %dst = getelementptr inbounds i8, ptr %p, i64 %done
  %got = call i64 @read(i32 0, ptr %dst, i64 %left)
  %bad = icmp slt i64 %got, 1
  br i1 %bad, label %out, label %cont
cont:
  %next = add i64 %done, %got
  br label %loop
out:
  %total = phi i64 [ %done, %loop ], [ %done, %again ]
  ret i64 %total
}

define internal i64 @"beck.write_all"(ptr %p, i64 %n) {
entry:
  br label %loop
loop:
  %done = phi i64 [ 0, %entry ], [ %next, %cont ]
  %left = sub i64 %n, %done
  %full = icmp eq i64 %left, 0
  br i1 %full, label %out, label %again
again:
  %src = getelementptr inbounds i8, ptr %p, i64 %done
  %put = call i64 @write(i32 1, ptr %src, i64 %left)
  %bad = icmp slt i64 %put, 1
  br i1 %bad, label %out, label %cont
cont:
  %next = add i64 %done, %put
  br label %loop
out:
  %total = phi i64 [ %done, %loop ], [ %done, %again ]
  ret i64 %total
}

define i32 @main() {
entry:
  %req = alloca [1 x i64]
  %args = alloca [16 x i64]
  %err = alloca [3 x i64]
  %resp = alloca [3 x i64]
  br label %loop
loop:
  %head = call i64 @"beck.read_exact"(ptr %req, i64 8)
  %closed = icmp ne i64 %head, 8
  br i1 %closed, label %done, label %sized
sized:
  %idx = load i32, ptr %req
  %cntp = getelementptr inbounds i8, ptr %req, i64 4
  %cnt32 = load i32, ptr %cntp
  %cnt = zext i32 %cnt32 to i64
  %bytes = mul i64 %cnt, 8
  %read = call i64 @"beck.read_exact"(ptr %args, i64 %bytes)
  %short = icmp ne i64 %read, %bytes
  br i1 %short, label %done, label %run
run:
  store i64 0, ptr %err
  %plz = getelementptr inbounds i8, ptr %err, i64 8
  store i64 0, ptr %plz
  %res = call i64 @"beck.dispatch"(i32 %idx, ptr %err, ptr %args)
  %cell = load i64, ptr %err
  store i64 %cell, ptr %resp
  %pl = load i64, ptr %plz
  %rpl = getelementptr inbounds i8, ptr %resp, i64 8
  store i64 %pl, ptr %rpl
  %rv = getelementptr inbounds i8, ptr %resp, i64 16
  store i64 %res, ptr %rv
  %wrote = call i64 @"beck.write_all"(ptr %resp, i64 24)
  %gone = icmp ne i64 %wrote, 24
  br i1 %gone, label %done, label %loop
done:
  ret i32 0
}
"#;
