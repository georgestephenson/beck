//! `Core` → WebAssembly.
//!
//! # The subset
//!
//! A definition whose parameters and result have a [`beck_llvm::heap::Repr`] — `Int`, `Float`,
//! `Bool`, a `Str`, a `list`, a `Map`, an `Html`, an `Attr`, or a `model`, `union` or `newtype`
//! [`beck_llvm::heap`] can lay out — and whose body is built from constants, variables, `let`,
//! `if`, `match`, direct calls, record and variant construction, field reads, `with`, lambdas and
//! applications, and the arithmetic, comparison, logical, text, collection and view primitives.
//!
//! [`docs/103`](../../../../../docs/103-the-wasm-emitter-report.md) is the half of this that had no
//! heap in it, and [`docs/106`](../../../../../docs/106-the-wasm-heap-report.md) is the heap.
//! [`adr/0032`](../../../../../docs/adr/0032-the-webassembly-heap-is-the-arena-in-linear-memory.md)
//! is the memory model: the module's linear memory *is*
//! [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md)'s arena, so
//! a value that does not fit in a register is a byte offset into it and the host marshals with
//! `beck_llvm::heap`'s own encoder rather than with anything generated here.
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
//!   division's divisor, and **the way into a field**, which is where every real on the heap
//!   becomes the one the evaluator would hold.
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
use beck_core::ty::Ty;
use beck_core::Value;
use beck_diag::Span;
use beck_llvm::heap::{self, Heap, Repr};
use beck_llvm::{prim, Refusal, Scalar, Signature, Trap, Upcall};

use crate::binary::{Import, Ins, ModuleBuilder, ValType};
use crate::rt::{self, Fun, Helper, Registry, Rt};

/// The globals a compiled module exports, in index order.
///
/// The first three are a trap, which is three facts: which failure, where, and — for the three
/// `no match` codes — the value nothing matched. [`Trap::message`] takes exactly that payload, so
/// the host builds the evaluator's own sentence out of what is here.
pub const TRAP: u32 = 0;
pub const TRAP_SPAN: u32 = 1;
pub const TRAP_PAYLOAD: u32 = 2;
/// The arena's bump pointer. Exported because the host writes the call's arguments into the memory
/// and has to say where they end — which is the same "reset the arena to the end of the arguments"
/// the native worker does before every call.
pub const HEAP: u32 = 3;
/// The **name** of the type a [`Trap::Raised`] carries, as its offset in the literal pool.
///
/// A name and not a shape, because two instantiations of one generic type are two layouts and one
/// name, and it is the name a `try:` compares — `beck_core`'s own rule, since the atom is
/// `raises(T)`. It is the third word of the native backends' error cell, one global over.
pub const TRAP_TYPE: u32 = 4;

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
    /// What every object in this module looks like — and therefore how the host writes one into the
    /// memory and reads one back out of it. [`beck_llvm::heap`] is why this is one table and not
    /// four.
    pub heap: Heap,
}

impl Module {
    pub fn signature(&self, name: &str) -> Option<&Signature> {
        self.functions.iter().find(|f| &*f.name == name)
    }
}

/// Compile every definition of `program` this backend can compile.
///
/// Never fails, for [`beck_llvm::emit::module`]'s reason: a program with nothing this backend lays
/// out yields a module with no functions and a refusal per definition, and whether that is worth
/// running is the caller's decision.
pub fn module(program: &Program) -> Module {
    // Specialised first, so nothing below ever sees a type parameter — the same pass the other two
    // emitters run, because monomorphisation is a property of the language and not of a target.
    let mono = beck_llvm::mono::specialise(program);
    let program = &mono.program;
    let mut heap = Heap::new();
    heap::survey(program, &mut heap);
    let mut refusals: Vec<Refusal> = Vec::new();
    let mut sigs: BTreeMap<Arc<str>, Signature> = BTreeMap::new();

    // Round one: the signature. A definition whose parameters or result have no machine
    // representation cannot be called at all here, whatever its body is.
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
            // A throwaway registry: which definitions survive is what these rounds decide, so the
            // indices they hand out are about a module that may not exist.
            let mut scratch = Registry::new(0);
            let mut fun = Function::new(&sigs, &eligible, program, &mut heap, &mut scratch);
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

    // The one import, decided before any index is handed out: an imported function comes *before*
    // every defined one in the index space, so whether there is one has to be known first. It is
    // asked of the definitions the fixed point kept, so a module whose only user of `now()` was
    // refused declares nothing and instantiates against `{}`.
    let mut builder = ModuleBuilder::new();
    if order.iter().any(|n| asks(&program.defs[n].body)) {
        // The frame is `beck_llvm::Question`'s five fields and then a shape and a word per
        // argument, in that order — the same protocol the native worker writes down a pipe, with
        // the pipe taken out.
        let ty = builder.ty(
            vec![
                ValType::I32, // which question
                ValType::I32, // the span that asked
                ValType::I32, // the shape the answer is expected to have
                ValType::I32, // the shape a failure would carry
                ValType::I64, // the name of the type a failure raises, in the pool
                ValType::I64, // the first argument, and its shape
                ValType::I32,
                ValType::I64, // the second
                ValType::I32,
            ],
            vec![ValType::I64],
        );
        builder.imports.push(Import {
            module: "beck".into(),
            field: "upcall".into(),
            ty,
        });
    }
    let upcall = builder.defined_at().checked_sub(1);
    let mut registry = Registry::new(builder.defined_at());
    let mut indexed: BTreeMap<Arc<str>, Signature> = BTreeMap::new();
    for name in &order {
        let mut sig = sigs[name].clone();
        sig.index = registry.index(name);
        indexed.insert(name.clone(), sig);
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut lambdas: BTreeMap<u32, Fun> = BTreeMap::new();
    let mut bodies: BTreeMap<u32, Fun> = BTreeMap::new();
    for name in &order {
        let def = &program.defs[name];
        let mut fun = Function::new(&indexed, &eligible, program, &mut heap, &mut registry);
        fun.upcall = upcall;
        fun.spans = std::mem::take(&mut spans);
        let body = fun
            .emit(def)
            .expect("the fixed point already proved this emits");
        spans = std::mem::take(&mut fun.spans);
        for (rank, lam) in std::mem::take(&mut fun.lambdas) {
            lambdas.entry(rank).or_insert(lam);
        }
        let sig = &indexed[name];
        bodies.insert(sig.index, body);
    }

    // The lambdas the bodies built, then the runtime the whole of it asked for. A `lam` cannot
    // appear after this point: an application is a table lookup, and a table's arms are drawn from
    // the ranks that became code.
    let emitted: BTreeSet<u32> = lambdas.keys().copied().collect();
    for (rank, lam) in lambdas {
        let index = registry.helper(Helper::Lam(rank));
        bodies.insert(index, lam);
    }
    loop {
        let queue = registry.take_queue();
        if queue.is_empty() {
            break;
        }
        for h in queue {
            let index = registry.helper(h);
            if bodies.contains_key(&index) {
                continue;
            }
            let mut rt = Rt {
                reg: &mut registry,
                heap: &heap,
                types: &mut builder,
                emitted: &emitted,
                compiled: &indexed,
            };
            if let Some(fun) = rt.build(h) {
                bodies.insert(index, fun);
            }
        }
    }

    builder_setup(&mut builder, &heap);
    assemble(&mut builder, bodies, &registry, &order, &indexed);

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    refusals.sort_by(|a, b| a.name.cmp(&b.name));
    Module {
        wasm: builder.encode(),
        text: crate::text::render(&builder),
        functions,
        spans,
        refusals,
        heap,
    }
}

/// The memory, the globals, the literal pool and one table per closure family.
fn builder_setup(builder: &mut ModuleBuilder, heap: &Heap) {
    builder
        .globals
        .push(("beck_trap".into(), ValType::I32, true, 0));
    builder
        .globals
        .push(("beck_trap_span".into(), ValType::I32, true, 0));
    builder
        .globals
        .push(("beck_trap_payload".into(), ValType::I64, true, 0));

    // The arena begins where the literal pool ends, which is a compile-time constant. A host that
    // writes arguments into the memory moves it past them; a program that allocates nothing never
    // touches it.
    let pool_end = heap::FIRST + heap.pool_bytes();
    builder
        .globals
        .push(("beck_heap".into(), ValType::I64, true, pool_end as i64));
    builder
        .globals
        .push(("beck_trap_type".into(), ValType::I64, true, 0));

    let arena = !heap.is_empty() || heap.uses_text() || heap.uses_lists() || heap.uses_maps();
    if arena {
        let need = pool_end.div_ceil(rt::PAGE).max(1) as u32;
        builder.memory = Some((need, rt::MAX_PAGES));
        // The pool is written by `Heap` itself — the same bytes the native host puts at the front
        // of a request, so a literal's offset is one definition rather than two. Byte `i` of what
        // it answers is offset `i`, which is why the segment starts at zero.
        let (_, pool) = heap
            .encode_args(&[], &[])
            .expect("no arguments to encode, so nothing to fail on");
        if !pool.is_empty() {
            builder.data.push((0, pool));
        }
    }
}

/// Lay the bodies out in index order and write the exports.
fn assemble(
    builder: &mut ModuleBuilder,
    mut bodies: BTreeMap<u32, Fun>,
    registry: &Registry,
    order: &[Arc<str>],
    indexed: &BTreeMap<Arc<str>, Signature>,
) {
    let base = builder.defined_at();
    for (i, name) in registry.names.iter().enumerate() {
        let index = base + i as u32;
        let fun = bodies
            .remove(&index)
            .unwrap_or_else(|| panic!("`{name}` was named and never built"));
        let ty = builder.ty(fun.params.clone(), vec![fun.result]);
        assert!(
            !fun.code
                .iter()
                .any(|i| matches!(i, Ins::Call(u32::MAX) | Ins::ReturnCall(u32::MAX))),
            "`{name}` kept a call index from the fixed point, where nothing has one yet"
        );
        builder.funcs.push(ty);
        builder.names.push(name.clone());
        builder.bodies.push(fun.body());
    }
    for name in order {
        builder
            .exports
            .push((name.to_string(), indexed[name].index));
    }
}

/// The WebAssembly type a value is held in.
///
/// A `Bool` is an `i32` because that is what WebAssembly's own comparisons and `if` produce; an
/// offset is an `i64` whatever it points at, which is [`Repr::machine`]'s answer and therefore one
/// answer for three backends.
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
        Heap::crossing(r).map_err(|why| format!("parameter `{name}` is {why}"))?;
        heap.inbound(r)
            .map_err(|why| format!("parameter `{name}` is {why}"))?;
        params.push(r);
    }
    let ret = heap
        .repr(&def.ret, program)
        .map_err(|why| format!("returns {why}"))?;
    Heap::crossing(ret).map_err(|why| format!("returns {why}"))?;
    Ok(Signature {
        name: def.name.clone(),
        params,
        ret,
        index: u32::MAX,
    })
}

/// A value in a local: what the machine holds, and what the language says it is.
#[derive(Clone, Copy, Debug)]
pub struct Val {
    local: u32,
    ty: Repr,
}

/// One function being emitted.
pub struct Function<'a> {
    sigs: &'a BTreeMap<Arc<str>, Signature>,
    eligible: &'a BTreeSet<Arc<str>>,
    program: &'a Program,
    heap: &'a mut Heap,
    reg: &'a mut Registry,
    /// Every local beyond the parameters, in declaration order.
    locals: Vec<ValType>,
    params: usize,
    env: BTreeMap<VarId, Val>,
    code: Vec<Ins>,
    ret: Repr,
    /// The spans a trap in this module can name, shared across the module's functions.
    pub spans: Vec<Span>,
    /// The lambdas this body built, by rank, each already a function of its own — collected
    /// upwards because a `lam` is an *expression* and WebAssembly has no nested definitions.
    pub lambdas: BTreeMap<u32, Fun>,
    /// Which import asks the host a question, when this module has one.
    ///
    /// `None` during the fixed point, where which definitions survive is what is being decided and
    /// therefore whether the import exists at all is not yet known. Nothing those rounds emit is
    /// kept.
    upcall: Option<u32>,
    /// How many control frames are open here, so a `br` can be counted from the inside out.
    depth: u32,
    /// Where a failure inside the block being emitted goes, innermost last — as the [`depth`] the
    /// handler's block was opened at.
    ///
    /// Empty means the function's own exit. A `try:` pushes one while its block is emitted, so
    /// every check a call makes and every trap a primitive stores lands in the handler rather than
    /// leaving the function — which is the whole of what makes a handler lexical
    /// ([`docs/38`](../../../../../docs/38-literature-survey.md) §38.4): there is no dynamic search
    /// for who handles what, because the distance is decided where the block is written.
    handlers: Vec<u32>,
}

