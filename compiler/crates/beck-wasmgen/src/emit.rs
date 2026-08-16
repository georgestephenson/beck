//! `Core` → WebAssembly.
//!
//! # The subset, and why it is this one
//!
//! A definition whose parameters and result are `Int`, `Float` or `Bool`, and whose body is built
//! from constants, variables, `let`, `if`, `match` on scalars, direct calls to other compiled
//! definitions, and the arithmetic, comparison, logical and conversion primitives.
//!
//! That is where [`beck_llvm`] started ([`docs/93`](../../../../../docs/93-the-native-backends-report.md)
//! §93.6) and it is where this starts, for a reason that is not tidiness: **the heap is the whole
//! of the remaining work**, and it is the same work on every target. What is new here is
//! everything *around* a heap — a binary format written by hand, structured control flow with no
//! jumps in it, a trap that cannot be a signal, and a tail call that is a proposal rather than a
//! calling convention. Doing those against a subset with an oracle already attached is what makes
//! them checkable now.
//!
//! # Agreeing with the evaluator
//!
//! Every rule [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.3 found for the
//! native backends applies here unchanged, and the third emitter inherits the list rather than
//! rediscovering it:
//!
//! * **Integer arithmetic is checked.** `beck-eval` uses `i64::checked_*`; WebAssembly's `i64.add`
//!   wraps and its `i64.div_s` *traps the whole instance*, which is neither. Each operator carries
//!   its own guard and stores a [`Trap`] code in a global.
//! * **Reals are compared by `beck_core`'s order key**, not by `f64.lt`: `Value::Float` stores a
//!   monotone transform of the bits, under which `-0.0 < 0.0` and NaN is the maximum
//!   ([`docs/27`](../../../../../docs/27-the-walls-come-down-report.md) §27.8). `f64.lt` says
//!   something else for both.
//! * **A real is normalised where a signed zero or a NaN is observable** — a comparison, a
//!   division's divisor — and nowhere else, because every float operation maps zeros to zeros.
//! * **`trunc` saturates**, so it is `i64.trunc_sat_f64_s` and not `i64.trunc_f64_s`, which traps
//!   out of range.
//!
//! # A trap is a value, not a WebAssembly trap
//!
//! `unreachable` and a division by zero abort the *instance*, and a Beck program that overflows has
//! failed the way its type says it can. So a computation that cannot produce a value stores the
//! trap code and a span index in exported globals and returns a zero, exactly as the two native
//! backends store one in an error cell — the codes are [`beck_llvm::Trap`]'s, so the host decodes
//! one wire rather than three.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_core::check::{Def, Program};
use beck_core::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use beck_core::Value;
use beck_diag::Span;
use beck_llvm::heap::{Heap, Repr};
use beck_llvm::{Refusal, Scalar, Signature, Trap};

use crate::binary::{Body, Ins, ModuleBuilder, ValType};

/// The globals a compiled module exports, in index order.
///
/// Three rather than one because a trap is three facts: which failure, where, and — for the three
/// `no match` codes — the value nothing matched. [`Trap::message`] takes exactly that payload, so
/// the host builds the evaluator's own sentence out of what is here.
pub const TRAP: u32 = 0;
pub const TRAP_SPAN: u32 = 1;
pub const TRAP_PAYLOAD: u32 = 2;

/// The most parameters a compiled function may have.
///
/// The same bound the other two emitters carry, and for the same reason: a host reading a call's
/// arguments out of a fixed buffer decides it, and nothing in this repository comes close.
pub const MAX_PARAMS: usize = 16;

/// A whole module of compiled definitions.
pub struct Module {
    /// The bytes a WebAssembly runtime loads.
    pub wasm: Vec<u8>,
    /// The same module, as text a person reads. [`crate::text`] is why this is not a second
    /// account of what was emitted.
    pub text: String,
    /// One per compiled definition, in export order.
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

/// Compile every definition of `program` this backend can compile.
///
/// Never fails, for [`beck_llvm::emit::module`]'s reason: a program with nothing scalar in it
/// yields a module with no functions and a refusal per definition, and whether that is worth
/// running is the caller's decision.
pub fn module(program: &Program) -> Module {
    // Specialised first, so nothing below ever sees a type parameter — the same pass the other two
    // emitters run, because monomorphisation is a property of the language and not of a target.
    let mono = beck_llvm::mono::specialise(program);
    let program = &mono.program;
    let mut heap = Heap::new();
    let mut refusals: Vec<Refusal> = Vec::new();
    let mut sigs: BTreeMap<Arc<str>, Signature> = BTreeMap::new();

    // Round one: the signature. A definition whose parameters or result are not scalar cannot be
    // called at all here, whatever its body is.
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        match signature_of(def, &mut heap, program) {
            Ok(sig) => {
                sigs.insert(name.clone(), sig);
            }
            Err(reason) => refusals.push(Refusal {
                name: name.clone(),
                reason,
            }),
        }
    }

