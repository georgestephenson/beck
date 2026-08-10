//! `Core` → Cranelift IR → an object file.
//!
//! # The same subset, a second time
//!
//! This compiles exactly what [`beck_llvm::emit`] compiles — the **scalar subset**: a definition
//! whose parameters and result are all `Int`, `Float` or `Bool`, and whose body is built from
//! constants, variables, `let`, `if`, `match` on scalar constants, direct calls to other compiled
//! definitions, and the arithmetic, comparison and logical primitives. A heap value is refused by
//! name, with a reason, exactly as it is there.
//!
//! Writing the selection a second time rather than importing it is deliberate. The two emitters
//! are held to *agreeing* — `cranelift.rs` asserts that they accept and refuse the same
//! definitions over every program in the tree — and a shared implementation would make that
//! agreement true by construction and therefore worth nothing. What *is* shared is the vocabulary
//! and the wire: [`beck_llvm::Scalar`], [`beck_llvm::Signature`], [`beck_llvm::Refusal`] and
//! [`beck_llvm::Trap`] are types, and [`beck_llvm::Trap`]'s codes are a protocol the host decodes.
//! A second copy of *those* would be two spellings of one contract.
//!
//! # Agreeing with the evaluator exactly
//!
//! Every decision [`docs/93`](../../../../../docs/93-llvm-backend-report.md) §93.2 records is made
//! again here, because they are decisions about *the language* rather than about LLVM:
//!
//! * **Integer arithmetic is checked**, through `sadd_overflow` and friends, with an explicit
//!   guard on `/` and `%` for a zero divisor and for `INT_MIN / -1`. An overflow is a value the
//!   host turns back into the evaluator's own message, not a wrapped result and not a signal.
//! * **Reals compare through [`beck_core::Value`]'s order key**, not through `fcmp`: under it
//!   `-0.0 < 0.0` and NaN is the maximum, and `fcmp` says something else for both.
//! * **A real is normalised where a signed zero or a NaN is observable** — a comparison, a
//!   division's divisor, and a trap's payload. `Body::normalise` in this module's source carries
//!   the argument.
//! * **`trunc` saturates**, because the evaluator's `f as i64` is Rust's saturating cast:
//!   `fcvt_to_sint_sat`, whose NaN is `0`.
//!
//! # What is Cranelift's rather than LLVM's
//!
//! * **A tail call is `return_call`** under [`CallConv::Tail`], which Cranelift *verifies* rather
//!   than attempts — the same guarantee `musttail` gives and the reason
//!   [`31`](../../../../../docs/31-tail-calls-report.md)'s property holds on this backend too.
//! * **A block parameter replaces a `phi`.** The joins are the same joins; Cranelift's SSA is
//!   built by [`cranelift_frontend`] rather than written out.
//! * **A `Bool` is an `I8` holding 0 or 1**, because Cranelift has no `i1`. Every comparison here
//!   produces one, `not` is `bxor 1` rather than a complement, and the invariant is what makes
//!   `band`/`bor` correct.
//! * **There are no intrinsics.** `sqrt` and `fabs` are instructions; `sin` and `cos` are calls to
//!   the C library, which is linked in anyway.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_core::check::{Def, Program};
use beck_core::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use beck_diag::Span;
use beck_llvm::{Refusal, Scalar, Signature, Trap, MAX_PARAMS};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Function, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Type,
    UserFuncName, Value as IrValue,
};
use cranelift_codegen::isa::{self, CallConv, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module as _};
use cranelift_object::{ObjectBuilder, ObjectModule};

/// The trap cell's layout, which is the host's [`beck_llvm::Worker`] protocol: a `u32` code, a
/// `u32` span index, and an `i64` payload.
const CELL_SPAN: i32 = 4;
const CELL_PAYLOAD: i32 = 8;