impl<'a> Function<'a> {
    fn new(
        sigs: &'a BTreeMap<Arc<str>, Signature>,
        eligible: &'a BTreeSet<Arc<str>>,
        program: &'a Program,
        heap: &'a mut Heap,
        reg: &'a mut Registry,
    ) -> Function<'a> {
        Function {
            sigs,
            eligible,
            program,
            heap,
            reg,
            locals: Vec::new(),
            params: 0,
            env: BTreeMap::new(),
            code: Vec::new(),
            ret: Repr::Int,
            spans: Vec::new(),
            lambdas: BTreeMap::new(),
            upcall: None,
            depth: 0,
            handlers: Vec::new(),
        }
    }

    fn emit(&mut self, def: &Def) -> Result<Fun, String> {
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
            self.env.insert(
                *var,
                Val {
                    local: i as u32,
                    ty: sig.params[i],
                },
            );
        }

        let got = self.expr(body, true)?;
        if got.machine() != sig.ret.machine() {
            return Err(format!(
                "returns {:?} where the signature says {:?}",
                got, sig.ret
            ));
        }
        Ok(Fun {
            params: sig.params.iter().map(|r| val(*r)).collect(),
            result: val(sig.ret),
            locals: std::mem::take(&mut self.locals),
            code: std::mem::take(&mut self.code),
        })
    }

    /// The body of one `lam`, as a function of its own.
    ///
    /// Two things separate it from [`Function::emit`]: the closure itself is the first parameter,
    /// and the captures are loaded off it into the environment before the body is compiled — so the
    /// body reads a capture exactly as it reads a parameter, and nothing below this knows the
    /// difference.
    fn lam_body(
        &mut self,
        params: &[VarId],
        body: &Core,
        fam: &heap::Family,
        captures: &[(VarId, Ty)],
    ) -> Result<Fun, String> {
        self.ret = fam.ret;
        self.params = 1 + fam.params.len();
        for (i, p) in fam.params.iter().enumerate() {
            self.env.insert(
                params[i],
                Val {
                    local: 1 + i as u32,
                    ty: *p,
                },
            );
        }
        for (i, (var, ty)) in captures.iter().enumerate() {
            let want = self
                .repr_of(ty)
                .map_err(|why| format!("captures a variable that is {why}"))?;
            let v = self.load_field(0, i + 1, want);
            self.env.insert(*var, v);
        }
        let got = self.expr(body, true)?;
        if got.machine() != fam.ret.machine() {
            return Err("a lambda whose body is not what its family answers".into());
        }
        let mut shape = vec![ValType::I64];
        shape.extend(fam.params.iter().map(|r| val(*r)));
        Ok(Fun {
            params: shape,
            result: val(fam.ret),
            locals: std::mem::take(&mut self.locals),
            code: std::mem::take(&mut self.code),
        })
    }

    fn push(&mut self, ins: Ins) {
        match ins {
            Ins::Block(_) | Ins::Loop(_) | Ins::If(_) => self.depth += 1,
            Ins::End => self.depth -= 1,
            _ => {}
        }
        self.code.push(ins);
    }

    fn all(&mut self, xs: impl IntoIterator<Item = Ins>) {
        for ins in xs {
            self.push(ins);
        }
    }

    /// Leave the computation: to the innermost `try:` if there is one, and out of the function if
    /// there is not.
    fn escape(&mut self) {
        match self.handlers.last().copied() {
            Some(h) => {
                let out = self.depth - h;
                self.push(Ins::Br(out));
            }
            None => {
                let z = zero(self.ret);
                self.push(z);
                self.push(Ins::Return);
            }
        }
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
        self.escape();
    }

    /// After a call that may have trapped: stop, leaving the reason where the callee put it.
    fn checked(&mut self) {
        self.all([Ins::GlobalGet(TRAP), Ins::If(None)]);
        self.escape();
        self.push(Ins::End);
    }

    fn repr_of(&mut self, ty: &Ty) -> Result<Repr, String> {
        self.heap.repr(ty, self.program)
    }

    /// The representation of an expression, as the language sees it.
    fn repr(&mut self, c: &Core) -> Result<Repr, String> {
        self.repr_of(&c.ty)
    }

    /// Emit `c` into a fresh local, so everything downstream refers to it by name.
    fn value(&mut self, c: &Core) -> Result<Val, String> {
        let ty = self.expr(c, false)?;
        let local = self.local(val(ty));
        self.push(Ins::LocalSet(local));
        Ok(Val { local, ty })
    }

    fn load(&mut self, v: Val) {
        self.push(Ins::LocalGet(v.local));
    }

    /// Emit `c`, leaving its value on the stack.
    ///
    /// `tail` says the value is the function's result, which is what makes a call a `return_call`
    /// and therefore a jump (§93.4).
    fn expr(&mut self, c: &Core, tail: bool) -> Result<Repr, String> {
        match &c.kind {
            CoreKind::Const(k) => self.constant(k, c),
            CoreKind::Var(v) => {
                let held = *self
                    .env
                    .get(v)
                    .ok_or_else(|| "a variable this emitter never bound".to_string())?;
                self.push(Ins::LocalGet(held.local));
                Ok(held.ty)
            }
            CoreKind::Let { var, value, body } => {
                let v = self.value(value)?;
                self.env.insert(*var, v);
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
            CoreKind::Prim { op, args } => {
                let v = self.prim(*op, args, c)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::Global(name) => {
                let v = self.named(name, &c.ty, c.span)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::Lam { params, body } => {
                let v = self.closure(params, body, &c.ty, c.span)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::Make {
                variant, fields, ..
            } => {
                let v = self.make(&c.ty, variant.as_deref(), fields, c.span)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::With { base, fields } => {
                let v = self.with(base, fields, c.span)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::Field { base, name } => {
                let v = self.field(base, name)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::ListLit(xs) => {
                let v = self.list_lit(&c.ty, xs, c.span)?;
                self.load(v);
                Ok(v.ty)
            }
            CoreKind::MapLit(kvs) => {
                let v = self.map_lit(&c.ty, kvs, c.span)?;
                self.load(v);
                Ok(v.ty)
            }
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
            Const::Str(s) => {
                // A literal is an offset into the pool the module's own data segment writes,
                // decided when the module is emitted.
                let at = self.heap.intern(s);
                let offset = self.heap.string_offset(at);
                self.push(Ins::I64Const(offset as i64));
                Ok(Repr::Str)
            }
            Const::Unit => {
                let _ = c;
                Err("unit has no machine representation here".into())
            }
        }
    }

    // ------------------------------------------------------------------------------ the heap

    /// One raw word of an object — a tag, or a field read for copying rather than for using.
    fn load_word(&mut self, off: u32, slot: usize) {
        self.push(Ins::LocalGet(off));
        self.push(Ins::I32WrapI64);
        self.push(Ins::I64Load(rt::at(slot)));
    }

    /// A field, as the value its [`Repr`] says it is, into a local.
    fn load_field(&mut self, off: u32, slot: usize, repr: Repr) -> Val {
        self.push(Ins::LocalGet(off));
        self.push(Ins::I32WrapI64);
        match repr.machine() {
            Scalar::Float => self.push(Ins::F64Load(rt::at(slot))),
            Scalar::Bool => {
                self.push(Ins::I64Load(rt::at(slot)));
                self.push(Ins::I64Eqz);
                self.push(Ins::I32Eqz);
            }
            Scalar::Int => self.push(Ins::I64Load(rt::at(slot))),
        }
        let local = self.local(val(repr));
        self.push(Ins::LocalSet(local));
        Val { local, ty: repr }
    }

    /// Put a value in a field.
    ///
    /// A real is **normalised** on the way in, which is the one place this backend's invariant
    /// about zeros and NaNs is paid for rather than argued away: a stored real is compared with
    /// another stored real by a generated comparison, is read back by the host's `Heap::decode`,
    /// and is part of what a record's `==` answers.
    fn store_field(&mut self, off: u32, slot: usize, v: Val) {
        self.push(Ins::LocalGet(off));
        self.push(Ins::I32WrapI64);
        match v.ty.machine() {
            Scalar::Float => {
                self.load(v);
                self.normalise();
                self.push(Ins::F64Store(rt::at(slot)));
            }
            Scalar::Bool => {
                self.load(v);
                self.push(Ins::I64ExtendI32U);
                self.push(Ins::I64Store(rt::at(slot)));
            }
            Scalar::Int => {
                self.load(v);
                self.push(Ins::I64Store(rt::at(slot)));
            }
        }
    }

    /// Put a raw word in a slot.
    fn store_word(&mut self, off: u32, slot: usize, word: i64) {
        self.push(Ins::LocalGet(off));
        self.push(Ins::I32WrapI64);
        self.push(Ins::I64Const(word));
        self.push(Ins::I64Store(rt::at(slot)));
    }

    /// Reserve `bytes` in the arena and answer the offset, or stop if there is no room.
    fn alloc_bytes(&mut self, bytes: u64, span: Span) -> Val {
        self.push(Ins::I64Const(bytes as i64));
        self.alloc(span)
    }

    /// The same, where the size is already on the stack.
    fn alloc(&mut self, span: Span) -> Val {
        let index = self.span_index(span);
        let alloc = self.reg.helper(Helper::Alloc);
        self.push(Ins::I32Const(index as i32));
        self.push(Ins::Call(alloc));
        let local = self.local(ValType::I64);
        self.push(Ins::LocalSet(local));
        self.checked();
        Val {
            local,
            ty: Repr::Int,
        }
    }

    /// Call a runtime function whose arguments are already on the stack, with a span, and check.
    fn rt_call(&mut self, h: Helper, span: Span, ty: Repr) -> Val {
        let index = self.span_index(span);
        let f = self.reg.helper(h);
        self.push(Ins::I32Const(index as i32));
        self.push(Ins::Call(f));
        let local = self.local(val(ty));
        self.push(Ins::LocalSet(local));
        self.checked();
        Val { local, ty }
    }

    /// Call a runtime function that cannot fail, whose arguments are already on the stack.
    fn rt_pure(&mut self, h: Helper, ty: Repr) -> Val {
        let f = self.reg.helper(h);
        self.push(Ins::Call(f));
        let local = self.local(val(ty));
        self.push(Ins::LocalSet(local));
        Val { local, ty }
    }

    /// A value as the raw word that carries it, for a slot that does not know what it holds.
    fn widen(&mut self, v: Val) {
        match v.ty.machine() {
            Scalar::Float => {
                self.load(v);
                self.normalise();
                self.push(Ins::I64ReinterpretF64);
            }
            Scalar::Bool => {
                self.load(v);
                self.push(Ins::I64ExtendI32U);
            }
            Scalar::Int => self.load(v),
        }
    }

    /// A raw word read back as the value its [`Repr`] says it is, into a local.
    fn narrow(&mut self, repr: Repr) -> Val {
        match repr.machine() {
            Scalar::Float => self.push(Ins::F64ReinterpretI64),
            Scalar::Bool => {
                self.push(Ins::I64Eqz);
                self.push(Ins::I32Eqz);
            }
            Scalar::Int => {}
        }
        let local = self.local(val(repr));
        self.push(Ins::LocalSet(local));
        Val { local, ty: repr }
    }

    /// `Point(x=1, y=2)`, `Some(v)`, `Id(3)` — one object, filled in.
    fn make(
        &mut self,
        ty: &Ty,
        variant: Option<&str>,
        fields: &[(Arc<str>, Core)],
        span: Span,
    ) -> Result<Val, String> {
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("builds a value that is {why}"))?;
        let Repr::Obj(at) = repr else {
            return Err(format!("builds a `{ty}`, which is not an object"));
        };
        let (tag, layout) = {
            let l = self.heap.layout(at);
            let tag = l
                .tag_of(variant)
                .ok_or_else(|| format!("builds a `{}` with no such variant", l.shown))?;
            (tag, l.variants[tag as usize].clone())
        };
        if fields.len() != layout.fields.len() {
            return Err(format!(
                "builds a `{ty}` with {} fields where the layout has {}",
                fields.len(),
                layout.fields.len()
            ));
        }
        // Evaluated in the order they are written, because a field expression can trap and which
        // trap the caller sees is part of what the evaluator answers.
        let mut placed = Vec::with_capacity(fields.len());
        for (name, expr) in fields {
            let v = self.value(expr)?;
            let (slot, want) = layout
                .slot(name)
                .ok_or_else(|| format!("`{ty}` has no field `{name}`"))?;
            if v.ty != want {
                return Err(format!(
                    "the field `{name}` of `{ty}` is the wrong type here"
                ));
            }
            placed.push((slot, v));
        }
        let off = self.alloc_bytes(layout.bytes(), span);
        self.store_word(off.local, 0, i64::from(tag));
        for (slot, v) in placed {
            self.store_field(off.local, slot, v);
        }
        Ok(Val {
            local: off.local,
            ty: repr,
        })
    }

    /// `p.x`.
    fn field(&mut self, base: &Core, name: &str) -> Result<Val, String> {
        let b = self.value(base)?;
        let Repr::Obj(at) = b.ty else {
            return Err(format!(
                "reads the field `{name}` of something that is not a record"
            ));
        };
        let (slot, repr) = {
            let layout = self.heap.layout(at);
            // A union's fields are read by matching it, never by naming one: which fields there
            // are is a question about the variant.
            if layout.tagged {
                return Err(format!(
                    "reads the field `{name}` of `{}`, which is a union",
                    layout.shown
                ));
            }
            layout.variants[0]
                .slot(name)
                .ok_or_else(|| format!("no field `{name}` on `{}`", layout.shown))?
        };
        Ok(self.load_field(b.local, slot, repr))
    }

    /// `p.with(x = 3)` — a new object with the old one's other fields.
    ///
    /// Always a fresh object. The evaluator rebuilds in place when the base is held by nobody else
    /// ([`docs/70`](../../../../../docs/70-the-evaluator-gets-fast-report.md)), and this cannot: an
    /// arena with no ownership in it cannot prove nobody else holds an offset.
    fn with(
        &mut self,
        base: &Core,
        fields: &[(Arc<str>, Core)],
        span: Span,
    ) -> Result<Val, String> {
        let b = self.value(base)?;
        let Repr::Obj(at) = b.ty else {
            return Err("updates something that is not a record".into());
        };
        let layout = {
            let l = self.heap.layout(at);
            if l.tagged {
                return Err(format!("updates `{}`, which is a union", l.shown));
            }
            l.variants[0].clone()
        };
        let mut placed = Vec::with_capacity(fields.len());
        for (name, expr) in fields {
            let v = self.value(expr)?;
            let (slot, want) = layout
                .slot(name)
                .ok_or_else(|| format!("no field `{name}` to update"))?;
            if v.ty != want {
                return Err(format!("the field `{name}` is the wrong type here"));
            }
            placed.push((slot, v));
        }
        let off = self.alloc_bytes(layout.bytes(), span);
        // Word for word, because a copy does not care what a field means — and then the named ones
        // are written over. One `memory.copy` rather than a load and a store per word, which is
        // the one bulk operation WebAssembly has.
        self.load(off);
        self.push(Ins::I32WrapI64);
        self.load(b);
        self.push(Ins::I32WrapI64);
        self.push(Ins::I32Const(layout.bytes() as i32));
        self.push(Ins::MemoryCopy);
        for (slot, v) in placed {
            self.store_field(off.local, slot, v);
        }
        Ok(Val {
            local: off.local,
            ty: b.ty,
        })
    }

    /// `[a, b, c]` — the block, and then the header that names it.
    fn list_lit(&mut self, ty: &Ty, xs: &[Core], span: Span) -> Result<Val, String> {
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("builds a value that is {why}"))?;
        let Repr::List(at) = repr else {
            return Err(format!("builds a `{ty}`, which is not a list"));
        };
        let element = self.heap.element(at);
        let mut vals = Vec::with_capacity(xs.len());
        for x in xs {
            let v = self.value(x)?;
            if v.ty != element {
                return Err(format!("an element of this `{ty}` is the wrong type"));
            }
            vals.push(v);
        }
        // The block and then the header, in that order, because the header holds the block's
        // offset — the same depth-first rule a record's fields follow.
        let n = xs.len() as u64;
        let data = self.alloc_bytes(heap::DATA_HEADER + n * heap::WORD, span);
        self.store_word(data.local, 0, n as i64);
        self.store_word(data.local, 1, n as i64);
        for (i, v) in vals.into_iter().enumerate() {
            self.store_field(data.local, i + 2, v);
        }
        let off = self.alloc_bytes(heap::LIST_HEADER, span);
        self.store_word(off.local, 0, n as i64);
        self.load(off);
        self.push(Ins::I32WrapI64);
        self.load(data);
        self.push(Ins::I64Store(rt::at(1)));
        Ok(Val {
            local: off.local,
            ty: repr,
        })
    }

    /// `{}` — and only `{}`.
    ///
    /// A map's entries are in key order and a literal's keys are expressions, so building a
    /// non-empty one means sorting at run time — for a form that is almost always written empty.
    fn map_lit(&mut self, ty: &Ty, kvs: &[(Core, Core)], _span: Span) -> Result<Val, String> {
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("builds a value that is {why}"))?;
        if !matches!(repr, Repr::Map(_)) {
            return Err(format!("builds a `{ty}`, which is not a map"));
        }
        if !kvs.is_empty() {
            return Err(
                "builds a map with entries in it, and their keys would have to be sorted at run \
                 time — only `{}` is compiled here"
                    .into(),
            );
        }
        // An empty map is the offset `0`, which is the one offset no live object has.
        self.push(Ins::I64Const(0));
        let local = self.local(ValType::I64);
        self.push(Ins::LocalSet(local));
        Ok(Val { local, ty: repr })
    }

    // --------------------------------------------------------------------------- calls

    /// A direct call to another compiled definition, or an application of a function value.
    fn call(&mut self, c: &Core, func: &Core, args: &[Core], tail: bool) -> Result<Repr, String> {
        let CoreKind::Global(name) = &func.kind else {
            return self.apply(func, args, tail);
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
        let mut vals = Vec::with_capacity(args.len());
        for (arg, want) in args.iter().zip(&sig.params) {
            let v = self.value(arg)?;
            if v.ty.machine() != want.machine() {
                return Err(format!("calls `{name}` with an argument of the wrong type"));
            }
            vals.push(v);
        }
        for v in vals {
            self.load(v);
        }
        // A tail call is a jump. Nothing is checked after one because there is no "after": the
        // callee's trap is the caller's, already in the globals.
        if tail && sig.ret.machine() == self.ret.machine() {
            self.push(Ins::ReturnCall(sig.index));
            return self.repr(c);
        }
        self.push(Ins::Call(sig.index));
        let held = self.local(val(sig.ret));
        self.push(Ins::LocalSet(held));
        self.checked();
        self.push(Ins::LocalGet(held));
        self.repr(c)
    }

    /// Applying a value rather than calling a name: the closure's rank, through the family's table.
    fn apply(&mut self, func: &Core, args: &[Core], tail: bool) -> Result<Repr, String> {
        let f = self.value(func)?;
        let Repr::Fn(family) = f.ty else {
            return Err("calls something that is neither a definition nor a function value".into());
        };
        let fam = self.heap.family(family).clone();
        if args.len() != fam.params.len() {
            return Err(format!(
                "applies a `{}` to {} arguments",
                fam.shown,
                args.len()
            ));
        }
        let mut vals = Vec::with_capacity(args.len());
        for (a, want) in args.iter().zip(&fam.params) {
            let v = self.value(a)?;
            if v.ty != *want {
                return Err(format!(
                    "an argument to a `{}` is the wrong type",
                    fam.shown
                ));
            }
            vals.push(v);
        }
        self.load(f);
        for v in vals {
            self.load(v);
        }
        let apply = self.reg.helper(Helper::Apply(family));
        if tail && fam.ret.machine() == self.ret.machine() {
            self.push(Ins::ReturnCall(apply));
            return Ok(fam.ret);
        }
        self.push(Ins::Call(apply));
        let held = self.local(val(fam.ret));
        self.push(Ins::LocalSet(held));
        self.checked();
        self.push(Ins::LocalGet(held));
        Ok(fam.ret)
    }

    /// `lambda x: …` — an object holding the lambda's rank and everything its body reads from here.
    fn closure(
        &mut self,
        params: &Arc<[VarId]>,
        body: &Arc<Core>,
        ty: &Ty,
        span: Span,
    ) -> Result<Val, String> {
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("builds a closure that is {why}"))?;
        let Repr::Fn(family) = repr else {
            return Err(format!("builds a closure whose type is `{ty}`"));
        };
        let rank = self
            .heap
            .rank_of(params, body.span.start)
            .ok_or("builds a closure from a `lam` the survey did not rank")?;
        let captures = self.heap.lam(rank).captures.clone();
        let mut vals = Vec::with_capacity(captures.len());
        for (var, ty) in &captures {
            let want = self
                .repr_of(ty)
                .map_err(|why| format!("captures a variable that is {why}"))?;
            let v = *self
                .env
                .get(var)
                .ok_or("captures a variable that is not bound here")?;
            if v.ty != want {
                return Err("captures a variable at a type this backend reads two ways".into());
            }
            vals.push(v);
        }
        self.lambda(rank, params, body, family)?;
        let off = self.alloc_bytes(heap::closure_bytes(captures.len() as u64), span);
        self.store_word(off.local, 0, i64::from(rank));
        for (i, v) in vals.into_iter().enumerate() {
            self.store_field(off.local, i + 1, v);
        }
        Ok(Val {
            local: off.local,
            ty: repr,
        })
    }

    /// A definition named where a value is expected — `map_list(xs, double)`.
    ///
    /// The closure carries nothing, because a definition closes over nothing. What the table holds
    /// at that rank is a one-instruction arm that drops the closure and jumps to the definition:
    /// a `call_indirect` table has one signature, so the arm exists here where the native
    /// backends' switch needed none.
    fn named(&mut self, name: &str, ty: &Ty, span: Span) -> Result<Val, String> {
        if !self.eligible.contains(name) {
            return Err(format!(
                "names `{name}` as a value, and it does not compile"
            ));
        }
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("names `{name}` as a value, which is {why}"))?;
        let Repr::Fn(family) = repr else {
            return Err(format!("names `{name}` as a value, whose type is `{ty}`"));
        };
        let def = self
            .program
            .defs
            .get(name)
            .ok_or_else(|| format!("names `{name}`, which this program does not define"))?;
        let CoreKind::Lam { params, body } = &def.body.kind else {
            return Err(format!("names `{name}`, whose body is not a lambda"));
        };
        let rank = self
            .heap
            .rank_of(params, body.span.start)
            .ok_or("names a definition the survey did not rank")?;
        let sig = self
            .sigs
            .get(name)
            .ok_or_else(|| format!("names `{name}`, which has no signature"))?
            .clone();
        let fam = self.heap.family(family);
        if sig.params != fam.params || sig.ret != fam.ret {
            return Err(format!(
                "names `{name}` as a `{}`, and that is not the shape it compiled to",
                fam.shown
            ));
        }
        let off = self.alloc_bytes(heap::closure_bytes(0), span);
        self.store_word(off.local, 0, i64::from(rank));
        Ok(Val {
            local: off.local,
            ty: repr,
        })
    }

    /// Emit the function a `lam` becomes, once per rank.
    fn lambda(
        &mut self,
        rank: u32,
        params: &Arc<[VarId]>,
        body: &Arc<Core>,
        family: u32,
    ) -> Result<(), String> {
        if self.lambdas.contains_key(&rank) {
            return Ok(());
        }
        let fam = self.heap.family(family).clone();
        if params.len() != fam.params.len() {
            return Err("builds a closure whose parameters are not the ones its type has".into());
        }
        let captures = self.heap.lam(rank).captures.clone();
        // Reserved before the body is emitted, so a `lam` reaching itself does not recurse here
        // forever — and overwritten with what comes back.
        self.lambdas.insert(rank, Fun::new(&[], ValType::I64));
        let mut inner = Function::new(self.sigs, self.eligible, self.program, self.heap, self.reg);
        inner.upcall = self.upcall;
        inner.spans = std::mem::take(&mut self.spans);
        let emitted = inner.lam_body(params, body, &fam, &captures);
        self.spans = std::mem::take(&mut inner.spans);
        let nested = std::mem::take(&mut inner.lambdas);
        for (r, fun) in nested {
            self.lambdas.entry(r).or_insert(fun);
        }
        self.lambdas.insert(rank, emitted?);
        Ok(())
    }

    // ---------------------------------------------------------------------------------- `match`

    /// A `match`, as one block per alternative inside a block that carries the answer.
    ///
    /// WebAssembly has no jumps, so the `fail` label the native backends branch to is a `block` an
    /// arm falls out of the end of — and the join a `phi` would be is the enclosing block's own
    /// result. That structure is what lets a test be *sequential*: a nested pattern reads a field
    /// only after the tag test that proved it is there, because a failed test has already left.
    fn match_(
        &mut self,
        c: &Core,
        scrutinee: &Core,
        arms: &[Arm],
        tail: bool,
    ) -> Result<Repr, String> {
        let v = self.value(scrutinee)?;
        let repr = self.repr(c)?;
        self.push(Ins::Block(Some(val(repr))));
        for arm in arms {
            for pattern in alternatives(&arm.pattern)? {
                self.push(Ins::Block(None));
                let mut undo: Vec<(VarId, Option<Val>)> = Vec::new();
                let probed = self.probe(&pattern, v, &mut undo);
                if let Err(e) = probed {
                    self.unbind(undo);
                    return Err(e);
                }
                if let Some(guard) = &arm.guard {
                    let g = self.expr(guard, false);
                    match g {
                        Ok(Repr::Bool) => {}
                        Ok(_) => {
                            self.unbind(undo);
                            return Err("a match guard is not a Bool".into());
                        }
                        Err(e) => {
                            self.unbind(undo);
                            return Err(e);
                        }
                    }
                    self.push(Ins::I32Eqz);
                    self.push(Ins::BrIf(0));
                }
                let body = self.expr(&arm.body, tail);
                self.unbind(undo);
                let got = body?;
                if got.machine() != repr.machine() {
                    return Err("an arm whose body has the wrong machine type".into());
                }
                self.push(Ins::Br(1));
                self.push(Ins::End);
            }
        }
        // Nothing matched. Unreachable — the checker proves a `match` exhaustive — and a code
        // rather than `unreachable` for the reason the other emitters give: a wrong exhaustiveness
        // check should be a message naming this trap, not a licence for a runtime to do anything at
        // all with the path it reached.
        let trap = match v.ty.machine() {
            Scalar::Int if v.ty == Repr::Int => Trap::NoMatchInt,
            Scalar::Float => Trap::NoMatchFloat,
            Scalar::Bool => Trap::NoMatchBool,
            Scalar::Int => Trap::NoMatchData,
        };
        let payload = self.payload_local(v);
        self.trap(trap, scrutinee.span, Some(payload));
        self.push(Ins::End);
        Ok(repr)
    }

    /// A local holding what a `no match` trap reports.
    fn payload_local(&mut self, v: Val) -> u32 {
        self.widen(v);
        let local = self.local(ValType::I64);
        self.push(Ins::LocalSet(local));
        local
    }

    fn unbind(&mut self, undo: Vec<(VarId, Option<Val>)>) {
        for (var, before) in undo.into_iter().rev() {
            match before {
                Some(v) => self.env.insert(var, v),
                None => self.env.remove(&var),
            };
        }
    }

    /// Test `pat` against `v`, binding what it names: fall through on a match, and `br 0` out of
    /// the enclosing arm block otherwise.
    fn probe(
        &mut self,
        pat: &Pattern,
        v: Val,
        undo: &mut Vec<(VarId, Option<Val>)>,
    ) -> Result<(), String> {
        match pat {
            Pattern::Wildcard => Ok(()),
            Pattern::Bind(var) => {
                undo.push((*var, self.env.insert(*var, v)));
                Ok(())
            }
            Pattern::At { var, inner } => {
                undo.push((*var, self.env.insert(*var, v)));
                self.probe(inner, v, undo)
            }
            Pattern::Const(k) => {
                self.equals_const(k, v)?;
                self.push(Ins::I32Eqz);
                self.push(Ins::BrIf(0));
                Ok(())
            }
            // Only the alternatives `alternatives` leaves whole reach here: every one is a test and
            // none of them binds, so the disjunction is one `i32` and one branch.
            Pattern::Or(alts) => {
                let mut emitted = 0;
                for alt in alts {
                    let Pattern::Const(k) = alt else {
                        return Err("an or-pattern that was not split".into());
                    };
                    self.equals_const(k, v)?;
                    emitted += 1;
                    if emitted > 1 {
                        self.push(Ins::I32Or);
                    }
                }
                if emitted == 0 {
                    return Err("an empty or-pattern".into());
                }
                self.push(Ins::I32Eqz);
                self.push(Ins::BrIf(0));
                Ok(())
            }
            Pattern::Ctor { variant, binds } => {
                let Repr::Obj(at) = v.ty else {
                    return Err(format!(
                        "matches the constructor `{variant}` against something that is not a record"
                    ));
                };
                let (tag, fields, tagged) = {
                    let layout = self.heap.layout(at);
                    let tag = layout.tag_of(Some(variant)).ok_or_else(|| {
                        format!("`{variant}` is not a variant of `{}`", layout.shown)
                    })?;
                    (tag, layout.variants[tag as usize].clone(), layout.tagged)
                };
                // A record has one variant, so its tag is known and there is nothing to test. A
                // union's is a word to load and compare.
                if tagged {
                    self.load_word(v.local, 0);
                    self.push(Ins::I64Const(i64::from(tag)));
                    self.push(Ins::I64Ne);
                    self.push(Ins::BrIf(0));
                }
                for (name, sub) in binds {
                    let (slot, repr) = fields
                        .slot(name)
                        .ok_or_else(|| format!("`{variant}` has no field `{name}` here"))?;
                    let field = self.load_field(v.local, slot, repr);
                    self.probe(sub, field, undo)?;
                }
                Ok(())
            }
            // `[]`, `[a, b]`, `[first, *rest]` — the length, then the fixed elements, then the
            // tail. The order matters: an element is read only after the length test has proved it
            // is there, so nothing here loads past the end of the block.
            Pattern::List { items, rest } => {
                let Repr::List(at) = v.ty else {
                    return Err(format!(
                        "matches a list pattern against {}",
                        self.heap.show(v.ty)
                    ));
                };
                let element = self.heap.element(at);
                let n = self.list_len(v);
                self.load(n);
                self.push(Ins::I64Const(items.len() as i64));
                // No tail binder means an exact length; a tail binder means "at least this many",
                // which is the evaluator's own rule.
                self.push(if rest.is_some() {
                    Ins::I64LtS
                } else {
                    Ins::I64Ne
                });
                self.push(Ins::BrIf(0));

                let data = self.list_data(v);
                for (i, sub) in items.iter().enumerate() {
                    self.push(Ins::LocalGet(data));
                    self.push(Ins::I64Load(rt::at(i)));
                    let x = self.narrow(element);
                    self.probe(sub, x, undo)?;
                }
                // The tail is a **fresh list**, copied, which is what the evaluator does — an
                // `Arc<Vec<_>>` cannot share a suffix either.
                if let Some(Some(var)) = rest {
                    self.load(v);
                    self.push(Ins::I64Const(items.len() as i64));
                    self.load(n);
                    self.push(Ins::I64Const(items.len() as i64));
                    self.push(Ins::I64Sub);
                    let tail = self.rt_call(Helper::ListCopy, Span::NONE, v.ty);
                    undo.push((*var, self.env.insert(*var, tail)));
                }
                Ok(())
            }
        }
    }

    /// `v == k`, as the *language's* equality rather than the machine's, leaving an `i32`.
    fn equals_const(&mut self, k: &Const, v: Val) -> Result<(), String> {
        match (k, v.ty) {
            (Const::Int(n), Repr::Int) => {
                self.load(v);
                self.push(Ins::I64Const(*n));
                self.push(Ins::I64Eq);
                Ok(())
            }
            (Const::Bool(b), Repr::Bool) => {
                self.load(v);
                self.push(Ins::I32Const(i32::from(*b)));
                self.push(Ins::I32Eq);
                Ok(())
            }
            (Const::Float(f), Repr::Float) => {
                // Both sides as order keys: a pattern `case -0.0:` and a scrutinee `0.0` are the
                // same value to the evaluator, and two bit patterns to `f64.eq`.
                self.load(v);
                self.order_key();
                self.push(Ins::I64Const(order_key(*f)));
                self.push(Ins::I64Eq);
                Ok(())
            }
            (Const::Str(s), Repr::Str) => {
                let at = self.heap.intern(s);
                let offset = self.heap.string_offset(at);
                self.load(v);
                self.push(Ins::I64Const(offset as i64));
                let cmp = self.reg.helper(Helper::StrCmp);
                self.push(Ins::Call(cmp));
                self.push(Ins::I64Eqz);
                Ok(())
            }
            _ => Err("a literal pattern whose type is not the scrutinee's".into()),
        }
    }

    // ------------------------------------------------------------------------------ primitives

    /// Normalise the `f64` on top of the stack: every NaN becomes one NaN, and `-0.0` becomes
    /// `0.0`.
    fn normalise(&mut self) {
        let x = self.local(ValType::F64);
        self.push(Ins::LocalTee(x));
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
        self.push(Ins::LocalTee(bits));
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

/// The most patterns one arm may be split into.
const MAX_ALTERNATIVES: usize = 16;

/// One arm's pattern as the patterns that have to be tried in turn.
///
/// An or-pattern of plain constants is left whole, because [`Function::probe`] can test it with one
/// `or` and it binds nothing. Anything else is **split**: two alternatives that take a value apart
/// bind the same names to different words, so one block reached from both would need a join per
/// binder, and copying the arm is the same behaviour with nothing to get wrong.
fn alternatives(pat: &Pattern) -> Result<Vec<Pattern>, String> {
    if testable(pat) {
        return Ok(vec![pat.clone()]);
    }
    let mut out = Vec::new();
    expand(pat, &mut out)?;
    Ok(out)
}

/// Whether a pattern is one this can test without splitting: a constant, or an or of constants.
fn testable(pat: &Pattern) -> bool {
    match pat {
        Pattern::Const(_) => true,
        Pattern::Or(alts) => alts.iter().all(|a| matches!(a, Pattern::Const(_))),
        _ => false,
    }
}

fn expand(pat: &Pattern, out: &mut Vec<Pattern>) -> Result<(), String> {
    if out.len() > MAX_ALTERNATIVES {
        return Err(format!(
            "an or-pattern with more than {MAX_ALTERNATIVES} alternatives once expanded"
        ));
    }
    match pat {
        Pattern::Or(alts) => {
            for alt in alts {
                expand(alt, out)?;
            }
            Ok(())
        }
        other => {
            out.push(other.clone());
            Ok(())
        }
    }
}

// -------------------------------------------------------------------------------------------
// The primitives
// -------------------------------------------------------------------------------------------

impl Function<'_> {
    /// Insist a value has an order before a comparison over it is demanded.
    ///
    /// Asked at the demand rather than where a comparison is written, and it recurses, because a
    /// record is compared field by field: a `model Card { body: Html }` has no order for the same
    /// reason its field has none, and finding that out while the module was being laid out would be
    /// a missing function rather than a refusal with a definition's name on it.
    fn wants(&mut self, r: Repr) -> Result<(), String> {
        self.heap.ordered(r)
    }

    fn list_arg(&self, v: Val, op: Prim) -> Result<u32, String> {
        match v.ty {
            Repr::List(at) => Ok(at),
            _ => Err(format!("`{}` on something that is not a list", op.name())),
        }
    }

    fn map_arg(&self, v: Val, op: Prim) -> Result<u32, String> {
        match v.ty {
            Repr::Map(at) => Ok(at),
            _ => Err(format!("`{}` on something that is not a Map", op.name())),
        }
    }

    fn text_arg(&self, v: Val, op: Prim) -> Result<(), String> {
        if v.ty == Repr::Str {
            Ok(())
        } else {
            Err(format!("`{}` on something that is not a Str", op.name()))
        }
    }

    /// How many elements, which is the list header's first word.
    fn list_len(&mut self, xs: Val) -> Val {
        self.load_field(xs.local, 0, Repr::Int)
    }

    /// The i32 address of a list's elements: through the block, which is the one load the
    /// header's indirection costs.
    ///
    /// Taken once and reused, because a WebAssembly memory never moves what is already in it — it
    /// only grows. That is the one place this target is *simpler* than the native backends, where
    /// a data pointer is re-read after every allocation.
    fn list_data(&mut self, xs: Val) -> u32 {
        self.load_word(xs.local, 1);
        self.push(Ins::I32WrapI64);
        self.push(Ins::I32Const(heap::DATA_HEADER as i32));
        self.push(Ins::I32Add);
        let local = self.local(ValType::I32);
        self.push(Ins::LocalSet(local));
        local
    }

    /// The i32 address of element `index` of the run at `data`.
    fn elem_addr(&mut self, data: u32, index: Val) {
        self.push(Ins::LocalGet(data));
        self.load(index);
        self.push(Ins::I32WrapI64);
        self.push(Ins::I32Const(heap::WORD as i32));
        self.push(Ins::I32Mul);
        self.push(Ins::I32Add);
    }

    /// One word of a `Str`'s header: slot 0 is its bytes and slot 1 its characters.
    fn text_word(&mut self, s: Val, slot: usize) -> Val {
        self.load_field(s.local, slot, Repr::Int)
    }

    /// The layout of the `Option[T]` a primitive answers with: the repr, its two tags, which word
    /// `Some`'s payload goes in, and how much to allocate for either.
    fn option_of(&mut self, ty: &Ty, want: Repr) -> Result<(Repr, u32, u32, usize, u64), String> {
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("answers with a value that is {why}"))?;
        let Repr::Obj(at) = repr else {
            return Err(format!("answers with `{ty}`, which is not an object"));
        };
        let layout = self.heap.layout(at);
        let some = layout
            .tag_of(Some("Some"))
            .ok_or_else(|| format!("`{}` has no `Some`", layout.shown))?;
        let none = layout
            .tag_of(Some("None"))
            .ok_or_else(|| format!("`{}` has no `None`", layout.shown))?;
        let (slot, carried) = layout.variants[some as usize]
            .slot("value")
            .ok_or_else(|| format!("`{}`'s `Some` has no `value`", layout.shown))?;
        if carried != want {
            return Err(format!(
                "`{}` does not carry what it is given",
                layout.shown
            ));
        }
        let bytes = layout
            .variants
            .iter()
            .map(|v| v.bytes())
            .max()
            .unwrap_or(heap::WORD);
        Ok((repr, some, none, slot, bytes))
    }

    /// The `Some` tag, the slot its payload is in, and what that payload is — for *consuming* one.
    fn option_taken(&mut self, repr: Repr) -> Result<(u32, usize, Repr), String> {
        let Repr::Obj(at) = repr else {
            return Err("an `Option` operation on something that is not an object".into());
        };
        let layout = self.heap.layout(at);
        let some = layout
            .tag_of(Some("Some"))
            .ok_or_else(|| format!("`{}` has no `Some`", layout.shown))?;
        let (slot, payload) = layout.variants[some as usize]
            .slot("value")
            .ok_or_else(|| format!("`{}`'s `Some` has no `value`", layout.shown))?;
        Ok((some, slot, payload))
    }

    /// `Some(value = found)` when `found` is not `-1`, and `None()` when it is.
    ///
    /// No branch: `Some` is two words and `None` is one, so allocating the larger and choosing the
    /// tag with a `select` answers both — the host reads a variant's own fields and nothing else,
    /// so the word a `None` leaves behind is never looked at.
    fn some_or_none(&mut self, ty: &Ty, found: Val, span: Span) -> Result<Val, String> {
        let (repr, some, none, slot, bytes) = self.option_of(ty, Repr::Int)?;
        let off = self.alloc_bytes(bytes, span);
        self.load(off);
        self.push(Ins::I32WrapI64);
        // `select` takes its *first* operand when the condition holds, so the missing case is
        // written first.
        self.push(Ins::I64Const(i64::from(none)));
        self.push(Ins::I64Const(i64::from(some)));
        self.load(found);
        self.push(Ins::I64Const(0));
        self.push(Ins::I64LtS);
        self.push(Ins::Select);
        self.push(Ins::I64Store(rt::at(0)));
        self.store_field(off.local, slot, found);
        Ok(Val {
            local: off.local,
            ty: repr,
        })
    }

    fn prim(&mut self, op: Prim, args: &[Core], c: &Core) -> Result<Val, String> {
        let (ty, span) = (&c.ty, c.span);
        // Before the arguments, because this one's first argument is a *block* and evaluating it
        // here would run it outside the protection it exists to have.
        if op == Prim::Try {
            return self.try_(args, ty, span);
        }
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
        let same = |vals: &[Val]| -> Result<Repr, String> {
            if vals[0].ty == vals[1].ty {
                Ok(vals[0].ty)
            } else {
                Err(format!("`{}` mixes two types", op.name()))
            }
        };

        // The four questions the host answers, and the fifteen the runtime library computes: a
        // WebAssembly module reaches neither, and both would be a second implementation of
        // something whose whole value is that there is one.
        if let Some(ask) = Upcall::of(op) {
            return self.upcall(ask, &vals, ty, span);
        }
        if prim::op_of(op).is_some() || prim::float_op_of(op).is_some() {
            return Err(format!(
                "`{}` is a call into the runtime library, which a WebAssembly module reaches only \
                 as an import the bundle does not carry yet",
                op.name()
            ));
        }

        match op {
            Prim::Add | Prim::Sub | Prim::Mul | Prim::Div | Prim::Rem => {
                arity(2)?;
                let r = same(&vals)?;
                if r == Repr::Str && op == Prim::Add {
                    // `+` on two strings is the one arithmetic operator text has.
                    self.load(vals[0]);
                    self.load(vals[1]);
                    return Ok(self.rt_call(Helper::StrConcat, span, Repr::Str));
                }
                self.arithmetic(op, vals[0], vals[1], span)
            }
            Prim::Neg | Prim::Abs => {
                arity(1)?;
                self.unary_arithmetic(op, vals[0], span)
            }
            Prim::Sqrt => {
                arity(1)?;
                if vals[0].ty != Repr::Float {
                    return Err("`sqrt` of something that is not a Float".into());
                }
                // IEEE-754 pins `sqrt` to one correctly-rounded answer, so `f64.sqrt` is the same
                // number the evaluator's is — which is why `sin` and `cos` are refused above and
                // this one is not.
                self.load(vals[0]);
                self.push(Ins::F64Sqrt);
                Ok(self.held(Repr::Float))
            }
            Prim::Trunc => {
                arity(1)?;
                if vals[0].ty != Repr::Float {
                    return Err("`trunc` of something that is not a Float".into());
                }
                self.load(vals[0]);
                self.push(Ins::I64TruncSatF64S);
                Ok(self.held(Repr::Int))
            }
            Prim::ToFloat => {
                arity(1)?;
                if vals[0].ty != Repr::Int {
                    return Err("`float` of something that is not an Int".into());
                }
                self.load(vals[0]);
                self.push(Ins::F64ConvertI64S);
                Ok(self.held(Repr::Float))
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                arity(2)?;
                same(&vals)?;
                self.compare(op, vals[0], vals[1])
            }
            Prim::Not => {
                arity(1)?;
                if vals[0].ty != Repr::Bool {
                    return Err("`not` of something that is not a Bool".into());
                }
                self.load(vals[0]);
                self.push(Ins::I32Eqz);
                Ok(self.held(Repr::Bool))
            }
            // Reached only by a *bare reference* — `and` passed as a value — because the checker
            // rewrites the operator form into an `if` so it short-circuits. A function value's
            // arguments are already evaluated, so this one is strict and the evaluator's is too.
            Prim::And | Prim::Or => {
                arity(2)?;
                if vals.iter().any(|v| v.ty != Repr::Bool) {
                    return Err("`and`/`or` of something that is not a Bool".into());
                }
                self.load(vals[0]);
                self.load(vals[1]);
                self.push(if op == Prim::And {
                    Ins::I32And
                } else {
                    Ins::I32Or
                });
                Ok(self.held(Repr::Bool))
            }

            // -- text ---------------------------------------------------------------------
            Prim::StrLen | Prim::StrIsEmpty => {
                arity(1)?;
                self.text_arg(vals[0], op)?;
                // Both counts are in the header, so both of these are a load: `str_len` is `O(1)`
                // in the evaluator, and a backend that counted here would make the loop that walks
                // a string by index quadratic in one implementation and not the other.
                let n = self.text_word(vals[0], usize::from(op == Prim::StrLen));
                if op == Prim::StrLen {
                    return Ok(n);
                }
                self.load(n);
                self.push(Ins::I64Eqz);
                Ok(self.held(Repr::Bool))
            }
            Prim::StrSlice => {
                arity(3)?;
                self.text_arg(vals[0], op)?;
                for v in &vals[1..] {
                    if v.ty != Repr::Int {
                        return Err("`str_slice` takes two Int positions".into());
                    }
                }
                self.load(vals[0]);
                self.load(vals[1]);
                self.load(vals[2]);
                Ok(self.rt_call(Helper::StrSlice, span, Repr::Str))
            }
            Prim::StrTrim => {
                arity(1)?;
                self.text_arg(vals[0], op)?;
                self.load(vals[0]);
                Ok(self.rt_call(Helper::StrTrim, span, Repr::Str))
            }
            Prim::StrSplit | Prim::StrChars => {
                // One function, because the evaluator answers characters for an empty separator —
                // so `str_chars(s)` *is* `str_split(s, "")`.
                self.text_arg(vals[0], op)?;
                let sep = if op == Prim::StrChars {
                    arity(1)?;
                    // The offset `0`, which is never a live object — so `str_chars` needs no
                    // literal and the pool stays a function of the program's own text.
                    self.push(Ins::I64Const(0));
                    self.held(Repr::Str)
                } else {
                    arity(2)?;
                    self.text_arg(vals[1], op)?;
                    vals[1]
                };
                let at = self.heap.word_of(Repr::Str);
                self.load(vals[0]);
                self.load(sep);
                Ok(self.rt_call(Helper::StrSplit, span, Repr::List(at)))
            }
            Prim::StrContains | Prim::StrStartsWith | Prim::StrEndsWith => {
                arity(2)?;
                self.text_arg(vals[0], op)?;
                self.text_arg(vals[1], op)?;
                self.text_search(op, vals[0], vals[1])
            }
            Prim::StrIndexOf => {
                arity(2)?;
                self.text_arg(vals[0], op)?;
                self.text_arg(vals[1], op)?;
                self.index_of(ty, vals[0], vals[1], span)
            }
            Prim::StrRepeat => {
                arity(2)?;
                self.text_arg(vals[0], op)?;
                if vals[1].ty != Repr::Int {
                    return Err("`str_repeat` takes an Int count".into());
                }
                self.load(vals[0]);
                self.load(vals[1]);
                Ok(self.rt_call(Helper::StrRepeat, span, Repr::Str))
            }
            Prim::StrJoin => {
                arity(2)?;
                let Repr::List(at) = vals[0].ty else {
                    return Err("`str_join` on something that is not a list".into());
                };
                if self.heap.element(at) != Repr::Str {
                    return Err(
                        "`str_join` over a list of something other than text, and the evaluator \
                         renders those through `display` rather than joining them"
                            .into(),
                    );
                }
                self.text_arg(vals[1], op)?;
                self.load(vals[0]);
                self.load(vals[1]);
                Ok(self.rt_call(Helper::StrJoin, span, Repr::Str))
            }
            Prim::ToStr => {
                arity(1)?;
                match vals[0].ty {
                    Repr::Str => Ok(vals[0]),
                    Repr::Int => {
                        self.load(vals[0]);
                        Ok(self.rt_call(Helper::StrFromInt, span, Repr::Str))
                    }
                    // Two literals from the pool, which is what `Value::display` answers.
                    Repr::Bool => {
                        let t = self.heap.intern("true");
                        let t = self.heap.string_offset(t);
                        let f = self.heap.intern("false");
                        let f = self.heap.string_offset(f);
                        self.push(Ins::I64Const(t as i64));
                        self.push(Ins::I64Const(f as i64));
                        self.load(vals[0]);
                        self.push(Ins::Select);
                        Ok(self.held(Repr::Str))
                    }
                    other => Err(format!(
                        "`str` of {}, whose rendering is not a decimal this backend can reproduce \
                         — a real's shortest round-trip form is an algorithm rather than a loop",
                        self.heap.show(other)
                    )),
                }
            }

            // -- lists --------------------------------------------------------------------
            Prim::ListLen | Prim::ListIsEmpty => {
                arity(1)?;
                self.list_arg(vals[0], op)?;
                let n = self.list_len(vals[0]);
                if op == Prim::ListLen {
                    return Ok(n);
                }
                self.load(n);
                self.push(Ins::I64Eqz);
                Ok(self.held(Repr::Bool))
            }
            Prim::ListGet => {
                arity(2)?;
                let at = self.list_arg(vals[0], op)?;
                if vals[1].ty != Repr::Int {
                    return Err("`list_get` takes an Int index".into());
                }
                self.list_get(ty, at, vals[0], vals[1], span)
            }
            Prim::ListAppend => {
                arity(2)?;
                let at = self.list_arg(vals[0], op)?;
                let element = self.heap.element(at);
                if vals[1].ty != element {
                    return Err("`list_append` of an element of another type".into());
                }
                self.load(vals[0]);
                self.widen(vals[1]);
                Ok(self.rt_call(Helper::ListAppend, span, vals[0].ty))
            }
            Prim::ListContains | Prim::ListIndexOf => {
                arity(2)?;
                let at = self.list_arg(vals[0], op)?;
                let element = self.heap.element(at);
                if vals[1].ty != element {
                    return Err(format!(
                        "`{}` against an element of another type",
                        op.name()
                    ));
                }
                self.wants(Repr::List(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                self.load(vals[0]);
                self.widen(vals[1]);
                let found = self.rt_pure(Helper::ListFind(at), Repr::Int);
                if op == Prim::ListContains {
                    self.load(found);
                    self.push(Ins::I64Const(0));
                    self.push(Ins::I64GeS);
                    return Ok(self.held(Repr::Bool));
                }
                self.some_or_none(ty, found, span)
            }
            Prim::ListSlice | Prim::ListTake | Prim::ListDrop => {
                arity(if op == Prim::ListSlice { 3 } else { 2 })?;
                self.list_arg(vals[0], op)?;
                for v in &vals[1..] {
                    if v.ty != Repr::Int {
                        return Err(format!("`{}` takes Int positions", op.name()));
                    }
                }
                self.list_range(op, &vals, span)
            }
            Prim::ListReverse => {
                arity(1)?;
                self.list_arg(vals[0], op)?;
                self.load(vals[0]);
                Ok(self.rt_call(Helper::ListReverse, span, vals[0].ty))
            }
            // A list of lists into one list. Not a growth: the total is a sum over the outer list's
            // header words, so the allocation happens once and after it.
            Prim::ConcatLists => {
                arity(1)?;
                let outer = self.list_arg(vals[0], op)?;
                let Repr::List(inner) = self.heap.element(outer) else {
                    return Err("`concat_lists` on something that is not a list of lists".into());
                };
                self.load(vals[0]);
                Ok(self.rt_call(Helper::ListConcat, span, Repr::List(inner)))
            }
            Prim::MapList | Prim::FilterList => {
                arity(2)?;
                let at = self.list_arg(vals[0], op)?;
                let element = self.heap.element(at);
                let fam = self.function_arg(vals[1], op, &[element])?;
                let family = self.heap.family(fam).clone();
                if op == Prim::MapList {
                    let repr = self
                        .repr_of(ty)
                        .map_err(|why| format!("`{}` answers with {why}", op.name()))?;
                    let Repr::List(out) = repr else {
                        return Err(format!("`{}` answers with a `{ty}`", op.name()));
                    };
                    if self.heap.element(out) != family.ret {
                        return Err(format!(
                            "`{}` answers a list of something other than what its function does",
                            op.name()
                        ));
                    }
                    return self.map_loop(vals[0], vals[1], fam, out, span);
                }
                if family.ret != Repr::Bool {
                    return Err("`filter_list`'s function does not answer a Bool".into());
                }
                self.filter_loop(vals[0], vals[1], fam, span)
            }
            Prim::ListFold => {
                arity(3)?;
                let at = self.list_arg(vals[0], op)?;
                let element = self.heap.element(at);
                let acc = vals[1].ty;
                let fam = self.function_arg(vals[2], op, &[acc, element])?;
                if self.heap.family(fam).ret != acc {
                    return Err(
                        "`list_fold`'s function answers something other than the accumulator it \
                         is given"
                            .into(),
                    );
                }
                self.fold_loop(vals[0], vals[1], vals[2], fam, span)
            }
            Prim::ListAll | Prim::ListAny => {
                arity(2)?;
                let at = self.list_arg(vals[0], op)?;
                let element = self.heap.element(at);
                let fam = self.function_arg(vals[1], op, &[element])?;
                if self.heap.family(fam).ret != Repr::Bool {
                    return Err(format!("`{}`'s function does not answer a Bool", op.name()));
                }
                self.every_loop(vals[0], vals[1], fam, op == Prim::ListAny, span)
            }
            // Decorate, sort, undecorate — and the keys are words like any others, so what compares
            // two of them is the function a list's element comparison already is.
            Prim::SortBy => {
                arity(2)?;
                let at = self.list_arg(vals[0], op)?;
                let element = self.heap.element(at);
                let fam = self.function_arg(vals[1], op, &[element])?;
                let key = self.heap.family(fam).ret;
                // Interned here rather than in the survey: the keys are not a list any program
                // wrote, so nothing else would have asked for their comparison.
                let keys = self.heap.word_of(key);
                self.wants(Repr::List(keys))
                    .map_err(|why| format!("`{}` by a key that is {why}", op.name()))?;
                self.sort_loop(vals[0], vals[1], fam, keys, span)
            }

            // -- maps ---------------------------------------------------------------------
            Prim::MapLen => {
                arity(1)?;
                self.map_arg(vals[0], op)?;
                Ok(self.map_size(vals[0]))
            }
            Prim::MapGet | Prim::MapContains => {
                arity(2)?;
                let at = self.map_arg(vals[0], op)?;
                let (k, v) = self.heap.entry(at);
                let key = self.heap.element(k);
                if vals[1].ty != key {
                    return Err(format!("`{}` with a key of another type", op.name()));
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                self.load(vals[0]);
                self.widen(vals[1]);
                let found = self.rt_pure(Helper::MapFind(at), Repr::Int);
                if op == Prim::MapContains {
                    self.load(found);
                    self.push(Ins::I64Eqz);
                    self.push(Ins::I32Eqz);
                    return Ok(self.held(Repr::Bool));
                }
                let value = self.heap.element(v);
                self.map_get(ty, found, value, span)
            }
            Prim::MapInsert | Prim::MapRemove => {
                arity(if op == Prim::MapInsert { 3 } else { 2 })?;
                let at = self.map_arg(vals[0], op)?;
                let (k, v) = self.heap.entry(at);
                let key = self.heap.element(k);
                if vals[1].ty != key {
                    return Err(format!("`{}` with a key of another type", op.name()));
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                if op == Prim::MapInsert {
                    if vals[2].ty != self.heap.element(v) {
                        return Err("`map_insert` with a value of another type".into());
                    }
                    self.load(vals[0]);
                    self.widen(vals[1]);
                    self.widen(vals[2]);
                    return Ok(self.rt_call(Helper::MapIns(at), span, vals[0].ty));
                }
                self.load(vals[0]);
                self.widen(vals[1]);
                Ok(self.rt_call(Helper::MapDel(at), span, vals[0].ty))
            }
            Prim::MapMerge => {
                arity(2)?;
                let at = self.map_arg(vals[0], op)?;
                if vals[1].ty != vals[0].ty {
                    return Err("`map_merge` of two maps of different types".into());
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                self.load(vals[0]);
                self.load(vals[1]);
                Ok(self.rt_call(Helper::MapMerge(at), span, vals[0].ty))
            }
            Prim::MapKeys | Prim::MapValues => {
                arity(1)?;
                let at = self.map_arg(vals[0], op)?;
                let repr = self
                    .repr_of(ty)
                    .map_err(|why| format!("`{}` answers with {why}", op.name()))?;
                let Repr::List(_) = repr else {
                    return Err(format!("`{}` answers with a `{ty}`", op.name()));
                };
                let _ = at;
                // One walk for both, told which word to take: the two differ by eight bytes and
                // nothing else, and neither compares a key — so this is not a per-map function.
                self.load(vals[0]);
                self.push(Ins::I64Const(if op == Prim::MapKeys {
                    heap::NODE_KEY as i64
                } else {
                    heap::NODE_VALUE as i64
                }));
                Ok(self.rt_call(Helper::MapRun, span, repr))
            }

            // -- `Option` -----------------------------------------------------------------
            Prim::OptionIsSome => {
                arity(1)?;
                let (some, ..) = self.option_taken(vals[0].ty)?;
                self.load_word(vals[0].local, 0);
                self.push(Ins::I64Const(i64::from(some)));
                self.push(Ins::I64Eq);
                Ok(self.held(Repr::Bool))
            }
            Prim::OptionUnwrapOr => {
                arity(2)?;
                let (some, slot, payload) = self.option_taken(vals[0].ty)?;
                if vals[1].ty != payload {
                    return Err("`unwrap_or`'s fallback is not what the `Option` carries".into());
                }
                // The *address*, not the value: a `None` the host wrote is one word long, because
                // `encode` allocates the variant's own size — so reading the payload slot
                // unconditionally can read past the end of what was written.
                self.load_word(vals[0].local, 0);
                self.push(Ins::I64Const(i64::from(some)));
                self.push(Ins::I64Eq);
                let is_some = self.local(ValType::I32);
                self.push(Ins::LocalSet(is_some));
                self.load(vals[0]);
                self.push(Ins::I32WrapI64);
                self.push(Ins::I32Const(rt::at(slot) as i32));
                self.push(Ins::I32Const(0));
                self.push(Ins::LocalGet(is_some));
                self.push(Ins::Select);
                self.push(Ins::I32Add);
                self.push(Ins::I64Load(0));
                let held = self.narrow(payload);
                self.load(held);
                self.load(vals[1]);
                self.push(Ins::LocalGet(is_some));
                self.push(Ins::Select);
                Ok(self.held(payload))
            }

            // `raise e` — the one failure that is not a fault, so it carries a value.
            //
            // Two words in the arena (the value's shape and its word, which is a view node's
            // deferred value one subsystem over) and the type *name* in a global of its own.
            Prim::Raise => {
                arity(1)?;
                let Repr::Obj(at) = vals[0].ty else {
                    return Err(format!(
                        "raises {}, and a raised value must have a declared type",
                        self.heap.show(vals[0].ty)
                    ));
                };
                let name = self.heap.layout(at).name.to_string();
                let shape = self.heap.word_of(vals[0].ty);
                let pair = self.alloc_bytes(heap::RAISED_WORDS * heap::WORD, span);
                self.store_word(pair.local, 0, i64::from(shape));
                self.store_field(pair.local, 1, vals[0]);
                let named = self.heap.intern(&name);
                let named = self.heap.string_offset(named);
                self.push(Ins::I64Const(named as i64));
                self.push(Ins::GlobalSet(TRAP_TYPE));
                self.trap(Trap::Raised, span, Some(pair.local));
                // Unreachable: `trap` left for the handler. `raise` has no type of its own, so the
                // checker gave the expression whatever the context wanted, and nothing reads this.
                let want = self.repr_of(ty).unwrap_or(Repr::Int);
                let z = zero(want);
                self.push(z);
                Ok(self.held(want))
            }

            // -- the page -----------------------------------------------------------------
            Prim::HtmlEl => {
                arity(3)?;
                let (attrs, children) = self.view_lists()?;
                if vals[0].ty != Repr::Str {
                    return Err("`html_el` with a tag that is not text".into());
                }
                if vals[1].ty != Repr::List(attrs) {
                    return Err("`html_el` with attributes that are not a `list[Attr]`".into());
                }
                if vals[2].ty != Repr::List(children) {
                    return Err("`html_el` with children that are not a `list[Html]`".into());
                }
                let off = self.alloc_bytes(heap::NODE_WORDS * heap::WORD, span);
                self.store_word(off.local, 0, heap::HTML_ELEMENT as i64);
                for (slot, v) in vals.iter().enumerate() {
                    self.store_field(off.local, slot + 1, *v);
                }
                Ok(Val {
                    local: off.local,
                    ty: Repr::Html,
                })
            }
            Prim::HtmlText => {
                arity(1)?;
                // A child that is already a tree is spliced rather than rendered, which is the
                // evaluator's own arm — and here it needs no node at all.
                if vals[0].ty == Repr::Html {
                    return Ok(vals[0]);
                }
                self.node(Repr::Html, heap::HTML_TEXT, None, Some(vals[0]), span)
            }
            Prim::HtmlAttr | Prim::HtmlOn => {
                arity(2)?;
                if vals[0].ty != Repr::Str {
                    return Err(format!("`{}` with a name that is not text", op.name()));
                }
                let tag = if op == Prim::HtmlAttr {
                    heap::ATTR_PLAIN
                } else {
                    heap::ATTR_ON
                };
                self.node(Repr::Attr, tag, Some(vals[0]), Some(vals[1]), span)
            }
            Prim::HtmlKey => {
                arity(1)?;
                self.node(Repr::Attr, heap::ATTR_KEY, None, Some(vals[0]), span)
            }
            other => Err(refusal(other)),
        }
    }

    /// Whatever is on the stack, put in a fresh local of `r`'s machine type.
    fn held(&mut self, r: Repr) -> Val {
        let local = self.local(val(r));
        self.push(Ins::LocalSet(local));
        Val { local, ty: r }
    }

    /// The `list[Attr]` and `list[Html]` reprs, resolving `Html` first if nothing has.
    fn view_lists(&mut self) -> Result<(u32, u32), String> {
        self.repr_of(&Ty::html())?;
        self.heap
            .html_lists()
            .ok_or_else(|| "a view node in a module with no view in it".to_string())
    }

    /// One view node or attribute: four words, a tag, a name for the two shapes that have one, and
    /// a deferred value for the four that have one.
    ///
    /// The unused words are **written**, not left as they were found: the used prefix of the memory
    /// is what the host reads back, and a word nobody reads is still a byte that would differ
    /// between two runs of the same program.
    fn node(
        &mut self,
        ty: Repr,
        tag: u64,
        name: Option<Val>,
        deferred: Option<Val>,
        span: Span,
    ) -> Result<Val, String> {
        if let Some(v) = deferred {
            // The host reads this word back as a `Value`, so what a closure gets everywhere else it
            // gets here: a shape with no form the host can read is not one to defer.
            Heap::crossing(v.ty)
                .map_err(|why| format!("puts {why} in a page, and a page is read by the host"))?;
        }
        self.view_lists()?;
        let off = self.alloc_bytes(heap::NODE_WORDS * heap::WORD, span);
        self.store_word(off.local, 0, tag as i64);
        match name {
            Some(v) => self.store_field(off.local, 1, v),
            None => self.store_word(off.local, 1, 0),
        }
        match deferred {
            Some(v) => {
                let at = self.heap.word_of(v.ty);
                self.store_word(off.local, heap::DEFERRED, i64::from(at));
                self.store_field(off.local, heap::DEFERRED + 1, v);
            }
            None => {
                self.store_word(off.local, heap::DEFERRED, 0);
                self.store_word(off.local, heap::DEFERRED + 1, 0);
            }
        }
        Ok(Val {
            local: off.local,
            ty,
        })
    }

    /// Insist an argument is a closure of the shape this primitive applies it at.
    fn function_arg(&mut self, v: Val, op: Prim, want: &[Repr]) -> Result<u32, String> {
        let Repr::Fn(fam) = v.ty else {
            return Err(format!(
                "`{}` on something that is not a function",
                op.name()
            ));
        };
        if self.heap.family(fam).params != want {
            return Err(format!(
                "`{}` applies its function to something it does not take",
                op.name()
            ));
        }
        Ok(fam)
    }
}

// -------------------------------------------------------------------------------------------
// Arithmetic, comparison, and the shapes a collection primitive is built out of
// -------------------------------------------------------------------------------------------

impl Function<'_> {
    fn arithmetic(&mut self, op: Prim, a: Val, b: Val, span: Span) -> Result<Val, String> {
        match a.ty.machine() {
            Scalar::Float => {
                if op == Prim::Rem {
                    return Err("`%` on reals, which WebAssembly has no instruction for".into());
                }
                self.load(a);
                self.load(b);
                if op == Prim::Div {
                    // The divisor is one of §93.3's three places a real is normalised: without it
                    // `1.0 / (0.0 * -1.0)` is `-inf` here and `+inf` in the evaluator.
                    self.normalise();
                }
                self.push(match op {
                    Prim::Add => Ins::F64Add,
                    Prim::Sub => Ins::F64Sub,
                    Prim::Mul => Ins::F64Mul,
                    Prim::Div => Ins::F64Div,
                    _ => unreachable!("only the five arithmetic operators reach here"),
                });
                Ok(self.held(Repr::Float))
            }
            Scalar::Int if a.ty == Repr::Int => {
                match op {
                    Prim::Add => self.checked_add(a, b, span),
                    Prim::Sub => self.checked_sub(a, b, span),
                    Prim::Mul => self.checked_mul(a, b, span),
                    _ => self.checked_div(op, a, b, span),
                }
                Ok(self.held(Repr::Int))
            }
            _ => Err(format!("`{}` on a value that is not a number", op.name())),
        }
    }

    /// `a + b`, overflowing to a trap rather than wrapping.
    ///
    /// The check is the classic sign test — `((a ^ r) & (b ^ r)) < 0` — because WebAssembly has no
    /// overflow flag and no widening multiply, so every one of these is arithmetic on the result.
    fn checked_add(&mut self, a: Val, b: Val, span: Span) {
        let r = self.local(ValType::I64);
        self.load(a);
        self.load(b);
        self.all([Ins::I64Add, Ins::LocalSet(r)]);
        self.load(a);
        self.all([Ins::LocalGet(r), Ins::I64Xor]);
        self.load(b);
        self.all([
            Ins::LocalGet(r),
            Ins::I64Xor,
            Ins::I64And,
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::If(None),
        ]);
        self.trap(Trap::AddOverflow, span, None);
        self.all([Ins::End, Ins::LocalGet(r)]);
    }

    /// `a - b`: overflow iff `((a ^ b) & (a ^ r)) < 0`.
    fn checked_sub(&mut self, a: Val, b: Val, span: Span) {
        let r = self.local(ValType::I64);
        self.load(a);
        self.load(b);
        self.all([Ins::I64Sub, Ins::LocalSet(r)]);
        self.load(a);
        self.load(b);
        self.push(Ins::I64Xor);
        self.load(a);
        self.all([
            Ins::LocalGet(r),
            Ins::I64Xor,
            Ins::I64And,
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::If(None),
        ]);
        self.trap(Trap::SubOverflow, span, None);
        self.all([Ins::End, Ins::LocalGet(r)]);
    }

    /// `a * b`, in the three cases the division test cannot be asked in one.
    ///
    /// `r / a != b` is the portable check and it is undefined exactly where `i64.div_s` traps, so
    /// `a == 0` and `a == -1` are answered before it is asked.
    fn checked_mul(&mut self, a: Val, b: Val, span: Span) {
        let r = self.local(ValType::I64);
        self.load(a);
        self.all([
            Ins::I64Eqz,
            Ins::If(Some(ValType::I64)),
            Ins::I64Const(0),
            Ins::Else,
        ]);
        self.load(a);
        self.all([Ins::I64Const(-1), Ins::I64Eq, Ins::If(Some(ValType::I64))]);
        self.load(b);
        self.all([Ins::I64Const(i64::MIN), Ins::I64Eq, Ins::If(None)]);
        self.trap(Trap::MulOverflow, span, None);
        self.all([Ins::End, Ins::I64Const(0)]);
        self.load(b);
        self.all([Ins::I64Sub, Ins::Else]);
        self.load(a);
        self.load(b);
        self.all([Ins::I64Mul, Ins::LocalSet(r), Ins::LocalGet(r)]);
        self.load(a);
        self.push(Ins::I64DivS);
        self.load(b);
        self.all([Ins::I64Ne, Ins::If(None)]);
        self.trap(Trap::MulOverflow, span, None);
        self.all([Ins::End, Ins::LocalGet(r), Ins::End, Ins::End]);
    }

    /// `a / b` and `a % b`, with the guard WebAssembly's own trap would otherwise take out of the
    /// program's hands.
    fn checked_div(&mut self, op: Prim, a: Val, b: Val, span: Span) {
        let trap = if op == Prim::Div {
            Trap::DivOverflow
        } else {
            Trap::RemOverflow
        };
        self.load(b);
        self.push(Ins::I64Eqz);
        self.load(a);
        self.all([Ins::I64Const(i64::MIN), Ins::I64Eq]);
        self.load(b);
        self.all([
            Ins::I64Const(-1),
            Ins::I64Eq,
            Ins::I32And,
            Ins::I32Or,
            Ins::If(None),
        ]);
        self.trap(trap, span, None);
        self.push(Ins::End);
        self.load(a);
        self.load(b);
        self.push(if op == Prim::Div {
            Ins::I64DivS
        } else {
            Ins::I64RemS
        });
    }

    fn unary_arithmetic(&mut self, op: Prim, a: Val, span: Span) -> Result<Val, String> {
        match a.ty.machine() {
            Scalar::Float => {
                self.load(a);
                self.push(if op == Prim::Abs {
                    Ins::F64Abs
                } else {
                    Ins::F64Neg
                });
                Ok(self.held(Repr::Float))
            }
            Scalar::Int if a.ty == Repr::Int => {
                self.load(a);
                self.all([Ins::I64Const(i64::MIN), Ins::I64Eq, Ins::If(None)]);
                let payload = self.payload_local(a);
                self.trap(
                    if op == Prim::Abs {
                        Trap::AbsOverflow
                    } else {
                        Trap::NegOverflow
                    },
                    span,
                    Some(payload),
                );
                self.push(Ins::End);
                if op == Prim::Abs {
                    self.load(a);
                    self.all([
                        Ins::I64Const(0),
                        Ins::I64LtS,
                        Ins::If(Some(ValType::I64)),
                        Ins::I64Const(0),
                    ]);
                    self.load(a);
                    self.all([Ins::I64Sub, Ins::Else]);
                    self.load(a);
                    self.push(Ins::End);
                } else {
                    self.push(Ins::I64Const(0));
                    self.load(a);
                    self.push(Ins::I64Sub);
                }
                Ok(self.held(Repr::Int))
            }
            _ => Err(format!("`{}` on a value that is not a number", op.name())),
        }
    }

    /// The six operators, over whatever [`Repr::order`] says this repr compares by.
    fn compare(&mut self, op: Prim, a: Val, b: Val) -> Result<Val, String> {
        // A Bool is an `i32` and compares unsigned, so `false < true` — which is the ordering
        // `Value`'s derived `Ord` gives.
        if a.ty == Repr::Bool {
            self.load(a);
            self.load(b);
            self.push(match op {
                Prim::Eq => Ins::I32Eq,
                Prim::Ne => Ins::I32Ne,
                Prim::Lt => Ins::I32LtU,
                Prim::Le => Ins::I32LeU,
                Prim::Gt => Ins::I32GtU,
                _ => Ins::I32GeU,
            });
            return Ok(self.held(Repr::Bool));
        }
        let signed = match a.ty.order() {
            heap::Order::Key => {
                self.load(a);
                self.order_key();
                let ka = self.local(ValType::I64);
                self.push(Ins::LocalSet(ka));
                self.load(b);
                self.order_key();
                let kb = self.local(ValType::I64);
                self.push(Ins::LocalSet(kb));
                self.all([Ins::LocalGet(ka), Ins::LocalGet(kb)]);
                false
            }
            heap::Order::Words { signed } => {
                self.load(a);
                self.load(b);
                signed
            }
            // Nothing to compare with: `Repr::order` names the reason and this is where a program
            // that asked hears it.
            heap::Order::Absent(why) => {
                return Err(format!("compares {}, which is {why}", self.heap.show(a.ty)))
            }
            // A reference decides through the three-way comparison for whatever it refers to. The
            // helper is derived from the `Repr` exhaustively, so a new reference kind is a compile
            // error rather than a `_` arm that swallows it.
            heap::Order::Call(_) => {
                self.wants(a.ty)
                    .map_err(|why| format!("compares {}, which is {why}", self.heap.show(a.ty)))?;
                self.load(a);
                self.load(b);
                let cmp = self.reg.helper(rt::cmp_helper(a.ty));
                self.push(Ins::Call(cmp));
                self.push(Ins::I64Const(0));
                true
            }
        };
        self.push(match (op, signed) {
            (Prim::Eq, _) => Ins::I64Eq,
            (Prim::Ne, _) => Ins::I64Ne,
            (Prim::Lt, true) => Ins::I64LtS,
            (Prim::Le, true) => Ins::I64LeS,
            (Prim::Gt, true) => Ins::I64GtS,
            (Prim::Ge, true) => Ins::I64GeS,
            (Prim::Lt, false) => Ins::I64LtU,
            (Prim::Le, false) => Ins::I64LeU,
            (Prim::Gt, false) => Ins::I64GtU,
            _ => Ins::I64GeU,
        });
        Ok(self.held(Repr::Bool))
    }

    /// `str_contains`, `str_starts_with` and `str_ends_with`.
    fn text_search(&mut self, op: Prim, hay: Val, needle: Val) -> Result<Val, String> {
        if op == Prim::StrContains {
            self.load(hay);
            self.load(needle);
            self.push(Ins::I64Const(0));
            let found = self.rt_pure(Helper::StrFindAt, Repr::Int);
            self.load(found);
            self.push(Ins::I64Const(0));
            self.push(Ins::I64GeS);
            return Ok(self.held(Repr::Bool));
        }
        let lh = self.text_word(hay, 0);
        let ln = self.text_word(needle, 0);
        // A needle longer than the haystack is refused by the length test rather than by not being
        // looked at, and the offset is clamped so the comparison never reads past the end.
        self.load(ln);
        self.load(lh);
        self.push(Ins::I64LeU);
        let fits = self.local(ValType::I32);
        self.push(Ins::LocalSet(fits));
        if op == Prim::StrStartsWith {
            self.push(Ins::I64Const(0));
        } else {
            self.load(lh);
            self.load(ln);
            self.push(Ins::I64Sub);
            self.push(Ins::I64Const(0));
            self.push(Ins::LocalGet(fits));
            self.push(Ins::Select);
        }
        let from = self.local(ValType::I64);
        self.push(Ins::LocalSet(from));
        self.load(hay);
        self.load(needle);
        self.push(Ins::LocalGet(from));
        let same = self.rt_pure(Helper::StrMatchAt, Repr::Int);
        self.load(same);
        self.push(Ins::I64Eqz);
        self.push(Ins::I32Eqz);
        self.push(Ins::LocalGet(fits));
        self.push(Ins::I32And);
        Ok(self.held(Repr::Bool))
    }

    /// `str_index_of` — a byte search, a byte-to-character conversion, and an `Option`.
    fn index_of(&mut self, ty: &Ty, hay: Val, needle: Val, span: Span) -> Result<Val, String> {
        self.load(hay);
        self.load(needle);
        self.push(Ins::I64Const(0));
        let found = self.rt_pure(Helper::StrFindAt, Repr::Int);
        self.load(found);
        self.push(Ins::I64Const(0));
        self.push(Ins::I64LtS);
        let missing = self.local(ValType::I32);
        self.push(Ins::LocalSet(missing));
        // Clamped before the conversion rather than after it: `-1` is not a byte offset, and a walk
        // that started there would read backwards off the front of the string.
        self.load(hay);
        self.push(Ins::I64Const(0));
        self.load(found);
        self.push(Ins::LocalGet(missing));
        self.push(Ins::Select);
        let index = self.rt_pure(Helper::StrCharAt, Repr::Int);
        self.push(Ins::I64Const(-1));
        self.load(index);
        self.push(Ins::LocalGet(missing));
        self.push(Ins::Select);
        let answer = self.held(Repr::Int);
        self.some_or_none(ty, answer, span)
    }

    /// `list_get(xs, i)` — an `Option[T]`, and **no branch**.
    ///
    /// The trick is the address rather than the value: an index outside the list would read a word
    /// that may be past the end of the memory, so the address is a `select` between the element's
    /// and the list's own **header**, which is always there.
    fn list_get(
        &mut self,
        ty: &Ty,
        at: u32,
        xs: Val,
        index: Val,
        span: Span,
    ) -> Result<Val, String> {
        let element = self.heap.element(at);
        let (option, some, none, slot, bytes) = self.option_of(ty, element)?;
        let n = self.list_len(xs);
        self.load(index);
        self.push(Ins::I64Const(0));
        self.push(Ins::I64GeS);
        self.load(index);
        self.load(n);
        self.push(Ins::I64LtS);
        self.push(Ins::I32And);
        let inside = self.local(ValType::I32);
        self.push(Ins::LocalSet(inside));
        self.load(index);
        self.push(Ins::I64Const(0));
        self.push(Ins::LocalGet(inside));
        self.push(Ins::Select);
        let safe = self.held(Repr::Int);
        let data = self.list_data(xs);
        self.elem_addr(data, safe);
        self.load(xs);
        self.push(Ins::I32WrapI64);
        self.push(Ins::LocalGet(inside));
        self.push(Ins::Select);
        self.push(Ins::I64Load(0));
        let word = self.held(Repr::Int);

        let off = self.alloc_bytes(bytes, span);
        self.load(off);
        self.push(Ins::I32WrapI64);
        self.push(Ins::I64Const(i64::from(some)));
        self.push(Ins::I64Const(i64::from(none)));
        self.push(Ins::LocalGet(inside));
        self.push(Ins::Select);
        self.push(Ins::I64Store(rt::at(0)));
        self.store_field(off.local, slot, word);
        Ok(Val {
            local: off.local,
            ty: option,
        })
    }

    /// `list_slice`, `list_take` and `list_drop`: one clamped range, one copy.
    fn list_range(&mut self, op: Prim, vals: &[Val], span: Span) -> Result<Val, String> {
        let n = self.list_len(vals[0]);
        let clamp = |me: &mut Self, v: Val| -> Val {
            me.push(Ins::I64Const(0));
            me.load(v);
            me.load(v);
            me.push(Ins::I64Const(0));
            me.push(Ins::I64LtS);
            me.push(Ins::Select);
            me.held(Repr::Int)
        };
        let (from, count) = match op {
            Prim::ListSlice => (clamp(self, vals[1]), clamp(self, vals[2])),
            Prim::ListTake => {
                self.push(Ins::I64Const(0));
                let zero = self.held(Repr::Int);
                (zero, clamp(self, vals[1]))
            }
            _ => (clamp(self, vals[1]), n),
        };
        // `from` first, then how many are left after it, then how many were asked for.
        self.load(n);
        self.load(from);
        self.load(from);
        self.load(n);
        self.push(Ins::I64GtU);
        self.push(Ins::Select);
        let start = self.held(Repr::Int);
        self.load(n);
        self.load(start);
        self.push(Ins::I64Sub);
        let left = self.held(Repr::Int);
        self.load(left);
        self.load(count);
        self.load(count);
        self.load(left);
        self.push(Ins::I64GtU);
        self.push(Ins::Select);
        let take = self.held(Repr::Int);

        self.load(vals[0]);
        self.load(start);
        self.load(take);
        Ok(self.rt_call(Helper::ListCopy, span, vals[0].ty))
    }

    /// How many entries a map has, which is its root's size word.
    ///
    /// No branch for the empty map: an empty map is the offset `0`, and the word at offset `0` is
    /// the one [`heap::FIRST`] reserves — never written, therefore always zero.
    fn map_size(&mut self, m: Val) -> Val {
        self.load_field(m.local, 0, Repr::Int)
    }

    /// `map_get` — an `Option[V]` from the node a search answered, and **no branch**.
    ///
    /// The search answers a node or `0`, and reading the value word of node `0` reads the reserved
    /// word and the first literal rather than past the end. The `None` tag means nobody looks.
    fn map_get(&mut self, ty: &Ty, found: Val, value: Repr, span: Span) -> Result<Val, String> {
        let (option, some, none, slot, bytes) = self.option_of(ty, value)?;
        self.load(found);
        self.push(Ins::I64Eqz);
        self.push(Ins::I32Eqz);
        let there = self.local(ValType::I32);
        self.push(Ins::LocalSet(there));
        let word = self.load_field(found.local, heap::NODE_VALUE, Repr::Int);
        let off = self.alloc_bytes(bytes, span);
        self.load(off);
        self.push(Ins::I32WrapI64);
        self.push(Ins::I64Const(i64::from(some)));
        self.push(Ins::I64Const(i64::from(none)));
        self.push(Ins::LocalGet(there));
        self.push(Ins::Select);
        self.push(Ins::I64Store(rt::at(0)));
        self.store_field(off.local, slot, word);
        Ok(Val {
            local: off.local,
            ty: option,
        })
    }
}

/// Why a primitive this backend does not compile is not compiled.
fn refusal(op: Prim) -> String {
    let why = match op {
        Prim::ListZip => "answers with a list of pairs, and there is no pair type to lay out",
        Prim::ListFlatMap => {
            "answers a list whose length is the sum of the lists its function answers, which is \
             growing a list under another name"
        }
        Prim::JsonParse => {
            "answers a `Json`, whose object variant is a `Map` this module lays out — so a parser \
             would have to build a balanced tree in a shape only the emitter knows"
        }
        Prim::JsonRender => {
            "reads a `Json`, and what a value of a declared type looks like in the memory is this \
             module's layout rather than a library's"
        }
        _ => {
            return format!(
                "`{}` is not one of the primitives this backend compiles",
                op.name()
            )
        }
    };
    format!("`{}` {why}", op.name())
}

// -------------------------------------------------------------------------------------------
// The higher-order list primitives
// -------------------------------------------------------------------------------------------

/// Each of these is one loop, written where it is used rather than as a function of its own.
///
/// [`beck_llvm`] generates a `beck.list.map.{family}` per shape because its emitter writes text
/// into a second builder; here the loop is ordinary emitted code, so inlining costs nothing and
/// saves a signature per shape — and the element conversions, the trap check after every
/// application and the bounds are the same either way.
impl Function<'_> {
    /// Apply a closure to values already in locals, checking the trap the closure may have left.
    fn apply_vals(&mut self, fam: u32, f: Val, args: &[Val]) -> Val {
        self.load(f);
        for a in args {
            self.load(*a);
        }
        let apply = self.reg.helper(Helper::Apply(fam));
        self.push(Ins::Call(apply));
        let ret = self.heap.family(fam).ret;
        let held = self.local(val(ret));
        self.push(Ins::LocalSet(held));
        // A closure can trap, and a loop that carried on would run the rest of the program's
        // iterations after the failure the caller is about to report.
        self.checked();
        Val {
            local: held,
            ty: ret,
        }
    }

    /// The counter, the bound and the two labels every one of these loops opens.
    fn loop_head(&mut self, n: Val) -> u32 {
        let i = self.local(ValType::I64);
        self.all([Ins::I64Const(0), Ins::LocalSet(i)]);
        self.all([Ins::Block(None), Ins::Loop(None), Ins::LocalGet(i)]);
        self.load(n);
        self.all([Ins::I64GeU, Ins::BrIf(1)]);
        i
    }

    fn loop_tail(&mut self, i: u32) {
        self.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
    }

    /// The element at `i` of the run at `data`, as a raw word and as a value.
    fn element_at(&mut self, data: u32, i: u32, element: Repr) -> (Val, Val) {
        let iv = Val {
            local: i,
            ty: Repr::Int,
        };
        self.elem_addr(data, iv);
        self.push(Ins::I64Load(0));
        let w = self.held(Repr::Int);
        self.load(w);
        let x = self.narrow(element);
        (w, x)
    }

    fn map_loop(&mut self, xs: Val, f: Val, fam: u32, out: u32, span: Span) -> Result<Val, String> {
        let element = self.heap.element(match xs.ty {
            Repr::List(at) => at,
            _ => unreachable!("`map_list` checked its argument"),
        });
        let n = self.list_len(xs);
        self.load(n);
        let answer = self.rt_call(Helper::ListAlloc, span, Repr::List(out));
        let src = self.list_data(xs);
        let dst = self.list_data(answer);
        let i = self.loop_head(n);
        let (_, x) = self.element_at(src, i, element);
        let y = self.apply_vals(fam, f, &[x]);
        let iv = Val {
            local: i,
            ty: Repr::Int,
        };
        self.elem_addr(dst, iv);
        self.widen(y);
        self.push(Ins::I64Store(0));
        self.loop_tail(i);
        Ok(answer)
    }

    fn filter_loop(&mut self, xs: Val, f: Val, fam: u32, span: Span) -> Result<Val, String> {
        let element = self.heap.element(match xs.ty {
            Repr::List(at) => at,
            _ => unreachable!("`filter_list` checked its argument"),
        });
        let n = self.list_len(xs);
        // A block of the input's capacity, `used` filled in once the answer's length is known —
        // which keeps `list_append`'s "this list stands at the end of its block" test true of it.
        self.load(n);
        self.push(Ins::I64Const(0));
        let data = self.rt_call(Helper::ListBlock, span, Repr::Int);
        let src = self.list_data(xs);
        self.load(data);
        self.push(Ins::I32WrapI64);
        self.push(Ins::I32Const(heap::DATA_HEADER as i32));
        self.push(Ins::I32Add);
        let dst = self.local(ValType::I32);
        self.push(Ins::LocalSet(dst));
        let k = self.local(ValType::I64);
        self.all([Ins::I64Const(0), Ins::LocalSet(k)]);
        let i = self.loop_head(n);
        let (w, x) = self.element_at(src, i, element);
        let keep = self.apply_vals(fam, f, &[x]);
        self.load(keep);
        self.push(Ins::If(None));
        let kv = Val {
            local: k,
            ty: Repr::Int,
        };
        self.elem_addr(dst, kv);
        self.load(w);
        self.push(Ins::I64Store(0));
        self.all([
            Ins::LocalGet(k),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(k),
            Ins::End,
        ]);
        self.loop_tail(i);
        // `used`, and then the header that says how many of them this list has.
        self.load(data);
        self.push(Ins::I32WrapI64);
        self.push(Ins::LocalGet(k));
        self.push(Ins::I64Store(rt::at(1)));
        self.push(Ins::LocalGet(k));
        self.load(data);
        Ok(self.rt_call(Helper::ListHead, span, xs.ty))
    }

    fn fold_loop(
        &mut self,
        xs: Val,
        init: Val,
        f: Val,
        fam: u32,
        _span: Span,
    ) -> Result<Val, String> {
        let element = self.heap.element(match xs.ty {
            Repr::List(at) => at,
            _ => unreachable!("`list_fold` checked its argument"),
        });
        let acc = self.local(val(init.ty));
        self.load(init);
        self.push(Ins::LocalSet(acc));
        let held = Val {
            local: acc,
            ty: init.ty,
        };
        let n = self.list_len(xs);
        let src = self.list_data(xs);
        let i = self.loop_head(n);
        let (_, x) = self.element_at(src, i, element);
        let next = self.apply_vals(fam, f, &[held, x]);
        self.load(next);
        self.push(Ins::LocalSet(acc));
        self.loop_tail(i);
        Ok(held)
    }

    fn every_loop(
        &mut self,
        xs: Val,
        f: Val,
        fam: u32,
        any: bool,
        _span: Span,
    ) -> Result<Val, String> {
        let element = self.heap.element(match xs.ty {
            Repr::List(at) => at,
            _ => unreachable!("`list_all`/`list_any` checked its argument"),
        });
        let out = self.local(ValType::I32);
        self.all([Ins::I32Const(i32::from(!any)), Ins::LocalSet(out)]);
        let n = self.list_len(xs);
        let src = self.list_data(xs);
        let i = self.loop_head(n);
        let (_, x) = self.element_at(src, i, element);
        let answered = self.apply_vals(fam, f, &[x]);
        self.load(answered);
        if !any {
            self.push(Ins::I32Eqz);
        }
        self.push(Ins::If(None));
        // Decided: the answer is the one that stopped the walk, and there is nothing left to ask.
        self.all([
            Ins::I32Const(i32::from(any)),
            Ins::LocalSet(out),
            Ins::Br(2),
            Ins::End,
        ]);
        self.loop_tail(i);
        self.push(Ins::LocalGet(out));
        Ok(self.held(Repr::Bool))
    }

    /// Decorate, sort, undecorate — and the keys are words like any others, so what compares two of
    /// them is the function a list's element comparison already is.
    fn sort_loop(
        &mut self,
        xs: Val,
        f: Val,
        fam: u32,
        keys: u32,
        span: Span,
    ) -> Result<Val, String> {
        let element = self.heap.element(match xs.ty {
            Repr::List(at) => at,
            _ => unreachable!("`sort_by` checked its argument"),
        });
        let n = self.list_len(xs);
        // Four runs of words, not four lists: nothing outside this loop sees them, and a header
        // over each would be four allocations for nothing.
        self.load(n);
        self.all([Ins::I64Const(heap::WORD as i64), Ins::I64Mul]);
        let bytes = self.held(Repr::Int);
        let mut runs: Vec<u32> = Vec::new();
        for _ in 0..4 {
            self.load(bytes);
            let at = self.alloc(span);
            self.load(at);
            self.push(Ins::I32WrapI64);
            let addr = self.local(ValType::I32);
            self.push(Ins::LocalSet(addr));
            runs.push(addr);
        }
        let (ka, va, tk, tv) = (runs[0], runs[1], runs[2], runs[3]);
        let src = self.list_data(xs);
        let i = self.loop_head(n);
        let (w, x) = self.element_at(src, i, element);
        let key = self.apply_vals(fam, f, &[x]);
        let iv = Val {
            local: i,
            ty: Repr::Int,
        };
        self.elem_addr(ka, iv);
        self.widen(key);
        self.push(Ins::I64Store(0));
        self.elem_addr(va, iv);
        self.load(w);
        self.push(Ins::I64Store(0));
        self.loop_tail(i);

        for r in [ka, va, tk, tv] {
            self.push(Ins::LocalGet(r));
        }
        self.push(Ins::I64Const(0));
        self.load(n);
        let sort = self.reg.helper(Helper::ListSort(keys));
        self.push(Ins::Call(sort));
        self.push(Ins::Drop);

        self.load(n);
        let answer = self.rt_call(Helper::ListAlloc, span, xs.ty);
        let dst = self.list_data(answer);
        self.push(Ins::LocalGet(dst));
        self.push(Ins::LocalGet(va));
        self.load(bytes);
        self.push(Ins::I32WrapI64);
        self.push(Ins::MemoryCopy);
        Ok(answer)
    }
}

// -------------------------------------------------------------------------------------------
// `try:`, which is the one control-flow shape a `block` is for
// -------------------------------------------------------------------------------------------

impl Function<'_> {
    /// `try: block` — run the block under a handler, and reify one failure as a `Result[T, E]`.
    ///
    /// The block is emitted **inline**, not as a closure applied. The checker wraps it in a `lam`
    /// of no parameters so the evaluator can delay it; here there is nothing to delay, and inlining
    /// is what puts the block's own calls under the handler.
    ///
    /// Emitting it for a **value** is load-bearing and not a style: a call in tail position is a
    /// `return_call` that does not check the trap globals (there is no frame left to check in),
    /// which is correct at the top of a function and would walk straight through a handler.
    ///
    /// What the handler does is the evaluator's `Prim::Try`, word for word: a raise of the caught
    /// type becomes `Err(value)`, and **everything else keeps travelling** — a fault is not a
    /// failure, and a different error type belongs to a handler further out.
    ///
    /// # Why this is two blocks and not a branch
    ///
    /// WebAssembly has no jumps, so the label a failure goes to is the end of a `block` it is
    /// *inside*. The inner one is the failure exit and the outer one carries the answer past it:
    /// the success path builds its `Ok` and `br`s over the handler, and every `checked()` under
    /// the handler `br`s to the inner block's end instead of returning.
    fn try_(&mut self, args: &[Core], ty: &Ty, span: Span) -> Result<Val, String> {
        let [block, caught] = args else {
            return Err("`try` takes a block and the name of what it catches".into());
        };
        let CoreKind::Lam { params, body } = &block.kind else {
            return Err("`try` over something that is not a block".into());
        };
        if !params.is_empty() {
            return Err("`try` over a block that takes arguments".into());
        }
        let CoreKind::Const(Const::Str(name)) = &caught.kind else {
            return Err("`try` whose caught type is not written down".into());
        };
        // `Result[T, E]`, from the type the checker gave this expression: `E` is what the raised
        // value is read back as and `T` is what the block answers.
        let repr = self
            .repr_of(ty)
            .map_err(|why| format!("catches into a value that is {why}"))?;
        let Repr::Obj(at) = repr else {
            return Err(format!("catches into `{ty}`, which is not an object"));
        };
        let (ok, err, layout) = {
            let l = self.heap.layout(at);
            let ok = l
                .tag_of(Some("Ok"))
                .ok_or_else(|| format!("`{}` has no `Ok`", l.shown))?;
            let err = l
                .tag_of(Some("Err"))
                .ok_or_else(|| format!("`{}` has no `Err`", l.shown))?;
            (ok, err, l.clone())
        };
        let (ok_slot, ok_ty) = layout.variants[ok as usize]
            .slot("value")
            .ok_or_else(|| format!("`{}`'s `Ok` has no `value`", layout.shown))?;
        let (err_slot, err_ty) = layout.variants[err as usize]
            .slot("error")
            .ok_or_else(|| format!("`{}`'s `Err` has no `error`", layout.shown))?;
        let bytes = layout
            .variants
            .iter()
            .map(|v| v.bytes())
            .max()
            .unwrap_or(heap::WORD);
        let want = self.heap.intern(name);
        let want = self.heap.string_offset(want);
        let out = self.local(ValType::I64);

        self.push(Ins::Block(None));
        self.push(Ins::Block(None));
        self.handlers.push(self.depth);
        let value = self.expr(body, false);
        self.handlers.pop();
        let value = value?;
        if value != ok_ty {
            return Err(format!(
                "`try` over a block answering {} where `{}` carries {}",
                self.heap.show(value),
                layout.shown,
                self.heap.show(ok_ty)
            ));
        }
        let held = self.held(value);
        let good = self.alloc_bytes(bytes, span);
        self.store_word(good.local, 0, i64::from(ok));
        self.store_field(good.local, ok_slot, held);
        self.load(good);
        self.push(Ins::LocalSet(out));
        self.push(Ins::Br(1));
        self.push(Ins::End);

        // Two tests and no search: is this failure a raise at all, and is it the one this `try:`
        // names. Anything else leaves for the *enclosing* handler with the globals untouched.
        self.push(Ins::GlobalGet(TRAP));
        self.push(Ins::I32Const(Trap::Raised.code() as i32));
        self.push(Ins::I32Ne);
        self.push(Ins::GlobalGet(TRAP_TYPE));
        self.push(Ins::I64Const(want as i64));
        self.push(Ins::I64Ne);
        self.push(Ins::I32Or);
        self.push(Ins::If(None));
        self.escape();
        self.push(Ins::End);

        // Handled. The globals are cleared **before** anything else, because the allocation below
        // checks them: a failure that stops here must not look like one to the next call.
        self.push(Ins::GlobalGet(TRAP_PAYLOAD));
        let pair = self.local(ValType::I64);
        self.push(Ins::LocalSet(pair));
        self.push(Ins::I32Const(0));
        self.push(Ins::GlobalSet(TRAP));
        self.push(Ins::I32Const(0));
        self.push(Ins::GlobalSet(TRAP_SPAN));
        self.push(Ins::I64Const(0));
        self.push(Ins::GlobalSet(TRAP_PAYLOAD));
        self.push(Ins::I64Const(0));
        self.push(Ins::GlobalSet(TRAP_TYPE));
        let carried = self.load_field(pair, 1, err_ty);
        let bad = self.alloc_bytes(bytes, span);
        self.store_word(bad.local, 0, i64::from(err));
        self.store_field(bad.local, err_slot, carried);
        self.load(bad);
        self.push(Ins::LocalSet(out));
        self.push(Ins::End);

        Ok(Val {
            local: out,
            ty: repr,
        })
    }
}

// -------------------------------------------------------------------------------------------
// The four questions a computation cannot answer
// -------------------------------------------------------------------------------------------

/// Whether any part of this body asks the host something.
///
/// Asked of a definition the fixed point kept, so the answer decides whether the module declares
/// the import — and a module that declares one it never calls would be a module a loader has to
/// supply a function for and a browser has to be told about.
fn asks(c: &Core) -> bool {
    if let CoreKind::Prim { op, .. } = &c.kind {
        if Upcall::of(*op).is_some() {
            return true;
        }
    }
    match &c.kind {
        CoreKind::Lam { body, .. } => asks(body),
        CoreKind::App { func, args } => asks(func) || args.iter().any(asks),
        CoreKind::Prim { args, .. } => args.iter().any(asks),
        CoreKind::Let { value, body, .. } => asks(value) || asks(body),
        CoreKind::If { cond, then, alt } => asks(cond) || asks(then) || asks(alt),
        CoreKind::Match { scrutinee, arms } => {
            asks(scrutinee)
                || arms
                    .iter()
                    .any(|a| a.guard.as_ref().is_some_and(asks) || asks(&a.body))
        }
        CoreKind::Make { fields, .. } => fields.iter().any(|(_, f)| asks(f)),
        CoreKind::With { base, fields } => asks(base) || fields.iter().any(|(_, f)| asks(f)),
        CoreKind::Field { base, .. } => asks(base),
        CoreKind::ListLit(xs) => xs.iter().any(asks),
        CoreKind::MapLit(kvs) => kvs.iter().any(|(k, v)| asks(k) || asks(v)),
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => false,
    }
}

impl Function<'_> {
    /// Ask the host one of the four questions compiled code cannot answer.
    ///
    /// # Why this is an import and not a pipe
    ///
    /// The native backends write a **question frame** into the arena and block on a pipe
    /// ([`beck_llvm::Upcall`]), because [`adr/0021`](../../../../../docs/adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)
    /// put the compiled program in another process. There is no other process in a browser tab and
    /// no pipe to block on: the loader is *in* the same tab, holds the memory, and can be called.
    /// So the frame's five fields become the call's arguments, and the answer comes back as the
    /// return value with its bytes appended at the mark — which is the same protocol with the
    /// blocking taken out.
    ///
    /// The **shapes** are what carry it. What goes across is a word per argument and a word saying
    /// what each word *is*, so the host decodes and encodes through
    /// [`beck_llvm::heap::Heap`] without a second table of what `secret_env` takes and what
    /// `http_fetch` answers — the same trick a view's deferred leaves play, one subsystem over.
    ///
    /// The name of the error type is passed by the **module** rather than chosen by the host, for
    /// [`Upcall::raises`]'s reason: a `try:` compares an interned literal's offset, and only this
    /// module knows which offset that is.
    fn upcall(&mut self, op: Upcall, vals: &[Val], ty: &Ty, span: Span) -> Result<Val, String> {
        if vals.len() != op.arity() {
            return Err(format!(
                "`{}` is applied to {} arguments here",
                op.name(),
                vals.len()
            ));
        }
        let Some(index) = self.upcall else {
            // Only reachable in the fixed point's rounds, where nothing emitted is kept: the
            // import is decided from the definitions those rounds leave standing.
            return self.placeholder(ty, op);
        };
        let ret = self
            .repr_of(ty)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        Heap::crossing(ret).map_err(|why| format!("`{}` answers {why}", op.name()))?;
        // The host *writes* the answer into the memory, which is the inbound direction and has the
        // one rule that is directional: an `Attr` is a value the host cannot name a shape for.
        self.heap
            .inbound(ret)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        let ret_shape = self.heap.word_of(ret);
        // The shape a failure carries and the **name** of its type: the shape is what the host
        // encodes the failure through, and the name is what a `try:` compares — an interned
        // literal's offset, which only this module knows.
        let (raises, named) = match op.raises() {
            Some(name) => {
                let repr = self
                    .repr_of(&Ty::con(name))
                    .map_err(|why| format!("`{}` raises {why}", op.name()))?;
                let at = self.heap.intern(name);
                (self.heap.word_of(repr), self.heap.string_offset(at))
            }
            None => (0, 0),
        };
        let mut shapes = Vec::with_capacity(vals.len());
        for v in vals {
            Heap::crossing(v.ty).map_err(|why| format!("`{}` is given {why}", op.name()))?;
            shapes.push(self.heap.word_of(v.ty));
        }

        let at = self.span_index(span);
        self.push(Ins::I32Const(op.code() as i32));
        self.push(Ins::I32Const(at as i32));
        self.push(Ins::I32Const(ret_shape as i32));
        self.push(Ins::I32Const(raises as i32));
        self.push(Ins::I64Const(named as i64));
        // Two argument slots, because [`Upcall::arity`] is at most two and an arity read out of
        // the frame would be an arity the host could not check.
        let mut given = vals.iter().zip(&shapes);
        for _ in 0..2 {
            match given.next() {
                Some((v, shape)) => {
                    self.widen(*v);
                    self.push(Ins::I32Const(*shape as i32));
                }
                None => {
                    self.push(Ins::I64Const(0));
                    self.push(Ins::I32Const(0));
                }
            }
        }
        self.push(Ins::Call(index));
        let held = self.local(ValType::I64);
        self.push(Ins::LocalSet(held));
        self.checked();
        self.push(Ins::LocalGet(held));
        Ok(self.narrow(ret))
    }

    /// A value of the right shape for a round whose output is thrown away.
    fn placeholder(&mut self, ty: &Ty, op: Upcall) -> Result<Val, String> {
        let ret = self
            .repr_of(ty)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        Heap::crossing(ret).map_err(|why| format!("`{}` answers {why}", op.name()))?;
        self.heap
            .inbound(ret)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        let z = zero(ret);
        self.push(z);
        Ok(self.held(ret))
    }
}