    // Round two, to a fixed point: a body that calls a definition dropped in an earlier round is
    // dropped in a later one, so a mutually recursive pair survives together or is refused
    // together.
    let mut eligible: BTreeSet<Arc<str>> = sigs.keys().cloned().collect();
    loop {
        let mut removed = false;
        for name in eligible.clone() {
            let def = &program.defs[&name];
            let mut fun = Function::new(&sigs, &eligible, program, &mut heap);
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

    // Declaration order, so a module is a function of the program and not of a hash seed: the same
    // source twice is the same bytes twice, which is what a build wants and a diff needs.
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

    let mut builder = ModuleBuilder::new();
    builder
        .globals
        .push(("beck_trap".into(), ValType::I32, true, 0));
    builder
        .globals
        .push(("beck_trap_span".into(), ValType::I32, true, 0));
    builder
        .globals
        .push(("beck_trap_payload".into(), ValType::I64, true, 0));

    let mut spans: Vec<Span> = Vec::new();
    for name in &order {
        let def = &program.defs[name];
        let mut fun = Function::new(&indexed, &eligible, program, &mut heap);
        fun.spans = std::mem::take(&mut spans);
        let body = fun
            .emit(def)
            .expect("the fixed point already proved this emits");
        assert!(
            !body
                .code
                .iter()
                .any(|i| matches!(i, Ins::Call(u32::MAX) | Ins::ReturnCall(u32::MAX))),
            "`{name}` kept a call index from the fixed point, where nothing has one yet"
        );
        spans = std::mem::take(&mut fun.spans);
        let sig = &indexed[name];
        let ty = builder.ty(
            sig.params.iter().map(|r| val(*r)).collect(),
            vec![val(sig.ret)],
        );
        builder.funcs.push(ty);
        builder.bodies.push(body);
        builder
            .exports
            .push((name.to_string(), (builder.bodies.len() - 1) as u32));
    }

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    refusals.sort_by(|a, b| a.name.cmp(&b.name));
    Module {
        wasm: builder.encode(),
        text: crate::text::render(&builder, &functions),
        functions,
        spans,
        refusals,
    }
}

/// The WebAssembly type a scalar is held in.
///
/// A `Bool` is an `i32` because that is what WebAssembly's own comparisons and `if` produce; the
/// other two are the machine types the language already has.
pub fn val(r: Repr) -> ValType {
    match r.machine() {
        Scalar::Int => ValType::I64,
        Scalar::Float => ValType::F64,
        Scalar::Bool => ValType::I32,
    }
}

fn zero(r: Repr) -> Ins {
    match r.machine() {
        Scalar::Int => Ins::I64Const(0),
        Scalar::Float => Ins::F64Const(0.0),
        Scalar::Bool => Ins::I32Const(0),
    }
}

fn signature_of(def: &Def, heap: &mut Heap, program: &Program) -> Result<Signature, String> {
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
            "{} parameters, and a call's argument buffer holds {MAX_PARAMS}",
            def.params.len()
        ));
    }
    let mut params = Vec::with_capacity(def.params.len());
    for (_, name, ty) in &def.params {
        let r = heap
            .repr(ty, program)
            .map_err(|why| format!("parameter `{name}` is {why}"))?;
        if r.is_ref() {
            return Err(format!(
                "parameter `{name}` lives on the heap, which this emitter does not lay out yet"
            ));
        }
        params.push(r);
    }
    let ret = heap
        .repr(&def.ret, program)
        .map_err(|why| format!("returns {why}"))?;
    if ret.is_ref() {
        return Err("returns a value on the heap, which this emitter does not lay out yet".into());
    }
    Ok(Signature {
        name: def.name.clone(),
        params,
        ret,
        index: u32::MAX,
    })
}

/// One function being emitted.
struct Function<'a> {
    sigs: &'a BTreeMap<Arc<str>, Signature>,
    eligible: &'a BTreeSet<Arc<str>>,
    program: &'a Program,
    heap: &'a mut Heap,
    /// Every local beyond the parameters, in declaration order.
    locals: Vec<ValType>,
    params: usize,
    env: BTreeMap<VarId, u32>,
    code: Vec<Ins>,
    ret: Repr,
    /// The spans a trap in this module can name, shared across the module's functions.
    spans: Vec<Span>,
}