/// What one compilation produced.
pub struct Module {
    /// The linkable object.
    pub object: Vec<u8>,
    /// The textual IR of every function, in dispatch-index order — what `beck native --out`
    /// writes beside the executable, and the only form of this backend's output a person reads.
    pub clif: String,
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

/// Compile every definition of `program` this backend can compile.
///
/// Fails only when the host machine has no Cranelift target — which is a fact about the machine,
/// like a missing `clang` — and never because of the program: a program with nothing scalar in it
/// yields a module with no functions and a refusal per definition.
pub fn module(program: &Program) -> Result<Module, String> {
    let isa = host_isa()?;
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

    // Round two, to a fixed point: emit every body, and drop whichever ones will not emit. A body
    // that calls a definition dropped in an earlier round fails in a later one, which is what
    // makes mutual recursion work — the pair survives together or is refused together.
    //
    // Each round builds the whole object again. That is the price of "compiles" and "emits" being
    // one question rather than two: an analysis that predicted emissibility would be a second
    // implementation of this file, and the two would drift. Rounds are bounded by the number of
    // definitions and are one in every program in this tree.
    let mut eligible: BTreeSet<Arc<str>> = sigs.keys().cloned().collect();
    loop {
        match build(program, &sigs, &eligible, isa.clone()) {
            Ok(built) => {
                refusals.sort_by(|a, b| a.name.cmp(&b.name));
                return Ok(Module {
                    object: built.object,
                    clif: built.clif,
                    functions: built.functions,
                    spans: built.spans,
                    refusals,
                });
            }
            Err(Failure::Refused(dropped)) => {
                for Refusal { name, reason } in dropped {
                    eligible.remove(&name);
                    refusals.push(Refusal { name, reason });
                }
            }
            Err(Failure::Fatal(e)) => return Err(e),
        }
    }
}

/// The host's target, or the reason there is not one.
fn host_isa() -> Result<Arc<dyn TargetIsa>, String> {
    let mut flags = settings::builder();
    // Speed rather than size, and this is the *dev* code generator: the setting that matters is
    // that it is not `none`, which would make every measurement of it a measurement of nothing.
    flags
        .set("opt_level", "speed")
        .map_err(|e| format!("cranelift setting: {e}"))?;
    // Not a preference. Cranelift's x86-64 `return_call` **asserts** on this, because its
    // implementation of a tail call restores the caller's frame through the frame pointer — so a
    // backend that guarantees `docs/31`'s tail calls has to keep them.
    flags
        .set("preserve_frame_pointers", "true")
        .map_err(|e| format!("cranelift setting: {e}"))?;
    isa::lookup(target_lexicon::Triple::host())
        .map_err(|e| format!("no Cranelift backend for this machine: {e}"))?
        .finish(settings::Flags::new(flags))
        .map_err(|e| format!("cranelift target: {e}"))
}

/// What went wrong with one round.
enum Failure {
    /// These definitions do not compile, and the caller should try again without them.
    Refused(Vec<Refusal>),
    /// Something that is not about the program.
    Fatal(String),
}

struct Built {
    object: Vec<u8>,
    clif: String,
    functions: Vec<Signature>,
    spans: Vec<Span>,
}

/// One round: declare everything eligible, define every body, and emit the object.
fn build(
    program: &Program,
    sigs: &BTreeMap<Arc<str>, Signature>,
    eligible: &BTreeSet<Arc<str>>,
    isa: Arc<dyn TargetIsa>,
) -> Result<Built, Failure> {
    // Declaration order is the program's, so the dispatch index a definition gets is a property of
    // the source rather than of a hash map's iteration.
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

    let builder = ObjectBuilder::new(isa, "beck", cranelift_module::default_libcall_names())
        .map_err(|e| Failure::Fatal(format!("cranelift object: {e}")))?;
    let mut object = ObjectModule::new(builder);
    let ptr = object.target_config().pointer_type();

    // Every compiled definition, declared before any is defined: a body may call one declared
    // after it, and mutual recursion is two bodies that each call the other.
    let mut ids: BTreeMap<Arc<str>, FuncId> = BTreeMap::new();
    for name in &order {
        let sig = beck_signature(&indexed[name], ptr);
        let id = object
            .declare_function(&symbol(name), Linkage::Local, &sig)
            .map_err(|e| Failure::Fatal(format!("declaring `{name}`: {e}")))?;
        ids.insert(name.clone(), id);
    }

    let mut ctx = object.make_context();
    let mut fctx = FunctionBuilderContext::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut clif = String::new();
    let mut refused: Vec<Refusal> = Vec::new();

    for (n, name) in order.iter().enumerate() {
        let def = &program.defs[name];
        let sig = beck_signature(&indexed[name], ptr);
        ctx.func = Function::with_name_signature(UserFuncName::user(0, n as u32), sig);
        let taken = std::mem::take(&mut spans);
        let mut body = Body::new(&indexed, eligible, &ids, taken);
        let emitted = {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            let outcome = body.emit(def, &mut b, &mut object);
            if outcome.is_ok() {
                b.finalize(object.target_config());
            }
            outcome
        };
        // A refused definition left the builder's context half-built, and `finalize` is the only
        // thing that resets one. A fresh context is the reset, and it costs an allocation on a
        // path that is already going to compile the whole module again.
        if emitted.is_err() {
            fctx = FunctionBuilderContext::new();
        }
        spans = std::mem::take(&mut body.spans);
        match emitted {
            Ok(()) => {}
            Err(reason) => {
                refused.push(Refusal {
                    name: name.clone(),
                    reason,
                });
                object.clear_context(&mut ctx);
                continue;
            }
        }
        clif.push_str(&format!("; {name}\n{}\n", ctx.func));
        object
            .define_function(ids[name], &mut ctx)
            .map_err(|e| Failure::Fatal(format!("defining `{name}`: {e}")))?;
        object.clear_context(&mut ctx);
    }
    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    driver(&mut object, &mut ctx, &mut fctx, &functions, &ids, &order).map_err(Failure::Fatal)?;

    let object = object
        .finish()
        .emit()
        .map_err(|e| Failure::Fatal(format!("emitting the object: {e}")))?;
    Ok(Built {
        object,
        clif,
        functions,
        spans,
    })
}

/// The signature, or the reason there is not one.
///
/// The same four rules [`beck_llvm`] applies, written again rather than imported — see this
/// module's own documentation for why, and `cranelift.rs` for what holds them together.
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
                    "parameter `{name}` is `{ty}`, and only Int, Float and Bool have a machine \
                     representation here"
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
        index: u32::MAX,
    })
}

/// The Cranelift type a scalar is carried in.
///
/// A `Bool` is an `I8` holding 0 or 1: Cranelift has no one-bit integer, and every comparison it
/// emits produces exactly this. The invariant is what makes `band` and `bor` the right
/// instructions for `and` and `or`, and it is why `not` is `bxor 1` rather than a complement.
fn machine(s: Scalar) -> Type {
    match s {
        Scalar::Int => types::I64,
        Scalar::Float => types::F64,
        Scalar::Bool => types::I8,
    }
}

/// A compiled definition's signature: the error cell, then its parameters.
///
/// [`CallConv::Tail`] because a call in tail position must be a jump — `docs/31` makes that a
/// property of the language rather than an optimisation — and `return_call` is only available
/// between functions that share it.
fn beck_signature(sig: &Signature, ptr: Type) -> cranelift_codegen::ir::Signature {
    let mut out = cranelift_codegen::ir::Signature::new(CallConv::Tail);
    out.params.push(AbiParam::new(ptr));
    for p in &sig.params {
        out.params.push(AbiParam::new(machine(*p)));
    }
    out.returns.push(AbiParam::new(machine(sig.ret)));
    out
}

/// A Beck name as an object symbol.
///
/// Hex-escaped rather than transliterated, for [`beck_llvm`]'s reason: identifiers are Unicode
/// (`docs/44` §44.4) and a scheme that folded characters could give two definitions one symbol.
fn symbol(name: &str) -> String {
    let mut out = String::from("beck.");
    for b in name.bytes() {
        match b {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("${b:02X}")),
        }
    }
    out
}

/// An SSA value and what it is.
#[derive(Clone, Copy, Debug)]
struct Val {
    v: IrValue,
    ty: Scalar,
}

/// Where the value an expression produces has to go.
///
/// [`Dest::Return`] is why this exists: an expression in tail position compiles to a jump rather
/// than a call, and "in tail position" is a fact about the context rather than about the
/// expression. It travels through `if`, `let` and `match`, because the interesting call is almost
/// never the outermost node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dest {
    Value,
    Return,
}

/// One function under construction.
struct Body<'a> {
    sigs: &'a BTreeMap<Arc<str>, Signature>,
    eligible: &'a BTreeSet<Arc<str>>,
    ids: &'a BTreeMap<Arc<str>, FuncId>,
    env: BTreeMap<VarId, Val>,
    spans: Vec<Span>,
    /// What this function returns, and therefore what a trapping exit has to return too.
    ret: Scalar,
    /// The error cell, this function's first parameter.
    err: Option<IrValue>,
}