impl<'a> Function<'a> {
    fn new(
        sigs: &'a BTreeMap<Arc<str>, Signature>,
        eligible: &'a BTreeSet<Arc<str>>,
        program: &'a Program,
        heap: &'a mut Heap,
    ) -> Function<'a> {
        Function {
            sigs,
            eligible,
            program,
            heap,
            locals: Vec::new(),
            params: 0,
            env: BTreeMap::new(),
            code: Vec::new(),
            ret: Repr::Int,
            spans: Vec::new(),
        }
    }

    fn emit(&mut self, def: &Def) -> Result<Body, String> {
        let sig = self
            .sigs
            .get(&def.name)
            .ok_or_else(|| "no signature".to_string())?
            .clone();
        self.ret = sig.ret;

        // A definition is stored as the lambda that is its whole body, so the parameters are the
        // lambda's and the types are the signature's.
        let CoreKind::Lam { params, body } = &def.body.kind else {
            return Err("the body is not a lambda".into());
        };
        if params.len() != sig.params.len() {
            return Err("the lambda's parameters do not match the signature".into());
        }
        self.params = params.len();
        for (i, var) in params.iter().enumerate() {
            self.env.insert(*var, i as u32);
        }

        let got = self.expr(body, true)?;
        if got.machine() != sig.ret.machine() {
            return Err(format!(
                "returns {:?} where the signature says {:?}",
                got, sig.ret
            ));
        }
        Ok(Body {
            locals: std::mem::take(&mut self.locals),
            code: std::mem::take(&mut self.code),
        })
    }

    fn push(&mut self, ins: Ins) {
        self.code.push(ins);
    }

    /// A fresh local. Never reused: a WebAssembly local costs a slot in a frame, and a reuse
    /// analysis would be an optimisation with a correctness question attached.
    fn local(&mut self, ty: ValType) -> u32 {
        self.locals.push(ty);
        (self.params + self.locals.len() - 1) as u32
    }

    fn span_index(&mut self, span: Span) -> u32 {
        if let Some(i) = self.spans.iter().position(|s| *s == span) {
            return i as u32;
        }
        self.spans.push(span);
        (self.spans.len() - 1) as u32
    }

    /// Store a trap and return the function's zero.
    ///
    /// `payload` is a local holding the value the three `no match` codes report; the others carry
    /// nothing, and [`Trap::message`] ignores what they carry.
    fn trap(&mut self, trap: Trap, span: Span, payload: Option<u32>) {
        if let Some(local) = payload {
            self.push(Ins::LocalGet(local));
            self.push(Ins::GlobalSet(TRAP_PAYLOAD));
        }
        let index = self.span_index(span);
        self.push(Ins::I32Const(trap.code() as i32));
        self.push(Ins::GlobalSet(TRAP));
        self.push(Ins::I32Const(index as i32));
        self.push(Ins::GlobalSet(TRAP_SPAN));
        let z = zero(self.ret);
        self.push(z);
        self.push(Ins::Return);
    }

    /// The representation of an expression, as the language sees it.
    fn repr(&mut self, c: &Core) -> Result<Repr, String> {
        let r = self.heap.repr(&c.ty, self.program)?;
        if r.is_ref() {
            return Err(format!(
                "a value on the heap ({r:?}), which this emitter does not lay out yet"
            ));
        }
        Ok(r)
    }

    /// Emit `c`, leaving its value on the stack.
    ///
    /// `tail` says the value is the function's result, which is what makes a call a `return_call`
    /// and therefore a jump (§93.4).
    fn expr(&mut self, c: &Core, tail: bool) -> Result<Repr, String> {
        match &c.kind {
            CoreKind::Const(k) => self.constant(k, c),
            CoreKind::Var(v) => {
                let repr = self.repr(c)?;
                let local = *self
                    .env
                    .get(v)
                    .ok_or_else(|| "a variable this emitter never bound".to_string())?;
                self.push(Ins::LocalGet(local));
                Ok(repr)
            }
            CoreKind::Let { var, value, body } => {
                let vr = self.expr(value, false)?;
                let local = self.local(val(vr));
                self.push(Ins::LocalSet(local));
                self.env.insert(*var, local);
                self.expr(body, tail)
            }
            CoreKind::If { cond, then, alt } => {
                let cr = self.expr(cond, false)?;
                if cr.machine() != Scalar::Bool {
                    return Err("a condition that is not a Bool".into());
                }
                let repr = self.repr(c)?;
                self.push(Ins::If(Some(val(repr))));
                let t = self.expr(then, tail)?;
                self.push(Ins::Else);
                let a = self.expr(alt, tail)?;
                self.push(Ins::End);
                if t.machine() != a.machine() {
                    return Err("the two arms of an `if` have different machine types".into());
                }
                Ok(repr)
            }
            CoreKind::Match { scrutinee, arms } => self.match_(c, scrutinee, arms, tail),
            CoreKind::App { func, args } => self.call(c, func, args, tail),
            CoreKind::Prim { op, args } => self.prim(*op, args, c),
            CoreKind::Global(name) => Err(format!(
                "`{name}` used as a value — a function value has no machine representation here"
            )),
            CoreKind::Lam { .. } => Err("a lambda: a closure lives on the heap".into()),
            CoreKind::Make { .. } | CoreKind::With { .. } => {
                Err("builds a record, which lives on the heap".into())
            }
            CoreKind::Field { .. } => Err("reads a field of a value on the heap".into()),
            CoreKind::ListLit(_) => Err("a list literal lives on the heap".into()),
            CoreKind::MapLit(_) => Err("a map literal lives on the heap".into()),
        }
    }

    fn constant(&mut self, k: &Const, c: &Core) -> Result<Repr, String> {
        match k {
            Const::Int(v) => {
                self.push(Ins::I64Const(*v));
                Ok(Repr::Int)
            }
            Const::Bool(v) => {
                self.push(Ins::I32Const(i32::from(*v)));
                Ok(Repr::Bool)
            }
            Const::Float(v) => {
                // Through `Value::float`, so a literal `-0.0` is the same `0.0` the evaluator
                // holds rather than a bit pattern that compares differently.
                self.push(Ins::F64Const(canonical(*v)));
                Ok(Repr::Float)
            }
            Const::Str(_) => Err("a string literal lives on the heap".into()),
            Const::Unit => {
                let _ = c;
                Err("unit has no machine representation here".into())
            }
        }
    }

    /// A direct call to another compiled definition.
    fn call(&mut self, c: &Core, func: &Core, args: &[Core], tail: bool) -> Result<Repr, String> {
        let CoreKind::Global(name) = &func.kind else {
            return Err("an indirect call: a function value lives on the heap".into());
        };
        if !self.eligible.contains(name) {
            return Err(format!("calls `{name}`, which this backend refused"));
        }
        let sig = self
            .sigs
            .get(name)
            .ok_or_else(|| format!("calls `{name}`, which has no signature"))?
            .clone();
        if sig.params.len() != args.len() {
            return Err(format!("calls `{name}` with the wrong number of arguments"));
        }
        for (arg, want) in args.iter().zip(&sig.params) {
            let got = self.expr(arg, false)?;
            if got.machine() != want.machine() {
                return Err(format!("calls `{name}` with an argument of the wrong type"));
            }
        }
        // The index is `u32::MAX` during the fixed point, because which definitions survive is
        // what the fixed point is deciding. Nothing is kept from those rounds; `module` asserts
        // that no placeholder reached a body it did keep.
        let index = sig.index;
        // A tail call is a jump. Nothing is checked after one because there is no "after": the
        // callee's trap is the caller's, already in the globals.
        if tail && sig.ret.machine() == self.ret.machine() {
            self.push(Ins::ReturnCall(index));
            return self.repr(c);
        }
        self.push(Ins::Call(index));
        // A callee that trapped left a code in the global and a zero on the stack, so the caller
        // has to stop as well: this is the error-cell check the other two emitters make after
        // every call, spelled as the one branch WebAssembly has.
        let held = self.local(val(sig.ret));
        self.push(Ins::LocalSet(held));
        self.push(Ins::GlobalGet(TRAP));
        self.push(Ins::If(None));
        let z = zero(self.ret);
        self.push(z);
        self.push(Ins::Return);
        self.push(Ins::End);
        self.push(Ins::LocalGet(held));
        self.repr(c)
    }

    // ---------------------------------------------------------------------------------- `match`

    fn match_(
        &mut self,
        c: &Core,
        scrutinee: &Core,
        arms: &[Arm],
        tail: bool,
    ) -> Result<Repr, String> {
        let sr = self.expr(scrutinee, false)?;
        let subject = self.local(val(sr));
        self.push(Ins::LocalSet(subject));
        let repr = self.repr(c)?;
        self.arms(arms, subject, sr, repr, tail, scrutinee.span)?;
        Ok(repr)
    }

    fn arms(
        &mut self,
        arms: &[Arm],
        subject: u32,
        sr: Repr,
        repr: Repr,
        tail: bool,
        span: Span,
    ) -> Result<(), String> {
        let Some((arm, rest)) = arms.split_first() else {
            // Nothing matched. Unreachable — the checker proves a `match` exhaustive — and a code
            // rather than `unreachable` for the reason the other emitters give: a wrong
            // exhaustiveness check should be a message naming this trap, not a licence for a
            // runtime to do anything at all with the path it reached.
            let trap = match sr.machine() {
                Scalar::Int => Trap::NoMatchInt,
                Scalar::Float => Trap::NoMatchFloat,
                Scalar::Bool => Trap::NoMatchBool,
            };
            // The float payload is the *order key*, which is what `Trap::message` reads back
            // through `Value::Float`.
            let payload = self.payload_local(subject, sr);
            self.trap(trap, span, Some(payload));
            return Ok(());
        };

        // The test goes first and the bindings second, which is only sound because a binder is
        // irrefutable: `test` emits nothing for one, so nothing it emits can depend on a name the
        // pattern introduces. A guard is the other way round — it reads what the pattern bound,
        // and a guard that fails falls through to the next arm, which is what makes it a guard
        // rather than an `if` in the body.
        let irrefutable = self.test(&arm.pattern, subject, sr)?;
        self.bind(&arm.pattern, subject);
        match (&arm.guard, irrefutable) {
            (None, true) => {
                let got = self.expr(&arm.body, tail)?;
                if got.machine() != repr.machine() {
                    return Err("an arm whose body has the wrong machine type".into());
                }
                return Ok(());
            }
            (Some(guard), irrefutable) => {
                let g = self.expr(guard, false)?;
                if g.machine() != Scalar::Bool {
                    return Err("a guard that is not a Bool".into());
                }
                if !irrefutable {
                    self.push(Ins::I32And);
                }
            }
            (None, false) => {}
        }
        self.push(Ins::If(Some(val(repr))));
        let got = self.expr(&arm.body, tail)?;
        if got.machine() != repr.machine() {
            return Err("an arm whose body has the wrong machine type".into());
        }
        self.push(Ins::Else);
        self.arms(rest, subject, sr, repr, tail, span)?;
        self.push(Ins::End);
        Ok(())
    }

    /// A local holding what a `no match` trap reports.
    fn payload_local(&mut self, subject: u32, sr: Repr) -> u32 {
        match sr.machine() {
            Scalar::Int => subject,
            Scalar::Bool => {
                let wide = self.local(ValType::I64);
                self.push(Ins::LocalGet(subject));
                self.push(Ins::If(Some(ValType::I64)));
                self.push(Ins::I64Const(1));
                self.push(Ins::Else);
                self.push(Ins::I64Const(0));
                self.push(Ins::End);
                self.push(Ins::LocalSet(wide));
                wide
            }
            Scalar::Float => {
                let key = self.local(ValType::I64);
                self.push(Ins::LocalGet(subject));
                self.order_key();
                self.push(Ins::LocalSet(key));
                key
            }
        }
    }

    /// Emit the test for `pattern`, leaving an `i32` on the stack — or answer `true` for a pattern
    /// that always matches, having emitted nothing.
    fn test(&mut self, pattern: &Pattern, subject: u32, sr: Repr) -> Result<bool, String> {
        match pattern {
            Pattern::Wildcard | Pattern::Bind(_) => Ok(true),
            Pattern::At { inner, .. } => self.test(inner, subject, sr),
            Pattern::Const(k) => {
                self.equals(k, subject, sr)?;
                Ok(false)
            }
            Pattern::Or(alts) => {
                let mut any_irrefutable = false;
                let mut emitted = 0;
                for alt in alts {
                    if self.test(alt, subject, sr)? {
                        any_irrefutable = true;
                        continue;
                    }
                    emitted += 1;
                    if emitted > 1 {
                        self.push(Ins::I32Or);
                    }
                }
                if any_irrefutable {
                    // One alternative matches everything, so the whole pattern does. Whatever the
                    // others left on the stack goes with them.
                    for _ in 0..emitted {
                        self.push(Ins::Drop);
                    }
                    return Ok(true);
                }
                if emitted == 0 {
                    return Err("an empty or-pattern".into());
                }
                Ok(false)
            }
            other => Err(format!(
                "a pattern this emitter cannot test on a scalar ({other:?})"
            )),
        }
    }

    /// `subject == k`, as the *language's* equality rather than the machine's.
    fn equals(&mut self, k: &Const, subject: u32, sr: Repr) -> Result<(), String> {
        match (k, sr.machine()) {
            (Const::Int(v), Scalar::Int) => {
                self.push(Ins::LocalGet(subject));
                self.push(Ins::I64Const(*v));
                self.push(Ins::I64Eq);
                Ok(())
            }
            (Const::Bool(v), Scalar::Bool) => {
                self.push(Ins::LocalGet(subject));
                self.push(Ins::I32Const(i32::from(*v)));
                self.push(Ins::I32Eq);
                Ok(())
            }
            (Const::Float(v), Scalar::Float) => {
                // Both sides as order keys: a pattern `case -0.0:` and a scrutinee `0.0` are the
                // same value to the evaluator, and two bit patterns to `f64.eq`.
                self.push(Ins::LocalGet(subject));
                self.order_key();
                self.push(Ins::I64Const(order_key(*v)));
                self.push(Ins::I64Eq);
                Ok(())
            }
            _ => Err("a literal pattern whose type is not the scrutinee's".into()),
        }
    }

    /// Bind whatever names a pattern introduces to the subject.
    fn bind(&mut self, pattern: &Pattern, subject: u32) {
        match pattern {
            Pattern::Bind(v) => {
                self.env.insert(*v, subject);
            }
            Pattern::At { var, inner } => {
                self.env.insert(*var, subject);
                self.bind(inner, subject);
            }
            Pattern::Or(alts) => {
                for alt in alts {
                    self.bind(alt, subject);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------ primitives

    fn prim(&mut self, op: Prim, args: &[Core], c: &Core) -> Result<Repr, String> {
        match op {
            Prim::Add | Prim::Sub | Prim::Mul | Prim::Div | Prim::Rem => {
                self.arithmetic(op, args, c)
            }
            Prim::Neg | Prim::Abs => self.unary_arithmetic(op, args, c),
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                self.compare(op, args)
            }
            Prim::Not => {
                let r = self.one(args)?;
                if r.machine() != Scalar::Bool {
                    return Err("`not` of something that is not a Bool".into());
                }
                self.push(Ins::I32Eqz);
                Ok(Repr::Bool)
            }
            // Reached only by a *bare reference* — `and` passed as a value — because the checker
            // rewrites the operator form into an `if` so it short-circuits (`docs/53` §53.2). A
            // function value's arguments are already evaluated, so this one is strict and the
            // evaluator's is too.
            Prim::And | Prim::Or => {
                let [a, b] = args else {
                    return Err("`and`/`or` with the wrong arity".into());
                };
                let ra = self.expr(a, false)?;
                let rb = self.expr(b, false)?;
                if ra.machine() != Scalar::Bool || rb.machine() != Scalar::Bool {
                    return Err("`and`/`or` of something that is not a Bool".into());
                }
                self.push(if op == Prim::And {
                    Ins::I32And
                } else {
                    Ins::I32Or
                });
                Ok(Repr::Bool)
            }
            Prim::ToFloat => {
                let r = self.one(args)?;
                if r.machine() != Scalar::Int {
                    return Err("`float` of something that is not an Int".into());
                }
                self.push(Ins::F64ConvertI64S);
                Ok(Repr::Float)
            }
            Prim::Trunc => {
                let r = self.one(args)?;
                if r.machine() != Scalar::Float {
                    return Err("`trunc` of something that is not a Float".into());
                }
                // Saturating, because the evaluator's `f as i64` is Rust's saturating cast and the
                // plain conversion traps the instance out of range.
                self.push(Ins::I64TruncSatF64S);
                Ok(Repr::Int)
            }
            Prim::Sqrt => {
                let r = self.one(args)?;
                if r.machine() != Scalar::Float {
                    return Err("`sqrt` of something that is not a Float".into());
                }
                // IEEE-754 pins `sqrt` to one correctly-rounded answer, so `f64.sqrt` is the same
                // number the evaluator's is. `sin` and `cos` are refused below for exactly the
                // reason this one is allowed.
                self.push(Ins::F64Sqrt);
                Ok(Repr::Float)
            }
            Prim::Sin | Prim::Cos => Err(format!(
                "`{}` is not IEEE-pinned, so a WebAssembly runtime and the evaluator's libm may \
                 answer different digits — F9's question, and not one to prejudge from an emitter",
                if op == Prim::Sin { "sin" } else { "cos" }
            )),
            other => Err(format!(
                "`{}` needs the heap, which this emitter does not lay out yet",
                other.name()
            )),
        }
    }

    fn one(&mut self, args: &[Core]) -> Result<Repr, String> {
        let [a] = args else {
            return Err("a unary primitive with the wrong arity".into());
        };
        self.expr(a, false)
    }

    fn arithmetic(&mut self, op: Prim, args: &[Core], c: &Core) -> Result<Repr, String> {
        let [a, b] = args else {
            return Err("an arithmetic primitive with the wrong arity".into());
        };
        let ra = self.expr(a, false)?;
        let rb = self.expr(b, false)?;
        if ra.machine() != rb.machine() {
            return Err("arithmetic on two different machine types".into());
        }
        match ra.machine() {
            Scalar::Float => {
                if op == Prim::Rem {
                    return Err("`%` on reals, which WebAssembly has no instruction for".into());
                }
                if op == Prim::Div {
                    // The divisor is one of §93.3's three places a real is normalised: without it
                    // `1.0 / (0.0 * -1.0)` is `-inf` here and `+inf` in the evaluator.
                    let d = self.local(ValType::F64);
                    self.push(Ins::LocalSet(d));
                    self.push(Ins::LocalGet(d));
                    self.normalise();
                    self.push(Ins::LocalSet(d));
                    self.push(Ins::LocalGet(d));
                }
                self.push(match op {
                    Prim::Add => Ins::F64Add,
                    Prim::Sub => Ins::F64Sub,
                    Prim::Mul => Ins::F64Mul,
                    _ => Ins::F64Div,
                });
                Ok(Repr::Float)
            }
            Scalar::Int => {
                let rhs = self.local(ValType::I64);
                let lhs = self.local(ValType::I64);
                self.push(Ins::LocalSet(rhs));
                self.push(Ins::LocalSet(lhs));
                match op {
                    Prim::Add => self.checked_add(lhs, rhs, c.span),
                    Prim::Sub => self.checked_sub(lhs, rhs, c.span),
                    Prim::Mul => self.checked_mul(lhs, rhs, c.span),
                    Prim::Div | Prim::Rem => self.checked_div(op, lhs, rhs, c.span),
                    _ => unreachable!("only the five arithmetic operators reach here"),
                }
                Ok(Repr::Int)
            }
            Scalar::Bool => Err("arithmetic on a Bool".into()),
        }
    }

    /// `lhs + rhs`, overflowing to a trap rather than wrapping.
    ///
    /// The check is the classic sign test — `((a ^ r) & (b ^ r)) < 0` — because WebAssembly has no
    /// overflow flag and no widening multiply, so every one of these is arithmetic on the result.
    fn checked_add(&mut self, lhs: u32, rhs: u32, span: Span) {
        let r = self.local(ValType::I64);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Add);
        self.push(Ins::LocalSet(r));
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(r));
        self.push(Ins::I64Xor);
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::LocalGet(r));
        self.push(Ins::I64Xor);
        self.push(Ins::I64And);
        self.push(Ins::I64Const(0));
        self.push(Ins::I64LtS);
        self.push(Ins::If(None));
        self.trap(Trap::AddOverflow, span, None);
        self.push(Ins::End);
        self.push(Ins::LocalGet(r));
    }

    /// `lhs - rhs`: overflow iff `((a ^ b) & (a ^ r)) < 0`.
    fn checked_sub(&mut self, lhs: u32, rhs: u32, span: Span) {
        let r = self.local(ValType::I64);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Sub);
        self.push(Ins::LocalSet(r));
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Xor);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(r));
        self.push(Ins::I64Xor);
        self.push(Ins::I64And);
        self.push(Ins::I64Const(0));
        self.push(Ins::I64LtS);
        self.push(Ins::If(None));
        self.trap(Trap::SubOverflow, span, None);
        self.push(Ins::End);
        self.push(Ins::LocalGet(r));
    }

    /// `lhs * rhs`, in the three cases the division test cannot be asked in one.
    ///
    /// `r / a != b` is the portable check and it is undefined exactly where `i64.div_s` traps, so
    /// `a == 0` and `a == -1` are answered before it is asked: the first has no overflow and the
    /// second overflows only at `i64::MIN`.
    fn checked_mul(&mut self, lhs: u32, rhs: u32, span: Span) {
        let r = self.local(ValType::I64);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::I64Eqz);
        self.push(Ins::If(Some(ValType::I64)));
        self.push(Ins::I64Const(0));
        self.push(Ins::Else);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::I64Const(-1));
        self.push(Ins::I64Eq);
        self.push(Ins::If(Some(ValType::I64)));
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Const(i64::MIN));
        self.push(Ins::I64Eq);
        self.push(Ins::If(None));
        self.trap(Trap::MulOverflow, span, None);
        self.push(Ins::End);
        self.push(Ins::I64Const(0));
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Sub);
        self.push(Ins::Else);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Mul);
        self.push(Ins::LocalSet(r));
        self.push(Ins::LocalGet(r));
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::I64DivS);
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Ne);
        self.push(Ins::If(None));
        self.trap(Trap::MulOverflow, span, None);
        self.push(Ins::End);
        self.push(Ins::LocalGet(r));
        self.push(Ins::End);
        self.push(Ins::End);
    }

    /// `lhs / rhs` and `lhs % rhs`, with the guard WebAssembly's own trap would otherwise take out
    /// of the program's hands.
    fn checked_div(&mut self, op: Prim, lhs: u32, rhs: u32, span: Span) {
        let trap = if op == Prim::Div {
            Trap::DivOverflow
        } else {
            Trap::RemOverflow
        };
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Eqz);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::I64Const(i64::MIN));
        self.push(Ins::I64Eq);
        self.push(Ins::LocalGet(rhs));
        self.push(Ins::I64Const(-1));
        self.push(Ins::I64Eq);
        self.push(Ins::I32And);
        self.push(Ins::I32Or);
        self.push(Ins::If(None));
        self.trap(trap, span, None);
        self.push(Ins::End);
        self.push(Ins::LocalGet(lhs));
        self.push(Ins::LocalGet(rhs));
        self.push(if op == Prim::Div {
            Ins::I64DivS
        } else {
            Ins::I64RemS
        });
    }

    fn unary_arithmetic(&mut self, op: Prim, args: &[Core], c: &Core) -> Result<Repr, String> {
        let r = self.one(args)?;
        match r.machine() {
            Scalar::Float => {
                self.push(if op == Prim::Abs {
                    Ins::F64Abs
                } else {
                    Ins::F64Neg
                });
                Ok(Repr::Float)
            }
            Scalar::Int => {
                let x = self.local(ValType::I64);
                self.push(Ins::LocalSet(x));
                self.push(Ins::LocalGet(x));
                self.push(Ins::I64Const(i64::MIN));
                self.push(Ins::I64Eq);
                self.push(Ins::If(None));
                self.trap(
                    if op == Prim::Abs {
                        Trap::AbsOverflow
                    } else {
                        Trap::NegOverflow
                    },
                    c.span,
                    None,
                );
                self.push(Ins::End);
                if op == Prim::Abs {
                    self.push(Ins::LocalGet(x));
                    self.push(Ins::I64Const(0));
                    self.push(Ins::I64LtS);
                    self.push(Ins::If(Some(ValType::I64)));
                    self.push(Ins::I64Const(0));
                    self.push(Ins::LocalGet(x));
                    self.push(Ins::I64Sub);
                    self.push(Ins::Else);
                    self.push(Ins::LocalGet(x));
                    self.push(Ins::End);
                } else {
                    self.push(Ins::I64Const(0));
                    self.push(Ins::LocalGet(x));
                    self.push(Ins::I64Sub);
                }
                Ok(Repr::Int)
            }
            Scalar::Bool => Err("`abs`/`negate` of a Bool".into()),
        }
    }

    fn compare(&mut self, op: Prim, args: &[Core]) -> Result<Repr, String> {
        let [a, b] = args else {
            return Err("a comparison with the wrong arity".into());
        };
        let ra = self.expr(a, false)?;
        let rb = self.expr(b, false)?;
        if ra.machine() != rb.machine() {
            return Err("a comparison between two different machine types".into());
        }
        match ra.machine() {
            Scalar::Int => {
                self.push(match op {
                    Prim::Eq => Ins::I64Eq,
                    Prim::Ne => Ins::I64Ne,
                    Prim::Lt => Ins::I64LtS,
                    Prim::Le => Ins::I64LeS,
                    Prim::Gt => Ins::I64GtS,
                    _ => Ins::I64GeS,
                });
                Ok(Repr::Bool)
            }
            Scalar::Bool => match op {
                Prim::Eq => {
                    self.push(Ins::I32Eq);
                    Ok(Repr::Bool)
                }
                Prim::Ne => {
                    self.push(Ins::I32Ne);
                    Ok(Repr::Bool)
                }
                _ => Err("an ordering comparison on a Bool".into()),
            },
            Scalar::Float => {
                // Both operands as order keys, compared as *unsigned* integers — which is what
                // makes `-0.0 < 0.0` and NaN the maximum, as the evaluator's derived `Ord` says.
                let rhs = self.local(ValType::F64);
                self.push(Ins::LocalSet(rhs));
                self.order_key();
                let left = self.local(ValType::I64);
                self.push(Ins::LocalSet(left));
                self.push(Ins::LocalGet(rhs));
                self.order_key();
                let right = self.local(ValType::I64);
                self.push(Ins::LocalSet(right));
                self.push(Ins::LocalGet(left));
                self.push(Ins::LocalGet(right));
                self.push(match op {
                    Prim::Eq => Ins::I64Eq,
                    Prim::Ne => Ins::I64Ne,
                    Prim::Lt => Ins::I64LtU,
                    Prim::Le => Ins::I64LeU,
                    Prim::Gt => Ins::I64GtU,
                    _ => Ins::I64GeU,
                });
                Ok(Repr::Bool)
            }
        }
    }

    /// Normalise the `f64` on top of the stack: every NaN becomes one NaN, and `-0.0` becomes
    /// `0.0`.
    ///
    /// `Value::float` does this on every real it makes, and doing it after every operation costs
    /// more than it buys because every float operation maps zeros to zeros. It is needed where a
    /// signed zero or a NaN is *observable*: a comparison and a division's divisor (§93.3).
    fn normalise(&mut self) {
        let x = self.local(ValType::F64);
        self.push(Ins::LocalSet(x));
        self.push(Ins::LocalGet(x));
        self.push(Ins::LocalGet(x));
        self.push(Ins::F64Ne);
        self.push(Ins::If(Some(ValType::F64)));
        // The canonical NaN is `f64::NAN`'s bits, not the platform's default: on x86-64 `0.0 * inf`
        // has the sign bit set, which sorts *below* every number under the order key where
        // `f64::NAN` sorts above every one.
        self.push(Ins::F64Const(f64::NAN));
        self.push(Ins::Else);
        self.push(Ins::LocalGet(x));
        self.push(Ins::F64Const(0.0));
        self.push(Ins::F64Eq);
        self.push(Ins::If(Some(ValType::F64)));
        self.push(Ins::F64Const(0.0));
        self.push(Ins::Else);
        self.push(Ins::LocalGet(x));
        self.push(Ins::End);
        self.push(Ins::End);
    }

    /// The `f64` on top of the stack, normalised and turned into `beck_core`'s order key.
    fn order_key(&mut self) {
        self.normalise();
        self.push(Ins::I64ReinterpretF64);
        let bits = self.local(ValType::I64);
        self.push(Ins::LocalSet(bits));
        self.push(Ins::LocalGet(bits));
        self.push(Ins::I64Const(0));
        self.push(Ins::I64LtS);
        self.push(Ins::If(Some(ValType::I64)));
        // Negative: invert every bit.
        self.push(Ins::LocalGet(bits));
        self.push(Ins::I64Const(-1));
        self.push(Ins::I64Xor);
        self.push(Ins::Else);
        // Non-negative: flip the sign bit.
        self.push(Ins::LocalGet(bits));
        self.push(Ins::I64Const(i64::MIN));
        self.push(Ins::I64Xor);
        self.push(Ins::End);
    }
}

/// `Value::float`'s canonicalisation, so a literal here is the real the evaluator holds.
fn canonical(f: f64) -> f64 {
    Value::float(f).as_f64().unwrap_or(f)
}

/// `beck_core`'s order key for a literal, computed by `beck_core` rather than restated here.
fn order_key(f: f64) -> i64 {
    match Value::float(f) {
        Value::Float(key) => key as i64,
        _ => unreachable!("`Value::float` makes a `Float`"),
    }
}