impl<'a> Body<'a> {
    fn new(
        sigs: &'a BTreeMap<Arc<str>, Signature>,
        eligible: &'a BTreeSet<Arc<str>>,
        ids: &'a BTreeMap<Arc<str>, FuncId>,
        spans: Vec<Span>,
    ) -> Body<'a> {
        Body {
            sigs,
            eligible,
            ids,
            env: BTreeMap::new(),
            spans,
            ret: Scalar::Int,
            err: None,
        }
    }

    fn emit(
        &mut self,
        def: &Def,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<(), String> {
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

        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        self.err = Some(b.block_params(entry)[0]);
        for (i, (var, ty)) in params.iter().zip(&sig.params).enumerate() {
            let v = b.block_params(entry)[i + 1];
            self.env.insert(*var, Val { v, ty: *ty });
        }

        self.expr(body, Dest::Return, b, m)?;
        Ok(())
    }

    // -- plumbing ---------------------------------------------------------------------------

    fn err(&self) -> IrValue {
        self.err.expect("the error cell is the first parameter")
    }

    /// Record a span and answer the index the compiled code should store for it.
    fn span(&mut self, span: Span) -> u32 {
        self.spans.push(span);
        (self.spans.len() - 1) as u32
    }

    /// The zero of a scalar — what a trapping path returns, and never read: the host looks at the
    /// trap code first.
    fn zero(&self, ty: Scalar, b: &mut FunctionBuilder<'_>) -> IrValue {
        match ty {
            Scalar::Int => b.ins().iconst(types::I64, 0),
            Scalar::Bool => b.ins().iconst(types::I8, 0),
            Scalar::Float => b.ins().f64const(0.0),
        }
    }

    /// Store the trap and return. The caller carries on in a fresh block.
    ///
    /// A store and a `return`, rather than a jump to a shared exit: the two are the same
    /// behaviour, and the cell outlives every frame that shares it, so whoever eventually reads it
    /// — the worker's loop — sees the reason whatever happened to the frames in between.
    fn trap(
        &mut self,
        trap: Trap,
        span: Span,
        payload: IrValue,
        cond: IrValue,
        b: &mut FunctionBuilder<'_>,
    ) {
        let idx = self.span(span);
        let set = b.create_block();
        let cont = b.create_block();
        b.ins().brif(cond, set, &[], cont, &[]);

        b.switch_to_block(set);
        b.seal_block(set);
        let err = self.err();
        let flags = MemFlagsData::trusted();
        let code = b.ins().iconst(types::I32, i64::from(trap.code()));
        b.ins().store(flags, code, err, 0);
        let at = b.ins().iconst(types::I32, i64::from(idx));
        b.ins().store(flags, at, err, CELL_SPAN);
        b.ins().store(flags, payload, err, CELL_PAYLOAD);
        let z = self.zero(self.ret, b);
        b.ins().return_(&[z]);

        b.switch_to_block(cont);
        b.seal_block(cont);
    }

    /// Return if a callee trapped.
    fn check_call(&mut self, b: &mut FunctionBuilder<'_>) {
        let err = self.err();
        let code = b.ins().load(types::I32, MemFlagsData::trusted(), err, 0);
        let bad = b.create_block();
        let cont = b.create_block();
        let failed = b.ins().icmp_imm_s(IntCC::NotEqual, code, 0);
        b.ins().brif(failed, bad, &[], cont, &[]);

        b.switch_to_block(bad);
        b.seal_block(bad);
        let z = self.zero(self.ret, b);
        b.ins().return_(&[z]);

        b.switch_to_block(cont);
        b.seal_block(cont);
    }

    // -- expressions ------------------------------------------------------------------------

    fn value(
        &mut self,
        c: &Core,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        Ok(self
            .expr(c, Dest::Value, b, m)?
            .expect("value mode always produces a value"))
    }

    fn expr(
        &mut self,
        c: &Core,
        dest: Dest,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
        let value = match &c.kind {
            CoreKind::Const(k) => constant(k, b)?,
            CoreKind::Var(v) => *self
                .env
                .get(v)
                .ok_or_else(|| format!("variable {v} is not in scope here"))?,
            CoreKind::Let { var, value, body } => {
                let v = self.value(value, b, m)?;
                let shadowed = self.env.insert(*var, v);
                let r = self.expr(body, dest, b, m);
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
            CoreKind::If { cond, then, alt } => return self.if_(cond, then, alt, dest, b, m),
            CoreKind::Match { scrutinee, arms } => {
                return self.match_(scrutinee, arms, c.span, dest, b, m)
            }
            CoreKind::App { func, args } => return self.call(func, args, dest, b, m),
            CoreKind::Prim { op, args } => self.prim(*op, args, c.span, b, m)?,
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
        self.finish(value, dest, b)
    }

    fn finish(
        &mut self,
        v: Val,
        dest: Dest,
        b: &mut FunctionBuilder<'_>,
    ) -> Result<Option<Val>, String> {
        match dest {
            Dest::Value => Ok(Some(v)),
            Dest::Return => {
                if v.ty != self.ret {
                    return Err(format!(
                        "returns {:?} where the signature says {:?}",
                        v.ty, self.ret
                    ));
                }
                b.ins().return_(&[v.v]);
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
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
        let c = self.value(cond, b, m)?;
        if c.ty != Scalar::Bool {
            return Err("the condition of an `if` is not a Bool".into());
        }
        let lt = b.create_block();
        let lf = b.create_block();
        b.ins().brif(c.v, lt, &[], lf, &[]);

        // The join block's parameter is the `phi` a textual emitter would write. It is created
        // before either arm runs because an arm has to be able to jump to it, and its type is not
        // known until one of them has produced a value — so it is added lazily, which is legal
        // exactly while nothing has jumped to it yet.
        let join = b.create_block();
        let mut ty: Option<Scalar> = None;
        let mut arms = 0;

        b.switch_to_block(lt);
        b.seal_block(lt);
        let tv = self.expr(then, dest, b, m)?;
        if let Some(v) = tv {
            if ty.is_none() {
                b.append_block_param(join, machine(v.ty));
                ty = Some(v.ty);
            }
            b.ins().jump(join, &[v.v.into()]);
            arms += 1;
        }

        b.switch_to_block(lf);
        b.seal_block(lf);
        let fv = self.expr(alt, dest, b, m)?;
        if let Some(v) = fv {
            match ty {
                Some(t) if t != v.ty => {
                    return Err("the two branches of an `if` have different types".into())
                }
                Some(_) => {}
                None => {
                    b.append_block_param(join, machine(v.ty));
                    ty = Some(v.ty);
                }
            }
            b.ins().jump(join, &[v.v.into()]);
            arms += 1;
        }

        b.switch_to_block(join);
        b.seal_block(join);
        if arms == 0 {
            // Both branches returned. Nothing reaches the join, and Cranelift still wants the
            // block it was handed to be terminated.
            let z = self.zero(self.ret, b);
            b.ins().return_(&[z]);
            return Ok(None);
        }
        let ty = ty.expect("an arm produced a value");
        Ok(Some(Val {
            v: b.block_params(join)[0],
            ty,
        }))
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
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
        let v = self.value(scrutinee, b, m)?;
        let join = b.create_block();
        let mut ty: Option<Scalar> = None;
        let mut reached = 0;

        for arm in arms {
            let taken = b.create_block();
            let next = b.create_block();
            let cond = self.test(&arm.pattern, &v, b)?;
            b.ins().brif(cond, taken, &[], next, &[]);

            b.switch_to_block(taken);
            b.seal_block(taken);
            let bound = self.bind(&arm.pattern, &v);
            if let Some(guard) = &arm.guard {
                let g = self.value(guard, b, m)?;
                if g.ty != Scalar::Bool {
                    return Err("a match guard is not a Bool".into());
                }
                let run = b.create_block();
                b.ins().brif(g.v, run, &[], next, &[]);
                b.switch_to_block(run);
                b.seal_block(run);
            }
            let av = self.expr(&arm.body, dest, b, m)?;
            self.unbind(bound);
            if let Some(av) = av {
                match ty {
                    Some(t) if t != av.ty => return Err("match arms have different types".into()),
                    Some(_) => {}
                    None => {
                        b.append_block_param(join, machine(av.ty));
                        ty = Some(av.ty);
                    }
                }
                b.ins().jump(join, &[av.v.into()]);
                reached += 1;
            }
            b.switch_to_block(next);
            b.seal_block(next);
        }

        // Nothing matched. The checker proves a `match` exhaustive, so this is unreachable for a
        // program that compiled — and a wrong exhaustiveness check has to be a *message* rather
        // than whatever the machine does next, so it traps.
        let trap = match v.ty {
            Scalar::Int => Trap::NoMatchInt,
            Scalar::Float => Trap::NoMatchFloat,
            Scalar::Bool => Trap::NoMatchBool,
        };
        let payload = self.widen(&v, b);
        let always = b.ins().iconst(types::I8, 1);
        self.trap(trap, span, payload, always, b);
        // `trap` continues in a fresh block on the "did not trap" edge, which cannot be reached:
        // the condition was a constant. It still needs a terminator.
        let z = self.zero(self.ret, b);
        b.ins().return_(&[z]);

        b.switch_to_block(join);
        b.seal_block(join);
        if reached == 0 {
            let z = self.zero(self.ret, b);
            b.ins().return_(&[z]);
            return Ok(None);
        }
        let ty = ty.ok_or_else(|| "a `match` with no arms".to_string())?;
        Ok(Some(Val {
            v: b.block_params(join)[0],
            ty,
        }))
    }

    /// Whether `pat` matches `v`, as an `I8` holding 0 or 1.
    fn test(
        &mut self,
        pat: &Pattern,
        v: &Val,
        b: &mut FunctionBuilder<'_>,
    ) -> Result<IrValue, String> {
        match pat {
            Pattern::Wildcard | Pattern::Bind(_) => Ok(b.ins().iconst(types::I8, 1)),
            Pattern::At { inner, .. } => self.test(inner, v, b),
            Pattern::Const(k) => {
                let want = constant(k, b)?;
                if want.ty != v.ty {
                    return Err("a match arm compares against a constant of another type".into());
                }
                Ok(self.compare(Prim::Eq, v, &want, b).v)
            }
            Pattern::Or(alts) => {
                let mut acc: Option<IrValue> = None;
                for alt in alts {
                    let t = self.test(alt, v, b)?;
                    acc = Some(match acc {
                        None => t,
                        Some(prev) => b.ins().bor(prev, t),
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

    fn bind(&mut self, pat: &Pattern, v: &Val) -> Vec<(VarId, Option<Val>)> {
        let mut undo = Vec::new();
        self.bind_into(pat, v, &mut undo);
        undo
    }

    fn bind_into(&mut self, pat: &Pattern, v: &Val, undo: &mut Vec<(VarId, Option<Val>)>) {
        match pat {
            Pattern::Bind(var) => undo.push((*var, self.env.insert(*var, *v))),
            Pattern::At { var, inner } => {
                undo.push((*var, self.env.insert(*var, *v)));
                self.bind_into(inner, v, undo);
            }
            // Every alternative of an or-pattern binds the same names to the same scrutinee here,
            // because a scalar pattern takes nothing apart.
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
    /// `return_call` rather than a call and a return: Cranelift's verifier *requires* the frame to
    /// be discardable and refuses the function otherwise, which is the same guarantee `musttail`
    /// gives the other backend. `docs/31` §31.2 says 1,500 and 60,000 tail calls spend the same
    /// host stack, and an optimisation that "usually" fires cannot be what a language guarantee
    /// rests on.
    fn call(
        &mut self,
        func: &Core,
        args: &[Core],
        dest: Dest,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
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
        let mut operands = vec![self.err()];
        for (a, want) in args.iter().zip(&sig.params) {
            let v = self.value(a, b, m)?;
            if v.ty != *want {
                return Err(format!("an argument to `{name}` is the wrong type"));
            }
            operands.push(v.v);
        }
        let id = *self
            .ids
            .get(&**name)
            .ok_or_else(|| format!("`{name}` was not declared"))?;
        let f = m.declare_func_in_func(id, b.func);

        if dest == Dest::Return {
            if sig.ret != self.ret {
                return Err(format!(
                    "`{name}` returns {:?} in tail position of a {:?}",
                    sig.ret, self.ret
                ));
            }
            // No trap check: there is no frame left to check in. A callee that trapped stored the
            // reason before returning, and the cell outlives every frame that shared it.
            b.ins().return_call(f, &operands);
            return Ok(None);
        }

        let call = b.ins().call(f, &operands);
        let r = b.inst_results(call)[0];
        self.check_call(b);
        Ok(Some(Val { v: r, ty: sig.ret }))
    }

    // -- primitives -------------------------------------------------------------------------

    fn prim(
        &mut self,
        op: Prim,
        args: &[Core],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.value(a, b, m)?);
        }
        let arity = |n: usize, vals: &[Val]| -> Result<(), String> {
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
                arity(2, &vals)?;
                match same(&vals)? {
                    Scalar::Int => Ok(self.checked_int(op, &vals[0], &vals[1], span, b)),
                    Scalar::Float => {
                        let v = match op {
                            Prim::Add => b.ins().fadd(vals[0].v, vals[1].v),
                            Prim::Sub => b.ins().fsub(vals[0].v, vals[1].v),
                            _ => b.ins().fmul(vals[0].v, vals[1].v),
                        };
                        Ok(Val {
                            v,
                            ty: Scalar::Float,
                        })
                    }
                    Scalar::Bool => Err(format!("`{}` on two Bools", op.name())),
                }
            }
            Prim::Div | Prim::Rem => {
                arity(2, &vals)?;
                match same(&vals)? {
                    Scalar::Int => Ok(self.checked_divide(op, &vals[0], &vals[1], span, b)),
                    // `%` on reals is not in the language: the evaluator's arm answers only for
                    // two Ints. Division normalises its *divisor* — `1.0 / -0.0` is `-inf` where
                    // `1.0 / 0.0` is `+inf`, which is a difference a zero's sign has escaped into.
                    Scalar::Float if op == Prim::Div => {
                        let d = self.normalise(vals[1].v, b);
                        let v = b.ins().fdiv(vals[0].v, d);
                        Ok(Val {
                            v,
                            ty: Scalar::Float,
                        })
                    }
                    _ => Err(format!("`{}` on this type", op.name())),
                }
            }
            Prim::Neg => {
                arity(1, &vals)?;
                match vals[0].ty {
                    Scalar::Int => {
                        // `i64::checked_neg`: the one input without an answer is `i64::MIN`.
                        let bad = b.ins().icmp_imm_s(IntCC::Equal, vals[0].v, i64::MIN);
                        let payload = self.widen(&vals[0], b);
                        self.trap(Trap::NegOverflow, span, payload, bad, b);
                        let v = b.ins().ineg(vals[0].v);
                        Ok(Val { v, ty: Scalar::Int })
                    }
                    Scalar::Float => Ok(Val {
                        v: b.ins().fneg(vals[0].v),
                        ty: Scalar::Float,
                    }),
                    Scalar::Bool => Err("`negate` on a Bool".into()),
                }
            }
            Prim::Abs => {
                arity(1, &vals)?;
                match vals[0].ty {
                    Scalar::Int => {
                        let bad = b.ins().icmp_imm_s(IntCC::Equal, vals[0].v, i64::MIN);
                        let payload = self.widen(&vals[0], b);
                        self.trap(Trap::AbsOverflow, span, payload, bad, b);
                        Ok(Val {
                            v: b.ins().iabs(vals[0].v),
                            ty: Scalar::Int,
                        })
                    }
                    Scalar::Float => Ok(Val {
                        v: b.ins().fabs(vals[0].v),
                        ty: Scalar::Float,
                    }),
                    Scalar::Bool => Err("`abs` on a Bool".into()),
                }
            }
            Prim::Sqrt => {
                arity(1, &vals)?;
                if vals[0].ty != Scalar::Float {
                    return Err("`sqrt` of something that is not a Float".into());
                }
                Ok(Val {
                    v: b.ins().sqrt(vals[0].v),
                    ty: Scalar::Float,
                })
            }
            // Cranelift has no transcendental instructions, so these are calls into the C library
            // the executable is linked against — the same `libm` `clang` gives the other backend's
            // `llvm.sin.f64` when it lowers one.
            Prim::Sin | Prim::Cos => {
                arity(1, &vals)?;
                if vals[0].ty != Scalar::Float {
                    return Err(format!("`{}` of something that is not a Float", op.name()));
                }
                let name = if op == Prim::Sin { "sin" } else { "cos" };
                let v = self.libm(name, vals[0].v, b, m)?;
                Ok(Val {
                    v,
                    ty: Scalar::Float,
                })
            }
            Prim::Trunc => {
                arity(1, &vals)?;
                if vals[0].ty != Scalar::Float {
                    return Err("`trunc` of something that is not a Float".into());
                }
                // Saturating, because the evaluator's `f as i64` is: out of range is the nearest
                // representable and NaN is zero, which is what Rust's cast does.
                Ok(Val {
                    v: b.ins().fcvt_to_sint_sat(types::I64, vals[0].v),
                    ty: Scalar::Int,
                })
            }
            Prim::ToFloat => {
                arity(1, &vals)?;
                if vals[0].ty != Scalar::Int {
                    return Err("`float` of something that is not an Int".into());
                }
                // No normalisation: an integer converts to neither a negative zero nor a NaN.
                Ok(Val {
                    v: b.ins().fcvt_from_sint(types::F64, vals[0].v),
                    ty: Scalar::Float,
                })
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                arity(2, &vals)?;
                same(&vals)?;
                Ok(self.compare(op, &vals[0], &vals[1], b))
            }
            Prim::And | Prim::Or | Prim::Not => {
                let want = if op == Prim::Not { 1 } else { 2 };
                arity(want, &vals)?;
                if vals.iter().any(|v| v.ty != Scalar::Bool) {
                    return Err(format!("`{}` on something that is not a Bool", op.name()));
                }
                // `bxor 1` and not a complement: a Bool here is an `I8` holding 0 or 1, and
                // `bnot` would answer 254.
                let v = match op {
                    Prim::Not => b.ins().bxor_imm_s(vals[0].v, 1),
                    Prim::And => b.ins().band(vals[0].v, vals[1].v),
                    _ => b.ins().bor(vals[0].v, vals[1].v),
                };
                Ok(Val {
                    v,
                    ty: Scalar::Bool,
                })
            }
            other => Err(format!(
                "`{}` is not one of the scalar primitives",
                other.name()
            )),
        }
    }

    /// A call into the C library: `double f(double)`.
    fn libm(
        &mut self,
        name: &str,
        x: IrValue,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<IrValue, String> {
        let mut sig =
            cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
        sig.params.push(AbiParam::new(types::F64));
        sig.returns.push(AbiParam::new(types::F64));
        let id = m
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("declaring `{name}`: {e}"))?;
        let f = m.declare_func_in_func(id, b.func);
        let call = b.ins().call(f, &[x]);
        Ok(b.inst_results(call)[0])
    }

    /// `sadd`/`ssub`/`smul` with the overflow bit turned into a trap, which is what
    /// `i64::checked_*` answers `None` for.
    fn checked_int(
        &mut self,
        op: Prim,
        a: &Val,
        c: &Val,
        span: Span,
        b: &mut FunctionBuilder<'_>,
    ) -> Val {
        let (v, of) = match op {
            Prim::Add => b.ins().sadd_overflow(a.v, c.v),
            Prim::Sub => b.ins().ssub_overflow(a.v, c.v),
            _ => b.ins().smul_overflow(a.v, c.v),
        };
        let trap = match op {
            Prim::Add => Trap::AddOverflow,
            Prim::Sub => Trap::SubOverflow,
            _ => Trap::MulOverflow,
        };
        let zero = b.ins().iconst(types::I64, 0);
        self.trap(trap, span, zero, of, b);
        Val { v, ty: Scalar::Int }
    }

    /// `sdiv`/`srem`, guarded exactly where `i64::checked_div`/`checked_rem` answer `None`.
    ///
    /// Two cases, both caught before the instruction runs: a zero divisor, and `i64::MIN / -1`
    /// whose quotient is not representable. Cranelift would trap on either — a real trap, in the
    /// machine's sense, which kills the worker instead of telling the host which span was at
    /// fault.
    fn checked_divide(
        &mut self,
        op: Prim,
        a: &Val,
        c: &Val,
        span: Span,
        b: &mut FunctionBuilder<'_>,
    ) -> Val {
        let zero = b.ins().icmp_imm_s(IntCC::Equal, c.v, 0);
        let min = b.ins().icmp_imm_s(IntCC::Equal, a.v, i64::MIN);
        let neg1 = b.ins().icmp_imm_s(IntCC::Equal, c.v, -1);
        let both = b.ins().band(min, neg1);
        let bad = b.ins().bor(zero, both);
        let trap = if op == Prim::Div {
            Trap::DivOverflow
        } else {
            Trap::RemOverflow
        };
        let payload = b.ins().iconst(types::I64, 0);
        self.trap(trap, span, payload, bad, b);
        let v = if op == Prim::Div {
            b.ins().sdiv(a.v, c.v)
        } else {
            b.ins().srem(a.v, c.v)
        };
        Val { v, ty: Scalar::Int }
    }

    /// `-0.0` becomes `0.0` and every NaN becomes one NaN, because [`beck_core::Value::float`]
    /// does both on every real it makes.
    ///
    /// Applied in three places rather than after every operation — a comparison, a division's
    /// divisor, and a trap's payload — on the invariant [`beck_llvm`] records and measures: a
    /// value in a register here differs from the one the evaluator holds at most in the sign of a
    /// zero or in which NaN it is, and every float operation preserves that. Normalising
    /// everywhere cost 3× on the other backend and buys nothing.
    ///
    /// The NaN half is not theoretical. `0.0 * inf` on x86-64 is the *indefinite* QNaN with its
    /// sign bit set, which sorts below every number under the order key where `f64::NAN` sorts
    /// above every one — so `(0.0 * inf) > 0.0` answers differently without this
    /// (`docs/93` §93.2).
    fn normalise(&mut self, raw: IrValue, b: &mut FunctionBuilder<'_>) -> IrValue {
        let zero = b.ins().f64const(0.0);
        let is_zero = b.ins().fcmp(FloatCC::Equal, raw, zero);
        let zeroed = b.ins().select(is_zero, zero, raw);
        let nan = b.ins().f64const(f64::NAN);
        let is_nan = b.ins().fcmp(FloatCC::Unordered, raw, raw);
        b.ins().select(is_nan, nan, zeroed)
    }

    /// `beck_core`'s order key: `bits ^ ((bits >> 63) | sign)`, an arithmetic shift, so a negative
    /// becomes `!bits` and the derived `Ord` on the result is the numeric order.
    ///
    /// Normalises first, because the two zeros have different keys and the language has one zero.
    fn order_key(&mut self, v: &Val, b: &mut FunctionBuilder<'_>) -> IrValue {
        let normalised = self.normalise(v.v, b);
        let bits = b.ins().bitcast(types::I64, MemFlagsData::new(), normalised);
        let sign = b.ins().sshr_imm_s(bits, 63);
        let mask = b.ins().bor_imm_s(sign, i64::MIN);
        b.ins().bxor(bits, mask)
    }

    fn compare(&mut self, op: Prim, a: &Val, c: &Val, b: &mut FunctionBuilder<'_>) -> Val {
        // Reals compare through the order key and Bools compare unsigned, so `false < true`. Both
        // are the ordering `Value`'s derived `Ord` gives, which is the one the evaluator uses.
        let (lhs, rhs, signed) = match a.ty {
            // **Unsigned**, which is the whole point of the key: the transform maps every real
            // onto the unsigned order, so a signed comparison here answers `-1.0 < 0.0` with
            // `false`. That was this backend's first bug, and the differential is what said so.
            Scalar::Float => (self.order_key(a, b), self.order_key(c, b), false),
            Scalar::Int => (a.v, c.v, true),
            Scalar::Bool => (a.v, c.v, false),
        };
        let cc = match op {
            Prim::Eq => IntCC::Equal,
            Prim::Ne => IntCC::NotEqual,
            Prim::Lt if signed => IntCC::SignedLessThan,
            Prim::Le if signed => IntCC::SignedLessThanOrEqual,
            Prim::Gt if signed => IntCC::SignedGreaterThan,
            Prim::Ge if signed => IntCC::SignedGreaterThanOrEqual,
            Prim::Lt => IntCC::UnsignedLessThan,
            Prim::Le => IntCC::UnsignedLessThanOrEqual,
            Prim::Gt => IntCC::UnsignedGreaterThan,
            _ => IntCC::UnsignedGreaterThanOrEqual,
        };
        Val {
            v: b.ins().icmp(cc, lhs, rhs),
            ty: Scalar::Bool,
        }
    }

    /// The value as an `I64`, which is how it crosses the worker's protocol and how a trap carries
    /// the scrutinee that matched nothing.
    fn widen(&mut self, v: &Val, b: &mut FunctionBuilder<'_>) -> IrValue {
        match v.ty {
            Scalar::Int => v.v,
            // Normalised, because the one thing that reads this is a message: a scrutinee printed
            // as `-0` where the evaluator prints `0` is a divergence in the differential.
            Scalar::Float => {
                let n = self.normalise(v.v, b);
                b.ins().bitcast(types::I64, MemFlagsData::new(), n)
            }
            Scalar::Bool => b.ins().uextend(types::I64, v.v),
        }
    }
}

fn constant(k: &Const, b: &mut FunctionBuilder<'_>) -> Result<Val, String> {
    match k {
        Const::Int(i) => Ok(Val {
            v: b.ins().iconst(types::I64, *i),
            ty: Scalar::Int,
        }),
        Const::Bool(x) => Ok(Val {
            v: b.ins().iconst(types::I8, i64::from(*x)),
            ty: Scalar::Bool,
        }),
        Const::Float(f) => Ok(Val {
            v: b.ins().f64const(*f),
            ty: Scalar::Float,
        }),
        Const::Str(_) => Err("a string constant, and there is no heap here".into()),
        Const::Unit => Err("the unit value, which has no machine representation here".into()),
    }
}

// -------------------------------------------------------------------------------------------
// The worker: the same protocol, emitted rather than written
// -------------------------------------------------------------------------------------------

/// The thunks, the dispatch table and the loop that reads a call and answers it.
///
/// The protocol is [`beck_llvm::Worker`]'s, to the byte, because the host is the same host: eight
/// bytes of header, eight per argument, and a 24-byte reply of trap code, span index, payload and
/// result. Two spellings of one wire would be the drift this workspace spends its gates on.
fn driver(
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
    functions: &[Signature],
    ids: &BTreeMap<Arc<str>, FuncId>,
    order: &[Arc<str>],
) -> Result<(), String> {
    let ptr = m.target_config().pointer_type();
    let conv = CallConv::triple_default(m.isa().triple());
    let flags = MemFlagsData::trusted();

    // `read(0, p, n)` and `write(1, p, n)`, the two C functions this needs.
    let mut io = cranelift_codegen::ir::Signature::new(conv);
    io.params.push(AbiParam::new(types::I32));
    io.params.push(AbiParam::new(ptr));
    io.params.push(AbiParam::new(types::I64));
    io.returns.push(AbiParam::new(types::I64));
    let read = m
        .declare_function("read", Linkage::Import, &io)
        .map_err(|e| format!("declaring `read`: {e}"))?;
    let write = m
        .declare_function("write", Linkage::Import, &io)
        .map_err(|e| format!("declaring `write`: {e}"))?;

    // `(p, n) -> how many bytes moved`, one for each direction. A short count means the pipe is
    // closed, which is how the worker learns the host has gone.
    let mut movesig = cranelift_codegen::ir::Signature::new(conv);
    movesig.params.push(AbiParam::new(ptr));
    movesig.params.push(AbiParam::new(types::I64));
    movesig.returns.push(AbiParam::new(types::I64));
    let read_exact = m
        .declare_function("beck.read_exact", Linkage::Local, &movesig)
        .map_err(|e| format!("declaring the reader: {e}"))?;
    let write_all = m
        .declare_function("beck.write_all", Linkage::Local, &movesig)
        .map_err(|e| format!("declaring the writer: {e}"))?;

    for (id, io_id, fd) in [(read_exact, read, 0i64), (write_all, write, 1)] {
        ctx.func = Function::with_name_signature(UserFuncName::user(1, fd as u32), movesig.clone());
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let p = b.block_params(entry)[0];
            let n = b.block_params(entry)[1];

            // `loop(done)`: how many bytes have moved so far.
            let loop_ = b.create_block();
            b.append_block_param(loop_, types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.ins().jump(loop_, &[zero.into()]);
            b.seal_block(entry);

            b.switch_to_block(loop_);
            let done = b.block_params(loop_)[0];
            let left = b.ins().isub(n, done);
            let full = b.ins().icmp_imm_s(IntCC::Equal, left, 0);
            let out = b.create_block();
            // `out` takes the count as a parameter: two edges reach it — the loop finishing and
            // the pipe closing early — and this is the `phi` they join into.
            b.append_block_param(out, types::I64);
            let again = b.create_block();
            b.ins().brif(full, out, &[done.into()], again, &[]);

            b.switch_to_block(again);
            b.seal_block(again);
            let dst = b.ins().iadd(p, done);
            let f = m.declare_func_in_func(io_id, b.func);
            let fdv = b.ins().iconst(types::I32, fd);
            let call = b.ins().call(f, &[fdv, dst, left]);
            let moved = b.inst_results(call)[0];
            let stop = b.ins().icmp_imm_s(IntCC::SignedLessThan, moved, 1);
            let cont = b.create_block();
            b.ins().brif(stop, out, &[done.into()], cont, &[]);

            b.switch_to_block(cont);
            b.seal_block(cont);
            let next = b.ins().iadd(done, moved);
            b.ins().jump(loop_, &[next.into()]);
            b.seal_block(loop_);

            b.switch_to_block(out);
            b.seal_block(out);
            let moved = b.block_params(out)[0];
            b.ins().return_(&[moved]);
            b.finalize(m.target_config());
        }
        m.define_function(id, ctx)
            .map_err(|e| format!("defining the pipe helper: {e}"))?;
        m.clear_context(ctx);
    }

    // One thunk per compiled function: the protocol carries every argument as eight bytes, so this
    // is where an `i64` becomes a `double` or a `bool` and the result becomes eight bytes again.
    let mut thunk_sig = cranelift_codegen::ir::Signature::new(conv);
    thunk_sig.params.push(AbiParam::new(ptr));
    thunk_sig.params.push(AbiParam::new(ptr));
    thunk_sig.returns.push(AbiParam::new(types::I64));
    let mut thunks = Vec::new();
    for (i, name) in order.iter().enumerate() {
        let sig = &functions[i];
        let id = m
            .declare_function(
                &format!("beck.thunk.{}", sig.index),
                Linkage::Local,
                &thunk_sig,
            )
            .map_err(|e| format!("declaring a thunk: {e}"))?;
        thunks.push(id);
        ctx.func =
            Function::with_name_signature(UserFuncName::user(2, i as u32), thunk_sig.clone());
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            b.seal_block(entry);
            let err = b.block_params(entry)[0];
            let args = b.block_params(entry)[1];
            let mut operands = vec![err];
            for (k, p) in sig.params.iter().enumerate() {
                let raw = b.ins().load(types::I64, flags, args, (k * 8) as i32);
                operands.push(match p {
                    Scalar::Int => raw,
                    Scalar::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), raw),
                    // Any non-zero is `true`, which is what the host writes and what a Bool in a
                    // register here has to hold. `icmp` already answers in an `I8` holding 0 or 1,
                    // so there is nothing to extend — and extending an `I8` to an `I8` is what the
                    // verifier refuses.
                    Scalar::Bool => b.ins().icmp_imm_s(IntCC::NotEqual, raw, 0),
                });
            }
            let f = m.declare_func_in_func(ids[name], b.func);
            let call = b.ins().call(f, &operands);
            let out = b.inst_results(call)[0];
            let bits = match sig.ret {
                Scalar::Int => out,
                Scalar::Float => b.ins().bitcast(types::I64, MemFlagsData::new(), out),
                Scalar::Bool => b.ins().uextend(types::I64, out),
            };
            b.ins().return_(&[bits]);
            b.finalize(m.target_config());
        }
        m.define_function(id, ctx)
            .map_err(|e| format!("defining a thunk: {e}"))?;
        m.clear_context(ctx);
    }

    // `dispatch(idx, err, args)`. A chain of comparisons rather than a jump table: it runs once
    // per call, behind a pipe round trip that `docs/93` §93.5 measures at 36 µs, so what it costs
    // is not measurable and what it saves is a page of table-building.
    let mut disp_sig = cranelift_codegen::ir::Signature::new(conv);
    disp_sig.params.push(AbiParam::new(types::I32));
    disp_sig.params.push(AbiParam::new(ptr));
    disp_sig.params.push(AbiParam::new(ptr));
    disp_sig.returns.push(AbiParam::new(types::I64));
    let dispatch = m
        .declare_function("beck.dispatch", Linkage::Local, &disp_sig)
        .map_err(|e| format!("declaring the dispatch: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(3, 0), disp_sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let idx = b.block_params(entry)[0];
        let err = b.block_params(entry)[1];
        let args = b.block_params(entry)[2];

        for (i, thunk) in thunks.iter().enumerate() {
            let hit = b.create_block();
            let miss = b.create_block();
            let is = b.ins().icmp_imm_s(IntCC::Equal, idx, i as i64);
            b.ins().brif(is, hit, &[], miss, &[]);
            b.switch_to_block(hit);
            b.seal_block(hit);
            let f = m.declare_func_in_func(*thunk, b.func);
            let call = b.ins().call(f, &[err, args]);
            let v = b.inst_results(call)[0];
            b.ins().return_(&[v]);
            b.switch_to_block(miss);
            b.seal_block(miss);
        }
        // An index the host never sends is still an index the worker has to answer.
        let unknown = b.ins().iconst(types::I32, 255);
        b.ins().store(flags, unknown, err, 0);
        let z = b.ins().iconst(types::I64, 0);
        b.ins().return_(&[z]);
        b.finalize(m.target_config());
    }
    m.define_function(dispatch, ctx)
        .map_err(|e| format!("defining the dispatch: {e}"))?;
    m.clear_context(ctx);

    // `main`: read a call, answer it, repeat until the host closes the pipe.
    let mut main_sig = cranelift_codegen::ir::Signature::new(conv);
    main_sig.returns.push(AbiParam::new(types::I32));
    let main = m
        .declare_function("main", Linkage::Export, &main_sig)
        .map_err(|e| format!("declaring `main`: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(4, 0), main_sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let slot = |b: &mut FunctionBuilder<'_>, bytes: u32| {
            b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3))
        };
        let req = slot(&mut b, 8);
        let argbuf = slot(&mut b, (MAX_PARAMS * 8) as u32);
        let cell = slot(&mut b, 24);
        let resp = slot(&mut b, 24);
        let req = b.ins().stack_addr(ptr, req, 0);
        let argbuf = b.ins().stack_addr(ptr, argbuf, 0);
        let cell = b.ins().stack_addr(ptr, cell, 0);
        let resp = b.ins().stack_addr(ptr, resp, 0);

        let loop_ = b.create_block();
        let done = b.create_block();
        b.ins().jump(loop_, &[]);
        b.seal_block(entry);

        b.switch_to_block(loop_);
        let rd = m.declare_func_in_func(read_exact, b.func);
        let eight = b.ins().iconst(types::I64, 8);
        let call = b.ins().call(rd, &[req, eight]);
        let head = b.inst_results(call)[0];
        let closed = b.ins().icmp_imm_s(IntCC::NotEqual, head, 8);
        let sized = b.create_block();
        b.ins().brif(closed, done, &[], sized, &[]);

        b.switch_to_block(sized);
        b.seal_block(sized);
        let idx = b.ins().load(types::I32, flags, req, 0);
        let count = b.ins().load(types::I32, flags, req, 4);
        let count = b.ins().uextend(types::I64, count);
        let bytes = b.ins().imul_imm_s(count, 8);
        let call = b.ins().call(rd, &[argbuf, bytes]);
        let got = b.inst_results(call)[0];
        let short = b.ins().icmp(IntCC::NotEqual, got, bytes);
        let run = b.create_block();
        b.ins().brif(short, done, &[], run, &[]);

        b.switch_to_block(run);
        b.seal_block(run);
        // The cell is cleared before every call: a trap code left over from the last one would be
        // this one's answer.
        let z64 = b.ins().iconst(types::I64, 0);
        b.ins().store(flags, z64, cell, 0);
        b.ins().store(flags, z64, cell, CELL_PAYLOAD);
        let d = m.declare_func_in_func(dispatch, b.func);
        let call = b.ins().call(d, &[idx, cell, argbuf]);
        let result = b.inst_results(call)[0];
        let code = b.ins().load(types::I64, flags, cell, 0);
        let payload = b.ins().load(types::I64, flags, cell, CELL_PAYLOAD);
        b.ins().store(flags, code, resp, 0);
        b.ins().store(flags, payload, resp, 8);
        b.ins().store(flags, result, resp, 16);
        let wr = m.declare_func_in_func(write_all, b.func);
        let twenty4 = b.ins().iconst(types::I64, 24);
        let call = b.ins().call(wr, &[resp, twenty4]);
        let wrote = b.inst_results(call)[0];
        let gone = b.ins().icmp_imm_s(IntCC::NotEqual, wrote, 24);
        let round = b.create_block();
        b.ins().brif(gone, done, &[], round, &[]);

        b.switch_to_block(round);
        b.seal_block(round);
        b.ins().jump(loop_, &[]);
        b.seal_block(loop_);

        b.switch_to_block(done);
        b.seal_block(done);
        let ok = b.ins().iconst(types::I32, 0);
        b.ins().return_(&[ok]);
        b.finalize(m.target_config());
    }
    m.define_function(main, ctx)
        .map_err(|e| format!("defining `main`: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}
