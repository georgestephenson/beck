//! `Core` → Cranelift IR → an object file.
//!
//! # The same subset, a second time
//!
//! This compiles exactly what [`beck_llvm::emit`] compiles: a definition whose parameters and
//! result each have a [`beck_llvm::heap::Repr`] — `Int`, `Float`, `Bool`, a `Str`, a `list`, a
//! `Map`, an `Html`, or a `model`, `union` or `newtype` — and whose body is built from constants,
//! variables, `let`, `if`, `match`, direct calls, record and variant construction, field reads,
//! `with`, lambdas and applications, and the arithmetic, comparison, logical, text, collection and
//! view primitives. It asks the host the same four questions through the same frame
//! ([`beck_llvm::Upcall`]), calls the same runtime library for the fifteen primitives that are a
//! table or somebody else's parser ([`beck_llvm::prim`]), and refuses what is refused there by
//! name, with a reason.
//!
//! Writing the selection a second time rather than importing it is deliberate. The two emitters
//! are held to *agreeing* — `cranelift.rs` asserts that they accept and refuse the same
//! definitions over every program in the tree — and a shared implementation would make that
//! agreement true by construction and therefore worth nothing. What *is* shared is the vocabulary
//! and the wire: [`beck_llvm::Scalar`], [`beck_llvm::Signature`], [`beck_llvm::Refusal`] and
//! [`beck_llvm::Trap`] are types, [`beck_llvm::Trap`]'s codes are a protocol the host decodes, and
//! [`beck_llvm::heap`] is the **layout** — which word a field is in, which rank a variant has —
//! because that one is a contract with the host too, and a contract with three spellings drifts.
//! [`beck_llvm::prim`] joins that list for the same reason: the op codes, the arities and the
//! outcome record are a contract with a *linked library*, so what is written twice here is how the
//! call is made and not what it means. A second copy of any of those would be two opinions about
//! one thing.
//!
//! # Agreeing with the evaluator exactly
//!
//! Every decision [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.3 records is made
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
//!   [`27`](../../../../../docs/27-the-walls-come-down-report.md)'s property holds on this backend too.
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
use beck_llvm::heap::{self, Heap, Repr};
use beck_llvm::{prim, Refusal, Scalar, Signature, Trap, Upcall, MAX_PARAMS};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Block, Function, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind,
    Type, UserFuncName, Value as IrValue,
};
use cranelift_codegen::isa::{self, CallConv, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module as _};
use cranelift_object::{ObjectBuilder, ObjectModule};

/// The trap cell's layout, which is the host's [`beck_llvm::Worker`] protocol: a `u32` code, a
/// `u32` span index, and an `i64` payload.
const CELL_SPAN: i32 = 4;
const CELL_PAYLOAD: i32 = 8;
/// The third word, which only a `raise` writes and only a `try:` reads: the offset of the raised
/// type's name in the literal pool. It never reaches the host — see `beck_llvm::Trap::Raised`.
const CELL_RAISED: i32 = 16;

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
    /// Whether anything in it calls the runtime library, and therefore whether the link step has
    /// to put the archive on the line ([`beck_llvm::prim`]).
    pub links: bool,
    /// What every object in this module looks like. [`beck_llvm::heap`] decides the shape for both
    /// backends, because the host marshals against it and three spellings of one contract drift.
    pub heap: Heap,
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
    // Specialised first, so everything below sees one definition per instantiation and never a type
    // parameter. Shared with the other emitter — [`beck_llvm::mono`] is a pass over the *program*
    // and not a code generator, so writing it twice would mean two different subsets compile.
    let mono = beck_llvm::mono::specialise(program);
    let program = &mono.program;
    let isa = host_isa()?;
    let mut refusals: Vec<Refusal> = Vec::new();
    let mut sigs: BTreeMap<Arc<str>, Signature> = BTreeMap::new();
    let mut heap = Heap::new();
    heap::survey(program, &mut heap);

    // Round one: the signature alone. A definition whose parameters or result have no machine
    // representation cannot be called through the worker's protocol however simple its body is.
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
        match build(program, &sigs, &eligible, isa.clone(), &mut heap) {
            Ok(built) => {
                refusals.sort_by(|a, b| a.name.cmp(&b.name));
                return Ok(Module {
                    object: built.object,
                    clif: built.clif,
                    functions: built.functions,
                    spans: built.spans,
                    refusals,
                    links: built.links,
                    heap,
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
    // backend that guarantees `docs/27`'s tail calls has to keep them.
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
    /// Whether the runtime library's symbols are named in this object.
    links: bool,
}

/// One round: declare everything eligible, define every body, and emit the object.
fn build(
    program: &Program,
    sigs: &BTreeMap<Arc<str>, Signature>,
    eligible: &BTreeSet<Arc<str>>,
    isa: Arc<dyn TargetIsa>,
    heap: &mut Heap,
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

    // The arena, when this program has an object in it at all. A module of pure arithmetic gets
    // neither the globals nor the `malloc`, which is what keeps `docs/93` §93.5's round trip the
    // same round trip.
    let arena = if heap.is_empty() {
        None
    } else {
        Some(Arena::declare(&mut object, ptr).map_err(Failure::Fatal)?)
    };
    // Text's runtime, when the program has any: a `Str` has no layout, so `Heap::uses_text` is
    // what answers this and `Heap::layouts` cannot.
    let text = if heap.uses_text() {
        Some(Text::declare(&mut object, ptr).map_err(Failure::Fatal)?)
    } else {
        None
    };
    // A map turns into a list — `map_keys` and `map_values` allocate one — so its runtime needs
    // lists' even when the program never writes one down.
    let lists = if heap.uses_lists() || heap.uses_maps() {
        Some(Lists::declare(&mut object, ptr).map_err(Failure::Fatal)?)
    } else {
        None
    };
    let maps = if heap.uses_maps() {
        Some(Maps::declare(&mut object, ptr).map_err(Failure::Fatal)?)
    } else {
        None
    };
    let builds = match text {
        Some(_) => {
            Some(Builds::declare(&mut object, ptr, lists.is_some()).map_err(Failure::Fatal)?)
        }
        None => None,
    };
    // A question needs the arena, because an answer is written into it — and it has one, because
    // asking interns the shape of what it answers with and a module with an interned shape is not
    // an empty heap.
    let host = match (arena, asks_the_host(program, eligible)) {
        (Some(_), true) => Some(Host::declare(&mut object, ptr).map_err(Failure::Fatal)?),
        _ => None,
    };
    // The runtime library, on the same terms and for the same reason: a program that calls none of
    // its primitives must not name its symbols, or every object file would carry an undefined
    // reference to an archive its link step has no reason to put on the line.
    let linked = match (arena, calls_the_runtime_library(program, eligible)) {
        (Some(_), true) => Some(Linked::declare(&mut object, ptr).map_err(Failure::Fatal)?),
        _ => None,
    };
    let runtime = Runtime {
        arena,
        text,
        lists,
        maps,
        builds,
        host,
        linked,
    };

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
    let mut compared: BTreeSet<u32> = BTreeSet::new();
    let mut list_compared: BTreeSet<u32> = BTreeSet::new();
    let mut map_compared: BTreeSet<u32> = BTreeSet::new();
    let mut compared_fns = false;
    let mut applied: BTreeSet<u32> = BTreeSet::new();
    let mut loops: BTreeSet<(Loop, u32)> = BTreeSet::new();
    // Every `lam` still to be written, and every rank already written. A `lam` met while a body is
    // being built cannot be defined there — one context is being held — so it waits here.
    let mut pending: Vec<(Arc<str>, Pending)> = Vec::new();
    let mut lams: BTreeSet<u32> = BTreeSet::new();

    for (n, name) in order.iter().enumerate() {
        let def = &program.defs[name];
        let sig = beck_signature(&indexed[name], ptr);
        ctx.func = Function::with_name_signature(UserFuncName::user(0, n as u32), sig);
        let taken = std::mem::take(&mut spans);
        let mut body = Body::new(&indexed, eligible, &ids, taken, program, heap, runtime);
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
        compared.append(&mut body.compared);
        list_compared.append(&mut body.list_compared);
        map_compared.append(&mut body.map_compared);
        applied.append(&mut body.applied);
        loops.append(&mut body.loops);
        compared_fns |= body.compared_fns;
        pending.extend(
            std::mem::take(&mut body.pending)
                .into_iter()
                .map(|p| (name.clone(), p)),
        );
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
    // The lambdas, drained: each is its own function, and one met inside another goes on the end of
    // the queue. A `lam` that will not emit refuses the *definition* it was written in — the object
    // is thrown away and the round runs again without it, which is what makes this the same fixed
    // point a body's own refusal is part of.
    while let Some((owner, lam)) = pending.pop() {
        if !lams.insert(lam.rank) {
            continue;
        }
        if arena.is_none() {
            // The survey sets `uses_closures` for every `lam` under anything and `is_empty` reads
            // it, so a closure here without an arena would be those two disagreeing.
            return Err(Failure::Fatal(
                "a closure in a module with no arena: `Heap::is_empty` and \
                 `Heap::uses_closures` disagree"
                    .into(),
            ));
        }
        let fam = heap.family(lam.family).clone();
        let sig = family_signature(&fam, ptr);
        let id = object
            .declare_function(&lam_symbol(lam.rank), Linkage::Local, &sig)
            .map_err(|e| Failure::Fatal(format!("declaring a lambda: {e}")))?;
        ctx.func = Function::with_name_signature(UserFuncName::user(16, lam.rank), sig);
        let taken = std::mem::take(&mut spans);
        let mut body = Body::new(&indexed, eligible, &ids, taken, program, heap, runtime);
        let emitted = {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fctx);
            let outcome = body.emit_lam(&lam, &mut b, &mut object);
            if outcome.is_ok() {
                b.finalize(object.target_config());
            }
            outcome
        };
        if emitted.is_err() {
            fctx = FunctionBuilderContext::new();
        }
        spans = std::mem::take(&mut body.spans);
        compared.append(&mut body.compared);
        list_compared.append(&mut body.list_compared);
        map_compared.append(&mut body.map_compared);
        applied.append(&mut body.applied);
        loops.append(&mut body.loops);
        compared_fns |= body.compared_fns;
        pending.extend(
            std::mem::take(&mut body.pending)
                .into_iter()
                .map(|p| (owner.clone(), p)),
        );
        match emitted {
            Ok(()) => {}
            Err(reason) => {
                refused.push(Refusal {
                    name: owner,
                    reason,
                });
                object.clear_context(&mut ctx);
                continue;
            }
        }
        clif.push_str(&format!("; lam {}\n{}\n", lam.rank, ctx.func));
        object
            .define_function(id, &mut ctx)
            .map_err(|e| Failure::Fatal(format!("defining a lambda: {e}")))?;
        object.clear_context(&mut ctx);
    }

    if !refused.is_empty() {
        return Err(Failure::Refused(refused));
    }

    if let Some(arena) = arena {
        arena
            .define(&mut object, &mut ctx, &mut fctx, ptr)
            .map_err(Failure::Fatal)?;
        if let Some(host) = host {
            host.define(arena, &mut object, &mut ctx, &mut fctx, ptr)
                .map_err(Failure::Fatal)?;
        }
        if let Some(text) = text {
            text.define(arena, &mut object, &mut ctx, &mut fctx, ptr)
                .map_err(Failure::Fatal)?;
            if let Some(builds) = builds {
                builds
                    .define(runtime, &mut object, &mut ctx, &mut fctx, ptr)
                    .map_err(Failure::Fatal)?;
            }
        }
        if let Some(lists) = lists {
            lists
                .define(arena, &mut object, &mut ctx, &mut fctx, ptr)
                .map_err(Failure::Fatal)?;
            if let Some(maps) = maps {
                maps.define(arena, lists, &mut object, &mut ctx, &mut fctx, ptr)
                    .map_err(Failure::Fatal)?;
            }
        }
        let (layouts, element_reprs, entries) =
            reachable(&compared, &list_compared, &map_compared, heap);
        for at in layouts {
            compare_function(at, heap, arena, &mut object, &mut ctx, &mut fctx)
                .map_err(Failure::Fatal)?;
        }
        for at in element_reprs {
            element_functions(at, heap, arena, runtime, &mut object, &mut ctx, &mut fctx)
                .map_err(Failure::Fatal)?;
        }
        for at in entries {
            map_functions(at, heap, arena, runtime, &mut object, &mut ctx, &mut fctx)
                .map_err(Failure::Fatal)?;
        }
        for at in &applied {
            apply_function(
                *at,
                heap,
                &lams,
                &ids,
                &mut object,
                &mut ctx,
                &mut fctx,
                arena,
            )
            .map_err(Failure::Fatal)?;
        }
        for (which, at) in &loops {
            loop_function(
                *which,
                *at,
                heap,
                arena,
                runtime,
                &mut object,
                &mut ctx,
                &mut fctx,
            )
            .map_err(Failure::Fatal)?;
        }
        // One merge sort per key repr, over the families that sort — deduplicated here because two
        // families can sort by the same kind of key and one function has one definition.
        let sorted: BTreeSet<u32> = loops
            .iter()
            .filter(|(which, _)| *which == Loop::Sort)
            .filter_map(|(_, fam)| heap.word_at(heap.family(*fam).ret))
            .collect();
        for at in sorted {
            merge_sort(at, &mut object, &mut ctx, &mut fctx).map_err(Failure::Fatal)?;
        }
        if compared_fns {
            fn_compare(&mut object, &mut ctx, &mut fctx, arena).map_err(Failure::Fatal)?;
        }
    }

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    driver(
        &mut object,
        &mut ctx,
        &mut fctx,
        &functions,
        &ids,
        &order,
        arena,
        runtime.linked,
    )
    .map_err(Failure::Fatal)?;

    let object = object
        .finish()
        .emit()
        .map_err(|e| Failure::Fatal(format!("emitting the object: {e}")))?;
    Ok(Built {
        object,
        clif,
        functions,
        spans,
        links: runtime.linked.is_some(),
    })
}

/// The signature, or the reason there is not one.
///
/// The same four rules [`beck_llvm`] applies, written again rather than imported — see this
/// module's own documentation for why, and `cranelift.rs` for what holds them together.
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
            "{} parameters, and the worker's argument buffer holds {MAX_PARAMS}",
            def.params.len()
        ));
    }
    let mut params = Vec::with_capacity(def.params.len());
    for (_, name, ty) in &def.params {
        match heap.repr(ty, program) {
            Ok(r) => {
                Heap::crossing(r).map_err(|why| format!("parameter `{name}` is {why}"))?;
                heap.inbound(r)
                    .map_err(|why| format!("parameter `{name}` is {why}"))?;
                params.push(r);
            }
            Err(why) => return Err(format!("parameter `{name}` is {why}")),
        }
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

/// The Cranelift type a scalar is carried in.
///
/// A `Bool` is an `I8` holding 0 or 1: Cranelift has no one-bit integer, and every comparison it
/// emits produces exactly this. The invariant is what makes `band` and `bor` the right
/// instructions for `and` and `or`, and it is why `not` is `bxor 1` rather than a complement.
fn machine(r: Repr) -> Type {
    match r.machine() {
        Scalar::Int => types::I64,
        Scalar::Float => types::F64,
        Scalar::Bool => types::I8,
    }
}

/// A compiled definition's signature: the error cell, then its parameters.
///
/// [`CallConv::Tail`] because a call in tail position must be a jump — `docs/27` makes that a
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
///
/// `beck.def.` and not `beck.`, for the reason `beck_llvm::emit::mangle` gives: everything either
/// emitter generates for itself is `beck.<something>`, so a definition called `dispatch` used to
/// take the dispatcher's own symbol.
fn symbol(name: &str) -> String {
    let mut out = String::from("beck.def.");
    for b in name.bytes() {
        match b {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("${b:02X}")),
        }
    }
    out
}

/// What a module carries besides its own definitions: the arena, text's runtime and lists'.
///
/// One value rather than three parameters, because a body is handed whichever of them its module
/// has and the set is decided once — a module with text or a list in it is a module with an arena
/// by construction.
#[derive(Clone, Copy, Debug)]
struct Runtime {
    arena: Option<Arena>,
    text: Option<Text>,
    lists: Option<Lists>,
    maps: Option<Maps>,
    builds: Option<Builds>,
    /// The second direction of the protocol, when this program asks the host anything.
    host: Option<Host>,
    /// The runtime library, when this program calls one of its primitives.
    linked: Option<Linked>,
}

/// The four list functions that do not care what an element *is*.
///
/// A list is one header word — how many — and one word per element, so allocating one, taking a
/// range out of one and turning one around are word moves and nothing else. Everything that has to
/// know what a word means is generated per element repr instead, by [`element_functions`].
#[derive(Clone, Copy, Debug)]
struct Lists {
    /// A data block of a given capacity, with `used` written — see `beck_llvm::heap::LIST_HEADER`
    /// for why a list is two objects.
    block: FuncId,
    /// A header over a block.
    head: FuncId,
    alloc: FuncId,
    /// `list_append`: a new header over the same block when this list stands at its end and the
    /// block has room, and a doubled copy otherwise.
    append: FuncId,
    len: FuncId,
    /// `list_slice`, `list_take` and `list_drop` at once: all three are "a range, clamped", and the
    /// clamping is arithmetic the caller does.
    copy: FuncId,
    reverse: FuncId,
    /// `concat_lists`: a sum over the outer list's header words, then one allocation and one
    /// `memcpy` per inner list. Not a growth — see the other emitter's `refusal` for the correction
    /// that says why it was refused as one.
    concat: FuncId,
}

impl Lists {
    fn declare(m: &mut ObjectModule, ptr: Type) -> Result<Lists, String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut one = |name: &str, params: &[Type], ret: Type| -> Result<FuncId, String> {
            let mut sig = cranelift_codegen::ir::Signature::new(conv);
            for p in params {
                sig.params.push(AbiParam::new(*p));
            }
            sig.returns.push(AbiParam::new(ret));
            m.declare_function(name, Linkage::Local, &sig)
                .map_err(|e| format!("declaring `{name}`: {e}"))
        };
        Ok(Lists {
            block: one(
                "beck.list.block",
                &[ptr, types::I64, types::I64, types::I32],
                types::I64,
            )?,
            head: one(
                "beck.list.head",
                &[ptr, types::I64, types::I64, types::I32],
                types::I64,
            )?,
            alloc: one(
                "beck.list.alloc",
                &[ptr, types::I64, types::I32],
                types::I64,
            )?,
            append: one(
                "beck.list.append",
                &[ptr, types::I64, types::I64, types::I32],
                types::I64,
            )?,
            len: one("beck.list.len", &[types::I64], types::I64)?,
            copy: one(
                "beck.list.copy",
                &[ptr, types::I64, types::I64, types::I64, types::I32],
                types::I64,
            )?,
            reverse: one(
                "beck.list.reverse",
                &[ptr, types::I64, types::I32],
                types::I64,
            )?,
            concat: one(
                "beck.list.concat",
                &[ptr, types::I64, types::I32],
                types::I64,
            )?,
        })
    }

    /// Where a list's elements start, as an address.
    ///
    /// Through the block, which is the one load `beck_llvm::heap::LIST_HEADER`'s indirection costs —
    /// and it is paid once per operation rather than once per element, because every loop takes this
    /// pointer before it starts.
    fn data(
        self,
        arena: Arena,
        xs: IrValue,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let base = arena.base(b, m);
        let ph = b.ins().iadd(base, xs);
        let d = b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), ph, heap::WORD as i32);
        let at = b.ins().iadd(base, d);
        b.ins().iadd_imm_s(at, heap::DATA_HEADER as i64)
    }

    /// The header word, which is how many elements there are.
    fn count(
        self,
        arena: Arena,
        xs: IrValue,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let base = arena.base(b, m);
        let p = b.ins().iadd(base, xs);
        b.ins().load(types::I64, MemFlagsData::trusted(), p, 0)
    }

    fn define(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        // The data block: `[cap, used, elements…]`.
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.block, sig, 24, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let cap = b.block_params(entry)[1];
            let used = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            let body = b.ins().imul_imm_s(cap, heap::WORD as i64);
            let total = b.ins().iadd_imm_s(body, heap::DATA_HEADER as i64);
            let f = arena.alloc_in(b, m);
            let call = b.ins().call(f, &[err, total, span]);
            let off = b.inst_results(call)[0];
            let fill = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let failed = b.ins().icmp_imm_s(IntCC::Equal, off, 0);
            b.ins().brif(failed, out, &[off.into()], fill, &[]);
            b.switch_to_block(fill);
            let base = arena.base(b, m);
            let p = b.ins().iadd(base, off);
            b.ins().store(MemFlagsData::trusted(), cap, p, 0);
            b.ins()
                .store(MemFlagsData::trusted(), used, p, heap::WORD as i32);
            b.ins().jump(out, &[off.into()]);
            b.switch_to_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })?;

        // The header: `[count, block]`.
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.head, sig, 25, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let n = b.block_params(entry)[1];
            let data = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            let f = arena.alloc_in(b, m);
            let total = b.ins().iconst(types::I64, heap::LIST_HEADER as i64);
            let call = b.ins().call(f, &[err, total, span]);
            let off = b.inst_results(call)[0];
            let fill = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let failed = b.ins().icmp_imm_s(IntCC::Equal, off, 0);
            b.ins().brif(failed, out, &[off.into()], fill, &[]);
            b.switch_to_block(fill);
            let base = arena.base(b, m);
            let p = b.ins().iadd(base, off);
            b.ins().store(MemFlagsData::trusted(), n, p, 0);
            b.ins()
                .store(MemFlagsData::trusted(), data, p, heap::WORD as i32);
            b.ins().jump(out, &[off.into()]);
            b.switch_to_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })?;

        // A list of exactly `n`: a block that size, and a header over it.
        let sig = Text::signature(m, &[ptr, types::I64, types::I32], types::I64);
        Text::wrote(self.alloc, sig, 10, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let n = b.block_params(entry)[1];
            let span = b.block_params(entry)[2];
            let f = m.declare_func_in_func(self.block, b.func);
            let call = b.ins().call(f, &[err, n, n, span]);
            let d = b.inst_results(call)[0];
            let top = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let failed = b.ins().icmp_imm_s(IntCC::Equal, d, 0);
            b.ins().brif(failed, out, &[d.into()], top, &[]);
            b.switch_to_block(top);
            let f = m.declare_func_in_func(self.head, b.func);
            let call = b.ins().call(f, &[err, n, d, span]);
            let h = b.inst_results(call)[0];
            b.ins().jump(out, &[h.into()]);
            b.switch_to_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })?;

        // `list_append`. The test is `count == used`: every header over a block has a count of at
        // most `used`, so the slot at `used` is one no reader can see. Writing it and answering a
        // *new* header leaves every existing list exactly as it was.
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.append, sig, 26, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let xs = b.block_params(entry)[1];
            let w = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            let flags = MemFlagsData::trusted();
            let base = arena.base(b, m);
            let ph = b.ins().iadd(base, xs);
            let n = b.ins().load(types::I64, flags, ph, 0);
            let d = b.ins().load(types::I64, flags, ph, heap::WORD as i32);
            let pb = b.ins().iadd(base, d);
            let cap = b.ins().load(types::I64, flags, pb, 0);
            let used = b.ins().load(types::I64, flags, pb, heap::WORD as i32);

            let push = b.create_block();
            let grow = b.create_block();
            let done = b.create_block();
            b.append_block_param(done, types::I64);
            b.append_block_param(done, types::I64);
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let at_end = b.ins().icmp(IntCC::Equal, n, used);
            let room = b.ins().icmp(IntCC::UnsignedLessThan, used, cap);
            let fits = b.ins().band(at_end, room);
            b.ins().brif(fits, push, &[], grow, &[]);

            b.switch_to_block(push);
            b.seal_block(push);
            let off = b.ins().imul_imm_s(n, heap::WORD as i64);
            let slot = b.ins().iadd(pb, off);
            b.ins().store(flags, w, slot, heap::DATA_HEADER as i32);
            let n1 = b.ins().iadd_imm_s(n, 1);
            b.ins().store(flags, n1, pb, heap::WORD as i32);
            b.ins().jump(done, &[d.into(), n1.into()]);

            b.switch_to_block(grow);
            b.seal_block(grow);
            let want = b.ins().iadd_imm_s(n, 1);
            let twice = b.ins().imul_imm_s(want, 2);
            let four = b.ins().iconst(types::I64, 4);
            let small = b.ins().icmp(IntCC::UnsignedLessThan, twice, four);
            let cap2 = b.ins().select(small, four, twice);
            let f = m.declare_func_in_func(self.block, b.func);
            let call = b.ins().call(f, &[err, cap2, want, span]);
            let d2 = b.inst_results(call)[0];
            let move_ = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, d2, 0);
            b.ins().brif(failed, out, &[d2.into()], move_, &[]);

            b.switch_to_block(move_);
            b.seal_block(move_);
            let base = arena.base(b, m);
            let pb2 = b.ins().iadd(base, d2);
            let pe2 = b.ins().iadd_imm_s(pb2, heap::DATA_HEADER as i64);
            let from = self.data(arena, xs, b, m);
            let bytes = b.ins().imul_imm_s(n, heap::WORD as i64);
            b.call_memcpy(m.target_config(), pe2, from, bytes);
            let off = b.ins().imul_imm_s(n, heap::WORD as i64);
            let slot = b.ins().iadd(pe2, off);
            b.ins().store(flags, w, slot, 0);
            b.ins().jump(done, &[d2.into(), want.into()]);

            b.switch_to_block(done);
            b.seal_block(done);
            let block = b.block_params(done)[0];
            let len = b.block_params(done)[1];
            let f = m.declare_func_in_func(self.head, b.func);
            let call = b.ins().call(f, &[err, len, block, span]);
            let h = b.inst_results(call)[0];
            b.ins().jump(out, &[h.into()]);

            b.switch_to_block(out);
            b.seal_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })?;

        let sig = Text::signature(m, &[types::I64], types::I64);
        Text::wrote(self.len, sig, 11, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let xs = b.block_params(entry)[0];
            let n = self.count(arena, xs, b, m);
            b.ins().return_(&[n]);
        })?;

        let sig = Text::signature(
            m,
            &[ptr, types::I64, types::I64, types::I64, types::I32],
            types::I64,
        );
        Text::wrote(self.copy, sig, 12, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let xs = b.block_params(entry)[1];
            let from = b.block_params(entry)[2];
            let count = b.block_params(entry)[3];
            let span = b.block_params(entry)[4];
            let f = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(f, &[err, count, span]);
            let r = b.inst_results(call)[0];
            let move_ = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], move_, &[]);
            b.switch_to_block(move_);
            let pr = self.data(arena, r, b, m);
            let px = self.data(arena, xs, b, m);
            let skip = b.ins().imul_imm_s(from, heap::WORD as i64);
            let at = b.ins().iadd(px, skip);
            let bytes = b.ins().imul_imm_s(count, heap::WORD as i64);
            let config = m.target_config();
            b.call_memcpy(config, pr, at, bytes);
            b.ins().jump(out, &[]);
            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })?;

        let sig = Text::signature(m, &[ptr, types::I64, types::I32], types::I64);
        Text::wrote(self.reverse, sig, 13, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let xs = b.block_params(entry)[1];
            let span = b.block_params(entry)[2];
            let n = self.count(arena, xs, b, m);
            let f = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(f, &[err, n, span]);
            let r = b.inst_results(call)[0];
            let walk = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], walk, &[]);

            b.switch_to_block(walk);
            let pr = self.data(arena, r, b, m);
            let px = self.data(arena, xs, b, m);
            let i = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(i, zero);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let step = b.create_block();
            let at = b.use_var(i);
            let done = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at, n);
            b.ins().brif(done, out, &[], step, &[]);

            b.switch_to_block(step);
            let back = b.ins().isub(n, at);
            let k = b.ins().iadd_imm_s(back, -1);
            let off = b.ins().imul_imm_s(at, heap::WORD as i64);
            let src = b.ins().iadd(px, off);
            let koff = b.ins().imul_imm_s(k, heap::WORD as i64);
            let dst = b.ins().iadd(pr, koff);
            let w = b.ins().load(types::I64, MemFlagsData::trusted(), src, 0);
            b.ins().store(MemFlagsData::trusted(), w, dst, 0);
            let next = b.ins().iadd_imm_s(at, 1);
            b.def_var(i, next);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })?;

        let sig = Text::signature(m, &[ptr, types::I64, types::I32], types::I64);
        Text::wrote(self.concat, sig, 17, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let xss = b.block_params(entry)[1];
            let span = b.block_params(entry)[2];
            let n = self.count(arena, xss, b, m);
            let outer = self.data(arena, xss, b, m);
            let word = heap::WORD as i64;

            // One pass for the size. Every inner length is a header word, so the total is known
            // before a byte is reserved — which is what makes this an allocation and not a growth.
            let i = b.declare_var(types::I64);
            let total = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(i, zero);
            b.def_var(total, zero);
            let sum = b.create_block();
            let add = b.create_block();
            let build = b.create_block();
            b.ins().jump(sum, &[]);
            b.switch_to_block(sum);
            let at = b.use_var(i);
            let done = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at, n);
            b.ins().brif(done, build, &[], add, &[]);
            b.switch_to_block(add);
            let off = b.ins().imul_imm_s(at, word);
            let src = b.ins().iadd(outer, off);
            let inner = b.ins().load(types::I64, MemFlagsData::trusted(), src, 0);
            let len = self.count(arena, inner, b, m);
            let so_far = b.use_var(total);
            let grown = b.ins().iadd(so_far, len);
            b.def_var(total, grown);
            let next = b.ins().iadd_imm_s(at, 1);
            b.def_var(i, next);
            b.ins().jump(sum, &[]);

            b.switch_to_block(build);
            let want = b.use_var(total);
            let alloc = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(alloc, &[err, want, span]);
            let out = b.inst_results(call)[0];
            let move_ = b.create_block();
            let answer = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, out, 0);
            b.ins().brif(failed, answer, &[], move_, &[]);

            // One `memcpy` per inner list. An element is a word whatever it means, and an offset
            // stays an offset, so nothing here has to know what the elements are.
            b.switch_to_block(move_);
            let dst = self.data(arena, out, b, m);
            let j = b.declare_var(types::I64);
            let k = b.declare_var(types::I64);
            b.def_var(j, zero);
            b.def_var(k, zero);
            let loop_ = b.create_block();
            let one = b.create_block();
            b.ins().jump(loop_, &[]);
            b.switch_to_block(loop_);
            let at = b.use_var(j);
            let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at, n);
            b.ins().brif(past, answer, &[], one, &[]);
            b.switch_to_block(one);
            let off = b.ins().imul_imm_s(at, word);
            let src = b.ins().iadd(outer, off);
            let inner = b.ins().load(types::I64, MemFlagsData::trusted(), src, 0);
            let len = self.count(arena, inner, b, m);
            let from = self.data(arena, inner, b, m);
            let so_far = b.use_var(k);
            let skip = b.ins().imul_imm_s(so_far, word);
            let to = b.ins().iadd(dst, skip);
            let bytes = b.ins().imul_imm_s(len, word);
            let config = m.target_config();
            b.call_memcpy(config, to, from, bytes);
            let filled = b.ins().iadd(so_far, len);
            b.def_var(k, filled);
            let next = b.ins().iadd_imm_s(at, 1);
            b.def_var(j, next);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(answer);
            b.ins().return_(&[out]);
        })
    }
}

/// The arena: four globals and the only allocator this backend has.
///
/// A bump pointer and no free, exactly as [`beck_llvm::emit`] emits it — the shape is
/// [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md)'s and not
/// either backend's. What is Cranelift's is that a global is a `DataId` rather than a name.
#[derive(Clone, Copy, Debug)]
struct Arena {
    /// Where the arena is. Written once by `main`, read by everything.
    heap: DataId,
    /// The bump pointer, as an offset.
    next: DataId,
    /// How many bytes there are, or `0` when the `malloc` failed.
    limit: DataId,
    /// Whether the call in flight answers with something on the heap. The thunk knows; `main` asks.
    reply: DataId,
    alloc: FuncId,
}

impl Arena {
    fn declare(m: &mut ObjectModule, ptr: Type) -> Result<Arena, String> {
        let mut one = |name: &str, bytes: usize| -> Result<DataId, String> {
            let id = m
                .declare_data(name, Linkage::Local, true, false)
                .map_err(|e| format!("declaring `{name}`: {e}"))?;
            let mut desc = DataDescription::new();
            desc.define_zeroinit(bytes);
            m.define_data(id, &desc)
                .map_err(|e| format!("defining `{name}`: {e}"))?;
            Ok(id)
        };
        let heap = one("beck.heap", ptr.bytes() as usize)?;
        let next = one("beck.next", 8)?;
        let limit = one("beck.limit", 8)?;
        let reply = one("beck.reply", 8)?;

        let mut sig =
            cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I32));
        sig.returns.push(AbiParam::new(types::I64));
        let alloc = m
            .declare_function("beck.alloc", Linkage::Local, &sig)
            .map_err(|e| format!("declaring the allocator: {e}"))?;
        Ok(Arena {
            heap,
            next,
            limit,
            reply,
            alloc,
        })
    }

    /// The address of one of the globals, in the function being built.
    fn addr(self, which: DataId, b: &mut FunctionBuilder<'_>, m: &mut ObjectModule) -> IrValue {
        let gv = m.declare_data_in_func(which, b.func);
        let ptr = m.target_config().pointer_type();
        b.ins().symbol_value(ptr, gv)
    }

    /// The arena's base.
    ///
    /// Loaded wherever it is used rather than hoisted to the entry block — the other backend
    /// hoists because it writes text and can insert a line after the fact. The flags say
    /// **readonly**, which is true and is what lets Cranelift's alias analysis fold the repeats:
    /// `main` writes this word before any compiled code runs and nothing writes it again.
    fn base(self, b: &mut FunctionBuilder<'_>, m: &mut ObjectModule) -> IrValue {
        let at = self.addr(self.heap, b, m);
        let ptr = m.target_config().pointer_type();
        b.ins()
            .load(ptr, MemFlagsData::trusted().with_readonly(), at, 0)
    }

    /// The allocator: a bump pointer and no free.
    ///
    /// [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md) is the
    /// whole of the reasoning: a call is bounded, the arena is reset before every one, and running
    /// out is [`Trap::HeapExhausted`] rather than a crash.
    fn define(
        self,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let mut sig =
            cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I32));
        sig.returns.push(AbiParam::new(types::I64));
        ctx.func = Function::with_name_signature(UserFuncName::user(6, 0), sig);
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            b.seal_block(entry);
            let err = b.block_params(entry)[0];
            let bytes = b.block_params(entry)[1];
            let span = b.block_params(entry)[2];
            let flags = MemFlagsData::trusted();

            let next_at = self.addr(self.next, &mut b, m);
            let limit_at = self.addr(self.limit, &mut b, m);
            let n = b.ins().load(types::I64, flags, next_at, 0);
            let limit = b.ins().load(types::I64, flags, limit_at, 0);
            let new = b.ins().iadd(n, bytes);
            // Unsigned, and against the sum rather than against the room left: a null arena has a
            // limit of zero, and `limit - next` would underflow into "plenty".
            let over = b.ins().icmp(IntCC::UnsignedGreaterThan, new, limit);
            let full = b.create_block();
            let ok = b.create_block();
            b.ins().brif(over, full, &[], ok, &[]);

            b.switch_to_block(full);
            b.seal_block(full);
            let code = b
                .ins()
                .iconst(types::I32, i64::from(Trap::HeapExhausted.code()));
            b.ins().store(flags, code, err, 0);
            b.ins().store(flags, span, err, CELL_SPAN);
            let z = b.ins().iconst(types::I64, 0);
            b.ins().store(flags, z, err, CELL_PAYLOAD);
            b.ins().return_(&[z]);

            b.switch_to_block(ok);
            b.seal_block(ok);
            b.ins().store(flags, new, next_at, 0);
            b.ins().return_(&[n]);
            b.finalize(m.target_config());
        }
        m.define_function(self.alloc, ctx)
            .map_err(|e| format!("defining the allocator: {e}"))?;
        m.clear_context(ctx);
        Ok(())
    }

    fn alloc_in(
        self,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> cranelift_codegen::ir::FuncRef {
        m.declare_func_in_func(self.alloc, b.func)
    }
}

// -------------------------------------------------------------------------------------------
// Text
// -------------------------------------------------------------------------------------------

/// The six functions everything this backend does to a `Str` is built from.
///
/// The same six [`beck_llvm::emit`]'s `TEXT` writes as LLVM IR, and written again for this
/// module's stated reason: the two emitters are held to *agreeing*, and one implementation shared
/// between them would make the agreement true by construction. What is *not* written again is the
/// shape they all read — [`beck_llvm::heap`]'s two counts and bytes — because that one is a
/// contract with the host as well.
///
/// `memcmp` is imported; `memcpy` arrives through [`FunctionBuilder::call_memcpy`], which is a
/// libcall the object module already names.
#[derive(Clone, Copy, Debug)]
struct Text {
    /// A fresh, uninitialised `Str` of the given two counts, or `0` on a full arena.
    alloc: FuncId,
    /// `-1`, `0` or `1` — bytes first, then length, which is what `String`'s `Ord` gives.
    cmp: FuncId,
    concat: FuncId,
    /// Which byte character `i` begins at, clamped to the end.
    byteof: FuncId,
    slice: FuncId,
    /// The byte offset of a substring, or `-1`.
    find: FuncId,
    /// The byte width of the whitespace character at `i`, or `0` if the character there is not one.
    ws: FuncId,
    /// `str_trim` — `str::trim`, which is `char::is_whitespace` at both ends.
    trim: FuncId,
    /// The bytes of a `Str` in `[from, to)`, as a `Str` of its own.
    piece: FuncId,
    /// [`Text::find`], starting at a byte offset rather than at zero.
    findat: FuncId,
    /// How many characters begin before a byte offset — [`Text::byteof`]'s inverse, and what turns
    /// a search's answer into an index the language can use.
    charat: FuncId,
    memcmp: FuncId,
}

/// Which of [`Text`]'s allocating functions a call site wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Concat,
    Slice,
    Trim,
}

impl Text {
    fn declare(m: &mut ObjectModule, ptr: Type) -> Result<Text, String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut one = |name: &str, params: &[Type], ret: Type| -> Result<FuncId, String> {
            let mut sig = cranelift_codegen::ir::Signature::new(conv);
            for p in params {
                sig.params.push(AbiParam::new(*p));
            }
            sig.returns.push(AbiParam::new(ret));
            m.declare_function(name, Linkage::Local, &sig)
                .map_err(|e| format!("declaring `{name}`: {e}"))
        };
        let alloc = one(
            "beck.str.alloc",
            &[ptr, types::I64, types::I64, types::I32],
            types::I64,
        )?;
        let cmp = one("beck.str.cmp", &[types::I64, types::I64], types::I64)?;
        let concat = one(
            "beck.str.concat",
            &[ptr, types::I64, types::I64, types::I32],
            types::I64,
        )?;
        let byteof = one("beck.str.byteof", &[types::I64, types::I64], types::I64)?;
        let slice = one(
            "beck.str.slice",
            &[ptr, types::I64, types::I64, types::I64, types::I32],
            types::I64,
        )?;
        let find = one("beck.str.find", &[types::I64, types::I64], types::I64)?;
        let charat = one("beck.str.charat", &[types::I64, types::I64], types::I64)?;
        let ws = one("beck.str.ws", &[ptr, types::I64, types::I64], types::I64)?;
        let trim = one("beck.str.trim", &[ptr, types::I64, types::I32], types::I64)?;
        let piece = one(
            "beck.str.piece",
            &[ptr, types::I64, types::I64, types::I64, types::I32],
            types::I64,
        )?;
        let findat = one(
            "beck.str.findat",
            &[types::I64, types::I64, types::I64],
            types::I64,
        )?;

        let mut sig = cranelift_codegen::ir::Signature::new(conv);
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I32));
        let memcmp = m
            .declare_function("memcmp", Linkage::Import, &sig)
            .map_err(|e| format!("declaring `memcmp`: {e}"))?;

        Ok(Text {
            alloc,
            cmp,
            concat,
            byteof,
            slice,
            find,
            charat,
            ws,
            trim,
            piece,
            findat,
            memcmp,
        })
    }

    fn id(self, which: Which) -> FuncId {
        match which {
            Which::Concat => self.concat,
            Which::Slice => self.slice,
            Which::Trim => self.trim,
        }
    }

    /// Where a `Str`'s bytes start, as an address.
    fn data(
        self,
        arena: Arena,
        s: IrValue,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let base = arena.base(b, m);
        let at = b.ins().iadd(base, s);
        b.ins().iadd_imm_s(at, heap::STR_HEADER as i64)
    }

    /// One header word: `0` is the byte count and `8` is the character count.
    fn header(
        self,
        arena: Arena,
        s: IrValue,
        at: i64,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let base = arena.base(b, m);
        let p = b.ins().iadd(base, s);
        b.ins()
            .load(types::I64, MemFlagsData::trusted(), p, at as i32)
    }

    fn define(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        self.define_alloc(arena, m, ctx, fctx, ptr)?;
        self.define_cmp(arena, m, ctx, fctx)?;
        self.define_concat(arena, m, ctx, fctx, ptr)?;
        self.define_byteof(arena, m, ctx, fctx)?;
        self.define_slice(arena, m, ctx, fctx, ptr)?;
        self.define_find(arena, m, ctx, fctx)?;
        self.define_charat(arena, m, ctx, fctx)?;
        self.define_ws(m, ctx, fctx, ptr)?;
        self.define_piece(arena, m, ctx, fctx, ptr)?;
        self.define_findat(arena, m, ctx, fctx)?;
        self.define_trim(arena, m, ctx, fctx, ptr)?;
        Ok(())
    }

    /// The shape of every definition here: build the signature, run `body`, define the function.
    fn wrote(
        id: FuncId,
        sig: cranelift_codegen::ir::Signature,
        seq: u32,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        body: impl FnOnce(&mut FunctionBuilder<'_>, &mut ObjectModule),
    ) -> Result<(), String> {
        ctx.func = Function::with_name_signature(UserFuncName::user(7, seq), sig);
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            b.seal_block(entry);
            body(&mut b, m);
            b.seal_all_blocks();
            b.finalize(m.target_config());
        }
        m.define_function(id, ctx)
            .map_err(|e| format!("defining a text function: {e}"))?;
        m.clear_context(ctx);
        Ok(())
    }

    fn signature(m: &ObjectModule, params: &[Type], ret: Type) -> cranelift_codegen::ir::Signature {
        let mut sig =
            cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
        for p in params {
            sig.params.push(AbiParam::new(*p));
        }
        sig.returns.push(AbiParam::new(ret));
        sig
    }

    fn define_alloc(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.alloc, sig, 0, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let bytes = b.block_params(entry)[1];
            let chars = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            let flags = MemFlagsData::trusted();

            let padded = b.ins().iadd_imm_s(bytes, heap::WORD as i64 - 1);
            let body = b.ins().band_imm_s(padded, -(heap::WORD as i64));
            let total = b.ins().iadd_imm_s(body, heap::STR_HEADER as i64);
            let f = arena.alloc_in(b, m);
            let call = b.ins().call(f, &[err, total, span]);
            let off = b.inst_results(call)[0];

            let fill = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let failed = b.ins().icmp_imm_s(IntCC::Equal, off, 0);
            b.ins().brif(failed, out, &[off.into()], fill, &[]);

            b.switch_to_block(fill);
            let base = arena.base(b, m);
            let p = b.ins().iadd(base, off);
            b.ins().store(flags, bytes, p, 0);
            b.ins().store(flags, chars, p, heap::WORD as i32);
            // The padding is zeroed rather than left as whatever the arena held, so two runs of
            // one program leave the same bytes behind.
            let tail = b.create_block();
            let empty = b.ins().icmp_imm_s(IntCC::Equal, body, 0);
            b.ins().brif(empty, out, &[off.into()], tail, &[]);

            b.switch_to_block(tail);
            let last = b.ins().iadd(p, body);
            let z = b.ins().iconst(types::I64, 0);
            b.ins().store(flags, z, last, heap::WORD as i32);
            b.ins().jump(out, &[off.into()]);

            b.switch_to_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })
    }

    fn define_cmp(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[types::I64, types::I64], types::I64);
        Text::wrote(self.cmp, sig, 1, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let (sa, sb) = (b.block_params(entry)[0], b.block_params(entry)[1]);
            let la = self.header(arena, sa, 0, b, m);
            let lb = self.header(arena, sb, 0, b, m);
            let pa = self.data(arena, sa, b, m);
            let pb = self.data(arena, sb, b, m);
            let shorter = b.ins().icmp(IntCC::UnsignedLessThan, la, lb);
            let n = b.ins().select(shorter, la, lb);
            let f = m.declare_func_in_func(self.memcmp, b.func);
            let call = b.ins().call(f, &[pa, pb, n]);
            let c = b.inst_results(call)[0];

            // The three answers are made once, in the entry block, because both blocks below
            // choose between them and a value has to dominate the instruction that reads it.
            let down = b.ins().iconst(types::I64, -1);
            let up = b.ins().iconst(types::I64, 1);
            let zero = b.ins().iconst(types::I64, 0);
            let bytes = b.create_block();
            let lengths = b.create_block();
            let decided = b.ins().icmp_imm_s(IntCC::NotEqual, c, 0);
            b.ins().brif(decided, bytes, &[], lengths, &[]);

            b.switch_to_block(bytes);
            // `memcmp` may answer any negative or any positive number; the language wants one of
            // three, because this feeds `Value`'s three-way order and not a boolean.
            let neg = b.ins().icmp_imm_s(IntCC::SignedLessThan, c, 0);
            let sign = b.ins().select(neg, down, up);
            b.ins().return_(&[sign]);

            // Equal on their shared prefix, so the shorter one is the smaller: `"ab" < "abc"`.
            b.switch_to_block(lengths);
            let gt = b.ins().icmp(IntCC::UnsignedGreaterThan, la, lb);
            let ordered = b.ins().select(gt, up, down);
            let same = b.ins().icmp(IntCC::Equal, la, lb);
            let r = b.ins().select(same, zero, ordered);
            b.ins().return_(&[r]);
        })
    }

    fn define_concat(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.concat, sig, 2, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let (sa, sb) = (b.block_params(entry)[1], b.block_params(entry)[2]);
            let span = b.block_params(entry)[3];
            let la = self.header(arena, sa, 0, b, m);
            let lb = self.header(arena, sb, 0, b, m);
            let ca = self.header(arena, sa, heap::WORD as i64, b, m);
            let cb = self.header(arena, sb, heap::WORD as i64, b, m);
            let lt = b.ins().iadd(la, lb);
            let ct = b.ins().iadd(ca, cb);
            let f = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(f, &[err, lt, ct, span]);
            let r = b.inst_results(call)[0];

            let copy = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], copy, &[]);

            b.switch_to_block(copy);
            // Taken after the allocation: the arena never moves, and this way nothing depends on
            // that being true.
            let pr = self.data(arena, r, b, m);
            let pa = self.data(arena, sa, b, m);
            let pb = self.data(arena, sb, b, m);
            let config = m.target_config();
            b.call_memcpy(config, pr, pa, la);
            let second = b.ins().iadd(pr, la);
            b.call_memcpy(config, second, pb, lb);
            b.ins().jump(out, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })
    }

    fn define_byteof(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[types::I64, types::I64], types::I64);
        Text::wrote(self.byteof, sig, 3, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let (s, i) = (b.block_params(entry)[0], b.block_params(entry)[1]);
            let len = self.header(arena, s, 0, b, m);
            let chars = self.header(arena, s, heap::WORD as i64, b, m);

            let inside = b.create_block();
            let past = b.ins().icmp(IntCC::SignedGreaterThanOrEqual, i, chars);
            let end = b.create_block();
            b.ins().brif(past, end, &[], inside, &[]);

            b.switch_to_block(inside);
            let known = b.create_block();
            let start = b.create_block();
            let before = b.ins().icmp_imm_s(IntCC::SignedLessThanOrEqual, i, 0);
            b.ins().brif(before, start, &[], known, &[]);

            b.switch_to_block(known);
            // Every character is one byte exactly when there are as many bytes as characters, so
            // the two counts the header already carries are the ASCII test and no flag is stored.
            let walk = b.create_block();
            let direct = b.create_block();
            let ascii = b.ins().icmp(IntCC::Equal, len, chars);
            b.ins().brif(ascii, direct, &[], walk, &[]);

            b.switch_to_block(direct);
            b.ins().return_(&[i]);
            b.switch_to_block(start);
            let z = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[z]);
            b.switch_to_block(end);
            b.ins().return_(&[len]);

            b.switch_to_block(walk);
            let p = self.data(arena, s, b, m);
            let at = b.declare_var(types::I64);
            let seen = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(at, zero);
            b.def_var(seen, zero);
            let step = b.create_block();
            b.ins().jump(step, &[]);

            b.switch_to_block(step);
            let here = b.create_block();
            let advance = b.create_block();
            let k = b.use_var(seen);
            let done = b.ins().icmp(IntCC::Equal, k, i);
            b.ins().brif(done, here, &[], advance, &[]);

            b.switch_to_block(advance);
            // One character is its lead byte and every byte after it whose top two bits are `10`.
            let cur = b.use_var(at);
            let one = b.ins().iadd_imm_s(cur, 1);
            let cursor = b.declare_var(types::I64);
            b.def_var(cursor, one);
            let skip = b.create_block();
            b.ins().jump(skip, &[]);

            b.switch_to_block(skip);
            let look = b.create_block();
            let skipped = b.create_block();
            let j = b.use_var(cursor);
            let over = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, j, len);
            b.ins().brif(over, skipped, &[], look, &[]);

            b.switch_to_block(look);
            let again = b.create_block();
            let bp = b.ins().iadd(p, j);
            let byte = b.ins().load(types::I8, MemFlagsData::trusted(), bp, 0);
            let top = b.ins().band_imm_u(byte, 0xc0);
            let cont = b.ins().icmp_imm_u(IntCC::Equal, top, 0x80);
            b.ins().brif(cont, again, &[], skipped, &[]);

            b.switch_to_block(again);
            let next = b.ins().iadd_imm_s(j, 1);
            b.def_var(cursor, next);
            b.ins().jump(skip, &[]);

            b.switch_to_block(skipped);
            let stopped = b.use_var(cursor);
            b.def_var(at, stopped);
            let more = b.ins().iadd_imm_s(k, 1);
            b.def_var(seen, more);
            b.ins().jump(step, &[]);

            b.switch_to_block(here);
            let answer = b.use_var(at);
            b.ins().return_(&[answer]);
        })
    }

    /// The byte width of the whitespace character beginning at `i`, or `0` if the character there
    /// is not whitespace.
    ///
    /// `White_Space` is **25 code points**, none of them four bytes long, so this is a switch over
    /// five lead bytes rather than a table — which is the whole of why `str_trim` compiles here and
    /// `str_upper` does not. No continuation byte can be `0xC2`, `0xE1`, `0xE2`
    /// or `0xE3` — continuations are `0x80..=0xBF` — so this may be asked at *any* byte of
    /// well-formed UTF-8 and never answers inside a character, which is what lets
    /// [`Text::define_trim`] walk a byte at a time without decoding what it skips.
    fn define_ws(
        self,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[ptr, types::I64, types::I64], types::I64);
        Text::wrote(self.ws, sig, 8, m, ctx, fctx, |b, _m| {
            let entry = b.current_block().expect("an entry block");
            let p = b.block_params(entry)[0];
            let i = b.block_params(entry)[1];
            let len = b.block_params(entry)[2];

            let byte = |b: &mut FunctionBuilder<'_>, at: IrValue| {
                let q = b.ins().iadd(p, at);
                let raw = b.ins().load(types::I8, MemFlagsData::trusted(), q, 0);
                b.ins().uextend(types::I64, raw)
            };
            let is = |b: &mut FunctionBuilder<'_>, v: IrValue, n: i64| {
                b.ins().icmp_imm_u(IntCC::Equal, v, n)
            };

            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let zero = b.ins().iconst(types::I64, 0);

            // The six one-byte ones: TAB..CR and SPACE.
            let b0 = byte(b, i);
            let space = is(b, b0, 0x20);
            let ge9 = b
                .ins()
                .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, b0, 0x09);
            let led = b.ins().icmp_imm_u(IntCC::UnsignedLessThanOrEqual, b0, 0x0d);
            let control = b.ins().band(ge9, led);
            let one = b.ins().bor(space, control);
            let chk1 = b.create_block();
            let w1 = b.ins().iconst(types::I64, 1);
            b.ins().brif(one, out, &[w1.into()], chk1, &[]);

            // Two bytes: U+0085 NEL and U+00A0 NBSP, both behind `0xC2`.
            b.switch_to_block(chk1);
            let ld1 = b.create_block();
            let i1 = b.ins().iadd_imm_s(i, 1);
            let has1 = b.ins().icmp(IntCC::UnsignedLessThan, i1, len);
            b.ins().brif(has1, ld1, &[], out, &[zero.into()]);

            b.switch_to_block(ld1);
            let b1 = byte(b, i1);
            let c2 = is(b, b0, 0xc2);
            let nel = is(b, b1, 0x85);
            let nbsp = is(b, b1, 0xa0);
            let after = b.ins().bor(nel, nbsp);
            let two = b.ins().band(c2, after);
            let chk2 = b.create_block();
            let w2 = b.ins().iconst(types::I64, 2);
            b.ins().brif(two, out, &[w2.into()], chk2, &[]);

            b.switch_to_block(chk2);
            let ld2 = b.create_block();
            let i2 = b.ins().iadd_imm_s(i, 2);
            let has2 = b.ins().icmp(IntCC::UnsignedLessThan, i2, len);
            b.ins().brif(has2, ld2, &[], out, &[zero.into()]);

            // Three bytes, in the four families the encoding groups them into: U+1680 alone,
            // `E2 80 xx` (U+2000..U+200A, U+2028, U+2029, U+202F), U+205F, and U+3000.
            b.switch_to_block(ld2);
            let b2 = byte(b, i2);
            let e1 = is(b, b0, 0xe1);
            let x9a = is(b, b1, 0x9a);
            let x80 = is(b, b2, 0x80);
            let ogham = {
                let t = b.ins().band(e1, x9a);
                b.ins().band(t, x80)
            };
            let e2 = is(b, b0, 0xe2);
            let lead80 = is(b, b1, 0x80);
            let quads = {
                let ge = b
                    .ins()
                    .icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, b2, 0x80);
                let le = b.ins().icmp_imm_u(IntCC::UnsignedLessThanOrEqual, b2, 0x8a);
                b.ins().band(ge, le)
            };
            let sep = {
                let a = is(b, b2, 0xa8);
                let c = is(b, b2, 0xa9);
                b.ins().bor(a, c)
            };
            let narrow = is(b, b2, 0xaf);
            let general = {
                let t = b.ins().bor(quads, sep);
                let t = b.ins().bor(t, narrow);
                let u = b.ins().band(e2, lead80);
                b.ins().band(u, t)
            };
            let lead81 = is(b, b1, 0x81);
            let mmsp = {
                let x9f = is(b, b2, 0x9f);
                let t = b.ins().band(e2, lead81);
                b.ins().band(t, x9f)
            };
            let e3 = is(b, b0, 0xe3);
            let ideographic = {
                let t = b.ins().band(e3, lead80);
                b.ins().band(t, x80)
            };
            let three = {
                let t = b.ins().bor(ogham, general);
                let t = b.ins().bor(t, mmsp);
                b.ins().bor(t, ideographic)
            };
            let w3 = b.ins().iconst(types::I64, 3);
            b.ins().brif(three, out, &[w3.into()], out, &[zero.into()]);

            b.switch_to_block(out);
            let w = b.block_params(out)[0];
            b.ins().return_(&[w]);
        })
    }

    /// `str_trim`, in one pass.
    ///
    /// The leading run is skipped whole; then every byte is either the start of a whitespace
    /// character — skipped, and *not* recorded — or one byte of something else, which moves the
    /// end. So `end` finishes one past the last byte of the last non-whitespace character, which is
    /// what `str::trim` answers, and the character count is the bytes in `[start, end)` that are
    /// not continuations.
    fn define_trim(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[ptr, types::I64, types::I32], types::I64);
        Text::wrote(self.trim, sig, 9, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let s = b.block_params(entry)[1];
            let span = b.block_params(entry)[2];

            let len = self.header(arena, s, 0, b, m);
            let p = self.data(arena, s, b, m);
            let ws = m.declare_func_in_func(self.ws, b.func);

            let cursor = b.declare_var(types::I64);
            let start = b.declare_var(types::I64);
            let end = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(cursor, zero);

            let lead = b.create_block();
            let empty = b.create_block();
            let ltest = b.create_block();
            b.ins().jump(lead, &[]);

            b.switch_to_block(lead);
            let l = b.use_var(cursor);
            let over = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, l, len);
            b.ins().brif(over, empty, &[], ltest, &[]);

            b.switch_to_block(ltest);
            let call = b.ins().call(ws, &[p, l, len]);
            let lw = b.inst_results(call)[0];
            let skipping = b.create_block();
            let body = b.create_block();
            let blank = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, lw, 0);
            b.ins().brif(blank, skipping, &[], body, &[]);

            b.switch_to_block(skipping);
            let next = b.ins().iadd(l, lw);
            b.def_var(cursor, next);
            b.ins().jump(lead, &[]);

            b.switch_to_block(body);
            let from = b.use_var(cursor);
            b.def_var(start, from);
            b.def_var(end, from);
            let scan = b.create_block();
            b.ins().jump(scan, &[]);

            b.switch_to_block(scan);
            let cut = b.create_block();
            let test = b.create_block();
            let i = b.use_var(cursor);
            let done = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, i, len);
            b.ins().brif(done, cut, &[], test, &[]);

            b.switch_to_block(test);
            let call = b.ins().call(ws, &[p, i, len]);
            let w = b.inst_results(call)[0];
            let spaced = b.create_block();
            let kept = b.create_block();
            let isws = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, w, 0);
            b.ins().brif(isws, spaced, &[], kept, &[]);

            b.switch_to_block(spaced);
            let past = b.ins().iadd(i, w);
            b.def_var(cursor, past);
            b.ins().jump(scan, &[]);

            b.switch_to_block(kept);
            let on = b.ins().iadd_imm_s(i, 1);
            b.def_var(cursor, on);
            b.def_var(end, on);
            b.ins().jump(scan, &[]);

            b.switch_to_block(cut);
            let head = b.use_var(start);
            let tail = b.use_var(end);
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let f = m.declare_func_in_func(self.piece, b.func);
            let call = b.ins().call(f, &[err, s, head, tail, span]);
            let r = b.inst_results(call)[0];
            b.ins().jump(out, &[r.into()]);

            // All whitespace, or empty to begin with: the answer is a fresh empty `Str` rather than
            // the argument, because the evaluator's is one too and `docs/93`'s layout has no
            // interning in it.
            b.switch_to_block(empty);
            let f = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(f, &[err, zero, zero, span]);
            let e = b.inst_results(call)[0];
            b.ins().jump(out, &[e.into()]);

            b.switch_to_block(out);
            let answer = b.block_params(out)[0];
            b.ins().return_(&[answer]);
        })
    }

    /// The bytes of `s` in `[from, to)`, as a `Str` of its own.
    ///
    /// The character count is the bytes in the range that are not continuations, which is the same
    /// test [`Text::define_byteof`] walks with — and the range is always a whole number of
    /// characters, because every caller cuts at a boundary a scan stopped on.
    fn define_piece(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let sig = Text::signature(
            m,
            &[ptr, types::I64, types::I64, types::I64, types::I32],
            types::I64,
        );
        Text::wrote(self.piece, sig, 10, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let s = b.block_params(entry)[1];
            let from = b.block_params(entry)[2];
            let to = b.block_params(entry)[3];
            let span = b.block_params(entry)[4];

            let bytes = b.ins().isub(to, from);
            let p = self.data(arena, s, b, m);
            let at = b.declare_var(types::I64);
            let chars = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(at, from);
            b.def_var(chars, zero);
            let count = b.create_block();
            b.ins().jump(count, &[]);

            b.switch_to_block(count);
            let make = b.create_block();
            let counted = b.create_block();
            let k = b.use_var(at);
            let ran = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, k, to);
            b.ins().brif(ran, make, &[], counted, &[]);

            b.switch_to_block(counted);
            let cp = b.ins().iadd(p, k);
            let cb = b.ins().load(types::I8, MemFlagsData::trusted(), cp, 0);
            let top = b.ins().band_imm_u(cb, 0xc0);
            let cont = b.ins().icmp_imm_u(IntCC::Equal, top, 0x80);
            let seen = b.use_var(chars);
            let more = b.ins().iadd_imm_s(seen, 1);
            let held = b.ins().select(cont, seen, more);
            b.def_var(chars, held);
            let k1 = b.ins().iadd_imm_s(k, 1);
            b.def_var(at, k1);
            b.ins().jump(count, &[]);

            b.switch_to_block(make);
            let total = b.use_var(chars);
            let f = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(f, &[err, bytes, total, span]);
            let r = b.inst_results(call)[0];
            let copy = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], copy, &[]);

            b.switch_to_block(copy);
            // Both pointers are taken again: `beck.str.alloc` can move the arena, and the ones
            // above were read before it ran.
            let pr = self.data(arena, r, b, m);
            let ps = self.data(arena, s, b, m);
            let src = b.ins().iadd(ps, from);
            let config = m.target_config();
            b.call_memcpy(config, pr, src, bytes);
            b.ins().jump(out, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })
    }

    /// [`Text::define_find`], starting at a byte offset rather than at zero — what a repeated
    /// search needs, and the only thing `str_split` asks that `find` does not answer.
    fn define_findat(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[types::I64, types::I64, types::I64], types::I64);
        Text::wrote(self.findat, sig, 11, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let h = b.block_params(entry)[0];
            let n = b.block_params(entry)[1];
            let from = b.block_params(entry)[2];

            let lh = self.header(arena, h, 0, b, m);
            let ln = self.header(arena, n, 0, b, m);
            let room = b.ins().isub(lh, ln);
            let missing = b.create_block();
            let search = b.create_block();
            let too = b.ins().icmp_imm_s(IntCC::SignedLessThan, room, 0);
            b.ins().brif(too, missing, &[], search, &[]);

            b.switch_to_block(search);
            let ph = self.data(arena, h, b, m);
            let pn = self.data(arena, n, b, m);
            let i = b.declare_var(types::I64);
            b.def_var(i, from);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let attempt = b.create_block();
            let at = b.use_var(i);
            let over = b.ins().icmp(IntCC::SignedGreaterThan, at, room);
            b.ins().brif(over, missing, &[], attempt, &[]);

            b.switch_to_block(attempt);
            let found = b.create_block();
            let next = b.create_block();
            let here = b.ins().iadd(ph, at);
            let cmp = m.declare_func_in_func(self.memcmp, b.func);
            let call = b.ins().call(cmp, &[here, pn, ln]);
            let c = b.inst_results(call)[0];
            let hit = b.ins().icmp_imm_s(IntCC::Equal, c, 0);
            b.ins().brif(hit, found, &[], next, &[]);

            b.switch_to_block(next);
            let j = b.ins().iadd_imm_s(at, 1);
            b.def_var(i, j);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(found);
            let answer = b.use_var(i);
            b.ins().return_(&[answer]);

            b.switch_to_block(missing);
            let none = b.ins().iconst(types::I64, -1);
            b.ins().return_(&[none]);
        })
    }

    fn define_slice(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let sig = Text::signature(
            m,
            &[ptr, types::I64, types::I64, types::I64, types::I32],
            types::I64,
        );
        Text::wrote(self.slice, sig, 4, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let s = b.block_params(entry)[1];
            let start = b.block_params(entry)[2];
            let len = b.block_params(entry)[3];
            let span = b.block_params(entry)[4];

            // A negative index or a negative length is zero, which is `i64::max(0)` in the
            // evaluator, and `start + len` saturates rather than wrapping, which is its
            // `saturating_add`.
            let zero = b.ins().iconst(types::I64, 0);
            let sneg = b.ins().icmp_imm_s(IntCC::SignedLessThan, start, 0);
            let from = b.ins().select(sneg, zero, start);
            let lneg = b.ins().icmp_imm_s(IntCC::SignedLessThan, len, 0);
            let take = b.ins().select(lneg, zero, len);
            let sum = b.ins().iadd(from, take);
            let most = b.ins().iconst(types::I64, i64::MAX);
            let wrapped = b.ins().icmp_imm_s(IntCC::SignedLessThan, sum, 0);
            let upto = b.ins().select(wrapped, most, sum);

            let chars = self.header(arena, s, heap::WORD as i64, b, m);
            let over_from = b.ins().icmp(IntCC::UnsignedGreaterThan, from, chars);
            let cstart = b.ins().select(over_from, chars, from);
            let over_upto = b.ins().icmp(IntCC::UnsignedGreaterThan, upto, chars);
            let cend = b.ins().select(over_upto, chars, upto);
            let count = b.ins().isub(cend, cstart);

            let byteof = m.declare_func_in_func(self.byteof, b.func);
            let call = b.ins().call(byteof, &[s, cstart]);
            let a = b.inst_results(call)[0];
            let call = b.ins().call(byteof, &[s, cend]);
            let e = b.inst_results(call)[0];
            let bytes = b.ins().isub(e, a);

            let f = m.declare_func_in_func(self.alloc, b.func);
            let call = b.ins().call(f, &[err, bytes, count, span]);
            let r = b.inst_results(call)[0];

            let copy = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], copy, &[]);

            b.switch_to_block(copy);
            let pr = self.data(arena, r, b, m);
            let ps = self.data(arena, s, b, m);
            let at = b.ins().iadd(ps, a);
            let config = m.target_config();
            b.call_memcpy(config, pr, at, bytes);
            b.ins().jump(out, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })
    }

    /// How many characters begin before byte `byte`, which is how many bytes before it are not
    /// continuation bytes. [`Text::define_byteof`]'s inverse: a search answers in bytes and the
    /// language indexes in characters.
    fn define_charat(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[types::I64, types::I64], types::I64);
        Text::wrote(self.charat, sig, 6, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let (s, byte) = (b.block_params(entry)[0], b.block_params(entry)[1]);
            let len = self.header(arena, s, 0, b, m);
            let chars = self.header(arena, s, heap::WORD as i64, b, m);

            let direct = b.create_block();
            let count = b.create_block();
            let ascii = b.ins().icmp(IntCC::Equal, len, chars);
            b.ins().brif(ascii, direct, &[], count, &[]);

            b.switch_to_block(direct);
            b.ins().return_(&[byte]);

            b.switch_to_block(count);
            let p = self.data(arena, s, b, m);
            let k = b.declare_var(types::I64);
            let n = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(k, zero);
            b.def_var(n, zero);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let look = b.create_block();
            let out = b.create_block();
            let at = b.use_var(k);
            let done = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at, byte);
            b.ins().brif(done, out, &[], look, &[]);

            b.switch_to_block(look);
            let bp = b.ins().iadd(p, at);
            let byte_at = b.ins().load(types::I8, MemFlagsData::trusted(), bp, 0);
            let top = b.ins().band_imm_u(byte_at, 0xc0);
            let cont = b.ins().icmp_imm_u(IntCC::Equal, top, 0x80);
            let one = b.ins().iconst(types::I64, 1);
            let step = b.ins().select(cont, zero, one);
            let seen = b.use_var(n);
            let more = b.ins().iadd(seen, step);
            b.def_var(n, more);
            let next = b.ins().iadd_imm_s(at, 1);
            b.def_var(k, next);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(out);
            let answer = b.use_var(n);
            b.ins().return_(&[answer]);
        })
    }

    fn define_find(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
    ) -> Result<(), String> {
        let sig = Text::signature(m, &[types::I64, types::I64], types::I64);
        Text::wrote(self.find, sig, 5, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let (h, n) = (b.block_params(entry)[0], b.block_params(entry)[1]);
            let lh = self.header(arena, h, 0, b, m);
            let ln = self.header(arena, n, 0, b, m);

            let search = b.create_block();
            let missing = b.create_block();
            let too = b.ins().icmp(IntCC::UnsignedGreaterThan, ln, lh);
            b.ins().brif(too, missing, &[], search, &[]);

            b.switch_to_block(search);
            // Naive, and correct on UTF-8 for the reason a byte search is: the encoding is
            // self-synchronising, so a well-formed needle cannot match starting inside a character.
            let last = b.ins().isub(lh, ln);
            let ph = self.data(arena, h, b, m);
            let pn = self.data(arena, n, b, m);
            let i = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(i, zero);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let try_ = b.create_block();
            let at = b.use_var(i);
            let over = b.ins().icmp(IntCC::UnsignedGreaterThan, at, last);
            b.ins().brif(over, missing, &[], try_, &[]);

            b.switch_to_block(try_);
            let found = b.create_block();
            let next = b.create_block();
            let p = b.ins().iadd(ph, at);
            let f = m.declare_func_in_func(self.memcmp, b.func);
            let call = b.ins().call(f, &[p, pn, ln]);
            let c = b.inst_results(call)[0];
            let hit = b.ins().icmp_imm_s(IntCC::Equal, c, 0);
            b.ins().brif(hit, found, &[], next, &[]);

            b.switch_to_block(next);
            let j = b.ins().iadd_imm_s(at, 1);
            b.def_var(i, j);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(found);
            let r = b.use_var(i);
            b.ins().return_(&[r]);

            b.switch_to_block(missing);
            let none = b.ins().iconst(types::I64, -1);
            b.ins().return_(&[none]);
        })
    }
}

/// The most patterns one arm may be split into. [`beck_llvm::emit`]'s number, and it has to be:
/// the two emitters are held to refusing the same definitions.
const MAX_ALTERNATIVES: usize = 16;

/// One arm's pattern as the patterns that have to be tried in turn.
///
/// An or-pattern of plain constants is left whole, because [`Body::probe`] can test it with one
/// `bor` and it binds nothing. Anything else is **split**: two alternatives that take a value apart
/// bind the same names to different words, so one block reached from both would need a block
/// parameter per binder, and copying the arm is the same behaviour with no join to get wrong.
fn alternatives(pat: &Pattern) -> Result<Vec<Pattern>, String> {
    let mut out = Vec::new();
    expand(pat, &mut out)?;
    Ok(out)
}

/// Whether every alternative here is a constant, and therefore testable without control flow.
fn testable(pat: &Pattern) -> bool {
    match pat {
        Pattern::Const(_) => true,
        Pattern::Or(alts) => alts.iter().all(testable),
        _ => false,
    }
}

fn expand(pat: &Pattern, out: &mut Vec<Pattern>) -> Result<(), String> {
    match pat {
        Pattern::Or(alts) if !testable(pat) => {
            for alt in alts {
                expand(alt, out)?;
            }
        }
        Pattern::At { var, inner } => {
            let mut inners = Vec::new();
            expand(inner, &mut inners)?;
            for i in inners {
                out.push(Pattern::At {
                    var: *var,
                    inner: Box::new(i),
                });
            }
        }
        Pattern::Ctor { variant, binds } => {
            let mut rows: Vec<Vec<(Arc<str>, Pattern)>> = vec![Vec::new()];
            for (name, sub) in binds {
                let mut subs = Vec::new();
                expand(sub, &mut subs)?;
                let mut next = Vec::with_capacity(rows.len() * subs.len());
                for row in &rows {
                    for one in &subs {
                        let mut row = row.clone();
                        row.push((name.clone(), one.clone()));
                        next.push(row);
                    }
                }
                rows = next;
                if rows.len() > MAX_ALTERNATIVES {
                    return Err(format!(
                        "a pattern with more than {MAX_ALTERNATIVES} alternatives in it"
                    ));
                }
            }
            for row in rows {
                out.push(Pattern::Ctor {
                    variant: variant.clone(),
                    binds: row,
                });
            }
        }
        other => out.push(other.clone()),
    }
    if out.len() > MAX_ALTERNATIVES {
        return Err(format!(
            "a pattern with more than {MAX_ALTERNATIVES} alternatives in it"
        ));
    }
    Ok(())
}

/// Every layout that needs a comparison, given the ones a body asked to compare.
///
/// Transitive, because comparing a record compares its fields.
fn reachable(
    layouts: &BTreeSet<u32>,
    lists: &BTreeSet<u32>,
    maps: &BTreeSet<u32>,
    heap: &Heap,
) -> (BTreeSet<u32>, BTreeSet<u32>, BTreeSet<u32>) {
    let (mut out_l, mut out_x, mut out_m) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut todo: Vec<Repr> = layouts
        .iter()
        .map(|a| Repr::Obj(*a))
        .chain(lists.iter().map(|a| Repr::List(*a)))
        .chain(maps.iter().map(|a| Repr::Map(*a)))
        .collect();
    while let Some(r) = todo.pop() {
        match r {
            Repr::Obj(at) => {
                if !out_l.insert(at) {
                    continue;
                }
                for v in &heap.layout(at).variants {
                    todo.extend(v.fields.iter().map(|(_, r)| *r));
                }
            }
            Repr::List(at) => {
                if !out_x.insert(at) {
                    continue;
                }
                todo.push(heap.element(at));
            }
            Repr::Map(at) => {
                if !out_m.insert(at) {
                    continue;
                }
                let (k, v) = heap.entry(at);
                out_x.insert(k);
                out_x.insert(v);
                todo.push(heap.element(k));
                todo.push(heap.element(v));
            }
            _ => {}
        }
    }
    (out_l, out_x, out_m)
}

/// Building text: the three that make one out of something that is not text.
///
/// `from_int` is Rust's `i64::to_string` and has to be, to the digit. `join` reads a **list** and
/// `split` writes one, so both are declared only when that runtime is there too — which is why they
/// are `Option`s here.
#[derive(Clone, Copy, Debug)]
struct Builds {
    from_int: FuncId,
    repeat: FuncId,
    join: Option<FuncId>,
    /// `str_split`, and `str_chars` with it: the evaluator answers characters for an empty
    /// separator, so the two primitives are one function with two ways of cutting.
    split: Option<FuncId>,
}

impl Builds {
    fn declare(m: &mut ObjectModule, ptr: Type, lists: bool) -> Result<Builds, String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut one = |name: &str, params: &[Type]| -> Result<FuncId, String> {
            let mut sig = cranelift_codegen::ir::Signature::new(conv);
            for p in params {
                sig.params.push(AbiParam::new(*p));
            }
            sig.returns.push(AbiParam::new(types::I64));
            m.declare_function(name, Linkage::Local, &sig)
                .map_err(|e| format!("declaring `{name}`: {e}"))
        };
        let from_int = one("beck.str.from_int", &[ptr, types::I64, types::I32])?;
        let repeat = one(
            "beck.str.repeat",
            &[ptr, types::I64, types::I64, types::I32],
        )?;
        let (join, split) = if lists {
            (
                Some(one(
                    "beck.str.join",
                    &[ptr, types::I64, types::I64, types::I32],
                )?),
                Some(one(
                    "beck.str.split",
                    &[ptr, types::I64, types::I64, types::I32],
                )?),
            )
        } else {
            (None, None)
        };
        Ok(Builds {
            from_int,
            repeat,
            join,
            split,
        })
    }

    fn define(
        self,
        runtime: Runtime,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let arena = runtime.arena.ok_or("text without an arena")?;
        let text = runtime.text.ok_or("a text builder without text")?;
        let lists = runtime.lists;
        let flags = MemFlagsData::trusted();

        let sig = Text::signature(m, &[ptr, types::I64, types::I32], types::I64);
        Text::wrote(self.from_int, sig, 20, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let n = b.block_params(entry)[1];
            let span = b.block_params(entry)[2];
            let neg = b.ins().icmp_imm_s(IntCC::SignedLessThan, n, 0);
            // `0 - i64::MIN` wraps to 2^63, which read as unsigned is exactly its magnitude — the
            // one input where negating in signed arithmetic has no answer.
            let flip = b.ins().ineg(n);
            let u = b.ins().select(neg, flip, n);

            // How many digits, by dividing it away.
            let d = b.declare_var(types::I64);
            let t = b.declare_var(types::I64);
            let one = b.ins().iconst(types::I64, 1);
            b.def_var(d, one);
            b.def_var(t, u);
            let count = b.create_block();
            b.ins().jump(count, &[]);

            b.switch_to_block(count);
            let more = b.create_block();
            let sized = b.create_block();
            let tv = b.use_var(t);
            let t1 = b.ins().udiv_imm_s(tv, 10);
            b.def_var(t, t1);
            let done = b.ins().icmp_imm_s(IntCC::Equal, t1, 0);
            b.ins().brif(done, sized, &[], more, &[]);

            b.switch_to_block(more);
            let dv = b.use_var(d);
            let d1 = b.ins().iadd_imm_s(dv, 1);
            b.def_var(d, d1);
            b.ins().jump(count, &[]);

            b.switch_to_block(sized);
            let digits = b.use_var(d);
            let sign = b.ins().uextend(types::I64, neg);
            let bytes = b.ins().iadd(digits, sign);
            // Every byte is a digit or a minus, so the character count is the byte count.
            let f = m.declare_func_in_func(text.alloc, b.func);
            let call = b.ins().call(f, &[err, bytes, bytes, span]);
            let r = b.inst_results(call)[0];
            let fill = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], fill, &[]);

            b.switch_to_block(fill);
            let p = text.data(arena, r, b, m);
            let minus = b.create_block();
            let start = b.create_block();
            b.ins().brif(neg, minus, &[], start, &[]);
            b.switch_to_block(minus);
            let dash = b.ins().iconst(types::I8, 45);
            b.ins().store(MemFlagsData::trusted(), dash, p, 0);
            b.ins().jump(start, &[]);

            // Backwards from the last byte, which is the order division produces them in.
            b.switch_to_block(start);
            let i = b.declare_var(types::I64);
            let v = b.declare_var(types::I64);
            b.def_var(i, bytes);
            b.def_var(v, u);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let iv = b.use_var(i);
            let vv = b.use_var(v);
            let i1 = b.ins().iadd_imm_s(iv, -1);
            let rem = b.ins().urem_imm_s(vv, 10);
            let v1 = b.ins().udiv_imm_s(vv, 10);
            let ch = b.ins().ireduce(types::I8, rem);
            let byte = b.ins().iadd_imm_s(ch, 48);
            let at = b.ins().iadd(p, i1);
            b.ins().store(MemFlagsData::trusted(), byte, at, 0);
            b.def_var(i, i1);
            b.def_var(v, v1);
            let left = b.ins().icmp_imm_s(IntCC::Equal, v1, 0);
            b.ins().brif(left, out, &[], loop_, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })?;

        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.repeat, sig, 21, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let s = b.block_params(entry)[1];
            let n = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            // The evaluator's bound, so the same answer.
            let zero = b.ins().iconst(types::I64, 0);
            let neg = b.ins().icmp_imm_s(IntCC::SignedLessThan, n, 0);
            let low = b.ins().select(neg, zero, n);
            let cap = b.ins().iconst(types::I64, 1_000_000);
            let big = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, low, 1_000_000);
            let k = b.ins().select(big, cap, low);
            let lb = text.header(arena, s, 0, b, m);
            let lc = text.header(arena, s, heap::WORD as i64, b, m);
            let tb = b.ins().imul(lb, k);
            let tc = b.ins().imul(lc, k);
            let f = m.declare_func_in_func(text.alloc, b.func);
            let call = b.ins().call(f, &[err, tb, tc, span]);
            let r = b.inst_results(call)[0];
            let copy = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], copy, &[]);

            b.switch_to_block(copy);
            let pr = text.data(arena, r, b, m);
            let ps = text.data(arena, s, b, m);
            let i = b.declare_var(types::I64);
            let z = b.ins().iconst(types::I64, 0);
            b.def_var(i, z);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let step = b.create_block();
            let iv = b.use_var(i);
            let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, iv, k);
            b.ins().brif(past, out, &[], step, &[]);

            b.switch_to_block(step);
            let at = b.ins().imul(iv, lb);
            let dst = b.ins().iadd(pr, at);
            let config = m.target_config();
            b.call_memcpy(config, dst, ps, lb);
            let j = b.ins().iadd_imm_s(iv, 1);
            b.def_var(i, j);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })?;

        let (Some(join), Some(lists)) = (self.join, lists) else {
            return Ok(());
        };
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(join, sig, 22, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let xs = b.block_params(entry)[1];
            let sep = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            let n = lists.count(arena, xs, b, m);
            let p = lists.data(arena, xs, b, m);
            let sb = text.header(arena, sep, 0, b, m);
            let sc = text.header(arena, sep, heap::WORD as i64, b, m);
            let alloc = m.declare_func_in_func(text.alloc, b.func);

            let none = b.create_block();
            let measure = b.create_block();
            let empty = b.ins().icmp_imm_s(IntCC::Equal, n, 0);
            b.ins().brif(empty, none, &[], measure, &[]);

            b.switch_to_block(none);
            let z = b.ins().iconst(types::I64, 0);
            let call = b.ins().call(alloc, &[err, z, z, span]);
            let e = b.inst_results(call)[0];
            b.ins().return_(&[e]);

            b.switch_to_block(measure);
            let i = b.declare_var(types::I64);
            let bs = b.declare_var(types::I64);
            let cs = b.declare_var(types::I64);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(i, zero);
            b.def_var(bs, zero);
            b.def_var(cs, zero);
            let sum = b.create_block();
            b.ins().jump(sum, &[]);

            b.switch_to_block(sum);
            let add = b.create_block();
            let sized = b.create_block();
            let iv = b.use_var(i);
            let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, iv, n);
            b.ins().brif(past, sized, &[], add, &[]);

            b.switch_to_block(add);
            let off = b.ins().imul_imm_s(iv, heap::WORD as i64);
            let cell = b.ins().iadd(p, off);
            let x = b.ins().load(types::I64, flags, cell, 0);
            let xb = text.header(arena, x, 0, b, m);
            let xc = text.header(arena, x, heap::WORD as i64, b, m);
            let b0 = b.use_var(bs);
            let c0 = b.use_var(cs);
            let b1 = b.ins().iadd(b0, xb);
            let c1 = b.ins().iadd(c0, xc);
            b.def_var(bs, b1);
            b.def_var(cs, c1);
            let i1 = b.ins().iadd_imm_s(iv, 1);
            b.def_var(i, i1);
            b.ins().jump(sum, &[]);

            // One separator between each pair, which is `n - 1` of them.
            b.switch_to_block(sized);
            let gaps = b.ins().iadd_imm_s(n, -1);
            let sepb = b.ins().imul(gaps, sb);
            let sepc = b.ins().imul(gaps, sc);
            let bt = b.use_var(bs);
            let ct = b.use_var(cs);
            let tb = b.ins().iadd(bt, sepb);
            let tc = b.ins().iadd(ct, sepc);
            let call = b.ins().call(alloc, &[err, tb, tc, span]);
            let r = b.inst_results(call)[0];
            let write = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], write, &[]);

            b.switch_to_block(write);
            let pr = text.data(arena, r, b, m);
            let pv = text.data(arena, sep, b, m);
            let k = b.declare_var(types::I64);
            let at = b.declare_var(types::I64);
            b.def_var(k, zero);
            b.def_var(at, zero);
            let walk = b.create_block();
            b.ins().jump(walk, &[]);

            b.switch_to_block(walk);
            let maybe = b.create_block();
            let kv = b.use_var(k);
            let fin = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, kv, n);
            b.ins().brif(fin, out, &[], maybe, &[]);

            b.switch_to_block(maybe);
            let gap = b.create_block();
            let part = b.create_block();
            let first = b.ins().icmp_imm_s(IntCC::Equal, kv, 0);
            b.ins().brif(first, part, &[], gap, &[]);

            b.switch_to_block(gap);
            let here = b.use_var(at);
            let gd = b.ins().iadd(pr, here);
            let config = m.target_config();
            b.call_memcpy(config, gd, pv, sb);
            let at1 = b.ins().iadd(here, sb);
            b.def_var(at, at1);
            b.ins().jump(part, &[]);

            b.switch_to_block(part);
            let cursor = b.use_var(at);
            let off = b.ins().imul_imm_s(kv, heap::WORD as i64);
            let pc = b.ins().iadd(p, off);
            let px = b.ins().load(types::I64, flags, pc, 0);
            let pb = text.header(arena, px, 0, b, m);
            let pd = text.data(arena, px, b, m);
            let dst = b.ins().iadd(pr, cursor);
            b.call_memcpy(config, dst, pd, pb);
            let at2 = b.ins().iadd(cursor, pb);
            b.def_var(at, at2);
            let k1 = b.ins().iadd_imm_s(kv, 1);
            b.def_var(k, k1);
            b.ins().jump(walk, &[]);

            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })?;

        let Some(split) = self.split else {
            return Ok(());
        };
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(split, sig, 23, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let s = b.block_params(entry)[1];
            let sep = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];

            let len = text.header(arena, s, 0, b, m);
            let findat = m.declare_func_in_func(text.findat, b.func);
            let piece = m.declare_func_in_func(text.piece, b.func);
            let zero = b.ins().iconst(types::I64, 0);
            let one = b.ins().iconst(types::I64, 1);

            // `str_chars` passes the offset **0**, which is never a live object — `beck.alloc`
            // answers it only on a full arena — so it costs no literal, and a program that writes
            // `str_split(s, "")` reaches the same path through the length test below.
            let chars = b.create_block();
            let measure = b.create_block();
            let tally = b.create_block();
            let take = b.create_block();
            b.append_block_param(take, types::I64);
            b.append_block_param(take, types::I64);
            // `seplen` is measured on one of the two paths in, and the cutting loop below is
            // reachable from both — so it travels as a block parameter rather than on a dominance
            // that is not there.
            b.append_block_param(take, types::I64);
            let none = b.ins().icmp_imm_s(IntCC::Equal, sep, 0);
            b.ins().brif(none, chars, &[], measure, &[]);

            b.switch_to_block(measure);
            let seplen = text.header(arena, sep, 0, b, m);
            let bychar = b.ins().icmp_imm_s(IntCC::Equal, seplen, 0);
            b.ins().brif(bychar, chars, &[], tally, &[]);

            // Every character is a piece, and the header already knows how many there are.
            b.switch_to_block(chars);
            let n = text.header(arena, s, heap::WORD as i64, b, m);
            b.ins().jump(take, &[n.into(), one.into(), zero.into()]);

            // One more piece than there are occurrences, which is what `str::split` answers —
            // including for the empty string, where nothing is found and the one piece is the
            // string itself.
            b.switch_to_block(tally);
            let at = b.declare_var(types::I64);
            let seen = b.declare_var(types::I64);
            b.def_var(at, zero);
            b.def_var(seen, zero);
            let counting = b.create_block();
            b.ins().jump(counting, &[]);

            b.switch_to_block(counting);
            let again = b.create_block();
            let counted = b.create_block();
            let from = b.use_var(at);
            let call = b.ins().call(findat, &[s, sep, from]);
            let hit = b.inst_results(call)[0];
            let gone = b.ins().icmp_imm_s(IntCC::SignedLessThan, hit, 0);
            b.ins().brif(gone, counted, &[], again, &[]);

            b.switch_to_block(again);
            let past = b.ins().iadd(hit, seplen);
            b.def_var(at, past);
            let more = b.use_var(seen);
            let up = b.ins().iadd_imm_s(more, 1);
            b.def_var(seen, up);
            b.ins().jump(counting, &[]);

            b.switch_to_block(counted);
            let occurrences = b.use_var(seen);
            let parts = b.ins().iadd_imm_s(occurrences, 1);
            b.ins()
                .jump(take, &[parts.into(), zero.into(), seplen.into()]);

            b.switch_to_block(take);
            let count = b.block_params(take)[0];
            let onechar = b.block_params(take)[1];
            let width = b.block_params(take)[2];
            let alloc = m.declare_func_in_func(lists.alloc, b.func);
            let call = b.ins().call(alloc, &[err, count, span]);
            let xs = b.inst_results(call)[0];
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let which = b.create_block();
            let nolist = b.ins().icmp_imm_s(IntCC::Equal, xs, 0);
            b.ins().brif(nolist, out, &[zero.into()], which, &[]);

            b.switch_to_block(which);
            let walking = b.create_block();
            let cutting = b.create_block();
            let cursor = b.declare_var(types::I64);
            let slot = b.declare_var(types::I64);
            b.def_var(cursor, zero);
            b.def_var(slot, zero);
            let single = b.ins().icmp_imm_s(IntCC::Equal, onechar, 1);
            b.ins().brif(single, walking, &[], cutting, &[]);

            // A character is its lead byte and every continuation after it. Nothing here decodes:
            // a piece is the byte range between two lead bytes.
            b.switch_to_block(walking);
            let stretch = b.create_block();
            let ci = b.use_var(cursor);
            let done = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, ci, len);
            b.ins().brif(done, out, &[xs.into()], stretch, &[]);

            b.switch_to_block(stretch);
            let cp = text.data(arena, s, b, m);
            let reach = b.create_block();
            let edge = b.declare_var(types::I64);
            let c1 = b.ins().iadd_imm_s(ci, 1);
            b.def_var(edge, c1);
            b.ins().jump(reach, &[]);

            b.switch_to_block(reach);
            let clook = b.create_block();
            let cend = b.create_block();
            let ck = b.use_var(edge);
            let cover = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, ck, len);
            b.ins().brif(cover, cend, &[], clook, &[]);

            b.switch_to_block(clook);
            let continues = b.create_block();
            let cbp = b.ins().iadd(cp, ck);
            let cb = b.ins().load(types::I8, MemFlagsData::trusted(), cbp, 0);
            let ctop = b.ins().band_imm_u(cb, 0xc0);
            let ccont = b.ins().icmp_imm_u(IntCC::Equal, ctop, 0x80);
            b.ins().brif(ccont, continues, &[], cend, &[]);

            b.switch_to_block(continues);
            let ck1 = b.ins().iadd_imm_s(ck, 1);
            b.def_var(edge, ck1);
            b.ins().jump(reach, &[]);

            b.switch_to_block(cend);
            let wrote = b.create_block();
            let cj = b.use_var(edge);
            let call = b.ins().call(piece, &[err, s, ci, cj, span]);
            let cpiece = b.inst_results(call)[0];
            let cbad = b.ins().icmp_imm_s(IntCC::Equal, cpiece, 0);
            b.ins().brif(cbad, out, &[zero.into()], wrote, &[]);

            // The data pointer is taken here rather than before the loop: a piece allocates, and an
            // allocation can move the arena under a pointer read before it.
            b.switch_to_block(wrote);
            let cdata = lists.data(arena, xs, b, m);
            let cs = b.use_var(slot);
            let coff = b.ins().imul_imm_s(cs, heap::WORD as i64);
            let cwp = b.ins().iadd(cdata, coff);
            b.ins().store(MemFlagsData::trusted(), cpiece, cwp, 0);
            let cnext = b.ins().iadd_imm_s(cs, 1);
            b.def_var(slot, cnext);
            b.def_var(cursor, cj);
            b.ins().jump(walking, &[]);

            b.switch_to_block(cutting);
            let store = b.create_block();
            let lo = b.use_var(cursor);
            let call = b.ins().call(findat, &[s, sep, lo]);
            let found = b.inst_results(call)[0];
            let last = b.ins().icmp_imm_s(IntCC::SignedLessThan, found, 0);
            let upto = b.ins().select(last, len, found);
            let call = b.ins().call(piece, &[err, s, lo, upto, span]);
            let part = b.inst_results(call)[0];
            let bad = b.ins().icmp_imm_s(IntCC::Equal, part, 0);
            b.ins().brif(bad, out, &[zero.into()], store, &[]);

            b.switch_to_block(store);
            let stored = b.create_block();
            let data = lists.data(arena, xs, b, m);
            let sl = b.use_var(slot);
            let off = b.ins().imul_imm_s(sl, heap::WORD as i64);
            let wp = b.ins().iadd(data, off);
            b.ins().store(MemFlagsData::trusted(), part, wp, 0);
            b.ins().brif(last, out, &[xs.into()], stored, &[]);

            b.switch_to_block(stored);
            let after = b.ins().iadd(found, width);
            b.def_var(cursor, after);
            let onwards = b.ins().iadd_imm_s(sl, 1);
            b.def_var(slot, onwards);
            b.ins().jump(cutting, &[]);

            b.switch_to_block(out);
            let answer = b.block_params(out)[0];
            b.ins().return_(&[answer]);
        })
    }
}

/// The three functions that do not care what a key or a value *is*.
///
/// A map is a count, then every key in key order, then every value in the same order. The keys
/// being one contiguous run is what makes the search a binary one; the values being another is what
/// makes `map_keys` and `map_values` one `memcpy` each into a fresh list.
#[derive(Clone, Copy, Debug)]
struct Maps {
    /// A subtree's size, which is `0` for the empty map and the root's first word otherwise.
    size: FuncId,
    /// A fresh node, its size summed from its children.
    node: FuncId,
    /// Adams's rebalance — see `beck_llvm::heap::MAP_NODE` and the LLVM emitter's `MAPS`.
    balance: FuncId,
    /// The `i`th entry in key order, by subtree size.
    nth: FuncId,
    /// The in-order walk that fills a list, told which word of a node to take.
    into: FuncId,
    /// `map_keys` and `map_values`: a fresh list of the map's size, filled by that walk.
    run: FuncId,
}

impl Maps {
    fn declare(m: &mut ObjectModule, ptr: Type) -> Result<Maps, String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut one = |name: &str, params: &[Type], ret: Type| -> Result<FuncId, String> {
            let mut sig = cranelift_codegen::ir::Signature::new(conv);
            for p in params {
                sig.params.push(AbiParam::new(*p));
            }
            sig.returns.push(AbiParam::new(ret));
            m.declare_function(name, Linkage::Local, &sig)
                .map_err(|e| format!("declaring `{name}`: {e}"))
        };
        Ok(Maps {
            size: one("beck.map.size", &[types::I64], types::I64)?,
            node: one(
                "beck.map.node",
                &[
                    ptr,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I32,
                ],
                types::I64,
            )?,
            balance: one(
                "beck.map.balance",
                &[
                    ptr,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I32,
                ],
                types::I64,
            )?,
            nth: one("beck.map.nth", &[types::I64, types::I64], types::I64)?,
            into: one(
                "beck.map.into",
                &[types::I64, ptr, types::I64, types::I64],
                types::I64,
            )?,
            run: one(
                "beck.map.run",
                &[ptr, types::I64, types::I64, types::I32],
                types::I64,
            )?,
        })
    }

    /// One word of a node, by slot. Inline rather than a runtime function, because a load off a
    /// known offset is what it is — the other emitter names them for the sake of reading its own
    /// rotations, and this one has variables to name them with.
    fn field(
        self,
        arena: Arena,
        node: IrValue,
        slot: usize,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let base = arena.base(b, m);
        let p = b.ins().iadd(base, node);
        b.ins().load(
            types::I64,
            MemFlagsData::trusted(),
            p,
            (slot * heap::WORD as usize) as i32,
        )
    }

    fn sized(self, node: IrValue, b: &mut FunctionBuilder<'_>, m: &mut ObjectModule) -> IrValue {
        let f = m.declare_func_in_func(self.size, b.func);
        let call = b.ins().call(f, &[node]);
        b.inst_results(call)[0]
    }

    fn define(
        self,
        arena: Arena,
        lists: Lists,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        // `size`: zero for the empty map, the root's first word otherwise.
        let sig = Text::signature(m, &[types::I64], types::I64);
        Text::wrote(self.size, sig, 15, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let node = b.block_params(entry)[0];
            let some = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let empty = b.ins().icmp_imm_s(IntCC::Equal, node, 0);
            let zero = b.ins().iconst(types::I64, 0);
            b.ins().brif(empty, out, &[zero.into()], some, &[]);
            b.switch_to_block(some);
            let base = arena.base(b, m);
            let p = b.ins().iadd(base, node);
            let n = b.ins().load(types::I64, MemFlagsData::trusted(), p, 0);
            b.ins().jump(out, &[n.into()]);
            b.switch_to_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })?;

        // `node`: five words, its size summed from its children.
        let sig = Text::signature(
            m,
            &[
                ptr,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I32,
            ],
            types::I64,
        );
        Text::wrote(self.node, sig, 14, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let k = b.block_params(entry)[1];
            let v = b.block_params(entry)[2];
            let l = b.block_params(entry)[3];
            let r = b.block_params(entry)[4];
            let span = b.block_params(entry)[5];
            let ls = self.sized(l, b, m);
            let rs = self.sized(r, b, m);
            let sub = b.ins().iadd(ls, rs);
            let size = b.ins().iadd_imm_s(sub, 1);
            let f = arena.alloc_in(b, m);
            let total = b.ins().iconst(types::I64, heap::MAP_NODE as i64);
            let call = b.ins().call(f, &[err, total, span]);
            let off = b.inst_results(call)[0];
            let fill = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let failed = b.ins().icmp_imm_s(IntCC::Equal, off, 0);
            b.ins().brif(failed, out, &[off.into()], fill, &[]);
            b.switch_to_block(fill);
            let base = arena.base(b, m);
            let p = b.ins().iadd(base, off);
            let flags = MemFlagsData::trusted();
            let w = heap::WORD as i32;
            b.ins().store(flags, size, p, 0);
            b.ins().store(flags, k, p, w * heap::NODE_KEY as i32);
            b.ins().store(flags, v, p, w * heap::NODE_VALUE as i32);
            b.ins().store(flags, l, p, w * heap::NODE_LEFT as i32);
            b.ins().store(flags, r, p, w * heap::NODE_RIGHT as i32);
            b.ins().jump(out, &[off.into()]);
            b.switch_to_block(out);
            let r = b.block_params(out)[0];
            b.ins().return_(&[r]);
        })?;

        // `balance`: Adams's four cases, with `beck_core::pmap`'s own DELTA and RATIO.
        let sig = Text::signature(
            m,
            &[
                ptr,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I32,
            ],
            types::I64,
        );
        Text::wrote(self.balance, sig, 16, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let k = b.block_params(entry)[1];
            let v = b.block_params(entry)[2];
            let l = b.block_params(entry)[3];
            let r = b.block_params(entry)[4];
            let span = b.block_params(entry)[5];
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let plain = |k: IrValue,
                         v: IrValue,
                         l: IrValue,
                         r: IrValue,
                         b: &mut FunctionBuilder<'_>,
                         m: &mut ObjectModule| {
                let f = m.declare_func_in_func(self.node, b.func);
                let call = b.ins().call(f, &[err, k, v, l, r, span]);
                b.inst_results(call)[0]
            };

            let ls = self.sized(l, b, m);
            let rs = self.sized(r, b, m);
            let tot = b.ins().iadd(ls, rs);
            let tiny = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, tot, 1);
            let flat = b.create_block();
            let ask_right = b.create_block();
            b.ins().brif(tiny, flat, &[], ask_right, &[]);

            b.switch_to_block(flat);
            let one = plain(k, v, l, r, b, m);
            b.ins().jump(out, &[one.into()]);

            b.switch_to_block(ask_right);
            let ld = b.ins().imul_imm_s(ls, heap::DELTA as i64);
            let heavy_r = b.ins().icmp(IntCC::UnsignedGreaterThan, rs, ld);
            let left_rot = b.create_block();
            let ask_left = b.create_block();
            b.ins().brif(heavy_r, left_rot, &[], ask_left, &[]);

            b.switch_to_block(left_rot);
            let rk = self.field(arena, r, heap::NODE_KEY, b, m);
            let rv = self.field(arena, r, heap::NODE_VALUE, b, m);
            let rl = self.field(arena, r, heap::NODE_LEFT, b, m);
            let rr = self.field(arena, r, heap::NODE_RIGHT, b, m);
            let rls = self.sized(rl, b, m);
            let rrs = self.sized(rr, b, m);
            let rrx = b.ins().imul_imm_s(rrs, heap::RATIO as i64);
            let single = b.ins().icmp(IntCC::UnsignedLessThan, rls, rrx);
            let single_left = b.create_block();
            let double_left = b.create_block();
            b.ins().brif(single, single_left, &[], double_left, &[]);

            b.switch_to_block(single_left);
            let inner = plain(k, v, l, rl, b, m);
            let sl = plain(rk, rv, inner, rr, b, m);
            b.ins().jump(out, &[sl.into()]);

            b.switch_to_block(double_left);
            let rlk = self.field(arena, rl, heap::NODE_KEY, b, m);
            let rlv = self.field(arena, rl, heap::NODE_VALUE, b, m);
            let rll = self.field(arena, rl, heap::NODE_LEFT, b, m);
            let rlr = self.field(arena, rl, heap::NODE_RIGHT, b, m);
            let a = plain(k, v, l, rll, b, m);
            let c = plain(rk, rv, rlr, rr, b, m);
            let dl = plain(rlk, rlv, a, c, b, m);
            b.ins().jump(out, &[dl.into()]);

            b.switch_to_block(ask_left);
            let rd = b.ins().imul_imm_s(rs, heap::DELTA as i64);
            let heavy_l = b.ins().icmp(IntCC::UnsignedGreaterThan, ls, rd);
            let right_rot = b.create_block();
            let settled = b.create_block();
            b.ins().brif(heavy_l, right_rot, &[], settled, &[]);

            b.switch_to_block(settled);
            let same = plain(k, v, l, r, b, m);
            b.ins().jump(out, &[same.into()]);

            b.switch_to_block(right_rot);
            let lk = self.field(arena, l, heap::NODE_KEY, b, m);
            let lv = self.field(arena, l, heap::NODE_VALUE, b, m);
            let ll = self.field(arena, l, heap::NODE_LEFT, b, m);
            let lr = self.field(arena, l, heap::NODE_RIGHT, b, m);
            let lls = self.sized(ll, b, m);
            let lrs = self.sized(lr, b, m);
            let llx = b.ins().imul_imm_s(lls, heap::RATIO as i64);
            let single_r = b.ins().icmp(IntCC::UnsignedLessThan, lrs, llx);
            let single_right = b.create_block();
            let double_right = b.create_block();
            b.ins().brif(single_r, single_right, &[], double_right, &[]);

            b.switch_to_block(single_right);
            let inner = plain(k, v, lr, r, b, m);
            let sr = plain(lk, lv, ll, inner, b, m);
            b.ins().jump(out, &[sr.into()]);

            b.switch_to_block(double_right);
            let lrk = self.field(arena, lr, heap::NODE_KEY, b, m);
            let lrv = self.field(arena, lr, heap::NODE_VALUE, b, m);
            let lrl = self.field(arena, lr, heap::NODE_LEFT, b, m);
            let lrr = self.field(arena, lr, heap::NODE_RIGHT, b, m);
            let a = plain(lk, lv, ll, lrl, b, m);
            let c = plain(k, v, lrr, r, b, m);
            let dr = plain(lrk, lrv, a, c, b, m);
            b.ins().jump(out, &[dr.into()]);

            b.switch_to_block(out);
            let answer = b.block_params(out)[0];
            b.ins().return_(&[answer]);
        })?;

        // `nth`: down the tree by subtree size.
        let sig = Text::signature(m, &[types::I64, types::I64], types::I64);
        Text::wrote(self.nth, sig, 17, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let root = b.block_params(entry)[0];
            let want = b.block_params(entry)[1];
            let node = b.declare_var(types::I64);
            let idx = b.declare_var(types::I64);
            b.def_var(node, root);
            b.def_var(idx, want);
            let loop_ = b.create_block();
            b.ins().jump(loop_, &[]);

            b.switch_to_block(loop_);
            let probe = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let n = b.use_var(node);
            let empty = b.ins().icmp_imm_s(IntCC::Equal, n, 0);
            let zero = b.ins().iconst(types::I64, 0);
            b.ins().brif(empty, out, &[zero.into()], probe, &[]);

            b.switch_to_block(probe);
            let step = b.create_block();
            let i = b.use_var(idx);
            let l = self.field(arena, n, heap::NODE_LEFT, b, m);
            let ls = self.sized(l, b, m);
            let here = b.ins().icmp(IntCC::Equal, i, ls);
            b.ins().brif(here, out, &[n.into()], step, &[]);

            b.switch_to_block(step);
            let down = b.ins().icmp(IntCC::UnsignedLessThan, i, ls);
            let r = self.field(arena, n, heap::NODE_RIGHT, b, m);
            let next = b.ins().select(down, l, r);
            let past = b.ins().iadd_imm_s(ls, 1);
            let rest = b.ins().isub(i, past);
            let i1 = b.ins().select(down, i, rest);
            b.def_var(node, next);
            b.def_var(idx, i1);
            b.ins().jump(loop_, &[]);

            b.switch_to_block(out);
            let answer = b.block_params(out)[0];
            b.ins().return_(&[answer]);
        })?;

        // `into`: the in-order walk, recursive, told which word to take.
        let sig = Text::signature(m, &[types::I64, ptr, types::I64, types::I64], types::I64);
        Text::wrote(self.into, sig, 18, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let node = b.block_params(entry)[0];
            let dst = b.block_params(entry)[1];
            let i = b.block_params(entry)[2];
            let slot = b.block_params(entry)[3];
            let walk = b.create_block();
            let out = b.create_block();
            b.append_block_param(out, types::I64);
            let empty = b.ins().icmp_imm_s(IntCC::Equal, node, 0);
            b.ins().brif(empty, out, &[i.into()], walk, &[]);

            b.switch_to_block(walk);
            let me = m.declare_func_in_func(self.into, b.func);
            let l = self.field(arena, node, heap::NODE_LEFT, b, m);
            let call = b.ins().call(me, &[l, dst, i, slot]);
            let i1 = b.inst_results(call)[0];
            let base = arena.base(b, m);
            let p = b.ins().iadd(base, node);
            let off = b.ins().imul_imm_s(slot, heap::WORD as i64);
            let at = b.ins().iadd(p, off);
            let w = b.ins().load(types::I64, MemFlagsData::trusted(), at, 0);
            let step = b.ins().imul_imm_s(i1, heap::WORD as i64);
            let cell = b.ins().iadd(dst, step);
            b.ins().store(MemFlagsData::trusted(), w, cell, 0);
            let i2 = b.ins().iadd_imm_s(i1, 1);
            let me = m.declare_func_in_func(self.into, b.func);
            let r = self.field(arena, node, heap::NODE_RIGHT, b, m);
            let call = b.ins().call(me, &[r, dst, i2, slot]);
            let i3 = b.inst_results(call)[0];
            b.ins().jump(out, &[i3.into()]);

            b.switch_to_block(out);
            let answer = b.block_params(out)[0];
            b.ins().return_(&[answer]);
        })?;

        // `run`: a fresh list of the map's size, filled by the walk.
        let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
        Text::wrote(self.run, sig, 19, m, ctx, fctx, |b, m| {
            let entry = b.current_block().expect("an entry block");
            let err = b.block_params(entry)[0];
            let map = b.block_params(entry)[1];
            let slot = b.block_params(entry)[2];
            let span = b.block_params(entry)[3];
            let n = self.sized(map, b, m);
            let f = m.declare_func_in_func(lists.alloc, b.func);
            let call = b.ins().call(f, &[err, n, span]);
            let r = b.inst_results(call)[0];
            let fill = b.create_block();
            let out = b.create_block();
            let failed = b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            b.ins().brif(failed, out, &[], fill, &[]);
            b.switch_to_block(fill);
            let dst = lists.data(arena, r, b, m);
            let me = m.declare_func_in_func(self.into, b.func);
            let zero = b.ins().iconst(types::I64, 0);
            b.ins().call(me, &[map, dst, zero, slot]);
            b.ins().jump(out, &[]);
            b.switch_to_block(out);
            b.ins().return_(&[r]);
        })
    }
}

/// The functions a map of a given key and value repr needs, generated per repr.
///
/// A `Map` is a weight-balanced tree (`beck_llvm::heap::MAP_NODE`), and everything that *moves*
/// nodes is one function for the whole module — only the ones that **compare** are per repr. That is
/// the search, the insert, the delete and the two-map order, plus the delete's two helpers.
fn map_functions(
    at: u32,
    heap: &Heap,
    arena: Arena,
    runtime: Runtime,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    let (key, value) = heap.entry(at);
    let maps = runtime
        .maps
        .ok_or("a map in a module with no map runtime")?;
    let elem = |i: u32, m: &mut ObjectModule| -> Result<FuncId, String> {
        let sig = compare_signature(m);
        m.declare_function(&format!("beck.elem.cmp.{i}"), Linkage::Local, &sig)
            .map_err(|e| format!("declaring a comparison: {e}"))
    };
    let ptr = m.isa().pointer_type();
    let key_cmp = elem(key, m)?;
    let value_cmp = elem(value, m)?;
    let one = |m: &mut ObjectModule, name: String, params: &[Type]| -> Result<FuncId, String> {
        let mut sig =
            cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
        for p in params {
            sig.params.push(AbiParam::new(*p));
        }
        sig.returns.push(AbiParam::new(types::I64));
        m.declare_function(&name, Linkage::Local, &sig)
            .map_err(|e| format!("declaring `{name}`: {e}"))
    };
    let find = one(m, format!("beck.map.find.{at}"), &[types::I64, types::I64])?;
    let ins = one(
        m,
        format!("beck.map.ins.{at}"),
        &[ptr, types::I64, types::I64, types::I64, types::I32],
    )?;
    let min = one(m, format!("beck.map.min.{at}"), &[types::I64])?;
    let pop = one(
        m,
        format!("beck.map.pop.{at}"),
        &[ptr, types::I64, types::I32],
    )?;
    let del = one(
        m,
        format!("beck.map.del.{at}"),
        &[ptr, types::I64, types::I64, types::I32],
    )?;
    let merge = one(
        m,
        format!("beck.map.merge.{at}"),
        &[ptr, types::I64, types::I64, types::I32],
    )?;
    let cmp = one(m, format!("beck.map.cmp.{at}"), &[types::I64, types::I64])?;

    // The search: down the tree, comparing keys. Answers the node, or 0.
    let sig = compare_signature(m);
    Text::wrote(find, sig, 40 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let (root, k) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let node = b.declare_var(types::I64);
        b.def_var(node, root);
        let loop_ = b.create_block();
        b.ins().jump(loop_, &[]);

        b.switch_to_block(loop_);
        let probe = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let n = b.use_var(node);
        let empty = b.ins().icmp_imm_s(IntCC::Equal, n, 0);
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().brif(empty, out, &[zero.into()], probe, &[]);

        b.switch_to_block(probe);
        let step = b.create_block();
        let nk = maps.field(arena, n, heap::NODE_KEY, b, m);
        let f = m.declare_func_in_func(key_cmp, b.func);
        let call = b.ins().call(f, &[k, nk]);
        let c = b.inst_results(call)[0];
        let hit = b.ins().icmp_imm_s(IntCC::Equal, c, 0);
        b.ins().brif(hit, out, &[n.into()], step, &[]);

        b.switch_to_block(step);
        let down = b.ins().icmp_imm_s(IntCC::SignedLessThan, c, 0);
        let l = maps.field(arena, n, heap::NODE_LEFT, b, m);
        let r = maps.field(arena, n, heap::NODE_RIGHT, b, m);
        let next = b.ins().select(down, l, r);
        b.def_var(node, next);
        b.ins().jump(loop_, &[]);

        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })?;

    // `map_insert`: rebuild the path, share everything off it, rebalance on the way out.
    let sig = Text::signature(
        m,
        &[ptr, types::I64, types::I64, types::I64, types::I32],
        types::I64,
    );
    Text::wrote(ins, sig, 4_000 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let err = b.block_params(entry)[0];
        let root = b.block_params(entry)[1];
        let k = b.block_params(entry)[2];
        let v = b.block_params(entry)[3];
        let span = b.block_params(entry)[4];
        let walk = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let empty = b.ins().icmp_imm_s(IntCC::Equal, root, 0);
        let fresh = b.create_block();
        b.ins().brif(empty, fresh, &[], walk, &[]);

        b.switch_to_block(fresh);
        let f = m.declare_func_in_func(maps.node, b.func);
        let zero = b.ins().iconst(types::I64, 0);
        let call = b.ins().call(f, &[err, k, v, zero, zero, span]);
        let leaf = b.inst_results(call)[0];
        b.ins().jump(out, &[leaf.into()]);

        b.switch_to_block(walk);
        let mk = maps.field(arena, root, heap::NODE_KEY, b, m);
        let mv = maps.field(arena, root, heap::NODE_VALUE, b, m);
        let ml = maps.field(arena, root, heap::NODE_LEFT, b, m);
        let mr = maps.field(arena, root, heap::NODE_RIGHT, b, m);
        let f = m.declare_func_in_func(key_cmp, b.func);
        let call = b.ins().call(f, &[k, mk]);
        let c = b.inst_results(call)[0];
        let go_left = b.create_block();
        let not_less = b.create_block();
        let lt = b.ins().icmp_imm_s(IntCC::SignedLessThan, c, 0);
        b.ins().brif(lt, go_left, &[], not_less, &[]);

        b.switch_to_block(go_left);
        let me = m.declare_func_in_func(ins, b.func);
        let call = b.ins().call(me, &[err, ml, k, v, span]);
        let nl = b.inst_results(call)[0];
        let f = m.declare_func_in_func(maps.balance, b.func);
        let call = b.ins().call(f, &[err, mk, mv, nl, mr, span]);
        let bl = b.inst_results(call)[0];
        b.ins().jump(out, &[bl.into()]);

        b.switch_to_block(not_less);
        let go_right = b.create_block();
        let replace = b.create_block();
        let gt = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, c, 0);
        b.ins().brif(gt, go_right, &[], replace, &[]);

        b.switch_to_block(go_right);
        let me = m.declare_func_in_func(ins, b.func);
        let call = b.ins().call(me, &[err, mr, k, v, span]);
        let nr = b.inst_results(call)[0];
        let f = m.declare_func_in_func(maps.balance, b.func);
        let call = b.ins().call(f, &[err, mk, mv, ml, nr, span]);
        let br = b.inst_results(call)[0];
        b.ins().jump(out, &[br.into()]);

        // The *new* key as well as the new value, which is the evaluator's `Ordering::Equal` arm.
        b.switch_to_block(replace);
        let f = m.declare_func_in_func(maps.node, b.func);
        let call = b.ins().call(f, &[err, k, v, ml, mr, span]);
        let same = b.inst_results(call)[0];
        b.ins().jump(out, &[same.into()]);

        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })?;

    // The leftmost node of a subtree, which is its smallest key.
    let sig = Text::signature(m, &[types::I64], types::I64);
    Text::wrote(min, sig, 5_000 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let root = b.block_params(entry)[0];
        let node = b.declare_var(types::I64);
        b.def_var(node, root);
        let loop_ = b.create_block();
        b.ins().jump(loop_, &[]);
        b.switch_to_block(loop_);
        let down = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let n = b.use_var(node);
        let l = maps.field(arena, n, heap::NODE_LEFT, b, m);
        let none = b.ins().icmp_imm_s(IntCC::Equal, l, 0);
        b.ins().brif(none, out, &[n.into()], down, &[]);
        b.switch_to_block(down);
        b.def_var(node, l);
        b.ins().jump(loop_, &[]);
        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })?;

    // The subtree with its smallest node taken out, rebalanced on the way.
    let sig = Text::signature(m, &[ptr, types::I64, types::I32], types::I64);
    Text::wrote(pop, sig, 6_000 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let err = b.block_params(entry)[0];
        let root = b.block_params(entry)[1];
        let span = b.block_params(entry)[2];
        let deeper = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let l = maps.field(arena, root, heap::NODE_LEFT, b, m);
        let r = maps.field(arena, root, heap::NODE_RIGHT, b, m);
        let none = b.ins().icmp_imm_s(IntCC::Equal, l, 0);
        b.ins().brif(none, out, &[r.into()], deeper, &[]);

        b.switch_to_block(deeper);
        let me = m.declare_func_in_func(pop, b.func);
        let call = b.ins().call(me, &[err, l, span]);
        let nl = b.inst_results(call)[0];
        let k = maps.field(arena, root, heap::NODE_KEY, b, m);
        let v = maps.field(arena, root, heap::NODE_VALUE, b, m);
        let f = m.declare_func_in_func(maps.balance, b.func);
        let call = b.ins().call(f, &[err, k, v, nl, r, span]);
        let bal = b.inst_results(call)[0];
        b.ins().jump(out, &[bal.into()]);

        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })?;

    // `map_remove`: the same path rebuild, with the two-child case joined by the right subtree's
    // smallest node.
    let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
    Text::wrote(del, sig, 7_000 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let err = b.block_params(entry)[0];
        let root = b.block_params(entry)[1];
        let k = b.block_params(entry)[2];
        let span = b.block_params(entry)[3];
        let walk = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let empty = b.ins().icmp_imm_s(IntCC::Equal, root, 0);
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().brif(empty, out, &[zero.into()], walk, &[]);

        b.switch_to_block(walk);
        let mk = maps.field(arena, root, heap::NODE_KEY, b, m);
        let mv = maps.field(arena, root, heap::NODE_VALUE, b, m);
        let ml = maps.field(arena, root, heap::NODE_LEFT, b, m);
        let mr = maps.field(arena, root, heap::NODE_RIGHT, b, m);
        let f = m.declare_func_in_func(key_cmp, b.func);
        let call = b.ins().call(f, &[k, mk]);
        let c = b.inst_results(call)[0];
        let go_left = b.create_block();
        let not_less = b.create_block();
        let lt = b.ins().icmp_imm_s(IntCC::SignedLessThan, c, 0);
        b.ins().brif(lt, go_left, &[], not_less, &[]);

        b.switch_to_block(go_left);
        let me = m.declare_func_in_func(del, b.func);
        let call = b.ins().call(me, &[err, ml, k, span]);
        let nl = b.inst_results(call)[0];
        let f = m.declare_func_in_func(maps.balance, b.func);
        let call = b.ins().call(f, &[err, mk, mv, nl, mr, span]);
        let bl = b.inst_results(call)[0];
        b.ins().jump(out, &[bl.into()]);

        b.switch_to_block(not_less);
        let go_right = b.create_block();
        let here = b.create_block();
        let gt = b.ins().icmp_imm_s(IntCC::SignedGreaterThan, c, 0);
        b.ins().brif(gt, go_right, &[], here, &[]);

        b.switch_to_block(go_right);
        let me = m.declare_func_in_func(del, b.func);
        let call = b.ins().call(me, &[err, mr, k, span]);
        let nr = b.inst_results(call)[0];
        let f = m.declare_func_in_func(maps.balance, b.func);
        let call = b.ins().call(f, &[err, mk, mv, ml, nr, span]);
        let br = b.inst_results(call)[0];
        b.ins().jump(out, &[br.into()]);

        b.switch_to_block(here);
        let maybe = b.create_block();
        let no_left = b.ins().icmp_imm_s(IntCC::Equal, ml, 0);
        b.ins().brif(no_left, out, &[mr.into()], maybe, &[]);

        b.switch_to_block(maybe);
        let join = b.create_block();
        let no_right = b.ins().icmp_imm_s(IntCC::Equal, mr, 0);
        b.ins().brif(no_right, out, &[ml.into()], join, &[]);

        b.switch_to_block(join);
        let f = m.declare_func_in_func(min, b.func);
        let call = b.ins().call(f, &[mr]);
        let least = b.inst_results(call)[0];
        let lk = maps.field(arena, least, heap::NODE_KEY, b, m);
        let lv = maps.field(arena, least, heap::NODE_VALUE, b, m);
        let f = m.declare_func_in_func(pop, b.func);
        let call = b.ins().call(f, &[err, mr, span]);
        let rest = b.inst_results(call)[0];
        let f = m.declare_func_in_func(maps.balance, b.func);
        let call = b.ins().call(f, &[err, lk, lv, ml, rest, span]);
        let joined = b.inst_results(call)[0];
        b.ins().jump(out, &[joined.into()]);

        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })?;

    // `map_merge`: every entry of the second inserted into the first, so the later map wins.
    let sig = Text::signature(m, &[ptr, types::I64, types::I64, types::I32], types::I64);
    Text::wrote(merge, sig, 8_000 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let err = b.block_params(entry)[0];
        let a = b.block_params(entry)[1];
        let other = b.block_params(entry)[2];
        let span = b.block_params(entry)[3];
        let n = maps.sized(other, b, m);
        let i = b.declare_var(types::I64);
        let acc = b.declare_var(types::I64);
        let zero = b.ins().iconst(types::I64, 0);
        b.def_var(i, zero);
        b.def_var(acc, a);
        let loop_ = b.create_block();
        b.ins().jump(loop_, &[]);

        b.switch_to_block(loop_);
        let step = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let at_i = b.use_var(i);
        let carried = b.use_var(acc);
        let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at_i, n);
        b.ins().brif(past, out, &[carried.into()], step, &[]);

        b.switch_to_block(step);
        let f = m.declare_func_in_func(maps.nth, b.func);
        let call = b.ins().call(f, &[other, at_i]);
        let node = b.inst_results(call)[0];
        let k = maps.field(arena, node, heap::NODE_KEY, b, m);
        let v = maps.field(arena, node, heap::NODE_VALUE, b, m);
        let f = m.declare_func_in_func(ins, b.func);
        let call = b.ins().call(f, &[err, carried, k, v, span]);
        let next = b.inst_results(call)[0];
        let j = b.ins().iadd_imm_s(at_i, 1);
        b.def_var(i, j);
        b.def_var(acc, next);
        b.ins().jump(loop_, &[]);

        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })?;

    // Two maps in key order: the key, then the value, entry by entry, then the sizes.
    let sig = compare_signature(m);
    Text::wrote(cmp, sig, 9_000 + at, m, ctx, fctx, |b, m| {
        let entry = b.current_block().expect("an entry block");
        let (ma, mb) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let la = maps.sized(ma, b, m);
        let lb = maps.sized(mb, b, m);
        let shorter = b.ins().icmp(IntCC::UnsignedLessThan, la, lb);
        let n = b.ins().select(shorter, la, lb);
        let i = b.declare_var(types::I64);
        let zero = b.ins().iconst(types::I64, 0);
        b.def_var(i, zero);
        let loop_ = b.create_block();
        b.ins().jump(loop_, &[]);

        b.switch_to_block(loop_);
        let keys = b.create_block();
        let lengths = b.create_block();
        let out = b.create_block();
        b.append_block_param(out, types::I64);
        let at_i = b.use_var(i);
        let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at_i, n);
        b.ins().brif(past, lengths, &[], keys, &[]);

        b.switch_to_block(keys);
        let values = b.create_block();
        let f = m.declare_func_in_func(maps.nth, b.func);
        let call = b.ins().call(f, &[ma, at_i]);
        let na = b.inst_results(call)[0];
        let f = m.declare_func_in_func(maps.nth, b.func);
        let call = b.ins().call(f, &[mb, at_i]);
        let nb = b.inst_results(call)[0];
        let ka = maps.field(arena, na, heap::NODE_KEY, b, m);
        let kb = maps.field(arena, nb, heap::NODE_KEY, b, m);
        let f = m.declare_func_in_func(key_cmp, b.func);
        let call = b.ins().call(f, &[ka, kb]);
        let ck = b.inst_results(call)[0];
        let decided = b.ins().icmp_imm_s(IntCC::NotEqual, ck, 0);
        b.ins().brif(decided, out, &[ck.into()], values, &[]);

        b.switch_to_block(values);
        let next = b.create_block();
        let va = maps.field(arena, na, heap::NODE_VALUE, b, m);
        let vb = maps.field(arena, nb, heap::NODE_VALUE, b, m);
        let f = m.declare_func_in_func(value_cmp, b.func);
        let call = b.ins().call(f, &[va, vb]);
        let cv = b.inst_results(call)[0];
        let decided = b.ins().icmp_imm_s(IntCC::NotEqual, cv, 0);
        b.ins().brif(decided, out, &[cv.into()], next, &[]);

        b.switch_to_block(next);
        let j = b.ins().iadd_imm_s(at_i, 1);
        b.def_var(i, j);
        b.ins().jump(loop_, &[]);

        // Equal as far as both go, so the smaller map is the smaller value.
        b.switch_to_block(lengths);
        let lt = b.ins().icmp(IntCC::UnsignedLessThan, la, lb);
        let gt = b.ins().icmp(IntCC::UnsignedGreaterThan, la, lb);
        let up = b.ins().iconst(types::I64, 1);
        let down = b.ins().iconst(types::I64, -1);
        let zero = b.ins().iconst(types::I64, 0);
        let high = b.ins().select(gt, up, zero);
        let r = b.ins().select(lt, down, high);
        b.ins().jump(out, &[r.into()]);

        b.switch_to_block(out);
        let answer = b.block_params(out)[0];
        b.ins().return_(&[answer]);
    })
}

/// The three functions a list of a given element repr needs, generated per repr.
///
/// One three-way comparison over two **words**, and two built on it: the lexicographic order over
/// two lists, and a linear search. Per repr rather than taking a function pointer, because an
/// indirect call is the one thing this backend does not have.
///
/// The order is `Vec<Value>`'s: element by element, and a list that is a prefix of another is less
/// than it.
fn element_functions(
    at: u32,
    heap: &Heap,
    arena: Arena,
    runtime: Runtime,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    let element = heap.element(at);
    // An element with no order at all: the three functions below are a comparison, a lexicographic
    // one and a search, and every one of them is that element's comparison in a loop. Nothing is
    // emitted rather than something that compares offsets — `Body::wants` refuses the demand
    // before this is reached, and a bug in that rule is then a missing symbol at link time rather
    // than a list of views that sorts by where they were allocated. The LLVM emitter returns here
    // for the same reason and in the same place.
    if let heap::Order::Absent(_) = element.order() {
        return Ok(());
    }
    let lists = runtime
        .lists
        .ok_or("a list in a module with no list runtime")?;

    let sig = compare_signature(m);
    let elem = m
        .declare_function(&format!("beck.elem.cmp.{at}"), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a comparison: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(8, at), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let (wa, wb) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let inner = match element {
            Repr::Str => Some(
                runtime
                    .text
                    .ok_or("a list of `Str` in a module with no text")?
                    .cmp,
            ),
            Repr::List(i) => {
                let sig = compare_signature(m);
                Some(
                    m.declare_function(&format!("beck.list.cmp.{i}"), Linkage::Local, &sig)
                        .map_err(|e| format!("declaring a comparison: {e}"))?,
                )
            }
            Repr::Obj(i) => {
                let sig = compare_signature(m);
                Some(
                    m.declare_function(&format!("beck.cmp.{i}"), Linkage::Local, &sig)
                        .map_err(|e| format!("declaring a comparison: {e}"))?,
                )
            }
            _ => None,
        };
        match inner {
            Some(id) => {
                let f = m.declare_func_in_func(id, b.func);
                let call = b.ins().call(f, &[wa, wb]);
                let r = b.inst_results(call)[0];
                b.ins().return_(&[r]);
            }
            None => {
                // A real compares through the order key; an `Int` is signed; a `Bool` is a 0 or a 1
                // and is therefore either way round.
                let (ka, kb) = if element == Repr::Float {
                    (order_key_bits(wa, &mut b), order_key_bits(wb, &mut b))
                } else {
                    (wa, wb)
                };
                let (lt, gt) = if element == Repr::Int {
                    (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
                } else {
                    (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
                };
                let down = b.ins().iconst(types::I64, -1);
                let up = b.ins().iconst(types::I64, 1);
                let zero = b.ins().iconst(types::I64, 0);
                let is_lt = b.ins().icmp(lt, ka, kb);
                let is_gt = b.ins().icmp(gt, ka, kb);
                let high = b.ins().select(is_gt, up, zero);
                let r = b.ins().select(is_lt, down, high);
                b.ins().return_(&[r]);
            }
        }
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(elem, ctx)
        .map_err(|e| format!("defining a comparison: {e}"))?;
    m.clear_context(ctx);

    // The lexicographic order over two lists.
    let sig = compare_signature(m);
    let cmp = m
        .declare_function(&format!("beck.list.cmp.{at}"), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a comparison: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(9, at), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let (xa, xb) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let la = lists.count(arena, xa, &mut b, m);
        let lb = lists.count(arena, xb, &mut b, m);
        let shorter = b.ins().icmp(IntCC::UnsignedLessThan, la, lb);
        let n = b.ins().select(shorter, la, lb);
        let pa = lists.data(arena, xa, &mut b, m);
        let pb = lists.data(arena, xb, &mut b, m);
        let i = b.declare_var(types::I64);
        let zero = b.ins().iconst(types::I64, 0);
        b.def_var(i, zero);
        let loop_ = b.create_block();
        b.ins().jump(loop_, &[]);

        b.switch_to_block(loop_);
        let one = b.create_block();
        let lengths = b.create_block();
        let k = b.use_var(i);
        let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, k, n);
        b.ins().brif(past, lengths, &[], one, &[]);

        b.switch_to_block(one);
        let answer = b.create_block();
        b.append_block_param(answer, types::I64);
        let next = b.create_block();
        let off = b.ins().imul_imm_s(k, heap::WORD as i64);
        let ea = b.ins().iadd(pa, off);
        let eb = b.ins().iadd(pb, off);
        let flags = MemFlagsData::trusted();
        let va = b.ins().load(types::I64, flags, ea, 0);
        let vb = b.ins().load(types::I64, flags, eb, 0);
        let f = m.declare_func_in_func(elem, b.func);
        let call = b.ins().call(f, &[va, vb]);
        let c = b.inst_results(call)[0];
        let decided = b.ins().icmp_imm_s(IntCC::NotEqual, c, 0);
        b.ins().brif(decided, answer, &[c.into()], next, &[]);

        b.switch_to_block(next);
        let j = b.ins().iadd_imm_s(k, 1);
        b.def_var(i, j);
        b.ins().jump(loop_, &[]);

        // Equal as far as both go, so the shorter one is the smaller: `[1] < [1, 2]`.
        b.switch_to_block(lengths);
        let lt = b.ins().icmp(IntCC::UnsignedLessThan, la, lb);
        let gt = b.ins().icmp(IntCC::UnsignedGreaterThan, la, lb);
        let down = b.ins().iconst(types::I64, -1);
        let up = b.ins().iconst(types::I64, 1);
        let z = b.ins().iconst(types::I64, 0);
        let high = b.ins().select(gt, up, z);
        let r = b.ins().select(lt, down, high);
        b.ins().jump(answer, &[r.into()]);

        b.switch_to_block(answer);
        let out = b.block_params(answer)[0];
        b.ins().return_(&[out]);
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(cmp, ctx)
        .map_err(|e| format!("defining a comparison: {e}"))?;
    m.clear_context(ctx);

    // The linear search.
    let sig = compare_signature(m);
    let find = m
        .declare_function(&format!("beck.list.find.{at}"), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a search: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(10, at), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let (xs, w) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let n = lists.count(arena, xs, &mut b, m);
        let p = lists.data(arena, xs, &mut b, m);
        let i = b.declare_var(types::I64);
        let zero = b.ins().iconst(types::I64, 0);
        b.def_var(i, zero);
        let loop_ = b.create_block();
        b.ins().jump(loop_, &[]);

        b.switch_to_block(loop_);
        let one = b.create_block();
        let missing = b.create_block();
        let k = b.use_var(i);
        let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, k, n);
        b.ins().brif(past, missing, &[], one, &[]);

        b.switch_to_block(one);
        let found = b.create_block();
        let next = b.create_block();
        let off = b.ins().imul_imm_s(k, heap::WORD as i64);
        let e = b.ins().iadd(p, off);
        let x = b.ins().load(types::I64, MemFlagsData::trusted(), e, 0);
        let f = m.declare_func_in_func(elem, b.func);
        let call = b.ins().call(f, &[x, w]);
        let c = b.inst_results(call)[0];
        let hit = b.ins().icmp_imm_s(IntCC::Equal, c, 0);
        b.ins().brif(hit, found, &[], next, &[]);

        b.switch_to_block(next);
        let j = b.ins().iadd_imm_s(k, 1);
        b.def_var(i, j);
        b.ins().jump(loop_, &[]);

        b.switch_to_block(found);
        let r = b.use_var(i);
        b.ins().return_(&[r]);

        b.switch_to_block(missing);
        let none = b.ins().iconst(types::I64, -1);
        b.ins().return_(&[none]);
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(find, ctx)
        .map_err(|e| format!("defining a search: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// One `lam` waiting to be written as a function.
struct Pending {
    rank: u32,
    params: Arc<[VarId]>,
    body: Arc<Core>,
    family: u32,
}

/// The list primitives whose argument is a function, each one loop.
///
/// The same four the other emitter generates, written again — `cranelift.rs` is what holds the two
/// to accepting and refusing the same definitions, and an agreement by construction would be worth
/// nothing (§93.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Loop {
    Map,
    Filter,
    Fold,
    Every,
    /// `sort_by` — decorate with the keys, merge stably, undecorate. The one that is not a pass.
    Sort,
}

impl Loop {
    fn symbol(self, fam: u32) -> String {
        match self {
            Loop::Map => format!("beck.list.map.{fam}"),
            Loop::Filter => format!("beck.list.filter.{fam}"),
            Loop::Fold => format!("beck.list.fold.{fam}"),
            Loop::Every => format!("beck.list.every.{fam}"),
            Loop::Sort => format!("beck.list.sort.{fam}"),
        }
    }

    /// A number for `UserFuncName`, which has to be distinct per generated function.
    fn seq(self) -> u32 {
        match self {
            Loop::Map => 0,
            Loop::Filter => 1,
            Loop::Fold => 2,
            Loop::Every => 3,
            Loop::Sort => 4,
        }
    }
}

fn apply_symbol(fam: u32) -> String {
    format!("beck.apply.{fam}")
}

fn lam_symbol(rank: u32) -> String {
    format!("beck.lam.{rank}")
}

/// What applying a closure of this family takes: the error cell, the closure, then its arguments.
///
/// [`CallConv::Tail`] for the reason a definition's is: the arm inside an application is a
/// `return_call`, and that is only available between functions that share the convention.
fn family_signature(fam: &heap::Family, ptr: Type) -> cranelift_codegen::ir::Signature {
    let mut out = cranelift_codegen::ir::Signature::new(CallConv::Tail);
    out.params.push(AbiParam::new(ptr));
    out.params.push(AbiParam::new(types::I64));
    for p in &fam.params {
        out.params.push(AbiParam::new(machine(*p)));
    }
    out.returns.push(AbiParam::new(machine(fam.ret)));
    out
}

/// The zero of a repr, which is what a function that trapped returns.
fn zero_of(r: Repr, b: &mut FunctionBuilder<'_>) -> IrValue {
    match r.machine() {
        Scalar::Int => b.ins().iconst(types::I64, 0),
        Scalar::Float => b.ins().f64const(0.0),
        Scalar::Bool => b.ins().iconst(types::I8, 0),
    }
}

/// Applying a closure of one family: read the rank, and jump to the arm that answers to it.
///
/// A chain of comparisons rather than a jump table, because a rank is a place in the *program's*
/// lambdas and the ranks of one family are not contiguous. Every family in this tree has one or two.
///
/// There is no indirect call and no code address in the arena — see [`heap::CLOSURE_HEADER`], which
/// is where that decision is recorded. An arm for a `lam` passes the closure on so the body can read
/// its captures; an arm for a *definition* does not, because a definition closes over nothing.
#[allow(clippy::too_many_arguments)]
fn apply_function(
    at: u32,
    heap: &Heap,
    emitted: &BTreeSet<u32>,
    compiled: &BTreeMap<Arc<str>, FuncId>,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
    arena: Arena,
) -> Result<(), String> {
    let fam = heap.family(at).clone();
    let ptr = m.target_config().pointer_type();
    let sig = family_signature(&fam, ptr);
    let id = m
        .declare_function(&apply_symbol(at), Linkage::Local, &sig)
        .map_err(|e| format!("declaring an application: {e}"))?;
    // Only the ranks that became code: a rank whose definition was refused has no function to call,
    // and an arm calling one would be a link error rather than a refusal.
    let arms: Vec<u32> = fam
        .ranks
        .iter()
        .copied()
        .filter(|r| match &heap.lam(*r).def {
            Some(name) => compiled.contains_key(name),
            None => emitted.contains(r),
        })
        .collect();

    ctx.func = Function::with_name_signature(UserFuncName::user(13, at), sig.clone());
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let err = b.block_params(entry)[0];
        let clo = b.block_params(entry)[1];
        let args: Vec<IrValue> = (0..fam.params.len())
            .map(|i| b.block_params(entry)[i + 2])
            .collect();
        let base = arena.base(&mut b, m);
        let p = b.ins().iadd(base, clo);
        let rank = b.ins().load(types::I64, MemFlagsData::trusted(), p, 0);

        for r in &arms {
            let take = b.create_block();
            let next = b.create_block();
            let is = b.ins().icmp_imm_s(IntCC::Equal, rank, i64::from(*r));
            b.ins().brif(is, take, &[], next, &[]);
            b.switch_to_block(take);
            b.seal_block(take);
            let lam = heap.lam(*r);
            let mut operands = vec![err];
            let callee = match &lam.def {
                Some(name) => compiled[name],
                None => {
                    operands.push(clo);
                    m.declare_function(&lam_symbol(*r), Linkage::Local, &sig)
                        .map_err(|e| format!("declaring a lambda: {e}"))?
                }
            };
            operands.extend(args.iter().copied());
            let f = m.declare_func_in_func(callee, b.func);
            b.ins().return_call(f, &operands);
            b.switch_to_block(next);
            b.seal_block(next);
        }

        // A rank no arm answers to: unreachable, because this module built the closure, and a trap
        // rather than whatever the machine does next for `Trap::NoSuchLambda`'s reason. The span
        // index is past the end of the table on purpose — there is no source position for a wrong
        // rank, and the host reads one it cannot find as `Span::NONE`.
        let flags = MemFlagsData::trusted();
        let code = b
            .ins()
            .iconst(types::I32, i64::from(Trap::NoSuchLambda.code()));
        b.ins().store(flags, code, err, 0);
        let span = b.ins().iconst(types::I32, i64::from(u32::MAX));
        b.ins().store(flags, span, err, CELL_SPAN);
        b.ins().store(flags, rank, err, CELL_PAYLOAD);
        let z = zero_of(fam.ret, &mut b);
        b.ins().return_(&[z]);
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(id, ctx)
        .map_err(|e| format!("defining an application: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// A stable merge sort over two parallel runs of words: the keys, and the elements they decorate.
///
/// Generated per **key** repr rather than per family, because what it needs to know is how to
/// compare two key words and nothing else. Recursive rather than bottom-up: the depth is `log n` on
/// the host's stack and `n` is bounded by the arena, and one loop is easier to be right about than
/// three nested ones.
///
/// **Stability is the property that matters**, and it is one `<=`: on equal keys the element from the
/// left run goes first. `beck-eval` says why that is a promise rather than a nicety — the input order
/// is itself deterministic, so a stable sort is what makes the answer total without a second key.
fn merge_sort(
    at: u32,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    let ptr = m.target_config().pointer_type();
    let mut sig = cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
    for _ in 0..4 {
        sig.params.push(AbiParam::new(ptr));
    }
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    let id = m
        .declare_function(&format!("beck.list.msort.{at}"), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a sort: {e}"))?;
    let cmp = m
        .declare_function(
            &format!("beck.elem.cmp.{at}"),
            Linkage::Local,
            &compare_signature(m),
        )
        .map_err(|e| format!("declaring a comparison: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(17, at), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let ps: Vec<IrValue> = b.block_params(entry).to_vec();
        let (keys, vals, tk, tv, lo, hi) = (ps[0], ps[1], ps[2], ps[3], ps[4], ps[5]);
        let flags = MemFlagsData::trusted();
        let word = heap::WORD as i64;
        let me = m.declare_func_in_func(id, b.func);
        let three_way = m.declare_func_in_func(cmp, b.func);

        let done = b.create_block();
        let split = b.create_block();
        let span = b.ins().isub(hi, lo);
        let tiny = b.ins().icmp_imm_s(IntCC::UnsignedLessThanOrEqual, span, 1);
        b.ins().brif(tiny, done, &[], split, &[]);

        b.switch_to_block(split);
        b.seal_block(split);
        let half = b.ins().udiv_imm_u(span, 2);
        let mid = b.ins().iadd(lo, half);
        b.ins().call(me, &[keys, vals, tk, tv, lo, mid]);
        b.ins().call(me, &[keys, vals, tk, tv, mid, hi]);

        // Three indices: where each run is up to, and where the answer is going.
        let i = b.declare_var(types::I64);
        let j = b.declare_var(types::I64);
        let k = b.declare_var(types::I64);
        b.def_var(i, lo);
        b.def_var(j, mid);
        b.def_var(k, lo);
        let merge = b.create_block();
        let pick = b.create_block();
        let maybe = b.create_block();
        let compare = b.create_block();
        let left = b.create_block();
        let right = b.create_block();
        let took = b.create_block();
        b.append_block_param(took, types::I64);
        b.append_block_param(took, types::I64);
        let back = b.create_block();
        b.ins().jump(merge, &[]);

        b.switch_to_block(merge);
        let filling = b.use_var(k);
        let filled = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, filling, hi);
        b.ins().brif(filled, back, &[], pick, &[]);

        b.switch_to_block(pick);
        b.seal_block(pick);
        let li = b.use_var(i);
        let more_left = b.ins().icmp(IntCC::UnsignedLessThan, li, mid);
        b.ins().brif(more_left, maybe, &[], right, &[]);

        b.switch_to_block(maybe);
        b.seal_block(maybe);
        let rj = b.use_var(j);
        let right_gone = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, rj, hi);
        b.ins().brif(right_gone, left, &[], compare, &[]);

        b.switch_to_block(compare);
        b.seal_block(compare);
        let ia = b.ins().imul_imm_s(li, word);
        let ja = b.ins().imul_imm_s(rj, word);
        let ka_at = b.ins().iadd(keys, ia);
        let kb_at = b.ins().iadd(keys, ja);
        let ka = b.ins().load(types::I64, flags, ka_at, 0);
        let kb = b.ins().load(types::I64, flags, kb_at, 0);
        let call = b.ins().call(three_way, &[ka, kb]);
        let c = b.inst_results(call)[0];
        // `<=`, which is the whole of the stability: on equal keys the left run goes first.
        let take_left = b.ins().icmp_imm_s(IntCC::SignedLessThanOrEqual, c, 0);
        b.ins().brif(take_left, left, &[], right, &[]);

        for (which, from) in [(left, true), (right, false)] {
            b.switch_to_block(which);
            b.seal_block(which);
            let at_i = if from { b.use_var(i) } else { b.use_var(j) };
            let off = b.ins().imul_imm_s(at_i, word);
            let key_at = b.ins().iadd(keys, off);
            let val_at = b.ins().iadd(vals, off);
            let key = b.ins().load(types::I64, flags, key_at, 0);
            let val = b.ins().load(types::I64, flags, val_at, 0);
            let stepped = b.ins().iadd_imm_s(at_i, 1);
            if from {
                b.def_var(i, stepped);
            } else {
                b.def_var(j, stepped);
            }
            b.ins().jump(took, &[key.into(), val.into()]);
        }

        b.switch_to_block(took);
        b.seal_block(took);
        let key = b.block_params(took)[0];
        let val = b.block_params(took)[1];
        let at_k = b.use_var(k);
        let off = b.ins().imul_imm_s(at_k, word);
        let key_to = b.ins().iadd(tk, off);
        let val_to = b.ins().iadd(tv, off);
        b.ins().store(flags, key, key_to, 0);
        b.ins().store(flags, val, val_to, 0);
        let next = b.ins().iadd_imm_s(at_k, 1);
        b.def_var(k, next);
        b.ins().jump(merge, &[]);

        // The merged run, back where the caller's half of it was.
        b.switch_to_block(back);
        b.seal_block(back);
        let bytes = b.ins().imul_imm_s(span, word);
        let skip = b.ins().imul_imm_s(lo, word);
        let from_k = b.ins().iadd(tk, skip);
        let to_k = b.ins().iadd(keys, skip);
        let config = m.target_config();
        b.call_memcpy(config, to_k, from_k, bytes);
        let from_v = b.ins().iadd(tv, skip);
        let to_v = b.ins().iadd(vals, skip);
        b.call_memcpy(config, to_v, from_v, bytes);
        b.ins().jump(done, &[]);

        b.switch_to_block(done);
        b.ins().return_(&[]);
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(id, ctx)
        .map_err(|e| format!("defining a sort: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// `sort_by` for one family: decorate every element with its key, sort stably, answer the elements.
///
/// Four runs of `n` words — the keys, the elements, and a scratch pair the merge writes into. The
/// elements come from `beck.list.copy`, which is also the list this answers with, so the sort is in
/// place in something nobody else holds. The closure is applied exactly `n` times, which is what the
/// evaluator does: it decorates, sorts and undecorates too.
#[allow(clippy::too_many_arguments)]
fn sort_function(
    at: u32,
    heap: &Heap,
    arena: Arena,
    runtime: Runtime,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    let fam = heap.family(at).clone();
    let element = fam.params[0];
    let key = fam.ret;
    let key_at = heap
        .word_at(key)
        .ok_or("a sort whose key repr was never interned")?;
    let ptr = m.target_config().pointer_type();
    let lists = runtime
        .lists
        .ok_or("a sort in a module with no list runtime")?;
    let apply = m
        .declare_function(
            &apply_symbol(at),
            Linkage::Local,
            &family_signature(&fam, ptr),
        )
        .map_err(|e| format!("declaring an application: {e}"))?;

    let mut sig = cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
    sig.params.push(AbiParam::new(ptr));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I32));
    sig.returns.push(AbiParam::new(types::I64));
    let id = m
        .declare_function(&Loop::Sort.symbol(at), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a sort: {e}"))?;

    let mut msig =
        cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
    for _ in 0..4 {
        msig.params.push(AbiParam::new(ptr));
    }
    msig.params.push(AbiParam::new(types::I64));
    msig.params.push(AbiParam::new(types::I64));
    let msort = m
        .declare_function(&format!("beck.list.msort.{key_at}"), Linkage::Local, &msig)
        .map_err(|e| format!("declaring a sort: {e}"))?;

    ctx.func =
        Function::with_name_signature(UserFuncName::user(15, at * 8 + Loop::Sort.seq()), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let ps: Vec<IrValue> = b.block_params(entry).to_vec();
        let (err, xs, clo, span) = (ps[0], ps[1], ps[2], ps[3]);
        let flags = MemFlagsData::trusted();
        let word = heap::WORD as i64;
        let failed = b.create_block();
        let zero = b.ins().iconst(types::I64, 0);

        let n = lists.count(arena, xs, &mut b, m);
        let src = lists.data(arena, xs, &mut b, m);

        // Four allocations, each one able to exhaust the arena.
        let alloc = m.declare_func_in_func(lists.alloc, b.func);
        let copy = m.declare_func_in_func(lists.copy, b.func);
        let mut runs = Vec::new();
        for which in 0..4 {
            let call = if which == 1 {
                b.ins().call(copy, &[err, xs, zero, n, span])
            } else {
                b.ins().call(alloc, &[err, n, span])
            };
            let off = b.inst_results(call)[0];
            let ok = b.create_block();
            let bad = b.ins().icmp_imm_s(IntCC::Equal, off, 0);
            b.ins().brif(bad, failed, &[], ok, &[]);
            b.switch_to_block(ok);
            b.seal_block(ok);
            runs.push(off);
        }
        let data: Vec<IrValue> = runs
            .iter()
            .map(|off| lists.data(arena, *off, &mut b, m))
            .collect();
        let vals = runs[1];

        // Decorate: one key per element.
        let f = m.declare_func_in_func(apply, b.func);
        let i = b.declare_var(types::I64);
        b.def_var(i, zero);
        let head = b.create_block();
        let one = b.create_block();
        let alive = b.create_block();
        let sorted = b.create_block();
        b.ins().jump(head, &[]);
        b.switch_to_block(head);
        let at_i = b.use_var(i);
        let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, at_i, n);
        b.ins().brif(past, sorted, &[], one, &[]);
        b.switch_to_block(one);
        b.seal_block(one);
        let off = b.ins().imul_imm_s(at_i, word);
        let from = b.ins().iadd(src, off);
        let w = b.ins().load(types::I64, flags, from, 0);
        let arg = from_word(w, element, &mut b);
        let call = b.ins().call(f, &[err, clo, arg]);
        let answer = b.inst_results(call)[0];
        let code = b.ins().load(types::I32, flags, err, 0);
        let trapped = b.ins().icmp_imm_s(IntCC::NotEqual, code, 0);
        b.ins().brif(trapped, failed, &[], alive, &[]);
        b.switch_to_block(alive);
        b.seal_block(alive);
        let stored = to_word(answer, key, &mut b);
        let to = b.ins().iadd(data[0], off);
        b.ins().store(flags, stored, to, 0);
        let next = b.ins().iadd_imm_s(at_i, 1);
        b.def_var(i, next);
        b.ins().jump(head, &[]);

        b.switch_to_block(sorted);
        b.seal_block(sorted);
        let sort = m.declare_func_in_func(msort, b.func);
        b.ins()
            .call(sort, &[data[0], data[1], data[2], data[3], zero, n]);
        b.ins().return_(&[vals]);

        b.switch_to_block(failed);
        b.seal_block(failed);
        let none = b.ins().iconst(types::I64, 0);
        b.ins().return_(&[none]);
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(id, ctx)
        .map_err(|e| format!("defining a sort: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// One of [`Loop`]'s four, for one family.
///
/// A list's element is a **word** and a closure takes a value, so each iteration converts one way
/// and — for a map — back again; and every application is followed by a look at the error cell,
/// because a closure can trap and a loop that carried on would run the rest of a program that has
/// already failed.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn loop_function(
    which: Loop,
    at: u32,
    heap: &Heap,
    arena: Arena,
    runtime: Runtime,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    // The one that is not a pass over a list: four runs of words, a decorating loop and a sort.
    if which == Loop::Sort {
        return sort_function(at, heap, arena, runtime, m, ctx, fctx);
    }
    let fam = heap.family(at).clone();
    let ptr = m.target_config().pointer_type();
    let lists = runtime
        .lists
        .ok_or("a higher-order list primitive in a module with no list runtime")?;
    let apply = m
        .declare_function(
            &apply_symbol(at),
            Linkage::Local,
            &family_signature(&fam, ptr),
        )
        .map_err(|e| format!("declaring an application: {e}"))?;

    // The four signatures, which are the four the emitter writes calls to.
    let mut sig = cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
    sig.params.push(AbiParam::new(ptr));
    sig.params.push(AbiParam::new(types::I64));
    if which == Loop::Fold {
        sig.params.push(AbiParam::new(machine(fam.ret)));
    }
    sig.params.push(AbiParam::new(types::I64));
    if which == Loop::Every {
        sig.params.push(AbiParam::new(types::I8));
    }
    sig.params.push(AbiParam::new(types::I32));
    sig.returns.push(AbiParam::new(match which {
        Loop::Map | Loop::Filter => types::I64,
        Loop::Fold => machine(fam.ret),
        // `Sort` answered above: it is not one of these loops, and the `unreachable` says so where a
        // `_` arm would have swallowed the next one added.
        Loop::Every | Loop::Sort => types::I8,
    }));
    let id = m
        .declare_function(&which.symbol(at), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a list loop: {e}"))?;

    ctx.func =
        Function::with_name_signature(UserFuncName::user(15, at * 4 + which.seq()), sig.clone());
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let ps: Vec<IrValue> = b.block_params(entry).to_vec();
        let err = ps[0];
        let xs = ps[1];
        let (init, clo, want, span) = match which {
            Loop::Fold => (Some(ps[2]), ps[3], None, ps[4]),
            Loop::Every => (None, ps[2], Some(ps[3]), ps[4]),
            _ => (None, ps[2], None, ps[3]),
        };
        let flags = MemFlagsData::trusted();
        let f = m.declare_func_in_func(apply, b.func);

        let count = lists.count(arena, xs, &mut b, m);
        let src = lists.data(arena, xs, &mut b, m);

        // The result list, for the two that build one. `filter` allocates room for every element and
        // writes the header at the end: one pass, and the words after what was kept are arena
        // nobody reads — bounded by the input and given back when it is reset. A count-then-fill
        // would call the predicate twice per element to save that.
        let out = match which {
            Loop::Map | Loop::Filter => {
                let alloc = m.declare_func_in_func(lists.alloc, b.func);
                let call = b.ins().call(alloc, &[err, count, span]);
                let off = b.inst_results(call)[0];
                let failed = b.create_block();
                let ready = b.create_block();
                let bad = b.ins().icmp_imm_s(IntCC::Equal, off, 0);
                b.ins().brif(bad, failed, &[], ready, &[]);
                b.switch_to_block(failed);
                b.seal_block(failed);
                let z = b.ins().iconst(types::I64, 0);
                b.ins().return_(&[z]);
                b.switch_to_block(ready);
                b.seal_block(ready);
                Some(off)
            }
            _ => None,
        };
        let dst = out.map(|off| lists.data(arena, off, &mut b, m));

        // The loop carries the index, and — for a fold — the accumulator, and — for a filter — how
        // many have been kept. A block parameter is the `phi` a textual emitter writes.
        let head = b.create_block();
        b.append_block_param(head, types::I64);
        match which {
            Loop::Fold => {
                b.append_block_param(head, machine(fam.ret));
            }
            Loop::Filter => {
                b.append_block_param(head, types::I64);
            }
            _ => {}
        }
        let zero = b.ins().iconst(types::I64, 0);
        let mut start = vec![zero.into()];
        match which {
            Loop::Fold => start.push(init.expect("a fold is given one").into()),
            Loop::Filter => start.push(zero.into()),
            _ => {}
        }
        b.ins().jump(head, &start);
        b.switch_to_block(head);
        let i = b.block_params(head)[0];
        let carried = match which {
            Loop::Fold | Loop::Filter => Some(b.block_params(head)[1]),
            _ => None,
        };

        let done = b.create_block();
        match which {
            Loop::Fold => {
                b.append_block_param(done, machine(fam.ret));
            }
            Loop::Filter => {
                b.append_block_param(done, types::I64);
            }
            _ => {}
        }
        let one = b.create_block();
        let past = b.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, i, count);
        let leaving: Vec<cranelift_codegen::ir::BlockArg> = match which {
            Loop::Fold | Loop::Filter => vec![carried.expect("carried").into()],
            _ => vec![],
        };
        b.ins().brif(past, done, &leaving, one, &[]);

        // One element: the word, the value it stands for, the application, and the trap check.
        b.switch_to_block(one);
        b.seal_block(one);
        let addr = b.ins().imul_imm_s(i, heap::WORD as i64);
        let at_i = b.ins().iadd(src, addr);
        let word = b.ins().load(types::I64, flags, at_i, 0);
        let arg = from_word(
            word,
            if which == Loop::Fold {
                fam.params[1]
            } else {
                fam.params[0]
            },
            &mut b,
        );
        let mut operands = vec![err, clo];
        if which == Loop::Fold {
            operands.push(carried.expect("a fold carries its accumulator"));
        }
        operands.push(arg);
        let call = b.ins().call(f, &operands);
        let answer = b.inst_results(call)[0];
        let failed = b.create_block();
        let alive = b.create_block();
        let code = b.ins().load(types::I32, flags, err, 0);
        let trapped = b.ins().icmp_imm_s(IntCC::NotEqual, code, 0);
        b.ins().brif(trapped, failed, &[], alive, &[]);
        b.switch_to_block(failed);
        b.seal_block(failed);
        let z = match which {
            Loop::Map | Loop::Filter => b.ins().iconst(types::I64, 0),
            Loop::Fold => zero_of(fam.ret, &mut b),
            Loop::Every | Loop::Sort => b.ins().iconst(types::I8, 0),
        };
        b.ins().return_(&[z]);
        b.switch_to_block(alive);
        b.seal_block(alive);

        let next = b.create_block();
        let step = b.ins().iadd_imm_s(i, 1);
        match which {
            Loop::Map => {
                let stored = to_word(answer, fam.ret, &mut b);
                let to = b.ins().iadd(dst.expect("a map builds a list"), addr);
                b.ins().store(flags, stored, to, 0);
                b.ins().jump(next, &[]);
                b.switch_to_block(next);
                b.seal_block(next);
                b.ins().jump(head, &[step.into()]);
            }
            Loop::Filter => {
                let kept = carried.expect("a filter carries how many it has kept");
                let take = b.create_block();
                let skip = b.create_block();
                b.append_block_param(next, types::I64);
                b.ins().brif(answer, take, &[], skip, &[]);
                b.switch_to_block(take);
                b.seal_block(take);
                let at_k = b.ins().imul_imm_s(kept, heap::WORD as i64);
                let to = b.ins().iadd(dst.expect("a filter builds a list"), at_k);
                b.ins().store(flags, word, to, 0);
                let more = b.ins().iadd_imm_s(kept, 1);
                b.ins().jump(next, &[more.into()]);
                b.switch_to_block(skip);
                b.seal_block(skip);
                b.ins().jump(next, &[kept.into()]);
                b.switch_to_block(next);
                b.seal_block(next);
                let now = b.block_params(next)[0];
                b.ins().jump(head, &[step.into(), now.into()]);
            }
            Loop::Fold => {
                b.ins().jump(next, &[]);
                b.switch_to_block(next);
                b.seal_block(next);
                b.ins().jump(head, &[step.into(), answer.into()]);
            }
            Loop::Every | Loop::Sort => {
                // Short-circuiting, which `beck-eval` documents as a promise rather than an
                // optimisation: `list_any` stops at the first `true` and `list_all` at the first
                // `false`, and the flag is which of the two this call is.
                let stop = b.create_block();
                let want = want.expect("every is given the answer it stops on");
                let hit = b.ins().icmp(IntCC::Equal, answer, want);
                b.ins().brif(hit, stop, &[], next, &[]);
                b.switch_to_block(stop);
                b.seal_block(stop);
                b.ins().return_(&[want]);
                b.switch_to_block(next);
                b.seal_block(next);
                b.ins().jump(head, &[step.into()]);
            }
        }

        b.switch_to_block(done);
        match which {
            Loop::Map => {
                b.ins().return_(&[out.expect("a map builds a list")]);
            }
            Loop::Filter => {
                // The count, at last: the list is as long as what was kept.
                let kept = b.block_params(done)[0];
                let off = out.expect("a filter builds a list");
                let base = arena.base(&mut b, m);
                let hdr = b.ins().iadd(base, off);
                b.ins().store(flags, kept, hdr, 0);
                b.ins().return_(&[off]);
            }
            Loop::Fold => {
                let acc = b.block_params(done)[0];
                b.ins().return_(&[acc]);
            }
            Loop::Every | Loop::Sort => {
                let want = want.expect("every is given the answer it stops on");
                let rest = b.ins().bxor_imm_u(want, 1);
                b.ins().return_(&[rest]);
            }
        }
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(id, ctx)
        .map_err(|e| format!("defining a list loop: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// One word of a list as the value a closure of this repr takes.
fn from_word(w: IrValue, repr: Repr, b: &mut FunctionBuilder<'_>) -> IrValue {
    match repr {
        Repr::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), w),
        Repr::Bool => b.ins().icmp_imm_s(IntCC::NotEqual, w, 0),
        _ => w,
    }
}

/// A closure's answer as the word a list holds.
///
/// A real is normalised on the way in, for the reason [`Body::store_field`] normalises one: every
/// real on this heap is the one the evaluator would have built, so nothing downstream has to
/// remember which ones are not.
fn to_word(v: IrValue, repr: Repr, b: &mut FunctionBuilder<'_>) -> IrValue {
    match repr {
        Repr::Float => {
            let zero = b.ins().f64const(0.0);
            let is_zero = b.ins().fcmp(FloatCC::Equal, v, zero);
            let zeroed = b.ins().select(is_zero, zero, v);
            let nan = b.ins().f64const(f64::NAN);
            let is_nan = b.ins().fcmp(FloatCC::NotEqual, v, v);
            let normal = b.ins().select(is_nan, nan, zeroed);
            b.ins().bitcast(types::I64, MemFlagsData::new(), normal)
        }
        Repr::Bool => b.ins().uextend(types::I64, v),
        _ => v,
    }
}

/// The one function that compares two closures, whatever their family.
///
/// One rather than one per family: a rank is unique across the module, and
/// [`heap::Repr::order`] says why comparing two ranks is comparing what the evaluator compares.
fn fn_compare(
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
    arena: Arena,
) -> Result<(), String> {
    let sig = compare_signature(m);
    let id = m
        .declare_function("beck.fn.cmp", Linkage::Local, &sig)
        .map_err(|e| format!("declaring a comparison: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(14, 0), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let (a, c) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let base = arena.base(&mut b, m);
        let flags = MemFlagsData::trusted();
        let pa = b.ins().iadd(base, a);
        let pc = b.ins().iadd(base, c);
        let ra = b.ins().load(types::I64, flags, pa, 0);
        let rc = b.ins().load(types::I64, flags, pc, 0);
        let down = b.ins().iconst(types::I64, -1);
        let up = b.ins().iconst(types::I64, 1);
        let same = b.ins().iconst(types::I64, 0);
        let lt = b.ins().icmp(IntCC::UnsignedLessThan, ra, rc);
        let gt = b.ins().icmp(IntCC::UnsignedGreaterThan, ra, rc);
        let high = b.ins().select(gt, up, same);
        let r = b.ins().select(lt, down, high);
        b.ins().return_(&[r]);
        b.seal_all_blocks();
        b.finalize(m.target_config());
    }
    m.define_function(id, ctx)
        .map_err(|e| format!("defining a comparison: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// The signature every comparison has: two offsets in, `-1`, `0` or `1` out.
fn compare_signature(m: &ObjectModule) -> cranelift_codegen::ir::Signature {
    let mut sig = cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

/// A three-way comparison over one layout, and the same answer `Value`'s derived `Ord` gives.
///
/// Tag first, then fields in the order they are laid out — which is name order, which is the order
/// `Fields` iterates and therefore the order `Ord` reads a record in. A field that is itself an
/// object is a call to *its* layout's comparison, so the recursion in the type is the recursion in
/// the code.
fn compare_function(
    at: u32,
    heap: &Heap,
    arena: Arena,
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
) -> Result<(), String> {
    let layout = heap.layout(at);
    let sig = compare_signature(m);
    let id = m
        .declare_function(&format!("beck.cmp.{at}"), Linkage::Local, &sig)
        .map_err(|e| format!("declaring a comparison: {e}"))?;
    ctx.func = Function::with_name_signature(UserFuncName::user(5, at), sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        let (oa, ob) = (b.block_params(entry)[0], b.block_params(entry)[1]);
        let base = arena.base(&mut b, m);
        let pa = b.ins().iadd(base, oa);
        let pb = b.ins().iadd(base, ob);
        let flags = MemFlagsData::trusted();

        // -1 and 1 have a block each, jumped to from wherever a comparison decides. One pair rather
        // than a pair per field, because every field decides the same two ways.
        let below = b.create_block();
        let above = b.create_block();

        let tail = if layout.tagged {
            let ta = b.ins().load(types::I64, flags, pa, 0);
            let tb = b.ins().load(types::I64, flags, pb, 0);
            let lt = b.ins().icmp(IntCC::UnsignedLessThan, ta, tb);
            let ordered = b.create_block();
            b.ins().brif(lt, below, &[], ordered, &[]);
            b.switch_to_block(ordered);
            b.seal_block(ordered);
            let gt = b.ins().icmp(IntCC::UnsignedGreaterThan, ta, tb);
            let same = b.create_block();
            b.ins().brif(gt, above, &[], same, &[]);
            b.switch_to_block(same);
            b.seal_block(same);
            Some(ta)
        } else {
            None
        };

        // One block per variant, chosen by the tag. A chain of tests rather than a jump table, for
        // the reason the dispatch is one: a union has a handful of variants.
        let mut arms = Vec::new();
        for _ in &layout.variants {
            arms.push(b.create_block());
        }
        if let Some(ta) = tail {
            for (i, arm) in arms.iter().enumerate() {
                let miss = b.create_block();
                let is = b.ins().icmp_imm_s(IntCC::Equal, ta, i as i64);
                b.ins().brif(is, *arm, &[], miss, &[]);
                b.switch_to_block(miss);
                b.seal_block(miss);
            }
            // A tag the table does not name cannot happen — the host writes one this table
            // produced — and "equal" is the one answer that cannot make a comparison asymmetric.
            let z = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[z]);
        } else {
            b.ins().jump(arms[0], &[]);
        }

        for (i, variant) in layout.variants.iter().enumerate() {
            b.switch_to_block(arms[i]);
            b.seal_block(arms[i]);
            for (slot, (_, repr)) in variant.fields.iter().enumerate() {
                let off = ((slot as u64 + 1) * heap::WORD) as i32;
                let xa = b.ins().load(types::I64, flags, pa, off);
                let xb = b.ins().load(types::I64, flags, pb, off);
                match repr.order() {
                    // A field that is itself a reference decides through the three-way comparison
                    // for whatever it refers to, and `Repr::order` is the only place that names
                    // one. Comparing the *offsets* would answer that two equal values differ
                    // whenever they were allocated at different places, which is almost always —
                    // and `docs/93` §93.8 is the **fourth** time a case analysis here let a
                    // reference kind fall through to exactly that.
                    heap::Order::Call(symbol) => {
                        let inner_sig = compare_signature(m);
                        let inner_id = m
                            .declare_function(&symbol, Linkage::Local, &inner_sig)
                            .map_err(|e| format!("declaring a comparison: {e}"))?;
                        let f = m.declare_func_in_func(inner_id, b.func);
                        let call = b.ins().call(f, &[xa, xb]);
                        let r = b.inst_results(call)[0];
                        let decided = b.create_block();
                        let next = b.create_block();
                        let any = b.ins().icmp_imm_s(IntCC::NotEqual, r, 0);
                        b.ins().brif(any, decided, &[], next, &[]);
                        b.switch_to_block(decided);
                        b.seal_block(decided);
                        b.ins().return_(&[r]);
                        b.switch_to_block(next);
                        b.seal_block(next);
                    }
                    // A field with no order at all. Unreachable, and by a rule rather than an
                    // argument: `Body::wants` asks `Heap::ordered` before it records a demand, and
                    // that walks a record's fields — so a layout holding one is never in the set
                    // this is generated for. "Equal" for the same reason the unnamed tag above
                    // answers it: it is the one answer that cannot make a comparison asymmetric.
                    heap::Order::Absent(_) => {}
                    order => {
                        // A real compares through its order key, and both are already normalised —
                        // `Body::store_field` is where that is paid for.
                        let (ka, kb) = match order {
                            heap::Order::Key => {
                                (order_key_bits(xa, &mut b), order_key_bits(xb, &mut b))
                            }
                            _ => (xa, xb),
                        };
                        let (lt, gt) = if order == (heap::Order::Words { signed: true }) {
                            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
                        } else {
                            (IntCC::UnsignedLessThan, IntCC::UnsignedGreaterThan)
                        };
                        let is_lt = b.ins().icmp(lt, ka, kb);
                        let test = b.create_block();
                        b.ins().brif(is_lt, below, &[], test, &[]);
                        b.switch_to_block(test);
                        b.seal_block(test);
                        let is_gt = b.ins().icmp(gt, ka, kb);
                        let next = b.create_block();
                        b.ins().brif(is_gt, above, &[], next, &[]);
                        b.switch_to_block(next);
                        b.seal_block(next);
                    }
                }
            }
            let z = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[z]);
        }

        b.switch_to_block(below);
        b.seal_block(below);
        let minus = b.ins().iconst(types::I64, -1);
        b.ins().return_(&[minus]);
        b.switch_to_block(above);
        b.seal_block(above);
        let plus = b.ins().iconst(types::I64, 1);
        b.ins().return_(&[plus]);
        b.finalize(m.target_config());
    }
    m.define_function(id, ctx)
        .map_err(|e| format!("defining a comparison: {e}"))?;
    m.clear_context(ctx);
    Ok(())
}

/// A raw word read back as the value its [`Repr`] says it is.
fn word_as(w: IrValue, repr: Repr, b: &mut FunctionBuilder<'_>) -> IrValue {
    match repr {
        Repr::Bool => b.ins().icmp_imm_s(IntCC::NotEqual, w, 0),
        Repr::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), w),
        _ => w,
    }
}

/// `beck_core`'s order key over raw bits already in an `I64`.
fn order_key_bits(bits: IrValue, b: &mut FunctionBuilder<'_>) -> IrValue {
    let sign = b.ins().sshr_imm_s(bits, 63);
    let mask = b.ins().bor_imm_s(sign, i64::MIN);
    b.ins().bxor(bits, mask)
}

/// An SSA value and what it is.
#[derive(Clone, Copy, Debug)]
struct Val {
    v: IrValue,
    ty: Repr,
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
    program: &'a Program,
    heap: &'a mut Heap,
    /// The arena and text's runtime, whichever of them this module has.
    runtime: Runtime,
    env: BTreeMap<VarId, Val>,
    spans: Vec<Span>,
    /// What this function returns, and therefore what a trapping exit has to return too.
    ret: Repr,
    /// The error cell, this function's first parameter.
    err: Option<IrValue>,
    /// The layouts this body compares two of, so the module defines a comparison for them.
    compared: BTreeSet<u32>,
    /// The list reprs it compares two of, likewise.
    list_compared: BTreeSet<u32>,
    /// And the map reprs.
    map_compared: BTreeSet<u32>,
    /// Whether this body compares two closures, which needs one function for the whole module.
    compared_fns: bool,
    /// The lambdas this body built, each still to be written as a function of its own.
    ///
    /// A queue rather than a nested emission: one [`FunctionBuilder`] owns the context while it is
    /// building, so the body of a `lam` met inside one is written after that one is defined — which
    /// is [`build`]'s drain, and which is also why a `lam` that will not compile refuses the
    /// definition it was written in rather than itself.
    pending: Vec<Pending>,
    /// Where a failure inside the block being emitted goes, innermost last.
    ///
    /// Empty means the function's own exit. A `try:` pushes a block while its own is emitted, so a
    /// call's check and a primitive's trap land in the handler rather than returning — the LLVM
    /// emitter's `handlers`, and the same reason: a handler is lexical because the destination is
    /// decided where the block is written.
    handlers: Vec<Block>,
    /// The closure families this body applies.
    applied: BTreeSet<u32>,
    /// The higher-order list primitives this body reaches, by shape.
    loops: BTreeSet<(Loop, u32)>,
}

impl<'a> Body<'a> {
    fn new(
        sigs: &'a BTreeMap<Arc<str>, Signature>,
        eligible: &'a BTreeSet<Arc<str>>,
        ids: &'a BTreeMap<Arc<str>, FuncId>,
        spans: Vec<Span>,
        program: &'a Program,
        heap: &'a mut Heap,
        runtime: Runtime,
    ) -> Body<'a> {
        Body {
            handlers: Vec::new(),
            sigs,
            eligible,
            ids,
            program,
            heap,
            runtime,
            env: BTreeMap::new(),
            spans,
            ret: Repr::Int,
            err: None,
            compared: BTreeSet::new(),
            list_compared: BTreeSet::new(),
            map_compared: BTreeSet::new(),
            compared_fns: false,
            pending: Vec::new(),
            applied: BTreeSet::new(),
            loops: BTreeSet::new(),
        }
    }

    /// What `ty` looks like at the machine, or the reason this body cannot be compiled.
    fn repr(&mut self, ty: &beck_core::ty::Ty) -> Result<Repr, String> {
        self.heap.repr(ty, self.program)
    }

    /// The arena, which a body only asks for once it has an object in its hands.
    fn arena(&self) -> Arena {
        self.runtime
            .arena
            .expect("a body with an object in it is a module with an arena")
    }

    fn text(&self) -> Text {
        self.runtime
            .text
            .expect("a body with text in it is a module with text's runtime")
    }

    /// Record that this repr's comparison has to exist, or refuse because it cannot.
    ///
    /// One method rather than three call sites, so adding a reference kind means teaching
    /// [`beck_llvm::heap::Repr::order`] and this, and `reachable` closes over whatever they name.
    ///
    /// It asks [`beck_llvm::heap::Heap::ordered`] first — see that method and the LLVM emitter's
    /// `wants`, which is the same rule written for the same reason.
    fn wants(&mut self, r: Repr) -> Result<(), String> {
        self.heap.ordered(r)?;
        match r {
            Repr::Obj(at) => {
                self.compared.insert(at);
            }
            Repr::List(at) => {
                self.list_compared.insert(at);
            }
            Repr::Map(at) => {
                self.map_compared.insert(at);
            }
            Repr::Int | Repr::Float | Repr::Bool | Repr::Str | Repr::Html | Repr::Attr => {}
            // One function for the whole module rather than one per family, because every closure's
            // rank is in the same table. See `beck_llvm::heap::Repr::order`.
            Repr::Fn(_) => self.compared_fns = true,
        }
        Ok(())
    }

    fn lists(&self) -> Lists {
        self.runtime
            .lists
            .expect("a body with a list in it is a module with the list runtime")
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
    fn zero(&self, ty: Repr, b: &mut FunctionBuilder<'_>) -> IrValue {
        match ty.machine() {
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
        self.escape(b);

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
        self.escape(b);

        b.switch_to_block(cont);
        b.seal_block(cont);
    }

    /// Leave for the innermost `try:`'s handler, or out of the function.
    ///
    /// One method rather than the `return_` at three sites, for the reason the LLVM emitter's
    /// `escape` is one: a handler only some of them honoured would catch a raise and miss an
    /// overflow.
    fn escape(&mut self, b: &mut FunctionBuilder<'_>) {
        match self.handlers.last() {
            Some(handler) => {
                b.ins().jump(*handler, &[]);
            }
            None => {
                let z = self.zero(self.ret, b);
                b.ins().return_(&[z]);
            }
        }
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

    #[allow(clippy::too_many_lines)]
    fn expr(
        &mut self,
        c: &Core,
        dest: Dest,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
        let value = match &c.kind {
            CoreKind::Const(k) => self.constant(k, b)?,
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
            CoreKind::Prim { op, args } => self.prim(*op, args, &c.ty, c.span, b, m)?,
            CoreKind::Lam { params, body } => self.closure(params, body, &c.ty, c.span, b, m)?,
            CoreKind::Global(name) => self.named(name, &c.ty, c.span, b, m)?,
            CoreKind::Make {
                variant, fields, ..
            } => self.make(&c.ty, variant.as_deref(), fields, c.span, b, m)?,
            CoreKind::Field { base, name } => self.field(base, name, b, m)?,
            CoreKind::With { base, fields } => self.with(base, fields, c.span, b, m)?,
            CoreKind::ListLit(xs) => self.list_lit(&c.ty, xs, c.span, b, m)?,
            CoreKind::MapLit(kvs) => self.map_lit(&c.ty, kvs, c.span, b, m)?,
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
        if c.ty != Repr::Bool {
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
        let mut ty: Option<Repr> = None;
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

    /// A `match`: a chain of tests, each falling through to the next.
    ///
    /// Falling through is what makes a guard a guard — an arm whose pattern matched but whose
    /// guard was false has to reach the arm after it, which is the evaluator's `continue`.
    ///
    /// An arm whose pattern takes a value apart is emitted once per alternative
    /// ([`alternatives`]), for the reason written there.
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
        let mut ty: Option<Repr> = None;
        let mut reached = 0;

        for arm in arms {
            for pattern in alternatives(&arm.pattern)? {
                let next = b.create_block();
                let mut undo: Vec<(VarId, Option<Val>)> = Vec::new();
                let probed = self.probe(&pattern, &v, next, &mut undo, b, m);
                if let Err(e) = probed {
                    self.unbind(undo);
                    return Err(e);
                }
                if let Some(guard) = &arm.guard {
                    let g = self.value(guard, b, m)?;
                    if g.ty != Repr::Bool {
                        self.unbind(undo);
                        return Err("a match guard is not a Bool".into());
                    }
                    let run = b.create_block();
                    b.ins().brif(g.v, run, &[], next, &[]);
                    b.switch_to_block(run);
                    b.seal_block(run);
                }
                let av = self.expr(&arm.body, dest, b, m)?;
                self.unbind(undo);
                if let Some(av) = av {
                    match ty {
                        Some(t) if t != av.ty => {
                            return Err("match arms have different types".into())
                        }
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
        }

        // Nothing matched. The checker proves a `match` exhaustive, so this is unreachable for a
        // program that compiled — and a wrong exhaustiveness check has to be a *message* rather
        // than whatever the machine does next, so it traps.
        let trap = match v.ty {
            Repr::Int => Trap::NoMatchInt,
            Repr::Float => Trap::NoMatchFloat,
            Repr::Bool => Trap::NoMatchBool,
            Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => Trap::NoMatchData,
        };
        let payload = self.widen(&v, b);
        let always = b.ins().iconst(types::I8, 1);
        self.trap(trap, span, payload, always, b);
        // `trap` continues in a fresh block on the "did not trap" edge, which cannot be reached:
        // the condition was a constant. It still needs a terminator, and it leaves the way every
        // other failure in this block does rather than out of the function.
        self.escape(b);

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

    /// Test `pat` against `v`, binding what it names: fall through on a match, branch to `fail`
    /// otherwise.
    ///
    /// Control flow rather than one boolean, and that is a memory-safety requirement rather than a
    /// tidiness one: `Some(Circle(r))` cannot read the field it matches on until the tag says
    /// there is one there, and a conjunction that evaluated both sides would read a word of a
    /// variant that is not present and follow it as an offset.
    fn probe(
        &mut self,
        pat: &Pattern,
        v: &Val,
        fail: cranelift_codegen::ir::Block,
        undo: &mut Vec<(VarId, Option<Val>)>,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<(), String> {
        match pat {
            Pattern::Wildcard => Ok(()),
            Pattern::Bind(var) => {
                undo.push((*var, self.env.insert(*var, *v)));
                Ok(())
            }
            Pattern::At { var, inner } => {
                undo.push((*var, self.env.insert(*var, *v)));
                self.probe(inner, v, fail, undo, b, m)
            }
            Pattern::Const(k) => {
                let want = self.constant(k, b)?;
                if want.ty != v.ty {
                    return Err("a match arm compares against a constant of another type".into());
                }
                let cond = self.compare(Prim::Eq, v, &want, b, m)?.v;
                self.branch(cond, fail, b);
                Ok(())
            }
            // Only the alternatives `alternatives` leaves whole reach here: every one is a test
            // and none of them binds, so the disjunction is one value and one branch.
            Pattern::Or(alts) => {
                let mut acc: Option<IrValue> = None;
                for alt in alts {
                    let Pattern::Const(k) = alt else {
                        return Err("an or-pattern that was not split".into());
                    };
                    let want = self.constant(k, b)?;
                    if want.ty != v.ty {
                        return Err(
                            "a match arm compares against a constant of another type".into()
                        );
                    }
                    let t = self.compare(Prim::Eq, v, &want, b, m)?.v;
                    acc = Some(match acc {
                        None => t,
                        Some(prev) => b.ins().bor(prev, t),
                    });
                }
                let cond = acc.ok_or_else(|| "an or-pattern with no alternatives".to_string())?;
                self.branch(cond, fail, b);
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
                // A record has one variant, so its tag is known and there is nothing to test.
                if tagged {
                    let got = self.load_word(v.v, 0, b, m);
                    let ok = b.ins().icmp_imm_s(IntCC::Equal, got, i64::from(tag));
                    self.branch(ok, fail, b);
                }
                for (name, sub) in binds {
                    let (slot, repr) = fields.slot(name).ok_or_else(|| {
                        format!("`{variant}` has no field `{name}` in this layout")
                    })?;
                    let field = self.load_field(v.v, slot, repr, b, m);
                    self.probe(sub, &field, fail, undo, b, m)?;
                }
                Ok(())
            }
            // `[]`, `[a, b]`, `[first, *rest]`. The length is tested before any element is read,
            // so nothing here can load past the end of the block — the other emitter's arm has the
            // same order for the same reason, and this is written again rather than shared because
            // the two are held to *agreeing* (`docs/93` §93.8).
            Pattern::List { items, rest } => {
                let Repr::List(at) = v.ty else {
                    return Err(format!(
                        "matches a list pattern against {}",
                        self.heap.show(v.ty)
                    ));
                };
                let element = self.heap.element(at);
                let lists = self.lists();
                let arena = self.arena();
                let n = lists.count(arena, v.v, b, m);
                // No tail binder means an exact length; a tail binder means "at least this many".
                let want = items.len() as i64;
                let long = if rest.is_some() {
                    b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, n, want)
                } else {
                    b.ins().icmp_imm_s(IntCC::Equal, n, want)
                };
                self.branch(long, fail, b);

                if !items.is_empty() {
                    let data = lists.data(arena, v.v, b, m);
                    for (i, sub) in items.iter().enumerate() {
                        let p = b.ins().iadd_imm_s(data, (i as u64 * heap::WORD) as i64);
                        let x = self.load_at(p, element, b);
                        self.probe(sub, &x, fail, undo, b, m)?;
                    }
                }

                // A fresh list, copied — which is what the evaluator does with the same `O(n)`
                // (`docs/27` §27.3), so neither backend is quietly quadratic against the other.
                if let Some(Some(var)) = rest {
                    let start = b.ins().iconst(types::I64, want);
                    let left = b.ins().iadd_imm_s(n, -want);
                    let idx = self.span(Span::NONE);
                    let at_span = b.ins().iconst(types::I32, i64::from(idx));
                    let err = self.err();
                    let f = m.declare_func_in_func(lists.copy, b.func);
                    let call = b.ins().call(f, &[err, v.v, start, left, at_span]);
                    let tail = b.inst_results(call)[0];
                    self.check_call(b);
                    let tail = Val { v: tail, ty: v.ty };
                    undo.push((*var, self.env.insert(*var, tail)));
                }
                Ok(())
            }
        }
    }

    /// A word at an address, as the value its [`Repr`] says it is.
    ///
    /// [`Body::load_field`]'s twin for something that is already a pointer — a list's element,
    /// where that one starts from an object's offset and a slot.
    fn load_at(&mut self, p: IrValue, repr: Repr, b: &mut FunctionBuilder<'_>) -> Val {
        let flags = MemFlagsData::trusted();
        let v = match repr {
            Repr::Float => b.ins().load(types::F64, flags, p, 0),
            // An `I8` holding 0 or 1, which is the invariant every comparison here keeps.
            Repr::Bool => {
                let w = b.ins().load(types::I64, flags, p, 0);
                let set = b.ins().icmp_imm_s(IntCC::NotEqual, w, 0);
                b.ins().uextend(types::I8, set)
            }
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => b.ins().load(types::I64, flags, p, 0),
        };
        Val { v, ty: repr }
    }

    /// Carry on if `cond`, and go to `fail` if not.
    fn branch(
        &mut self,
        cond: IrValue,
        fail: cranelift_codegen::ir::Block,
        b: &mut FunctionBuilder<'_>,
    ) {
        let cont = b.create_block();
        b.ins().brif(cond, cont, &[], fail, &[]);
        b.switch_to_block(cont);
        b.seal_block(cont);
    }

    fn unbind(&mut self, undo: Vec<(VarId, Option<Val>)>) {
        for (var, old) in undo.into_iter().rev() {
            match old {
                Some(v) => self.env.insert(var, v),
                None => self.env.remove(&var),
            };
        }
    }

    // -- the heap ---------------------------------------------------------------------------

    /// The address of word `slot` of the object at offset `off`.
    fn word_addr(
        &mut self,
        off: IrValue,
        slot: usize,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let base = self.arena().base(b, m);
        let at = b.ins().iadd(base, off);
        if slot == 0 {
            return at;
        }
        b.ins().iadd_imm_s(at, (slot as u64 * heap::WORD) as i64)
    }

    /// One raw word of an object — the tag, or a field read for copying rather than for using.
    fn load_word(
        &mut self,
        off: IrValue,
        slot: usize,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let at = self.word_addr(off, slot, b, m);
        b.ins().load(types::I64, MemFlagsData::trusted(), at, 0)
    }

    /// Ask the host one of the four questions compiled code cannot answer.
    ///
    /// The LLVM emitter's `upcall`, written again — and what is *not* written again is what goes
    /// on the wire: a shape and a word per argument, so the host decodes and encodes through
    /// [`beck_llvm::heap`] rather than through a second table of what each primitive's types are.
    ///
    /// The buffer is a **stack slot**, which is a function-level entity in Cranelift and therefore
    /// hoisted by construction: the other emitter writes text and has to insert its `alloca` at
    /// the top of the entry block by hand, or a `now()` inside a loop would grow the stack once an
    /// iteration.
    fn upcall(
        &mut self,
        op: Upcall,
        vals: &[Val],
        ty: &beck_core::ty::Ty,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let Some(host) = self.runtime.host else {
            return Err(format!(
                "`{}` reaches the host, and this module has no way to ask it",
                op.name()
            ));
        };
        if vals.len() != op.arity() {
            return Err(format!(
                "`{}` is applied to {} arguments here",
                op.name(),
                vals.len()
            ));
        }
        let ret = self
            .repr(ty)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        let ret_shape = self.heap.word_of(ret);
        let (raises, named) = match op.raises() {
            Some(name) => {
                let repr = self
                    .repr(&beck_core::ty::Ty::con(name))
                    .map_err(|why| format!("`{}` raises {why}", op.name()))?;
                (self.heap.word_of(repr), self.literal(name))
            }
            None => (0, 0),
        };
        let idx = self.span(span);
        let ptr = m.target_config().pointer_type();
        let slot = b.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            QUESTION_WORDS * heap::WORD as u32,
            3,
        ));
        let buf = b.ins().stack_addr(ptr, slot, 0);
        let flags = MemFlagsData::trusted();
        let mut words = vec![
            b.ins().iconst(types::I64, i64::from(ret_shape)),
            b.ins().iconst(types::I64, i64::from(raises)),
        ];
        for v in vals {
            let shape = self.heap.word_of(v.ty);
            words.push(b.ins().iconst(types::I64, i64::from(shape)));
            words.push(self.widen(v, b));
        }
        for (i, word) in words.iter().enumerate() {
            b.ins()
                .store(flags, *word, buf, (i as u64 * heap::WORD) as i32);
        }

        let f = m.declare_func_in_func(host.call, b.func);
        let code = b.ins().iconst(types::I32, i64::from(op.code()));
        let at = b.ins().iconst(types::I32, i64::from(idx));
        let name = b.ins().iconst(types::I64, named as i64);
        let count = b.ins().iconst(types::I64, words.len() as i64);
        let copy = b
            .ins()
            .iconst(types::I64, i64::from(u32::from(op.carries_arena())));
        let err = self.err();
        let call = b.ins().call(f, &[code, at, name, count, buf, copy, err]);
        let got = b.inst_results(call)[0];
        self.check_call(b);
        Ok(Val {
            v: self.narrow(got, ret, b),
            ty: ret,
        })
    }

    /// Call the runtime library, and turn its outcome record into a value.
    ///
    /// The LLVM emitter's `runtime`, written again — and, as with the upcall, what is *not*
    /// written again is the protocol: `beck_prim::abi` says the mark goes in, the new mark comes
    /// back, and two words sit above it. What is Cranelift's here is that the call is an
    /// **imported** symbol rather than a `declare`, and that a branch is two blocks rather than
    /// two labels.
    fn runtime(
        &mut self,
        op: prim::Op,
        vals: &[Val],
        ty: &beck_core::ty::Ty,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let Some(linked) = self.runtime.linked else {
            return Err(format!(
                "`{}` is the runtime library's, and this module does not link it",
                op.name()
            ));
        };
        if vals.len() != op.arity() {
            return Err(format!(
                "`{}` is applied to {} arguments here",
                op.name(),
                vals.len()
            ));
        }

        // One word per argument: an offset for text, the number itself for `time_format`.
        let mut words: Vec<IrValue> = Vec::with_capacity(3);
        for (i, v) in vals.iter().enumerate() {
            if i >= op.text_args() {
                if v.ty != Repr::Int {
                    return Err(format!(
                        "`{}` is given something that is not an Int",
                        op.name()
                    ));
                }
                words.push(v.v);
                continue;
            }
            match v.ty {
                Repr::Str => words.push(v.v),
                // `digest_keyed` takes a `secret[Str]`, and this is the one place in the compiled
                // half that opens one — the capability in the row is what pays for it (adr/0014).
                Repr::Obj(at) if op == prim::Op::DigestKeyed && i == 0 => {
                    let (slot, inner) = {
                        let layout = self.heap.layout(at);
                        layout.variants[0].slot("value").ok_or_else(|| {
                            format!("`{}` is given something that is not a secret", op.name())
                        })?
                    };
                    if inner != Repr::Str {
                        return Err(format!(
                            "`{}` is given a secret that is not text",
                            op.name()
                        ));
                    }
                    let text = self.load_field(v.v, slot, Repr::Str, b, m);
                    words.push(text.v);
                }
                _ => {
                    return Err(format!(
                        "`{}` is given something that is not text",
                        op.name()
                    ));
                }
            }
        }
        let zero = b.ins().iconst(types::I64, 0);
        while words.len() < 3 {
            words.push(zero);
        }

        let arena = self.arena();
        let flags = MemFlagsData::trusted();
        let next = arena.addr(arena.next, b, m);
        let mark = b.ins().load(types::I64, flags, next, 0);
        let f = m.declare_func_in_func(linked.call, b.func);
        let code = b.ins().iconst(types::I32, i64::from(op.code()));
        let call = b.ins().call(f, &[code, mark, words[0], words[1], words[2]]);
        let got = b.inst_results(call)[0];
        let full = b.ins().icmp_imm_s(IntCC::SignedLessThan, got, 0);
        self.trap(Trap::HeapExhausted, span, zero, full, b);
        // Re-taken after the trap's branch: the address is a symbol value, and a block other than
        // the one it was computed in cannot use it.
        let next = arena.addr(arena.next, b, m);
        b.ins().store(flags, got, next, 0);

        // The record is *at* the new mark — above the water line, so the call costs no arena
        // beyond its answer. Both words are read here, before anything can allocate over them.
        let base = arena.base(b, m);
        let rec = b.ins().iadd(base, got);
        let status = b.ins().load(types::I64, flags, rec, 0);
        let word = b.ins().load(types::I64, flags, rec, heap::WORD as i32);

        if let Some(raise) = op.raises() {
            self.raise_from(&raise, status, word, span, b, m)?;
        }

        // `str_to_int` is the one that can answer nothing, and what `None` looks like is the
        // `Option` the checker gave this expression rather than a shape this module invented.
        if op == prim::Op::StrToInt {
            let (option, some, none, slot, bytes) = self.option_of(ty, Repr::Int)?;
            let missing = b
                .ins()
                .icmp_imm_s(IntCC::Equal, status, prim::Status::Nothing.word());
            let cell = self.alloc(bytes, span, b, m);
            let some = b.ins().iconst(types::I64, i64::from(some));
            let none = b.ins().iconst(types::I64, i64::from(none));
            let tag = b.ins().select(missing, none, some);
            self.store_word(cell, 0, tag, b, m);
            self.store_word(cell, slot, word, b, m);
            return Ok(Val {
                v: cell,
                ty: option,
            });
        }

        let want = self
            .repr(ty)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        let expected = match op {
            prim::Op::DigestEq => Repr::Bool,
            prim::Op::UuidVersion | prim::Op::TimeParse => Repr::Int,
            _ => Repr::Str,
        };
        if want != expected {
            return Err(format!("`{}` answers something else here", op.name()));
        }
        Ok(Val {
            v: self.narrow(word, want, b),
            ty: want,
        })
    }

    /// Raise the declared value a runtime-library failure carries, when it failed.
    ///
    /// The message is the library's and everything around it is this module's: the variant, the
    /// fields the primitive fixes, and the type *name* as a literal's offset, which is what a
    /// `try:` compares against.
    fn raise_from(
        &mut self,
        raise: &prim::Raise,
        status: IrValue,
        why: IrValue,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<(), String> {
        let ty = beck_core::ty::Ty::con(raise.ty);
        let repr = self
            .repr(&ty)
            .map_err(|reason| format!("raises a value that is {reason}"))?;
        let Repr::Obj(at) = repr else {
            return Err(format!("raises `{}`, which is not an object", raise.ty));
        };
        let (tag, layout) = {
            let l = self.heap.layout(at);
            let tag = l
                .tag_of(Some(raise.variant))
                .ok_or_else(|| format!("`{}` has no `{}`", l.shown, raise.variant))?;
            (tag, l.variants[tag as usize].clone())
        };
        if layout.fields.len() != raise.constants.len() + 1 {
            return Err(format!(
                "`{}.{}` has fields this primitive does not fill",
                raise.ty, raise.variant
            ));
        }

        let failed = b
            .ins()
            .icmp_imm_s(IntCC::Equal, status, prim::Status::Raised.word());
        let bad = b.create_block();
        let good = b.create_block();
        b.ins().brif(failed, bad, &[], good, &[]);

        b.switch_to_block(bad);
        b.seal_block(bad);
        let mut placed: Vec<(usize, Val)> = Vec::with_capacity(layout.fields.len());
        for (name, text) in raise.constants {
            let (slot, want) = layout
                .slot(name)
                .ok_or_else(|| format!("`{}` has no field `{name}`", raise.ty))?;
            if want != Repr::Str {
                return Err(format!("the field `{name}` of `{}` is not text", raise.ty));
            }
            let at = self.literal(text);
            let v = b.ins().iconst(types::I64, at as i64);
            placed.push((slot, Val { v, ty: Repr::Str }));
        }
        let (slot, want) = layout
            .slot(raise.why)
            .ok_or_else(|| format!("`{}` has no field `{}`", raise.ty, raise.why))?;
        if want != Repr::Str {
            return Err(format!(
                "the field `{}` of `{}` is not text",
                raise.why, raise.ty
            ));
        }
        placed.push((
            slot,
            Val {
                v: why,
                ty: Repr::Str,
            },
        ));

        let off = self.alloc(layout.bytes(), span, b, m);
        let tag = b.ins().iconst(types::I64, i64::from(tag));
        self.store_word(off, 0, tag, b, m);
        for (slot, v) in &placed {
            self.store_field(off, *slot, v, b, m);
        }
        // The raised value, as `raise` itself carries one: a pair of the shape and the word, and
        // the type's name in the error cell for a handler to compare.
        let shape = self.heap.word_of(repr);
        let pair = self.alloc(heap::RAISED_WORDS * heap::WORD, span, b, m);
        let word = b.ins().iconst(types::I64, i64::from(shape));
        self.store_word(pair, 0, word, b, m);
        self.store_field(pair, 1, &Val { v: off, ty: repr }, b, m);
        let named = self.literal(raise.ty);
        let named = b.ins().iconst(types::I64, named as i64);
        let err = self.err();
        b.ins()
            .store(MemFlagsData::trusted(), named, err, CELL_RAISED);
        let always = b.ins().iconst(types::I8, 1);
        self.trap(Trap::Raised, span, pair, always, b);
        // The trap above left with a constant condition; the block it carried on into still needs
        // a terminator, and the value path is where it goes.
        b.ins().jump(good, &[]);

        b.switch_to_block(good);
        b.seal_block(good);
        Ok(())
    }

    /// The eight bytes the protocol carries, as the value its [`Repr`] says it is.
    fn narrow(&mut self, word: IrValue, ty: Repr, b: &mut FunctionBuilder<'_>) -> IrValue {
        match ty {
            Repr::Float => b.ins().bitcast(types::F64, MemFlagsData::new(), word),
            // An `I8` holding 0 or 1, which is what every comparison here produces and what
            // `band`/`bor` rely on — and `icmp` already answers one, so there is nothing to
            // widen. The `uextend` that was here was unreachable until `digest_eq` became the
            // first primitive to bring a `Bool` back through this protocol, and Cranelift's
            // verifier refuses an extension from a type to itself.
            Repr::Bool => b.ins().icmp_imm_s(IntCC::NotEqual, word, 0),
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => word,
        }
    }

    fn store_word(
        &mut self,
        off: IrValue,
        slot: usize,
        word: IrValue,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) {
        let at = self.word_addr(off, slot, b, m);
        b.ins().store(MemFlagsData::trusted(), word, at, 0);
    }

    /// A field, as the value its [`Repr`] says it is.
    fn load_field(
        &mut self,
        off: IrValue,
        slot: usize,
        repr: Repr,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Val {
        let at = self.word_addr(off, slot, b, m);
        let flags = MemFlagsData::trusted();
        let v = match repr {
            Repr::Float => b.ins().load(types::F64, flags, at, 0),
            Repr::Bool => {
                let raw = b.ins().load(types::I64, flags, at, 0);
                b.ins().icmp_imm_s(IntCC::NotEqual, raw, 0)
            }
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => b.ins().load(types::I64, flags, at, 0),
        };
        Val { v, ty: repr }
    }

    /// Put a value in a field.
    ///
    /// A real is **normalised** on the way in, for the reason `beck_llvm::emit`'s own
    /// `store_field` gives: a stored real is compared with another stored real, read back by the
    /// host, and part of what a record's `==` answers, so every real on the heap is the one the
    /// evaluator would have built.
    fn store_field(
        &mut self,
        off: IrValue,
        slot: usize,
        v: &Val,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) {
        let word = match v.ty {
            Repr::Float => self.normalise(v.v, b),
            Repr::Bool => b.ins().uextend(types::I64, v.v),
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => v.v,
        };
        let at = self.word_addr(off, slot, b, m);
        b.ins().store(MemFlagsData::trusted(), word, at, 0);
    }

    /// Reserve `bytes` in the arena and answer the offset, or trap if there is no room.
    fn alloc(
        &mut self,
        bytes: u64,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> IrValue {
        let arena = self.arena();
        let idx = self.span(span);
        let err = self.err();
        let f = arena.alloc_in(b, m);
        let n = b.ins().iconst(types::I64, bytes as i64);
        let sp = b.ins().iconst(types::I32, i64::from(idx));
        let call = b.ins().call(f, &[err, n, sp]);
        let off = b.inst_results(call)[0];
        self.check_call(b);
        off
    }

    /// `Point(x=1, y=2)`, `Some(v)`, `Id(3)` — one object, filled in.
    fn make(
        &mut self,
        ty: &beck_core::ty::Ty,
        variant: Option<&str>,
        fields: &[(Arc<str>, Core)],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let repr = self
            .repr(ty)
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
            let v = self.value(expr, b, m)?;
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

        let off = self.alloc(layout.bytes(), span, b, m);
        let tagv = b.ins().iconst(types::I64, i64::from(tag));
        self.store_word(off, 0, tagv, b, m);
        for (slot, v) in &placed {
            self.store_field(off, *slot, v, b, m);
        }
        Ok(Val { v: off, ty: repr })
    }

    /// `p.x`.
    fn field(
        &mut self,
        base: &Core,
        name: &str,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let v = self.value(base, b, m)?;
        let Repr::Obj(at) = v.ty else {
            return Err(format!(
                "reads the field `{name}` of something that is not a record"
            ));
        };
        let (slot, repr) = {
            let layout = self.heap.layout(at);
            // A union's fields are read by matching it, never by naming one.
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
        Ok(self.load_field(v.v, slot, repr, b, m))
    }

    /// `p.with(x = 3)` — a new object with the old one's other fields.
    fn with(
        &mut self,
        base: &Core,
        fields: &[(Arc<str>, Core)],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let v = self.value(base, b, m)?;
        let Repr::Obj(at) = v.ty else {
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
            let f = self.value(expr, b, m)?;
            let (slot, want) = layout
                .slot(name)
                .ok_or_else(|| format!("no field `{name}` to update"))?;
            if f.ty != want {
                return Err(format!("the field `{name}` is the wrong type here"));
            }
            placed.push((slot, f));
        }

        let off = self.alloc(layout.bytes(), span, b, m);
        // Word for word, because a copy does not care what a field means — and then the named ones
        // are written over.
        for slot in 0..=layout.fields.len() {
            let w = self.load_word(v.v, slot, b, m);
            self.store_word(off, slot, w, b, m);
        }
        for (slot, f) in &placed {
            self.store_field(off, *slot, f, b, m);
        }
        Ok(Val { v: off, ty: v.ty })
    }

    /// `lambda x: …` — the object, and the lambda's body queued to be written.
    ///
    /// The captures and their order are [`heap::Lambda`]'s, which is the same record
    /// [`Body::emit_lam`] reads them back out of. That is the contract, and it is in `beck_llvm`'s
    /// `heap` because it is shared with the other emitter and with nothing else.
    fn closure(
        &mut self,
        params: &Arc<[VarId]>,
        body: &Arc<Core>,
        ty: &beck_core::ty::Ty,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let repr = self
            .repr(ty)
            .map_err(|why| format!("builds a closure that is {why}"))?;
        let Repr::Fn(family) = repr else {
            return Err(format!("builds a closure whose type is `{ty}`"));
        };
        let rank = self
            .heap
            .rank_of(params, body.span.start)
            .ok_or("builds a closure from a `lam` the survey did not rank")?;
        if params.len() != self.heap.family(family).params.len() {
            return Err("builds a closure whose parameters are not the ones its type has".into());
        }
        let captures = self.heap.lam(rank).captures.clone();
        let mut vals = Vec::with_capacity(captures.len());
        for (var, ty) in &captures {
            let want = self
                .repr(ty)
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
        self.pending.push(Pending {
            rank,
            params: params.clone(),
            body: body.clone(),
            family,
        });
        let off = self.alloc(heap::closure_bytes(captures.len() as u64), span, b, m);
        let tag = b.ins().iconst(types::I64, i64::from(rank));
        self.store_word(off, 0, tag, b, m);
        for (i, v) in vals.iter().enumerate() {
            self.store_field(off, i + 1, v, b, m);
        }
        Ok(Val { v: off, ty: repr })
    }

    /// A definition named where a value is expected — `map_list(xs, double)`.
    ///
    /// No captures and no lambda of its own: the arm of the application that answers to this rank
    /// calls the definition, so what is allocated here is one word.
    fn named(
        &mut self,
        name: &str,
        ty: &beck_core::ty::Ty,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        if !self.eligible.contains(name) {
            return Err(format!(
                "names `{name}` as a value, and it does not compile"
            ));
        }
        let repr = self
            .repr(ty)
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
            .ok_or_else(|| format!("names `{name}`, which has no signature"))?;
        let fam = self.heap.family(family);
        if sig.params != fam.params || sig.ret != fam.ret {
            return Err(format!(
                "names `{name}` as a `{}`, and that is not the shape it compiled to",
                fam.shown
            ));
        }
        let off = self.alloc(heap::closure_bytes(0), span, b, m);
        let tag = b.ins().iconst(types::I64, i64::from(rank));
        self.store_word(off, 0, tag, b, m);
        Ok(Val { v: off, ty: repr })
    }

    /// Applying a value rather than calling a name.
    fn apply(
        &mut self,
        func: &Core,
        args: &[Core],
        dest: Dest,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
        let f = self.value(func, b, m)?;
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
        let mut operands = vec![self.err(), f.v];
        for (a, want) in args.iter().zip(&fam.params) {
            let v = self.value(a, b, m)?;
            if v.ty != *want {
                return Err(format!(
                    "an argument to a `{}` is the wrong type",
                    fam.shown
                ));
            }
            operands.push(v.v);
        }
        self.applied.insert(family);
        let ptr = m.target_config().pointer_type();
        let id = m
            .declare_function(
                &apply_symbol(family),
                Linkage::Local,
                &family_signature(&fam, ptr),
            )
            .map_err(|e| format!("declaring an application: {e}"))?;
        let fref = m.declare_func_in_func(id, b.func);
        // A call in tail position stays one, through the application and through the arm inside it —
        // `docs/27`'s guarantee is about the language, so a loop written as a closure calling itself
        // must not grow the stack.
        if dest == Dest::Return && fam.ret == self.ret {
            b.ins().return_call(fref, &operands);
            return Ok(None);
        }
        let call = b.ins().call(fref, &operands);
        let r = b.inst_results(call)[0];
        self.check_call(b);
        self.finish(Val { v: r, ty: fam.ret }, dest, b)
    }

    /// The body of one `lam`, as its own function: the captures, then the body.
    fn emit_lam(
        &mut self,
        pending: &Pending,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<(), String> {
        let fam = self.heap.family(pending.family).clone();
        self.ret = fam.ret;
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);
        self.err = Some(b.block_params(entry)[0]);
        let clo = b.block_params(entry)[1];
        for (i, (var, ty)) in pending.params.iter().zip(&fam.params).enumerate() {
            let v = b.block_params(entry)[i + 2];
            self.env.insert(*var, Val { v, ty: *ty });
        }
        let captures = self.heap.lam(pending.rank).captures.clone();
        for (i, (var, ty)) in captures.iter().enumerate() {
            let want = self
                .repr(ty)
                .map_err(|why| format!("captures a variable that is {why}"))?;
            let v = self.load_field(clo, i + 1, want, b, m);
            self.env.insert(*var, v);
        }
        self.expr(&pending.body, Dest::Return, b, m)?;
        Ok(())
    }

    /// rests on.
    /// A direct call of a named definition — and in tail position, a jump.
    ///
    /// `return_call` rather than a call and a return: Cranelift's verifier *requires* the frame to
    /// be discardable and refuses the function otherwise, which is the same guarantee `musttail`
    /// gives the other backend. `docs/27` §27.2 says 1,500 and 60,000 tail calls spend the same
    /// host stack, and an optimisation that "usually" fires cannot be what a language guarantee
    /// rests on.
    ///
    /// Anything that is not a name is a closure being applied, which is [`Body::apply`].
    fn call(
        &mut self,
        func: &Core,
        args: &[Core],
        dest: Dest,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Option<Val>, String> {
        let CoreKind::Global(name) = &func.kind else {
            return self.apply(func, args, dest, b, m);
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
        ty: &beck_core::ty::Ty,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        // Before the arguments, because this one's first argument is a *block* and evaluating it
        // here would run it outside the protection it exists to have.
        if op == Prim::Try {
            return self.try_(args, ty, span, b, m);
        }
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.value(a, b, m)?);
        }
        // The four that are questions rather than computations. Before the rest, because what
        // separates them is not what they do with their arguments.
        if let Some(ask) = Upcall::of(op) {
            return self.upcall(ask, &vals, ty, span, b, m);
        }
        // And the fifteen that are a table, a grammar or somebody else's parser: a call into the
        // runtime library, which is the same code the evaluator runs (`docs/93` §93.12).
        if let Some(linked) = prim::op_of(op) {
            return self.runtime(linked, &vals, ty, span, b, m);
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
        let same = |vals: &[Val]| -> Result<Repr, String> {
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
                    // `+` on two strings is the one arithmetic operator text has, and the only
                    // place this backend allocates for something that is not a constructor.
                    Repr::Str if op == Prim::Add => Ok(self.text_call(
                        Which::Concat,
                        &[vals[0].v, vals[1].v],
                        Repr::Str,
                        span,
                        b,
                        m,
                    )),
                    Repr::Int => Ok(self.checked_int(op, &vals[0], &vals[1], span, b)),
                    Repr::Float => {
                        let v = match op {
                            Prim::Add => b.ins().fadd(vals[0].v, vals[1].v),
                            Prim::Sub => b.ins().fsub(vals[0].v, vals[1].v),
                            _ => b.ins().fmul(vals[0].v, vals[1].v),
                        };
                        Ok(Val { v, ty: Repr::Float })
                    }
                    Repr::Bool
                    | Repr::Str
                    | Repr::List(_)
                    | Repr::Map(_)
                    | Repr::Obj(_)
                    | Repr::Fn(_)
                    | Repr::Html
                    | Repr::Attr => Err(format!("`{}` on a value that is not a number", op.name())),
                }
            }
            Prim::Div | Prim::Rem => {
                arity(2, &vals)?;
                match same(&vals)? {
                    Repr::Int => Ok(self.checked_divide(op, &vals[0], &vals[1], span, b)),
                    // `%` on reals is not in the language: the evaluator's arm answers only for
                    // two Ints. Division normalises its *divisor* — `1.0 / -0.0` is `-inf` where
                    // `1.0 / 0.0` is `+inf`, which is a difference a zero's sign has escaped into.
                    Repr::Float if op == Prim::Div => {
                        let d = self.normalise(vals[1].v, b);
                        let v = b.ins().fdiv(vals[0].v, d);
                        Ok(Val { v, ty: Repr::Float })
                    }
                    _ => Err(format!("`{}` on this type", op.name())),
                }
            }
            Prim::Neg => {
                arity(1, &vals)?;
                match vals[0].ty {
                    Repr::Int => {
                        // `i64::checked_neg`: the one input without an answer is `i64::MIN`.
                        let bad = b.ins().icmp_imm_s(IntCC::Equal, vals[0].v, i64::MIN);
                        let payload = self.widen(&vals[0], b);
                        self.trap(Trap::NegOverflow, span, payload, bad, b);
                        let v = b.ins().ineg(vals[0].v);
                        Ok(Val { v, ty: Repr::Int })
                    }
                    Repr::Float => Ok(Val {
                        v: b.ins().fneg(vals[0].v),
                        ty: Repr::Float,
                    }),
                    Repr::Bool
                    | Repr::Str
                    | Repr::List(_)
                    | Repr::Map(_)
                    | Repr::Obj(_)
                    | Repr::Fn(_)
                    | Repr::Html
                    | Repr::Attr => Err("`negate` on a value that is not a number".into()),
                }
            }
            Prim::Abs => {
                arity(1, &vals)?;
                match vals[0].ty {
                    Repr::Int => {
                        let bad = b.ins().icmp_imm_s(IntCC::Equal, vals[0].v, i64::MIN);
                        let payload = self.widen(&vals[0], b);
                        self.trap(Trap::AbsOverflow, span, payload, bad, b);
                        Ok(Val {
                            v: b.ins().iabs(vals[0].v),
                            ty: Repr::Int,
                        })
                    }
                    Repr::Float => Ok(Val {
                        v: b.ins().fabs(vals[0].v),
                        ty: Repr::Float,
                    }),
                    Repr::Bool
                    | Repr::Str
                    | Repr::List(_)
                    | Repr::Map(_)
                    | Repr::Obj(_)
                    | Repr::Fn(_)
                    | Repr::Html
                    | Repr::Attr => Err("`abs` on a value that is not a number".into()),
                }
            }
            Prim::Sqrt => {
                arity(1, &vals)?;
                if vals[0].ty != Repr::Float {
                    return Err("`sqrt` of something that is not a Float".into());
                }
                Ok(Val {
                    v: b.ins().sqrt(vals[0].v),
                    ty: Repr::Float,
                })
            }
            // Cranelift has no transcendental instructions, so these are calls into the C library
            // the executable is linked against — the same `libm` `clang` gives the other backend's
            // `llvm.sin.f64` when it lowers one.
            Prim::Sin | Prim::Cos => {
                arity(1, &vals)?;
                if vals[0].ty != Repr::Float {
                    return Err(format!("`{}` of something that is not a Float", op.name()));
                }
                let name = if op == Prim::Sin { "sin" } else { "cos" };
                let v = self.libm(name, vals[0].v, b, m)?;
                Ok(Val { v, ty: Repr::Float })
            }
            Prim::Trunc => {
                arity(1, &vals)?;
                if vals[0].ty != Repr::Float {
                    return Err("`trunc` of something that is not a Float".into());
                }
                // Saturating, because the evaluator's `f as i64` is: out of range is the nearest
                // representable and NaN is zero, which is what Rust's cast does.
                Ok(Val {
                    v: b.ins().fcvt_to_sint_sat(types::I64, vals[0].v),
                    ty: Repr::Int,
                })
            }
            Prim::ToFloat => {
                arity(1, &vals)?;
                if vals[0].ty != Repr::Int {
                    return Err("`float` of something that is not an Int".into());
                }
                // No normalisation: an integer converts to neither a negative zero nor a NaN.
                Ok(Val {
                    v: b.ins().fcvt_from_sint(types::F64, vals[0].v),
                    ty: Repr::Float,
                })
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                arity(2, &vals)?;
                same(&vals)?;
                self.compare(op, &vals[0], &vals[1], b, m)
            }
            Prim::And | Prim::Or | Prim::Not => {
                let want = if op == Prim::Not { 1 } else { 2 };
                arity(want, &vals)?;
                if vals.iter().any(|v| v.ty != Repr::Bool) {
                    return Err(format!("`{}` on something that is not a Bool", op.name()));
                }
                // `bxor 1` and not a complement: a Bool here is an `I8` holding 0 or 1, and
                // `bnot` would answer 254.
                let v = match op {
                    Prim::Not => b.ins().bxor_imm_s(vals[0].v, 1),
                    Prim::And => b.ins().band(vals[0].v, vals[1].v),
                    _ => b.ins().bor(vals[0].v, vals[1].v),
                };
                Ok(Val { v, ty: Repr::Bool })
            }
            Prim::StrLen | Prim::StrIsEmpty => {
                arity(1, &vals)?;
                self.text_arg(&vals[0], op)?;
                // Both counts are in the header, so both of these are a load — `str_len` is `O(1)`
                // in the evaluator since `docs/70`, and a backend that counted here would make the
                // loop that walks a string by index quadratic in one implementation and not the
                // other.
                let at = if op == Prim::StrLen {
                    heap::WORD as i64
                } else {
                    0
                };
                let n = self.text().header(self.arena(), vals[0].v, at, b, m);
                if op == Prim::StrLen {
                    return Ok(Val {
                        v: n,
                        ty: Repr::Int,
                    });
                }
                Ok(Val {
                    v: b.ins().icmp_imm_s(IntCC::Equal, n, 0),
                    ty: Repr::Bool,
                })
            }
            Prim::StrSlice => {
                arity(3, &vals)?;
                self.text_arg(&vals[0], op)?;
                for v in &vals[1..] {
                    if v.ty != Repr::Int {
                        return Err("`str_slice` takes two Int positions".into());
                    }
                }
                Ok(self.text_call(
                    Which::Slice,
                    &[vals[0].v, vals[1].v, vals[2].v],
                    Repr::Str,
                    span,
                    b,
                    m,
                ))
            }
            Prim::StrTrim => {
                arity(1, &vals)?;
                self.text_arg(&vals[0], op)?;
                Ok(self.text_call(Which::Trim, &[vals[0].v], Repr::Str, span, b, m))
            }
            Prim::StrSplit | Prim::StrChars => {
                // One function, because the evaluator answers characters for an empty separator —
                // so `str_chars(s)` *is* `str_split(s, "")`, and the two share a body as well as a
                // fixture.
                let sep = if op == Prim::StrChars {
                    arity(1, &vals)?;
                    self.text_arg(&vals[0], op)?;
                    // The offset `0`, which is never a live object — so `str_chars` needs no
                    // literal, and the pool stays a function of the program's own text.
                    b.ins().iconst(types::I64, 0)
                } else {
                    arity(2, &vals)?;
                    self.text_arg(&vals[0], op)?;
                    self.text_arg(&vals[1], op)?;
                    vals[1].v
                };
                let f = self
                    .builds()
                    .split
                    .ok_or("`str_split` in a module with no list runtime")?;
                // Interning the element is what puts the list runtime in the module: the answer is
                // a `list[Str]` no program in the module need have written down.
                let at = self.heap.word_of(Repr::Str);
                let mut r = self.build_call(f, &[vals[0].v, sep], span, b, m);
                r.ty = Repr::List(at);
                Ok(r)
            }
            Prim::StrContains | Prim::StrStartsWith | Prim::StrEndsWith => {
                arity(2, &vals)?;
                self.text_arg(&vals[0], op)?;
                self.text_arg(&vals[1], op)?;
                Ok(self.text_search(op, vals[0].v, vals[1].v, b, m))
            }
            Prim::StrIndexOf => {
                arity(2, &vals)?;
                self.text_arg(&vals[0], op)?;
                self.text_arg(&vals[1], op)?;
                self.index_of(ty, vals[0].v, vals[1].v, span, b, m)
            }
            // `list_append` — a new header, and a slot in the block when there is one. See the LLVM
            // emitter's arm and `beck_llvm::heap::LIST_HEADER`.
            Prim::ListAppend => {
                arity(2, &vals)?;
                let at = self.list_arg(&vals[0], op)?;
                let element = self.heap.element(at);
                if vals[1].ty != element {
                    return Err("`list_append` of an element of another type".into());
                }
                let lists = self.lists();
                let word = self.widen(&vals[1], b);
                let out = self.build_call(lists.append, &[vals[0].v, word], span, b, m);
                Ok(Val {
                    v: out.v,
                    ty: vals[0].ty,
                })
            }
            Prim::ListLen | Prim::ListIsEmpty => {
                arity(1, &vals)?;
                self.list_arg(&vals[0], op)?;
                let n = self.lists().count(self.arena(), vals[0].v, b, m);
                if op == Prim::ListLen {
                    return Ok(Val {
                        v: n,
                        ty: Repr::Int,
                    });
                }
                Ok(Val {
                    v: b.ins().icmp_imm_s(IntCC::Equal, n, 0),
                    ty: Repr::Bool,
                })
            }
            Prim::ListGet => {
                arity(2, &vals)?;
                self.list_arg(&vals[0], op)?;
                if vals[1].ty != Repr::Int {
                    return Err("`list_get` takes an Int index".into());
                }
                self.list_get(ty, &vals[0], &vals[1], span, b, m)
            }
            Prim::ListContains | Prim::ListIndexOf => {
                arity(2, &vals)?;
                let at = self.list_arg(&vals[0], op)?;
                let element = self.heap.element(at);
                if vals[1].ty != element {
                    return Err(format!(
                        "`{}` against an element of another type",
                        op.name()
                    ));
                }
                let word = self.widen(&vals[1], b);
                let sig = compare_signature(m);
                let id = m
                    .declare_function(&format!("beck.list.find.{at}"), Linkage::Local, &sig)
                    .map_err(|e| format!("declaring a search: {e}"))?;
                let f = m.declare_func_in_func(id, b.func);
                let call = b.ins().call(f, &[vals[0].v, word]);
                let found = b.inst_results(call)[0];
                self.wants(Repr::List(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                if op == Prim::ListContains {
                    return Ok(Val {
                        v: b.ins()
                            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, found, 0),
                        ty: Repr::Bool,
                    });
                }
                self.some_or_none(ty, found, span, b, m)
            }
            Prim::ListSlice | Prim::ListTake | Prim::ListDrop => {
                let want = if op == Prim::ListSlice { 3 } else { 2 };
                arity(want, &vals)?;
                self.list_arg(&vals[0], op)?;
                for v in &vals[1..] {
                    if v.ty != Repr::Int {
                        return Err(format!("`{}` takes Int positions", op.name()));
                    }
                }
                self.list_range(op, &vals, span, b, m)
            }
            Prim::ToStr => {
                arity(1, &vals)?;
                match vals[0].ty {
                    Repr::Str => Ok(vals[0]),
                    Repr::Int => {
                        let f = self.builds().from_int;
                        Ok(self.build_call(f, &[vals[0].v], span, b, m))
                    }
                    // Two literals from the pool, which is what `Value::display` answers.
                    Repr::Bool => {
                        let (t, f) = (self.literal("true"), self.literal("false"));
                        let t = b.ins().iconst(types::I64, t as i64);
                        let f = b.ins().iconst(types::I64, f as i64);
                        Ok(Val {
                            v: b.ins().select(vals[0].v, t, f),
                            ty: Repr::Str,
                        })
                    }
                    other => Err(format!(
                        "`str` of {}, whose rendering is not a decimal this backend can reproduce \
                         — a real's shortest round-trip form is an algorithm rather than a loop",
                        self.heap.show(other)
                    )),
                }
            }
            Prim::StrRepeat => {
                arity(2, &vals)?;
                self.text_arg(&vals[0], op)?;
                if vals[1].ty != Repr::Int {
                    return Err("`str_repeat` takes an Int count".into());
                }
                let f = self.builds().repeat;
                Ok(self.build_call(f, &[vals[0].v, vals[1].v], span, b, m))
            }
            Prim::StrJoin => {
                arity(2, &vals)?;
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
                self.text_arg(&vals[1], op)?;
                let f = self
                    .builds()
                    .join
                    .ok_or("`str_join` in a module with no list runtime")?;
                Ok(self.build_call(f, &[vals[0].v, vals[1].v], span, b, m))
            }
            Prim::OptionIsSome => {
                arity(1, &vals)?;
                let (some, ..) = self.option_taken(vals[0].ty)?;
                let tag = self.load_word(vals[0].v, 0, b, m);
                Ok(Val {
                    v: b.ins().icmp_imm_s(IntCC::Equal, tag, i64::from(some)),
                    ty: Repr::Bool,
                })
            }
            Prim::OptionUnwrapOr => {
                arity(2, &vals)?;
                let (some, slot, payload) = self.option_taken(vals[0].ty)?;
                if vals[1].ty != payload {
                    return Err("`unwrap_or`'s fallback is not what the `Option` carries".into());
                }
                let tag = self.load_word(vals[0].v, 0, b, m);
                let is_some = b.ins().icmp_imm_s(IntCC::Equal, tag, i64::from(some));
                // The address, not the value: a `None` the *host* wrote is one word long, because
                // `encode_object` allocates the variant's own size — so reading the payload slot
                // unconditionally can read past the end of the arena.
                let field = self.word_addr(vals[0].v, slot, b, m);
                let header = self.word_addr(vals[0].v, 0, b, m);
                let p = b.ins().select(is_some, field, header);
                let w = b.ins().load(types::I64, MemFlagsData::trusted(), p, 0);
                let held = word_as(w, payload, b);
                Ok(Val {
                    v: b.ins().select(is_some, held, vals[1].v),
                    ty: payload,
                })
            }
            Prim::MapLen => {
                arity(1, &vals)?;
                self.map_arg(&vals[0], op)?;
                let maps = self.maps();
                let n = maps.sized(vals[0].v, b, m);
                Ok(Val {
                    v: n,
                    ty: Repr::Int,
                })
            }
            Prim::MapGet | Prim::MapContains => {
                arity(2, &vals)?;
                let at = self.map_arg(&vals[0], op)?;
                let (k, _) = self.heap.entry(at);
                let key = self.heap.element(k);
                if vals[1].ty != key {
                    return Err(format!("`{}` with a key of another type", op.name()));
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                let word = self.widen(&vals[1], b);
                let sig = compare_signature(m);
                let id = m
                    .declare_function(&format!("beck.map.find.{at}"), Linkage::Local, &sig)
                    .map_err(|e| format!("declaring a search: {e}"))?;
                let f = m.declare_func_in_func(id, b.func);
                let call = b.ins().call(f, &[vals[0].v, word]);
                let found = b.inst_results(call)[0];
                if op == Prim::MapContains {
                    return Ok(Val {
                        v: b.ins().icmp_imm_s(IntCC::NotEqual, found, 0),
                        ty: Repr::Bool,
                    });
                }
                self.map_get(ty, &vals[0], found, span, b, m)
            }
            // The three that grow a map: a path rebuild over `docs/93`'s tree. See the LLVM
            // emitter's arms.
            Prim::MapInsert | Prim::MapRemove | Prim::MapMerge => {
                arity(
                    match op {
                        Prim::MapInsert => 3,
                        _ => 2,
                    },
                    &vals,
                )?;
                let at = self.map_arg(&vals[0], op)?;
                let (k, v) = self.heap.entry(at);
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                let name = match op {
                    Prim::MapInsert => format!("beck.map.ins.{at}"),
                    Prim::MapRemove => format!("beck.map.del.{at}"),
                    _ => format!("beck.map.merge.{at}"),
                };
                let ptr = m.isa().pointer_type();
                let mut args = vec![vals[0].v];
                let mut params = vec![ptr, types::I64];
                if op == Prim::MapMerge {
                    if vals[1].ty != vals[0].ty {
                        return Err("`map_merge` of two maps of different types".into());
                    }
                    args.push(vals[1].v);
                    params.push(types::I64);
                } else {
                    if vals[1].ty != self.heap.element(k) {
                        return Err(format!("`{}` with a key of another type", op.name()));
                    }
                    let kw = self.widen(&vals[1], b);
                    args.push(kw);
                    params.push(types::I64);
                    if op == Prim::MapInsert {
                        if vals[2].ty != self.heap.element(v) {
                            return Err("`map_insert` with a value of another type".into());
                        }
                        let vw = self.widen(&vals[2], b);
                        args.push(vw);
                        params.push(types::I64);
                    }
                }
                params.push(types::I32);
                let mut sig = cranelift_codegen::ir::Signature::new(CallConv::triple_default(
                    m.isa().triple(),
                ));
                for p in &params {
                    sig.params.push(AbiParam::new(*p));
                }
                sig.returns.push(AbiParam::new(types::I64));
                let id = m
                    .declare_function(&name, Linkage::Local, &sig)
                    .map_err(|e| format!("declaring `{name}`: {e}"))?;
                let out = self.build_call(id, &args, span, b, m);
                Ok(Val {
                    v: out.v,
                    ty: vals[0].ty,
                })
            }
            Prim::MapKeys | Prim::MapValues => {
                arity(1, &vals)?;
                self.map_arg(&vals[0], op)?;
                self.map_run(op, ty, &vals[0], span, b, m)
            }
            // A list of lists into one list. Not a growth: the total is a sum over the outer
            // list's header words, so the allocation happens once and after it.
            Prim::ConcatLists => {
                arity(1, &vals)?;
                let outer = self.list_arg(&vals[0], op)?;
                let Repr::List(inner) = self.heap.element(outer) else {
                    return Err("`concat_lists` on something that is not a list of lists".into());
                };
                let lists = self.lists();
                let idx = self.span(span);
                let span = b.ins().iconst(types::I32, i64::from(idx));
                let err = self.err();
                let f = m.declare_func_in_func(lists.concat, b.func);
                let call = b.ins().call(f, &[err, vals[0].v, span]);
                let r = b.inst_results(call)[0];
                self.check_call(b);
                Ok(Val {
                    v: r,
                    ty: Repr::List(inner),
                })
            }
            Prim::ListReverse => {
                arity(1, &vals)?;
                let at = self.list_arg(&vals[0], op)?;
                let lists = self.lists();
                let idx = self.span(span);
                let span = b.ins().iconst(types::I32, i64::from(idx));
                let err = self.err();
                let f = m.declare_func_in_func(lists.reverse, b.func);
                let call = b.ins().call(f, &[err, vals[0].v, span]);
                let r = b.inst_results(call)[0];
                self.check_call(b);
                Ok(Val {
                    v: r,
                    ty: Repr::List(at),
                })
            }
            Prim::MapList | Prim::FilterList => {
                arity(2, &vals)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let fam = self.function_arg(&vals[1], op, &[element])?;
                let family = self.heap.family(fam).clone();
                let out = if op == Prim::MapList {
                    let repr = self
                        .repr(ty)
                        .map_err(|why| format!("`{}` answers with {why}", op.name()))?;
                    let Repr::List(at) = repr else {
                        return Err(format!("`{}` answers with a `{ty}`", op.name()));
                    };
                    if self.heap.element(at) != family.ret {
                        return Err(format!(
                            "`{}` answers a list of something other than what its function does",
                            op.name()
                        ));
                    }
                    repr
                } else {
                    if family.ret != Repr::Bool {
                        return Err("`filter_list`'s function does not answer a Bool".into());
                    }
                    vals[0].ty
                };
                let which = if op == Prim::MapList {
                    Loop::Map
                } else {
                    Loop::Filter
                };
                let r = self.list_loop(which, fam, &[vals[0].v, vals[1].v], span, b, m)?;
                Ok(Val { v: r, ty: out })
            }
            Prim::ListFold => {
                arity(3, &vals)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let acc = vals[1].ty;
                let fam = self.function_arg(&vals[2], op, &[acc, element])?;
                if self.heap.family(fam).ret != acc {
                    return Err(
                        "`list_fold`'s function answers something other than the accumulator it \
                         is given"
                            .into(),
                    );
                }
                let r = self.list_loop(
                    Loop::Fold,
                    fam,
                    &[vals[0].v, vals[1].v, vals[2].v],
                    span,
                    b,
                    m,
                )?;
                Ok(Val { v: r, ty: acc })
            }
            // Decorate, sort, undecorate — and the keys are words like any others, so what compares
            // two of them is the function a list's element comparison already is.
            Prim::SortBy => {
                arity(2, &vals)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let fam = self.function_arg(&vals[1], op, &[element])?;
                let key = self.heap.family(fam).ret;
                // Interned here rather than in the survey: the keys are not a list any program
                // wrote, so nothing else would have asked for their comparison — and recording the
                // index is what makes the module generate it.
                let at = self.heap.word_of(key);
                self.wants(Repr::List(at))
                    .map_err(|why| format!("`{}` by a key that is {why}", op.name()))?;
                let r = self.list_loop(Loop::Sort, fam, &[vals[0].v, vals[1].v], span, b, m)?;
                Ok(Val {
                    v: r,
                    ty: vals[0].ty,
                })
            }
            Prim::ListAll | Prim::ListAny => {
                arity(2, &vals)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let fam = self.function_arg(&vals[1], op, &[element])?;
                if self.heap.family(fam).ret != Repr::Bool {
                    return Err(format!("`{}`'s function does not answer a Bool", op.name()));
                }
                let want = b.ins().iconst(types::I8, i64::from(op == Prim::ListAny));
                let r =
                    self.list_loop(Loop::Every, fam, &[vals[0].v, vals[1].v, want], span, b, m)?;
                Ok(Val {
                    v: r,
                    ty: Repr::Bool,
                })
            }
            // `raise e` — the one failure that is not a fault, so it carries a value. Two words in
            // the arena and the type *name* in the error cell's third word; see the LLVM emitter's
            // arm and `beck_llvm::Trap::Raised`.
            Prim::Raise => {
                arity(1, &vals)?;
                let Repr::Obj(at) = vals[0].ty else {
                    return Err(format!(
                        "raises {}, and a raised value must have a declared type",
                        self.heap.show(vals[0].ty)
                    ));
                };
                let name = self.heap.layout(at).name.to_string();
                let shape = self.heap.word_of(vals[0].ty);
                let pair = self.alloc(heap::RAISED_WORDS * heap::WORD, span, b, m);
                let word = b.ins().iconst(types::I64, i64::from(shape));
                self.store_word(pair, 0, word, b, m);
                let v = vals[0];
                self.store_field(pair, 1, &v, b, m);
                let named = self.literal(&name);
                let named = b.ins().iconst(types::I64, named as i64);
                let err = self.err();
                b.ins()
                    .store(MemFlagsData::trusted(), named, err, CELL_RAISED);
                let always = b.ins().iconst(types::I8, 1);
                self.trap(Trap::Raised, span, pair, always, b);
                // Unreachable: the trap above left with a constant condition, and the block it
                // carried on into still needs a terminator. `raise` has no type of its own, so
                // what this answers with is never read.
                self.escape(b);
                let gone = b.create_block();
                b.switch_to_block(gone);
                b.seal_block(gone);
                let want = self.repr(ty).unwrap_or(Repr::Int);
                Ok(Val {
                    v: self.zero(want, b),
                    ty: want,
                })
            }
            // The five that build a page: an allocation and some stores, because what goes in the
            // arena is the *call* rather than the tree. See `beck_llvm::heap::Repr::Html`, and the
            // LLVM emitter's five arms, which this is written twice with on purpose
            // (`docs/93` §93.8).
            Prim::HtmlEl => {
                arity(3, &vals)?;
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
                let off = self.alloc(heap::NODE_WORDS * heap::WORD, span, b, m);
                let tag = b.ins().iconst(types::I64, heap::HTML_ELEMENT as i64);
                self.store_word(off, 0, tag, b, m);
                for (slot, v) in vals.clone().into_iter().enumerate() {
                    self.store_field(off, slot + 1, &v, b, m);
                }
                Ok(Val {
                    v: off,
                    ty: Repr::Html,
                })
            }
            Prim::HtmlText => {
                arity(1, &vals)?;
                // A child that is already a tree is spliced rather than rendered, which is the
                // evaluator's own arm — and here it needs no node at all.
                if vals[0].ty == Repr::Html {
                    return Ok(vals[0]);
                }
                let v = vals[0];
                self.node(Repr::Html, heap::HTML_TEXT, None, Some(&v), span, b, m)
            }
            Prim::HtmlAttr | Prim::HtmlOn => {
                arity(2, &vals)?;
                if vals[0].ty != Repr::Str {
                    return Err(format!("`{}` with a name that is not text", op.name()));
                }
                let tag = if op == Prim::HtmlAttr {
                    heap::ATTR_PLAIN
                } else {
                    heap::ATTR_ON
                };
                let (name, value) = (vals[0], vals[1]);
                self.node(Repr::Attr, tag, Some(&name), Some(&value), span, b, m)
            }
            Prim::HtmlKey => {
                arity(1, &vals)?;
                let v = vals[0];
                self.node(Repr::Attr, heap::ATTR_KEY, None, Some(&v), span, b, m)
            }
            other => Err(refusal(other)),
        }
    }

    /// The `list[Attr]` and `list[Html]` reprs, resolving `Html` first if nothing has.
    fn view_lists(&mut self) -> Result<(u32, u32), String> {
        self.repr(&beck_core::ty::Ty::html())?;
        self.heap
            .html_lists()
            .ok_or_else(|| "a view node in a module with no view in it".to_string())
    }

    /// One view node or attribute: four words, a tag, a name for the two shapes that have one, and
    /// a deferred value for the four that have one. The LLVM emitter's `node`, twice over.
    #[allow(clippy::too_many_arguments)] // a tag, two optional words, and the three the emitter threads
    fn node(
        &mut self,
        ty: Repr,
        tag: u64,
        name: Option<&Val>,
        deferred: Option<&Val>,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        if let Some(v) = deferred {
            Heap::crossing(v.ty)
                .map_err(|why| format!("puts {why} in a page, and a page is read by the host"))?;
        }
        self.view_lists()?;
        let off = self.alloc(heap::NODE_WORDS * heap::WORD, span, b, m);
        let word = b.ins().iconst(types::I64, tag as i64);
        self.store_word(off, 0, word, b, m);
        match name {
            Some(v) => self.store_field(off, 1, v, b, m),
            None => {
                let z = b.ins().iconst(types::I64, 0);
                self.store_word(off, 1, z, b, m);
            }
        }
        // The unused words are written rather than left as they were found: the whole used arena
        // goes back down the pipe, and a word nobody reads is still a byte two runs would differ in.
        match deferred {
            Some(v) => {
                let at = self.heap.word_of(v.ty);
                let shape = b.ins().iconst(types::I64, i64::from(at));
                self.store_word(off, heap::DEFERRED, shape, b, m);
                self.store_field(off, heap::DEFERRED + 1, v, b, m);
            }
            None => {
                let z = b.ins().iconst(types::I64, 0);
                self.store_word(off, heap::DEFERRED, z, b, m);
                self.store_word(off, heap::DEFERRED + 1, z, b, m);
            }
        }
        Ok(Val { v: off, ty })
    }

    /// Insist an argument is a closure of the shape this primitive applies it at.
    fn function_arg(&mut self, v: &Val, op: Prim, want: &[Repr]) -> Result<u32, String> {
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

    /// Call one of the generated loops.
    ///
    /// The arguments are in the order [`loop_function`] writes its signatures in: the list, the
    /// accumulator when there is one, the closure, the flag when there is one, then the span. Both
    /// halves are here and in that function, so a change to one is an object the linker refuses
    /// rather than a wrong answer.
    fn list_loop(
        &mut self,
        which: Loop,
        fam: u32,
        args: &[IrValue],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<IrValue, String> {
        self.applied.insert(fam);
        self.loops.insert((which, fam));
        let family = self.heap.family(fam).clone();
        let ptr = m.target_config().pointer_type();
        let mut sig =
            cranelift_codegen::ir::Signature::new(CallConv::triple_default(m.isa().triple()));
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(types::I64));
        if which == Loop::Fold {
            sig.params.push(AbiParam::new(machine(family.ret)));
        }
        sig.params.push(AbiParam::new(types::I64));
        if which == Loop::Every {
            sig.params.push(AbiParam::new(types::I8));
        }
        sig.params.push(AbiParam::new(types::I32));
        sig.returns.push(AbiParam::new(match which {
            Loop::Map | Loop::Filter | Loop::Sort => types::I64,
            Loop::Fold => machine(family.ret),
            Loop::Every => types::I8,
        }));
        let id = m
            .declare_function(&which.symbol(fam), Linkage::Local, &sig)
            .map_err(|e| format!("declaring a list loop: {e}"))?;
        let f = m.declare_func_in_func(id, b.func);
        let idx = self.span(span);
        let sp = b.ins().iconst(types::I32, i64::from(idx));
        let err = self.err();
        // The list, then whatever this loop takes between it and the span. `args` is the primitive's
        // own operands in source order — the list, the closure, and the initial accumulator or the
        // flag — and this is where they become the order the signature has.
        let mut operands = vec![err, args[0]];
        match which {
            Loop::Fold => {
                operands.push(args[1]);
                operands.push(args[2]);
            }
            Loop::Every => {
                operands.push(args[1]);
                operands.push(args[2]);
            }
            _ => operands.push(args[1]),
        }
        operands.push(sp);
        let call = b.ins().call(f, &operands);
        let r = b.inst_results(call)[0];
        self.check_call(b);
        Ok(r)
    }

    /// A literal, as the value that carries it.
    ///
    /// A string literal is the one that is not an `iconst` of itself: it is an offset into the pool
    /// the host wrote at the front of the request's heap, decided when the module was emitted. See
    /// [`beck_llvm::heap`] for why it cannot be allocated where it is written and cannot be a
    /// global either.
    fn constant(&mut self, k: &Const, b: &mut FunctionBuilder<'_>) -> Result<Val, String> {
        match k {
            Const::Int(i) => Ok(Val {
                v: b.ins().iconst(types::I64, *i),
                ty: Repr::Int,
            }),
            Const::Bool(x) => Ok(Val {
                v: b.ins().iconst(types::I8, i64::from(*x)),
                ty: Repr::Bool,
            }),
            Const::Float(f) => Ok(Val {
                v: b.ins().f64const(*f),
                ty: Repr::Float,
            }),
            Const::Str(s) => {
                let at = self.heap.intern(s);
                let offset = self.heap.string_offset(at);
                Ok(Val {
                    v: b.ins().iconst(types::I64, offset as i64),
                    ty: Repr::Str,
                })
            }
            Const::Unit => Err("the unit value, which has no machine representation here".into()),
        }
    }

    /// `str_index_of` — a byte search, a byte-to-character conversion, and an `Option`.
    ///
    /// **No branch.** `Some(value=i)` is two words and `None()` is one, so allocating the larger
    /// and choosing the tag with a `select` answers both: the host reads a variant's own fields and
    /// nothing else, so the word a `None` leaves behind is never looked at.
    ///
    /// The tags are read off the layout. `Option`'s variants sort to `None` then `Some`, which is a
    /// fact about two strings and not one to write down twice.
    fn index_of(
        &mut self,
        ty: &beck_core::ty::Ty,
        hay: IrValue,
        needle: IrValue,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let text = self.text();
        let f = m.declare_func_in_func(text.find, b.func);
        let call = b.ins().call(f, &[hay, needle]);
        let found = b.inst_results(call)[0];
        let missing = b.ins().icmp_imm_s(IntCC::SignedLessThan, found, 0);
        // Clamped before the conversion rather than after it: `-1` is not a byte offset, and a walk
        // that started there would read off the front of the string.
        let zero = b.ins().iconst(types::I64, 0);
        let safe = b.ins().select(missing, zero, found);
        let f = m.declare_func_in_func(text.charat, b.func);
        let call = b.ins().call(f, &[hay, safe]);
        let index = b.inst_results(call)[0];
        // `-1` when it is missing, so the shape `some_or_none` answers is the shape here too.
        let absent = b.ins().iconst(types::I64, -1);
        let answer = b.ins().select(missing, absent, index);
        self.some_or_none(ty, answer, span, b, m)
    }

    /// `[a, b, c]` — one allocation, filled left to right.
    ///
    /// Left to right because an element expression can trap and *which* trap the caller sees is
    /// part of what the evaluator answers, which is the same reason a record's fields are.
    fn list_lit(
        &mut self,
        ty: &beck_core::ty::Ty,
        xs: &[Core],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let repr = self
            .repr(ty)
            .map_err(|why| format!("builds a value that is {why}"))?;
        let Repr::List(at) = repr else {
            return Err(format!("builds a `{ty}`, which is not a list"));
        };
        let element = self.heap.element(at);
        let mut vals = Vec::with_capacity(xs.len());
        for x in xs {
            let v = self.value(x, b, m)?;
            if v.ty != element {
                return Err(format!("an element of this `{ty}` is the wrong type"));
            }
            vals.push(v);
        }
        // The block and then the header, in that order, because the header holds the block's
        // offset — the same depth-first rule a record's fields follow.
        let count = xs.len() as u64;
        let data = self.alloc(heap::DATA_HEADER + count * heap::WORD, span, b, m);
        let n = b.ins().iconst(types::I64, count as i64);
        self.store_word(data, 0, n, b, m);
        self.store_word(data, 1, n, b, m);
        for (i, v) in vals.iter().enumerate() {
            self.store_field(data, i + 2, v, b, m);
        }
        let off = self.alloc(heap::LIST_HEADER, span, b, m);
        self.store_word(off, 0, n, b, m);
        self.store_word(off, 1, data, b, m);
        Ok(Val { v: off, ty: repr })
    }

    fn builds(&self) -> Builds {
        self.runtime
            .builds
            .expect("a body that builds text is a module with that runtime")
    }

    /// A string literal's offset in the pool, interned on the spot.
    fn literal(&mut self, s: &str) -> u64 {
        let at = self.heap.intern(s);
        self.heap.string_offset(at)
    }

    /// `try: block` — run the block under a handler, and reify one failure as a `Result[T, E]`.
    ///
    /// The LLVM emitter's `try_`, in blocks rather than labels, and the argument for every decision
    /// in it is written there: the block is emitted **inline** so that its own calls are under the
    /// handler, and for a **value** so that no tail call can walk through one.
    fn try_(
        &mut self,
        args: &[Core],
        ty: &beck_core::ty::Ty,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
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
        let repr = self
            .repr(ty)
            .map_err(|why| format!("catches into a value that is {why}"))?;
        let Repr::Obj(at) = repr else {
            return Err(format!("catches into `{ty}`, which is not an object"));
        };
        let (ok, err_tag, layout) = {
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
        let (err_slot, err_ty) = layout.variants[err_tag as usize]
            .slot("error")
            .ok_or_else(|| format!("`{}`'s `Err` has no `error`", layout.shown))?;
        let bytes = layout
            .variants
            .iter()
            .map(beck_llvm::Variant::bytes)
            .max()
            .unwrap_or(heap::WORD);

        let handler = b.create_block();
        let join = b.create_block();
        b.append_block_param(join, types::I64);

        self.handlers.push(handler);
        let value = self.expr(body, Dest::Value, b, m);
        self.handlers.pop();
        let value = value?.expect("a block emitted for a value produces one");
        if value.ty != ok_ty {
            return Err(format!(
                "`try` over a block answering {} where `{}` carries {}",
                self.heap.show(value.ty),
                layout.shown,
                self.heap.show(ok_ty)
            ));
        }
        let good = self.alloc(bytes, span, b, m);
        let tag = b.ins().iconst(types::I64, i64::from(ok));
        self.store_word(good, 0, tag, b, m);
        self.store_field(good, ok_slot, &value, b, m);
        b.ins().jump(join, &[good.into()]);

        // The handler. Two tests and no search: is this failure a raise at all, and is it the one
        // this `try:` names. Anything else leaves for the *enclosing* handler with the cell
        // untouched.
        b.switch_to_block(handler);
        b.seal_block(handler);
        let flags = MemFlagsData::trusted();
        let cell = self.err();
        let code = b.ins().load(types::I32, flags, cell, 0);
        let named = b.create_block();
        let away = b.create_block();
        let is_raise = b
            .ins()
            .icmp_imm_s(IntCC::Equal, code, i64::from(Trap::Raised.code()));
        b.ins().brif(is_raise, named, &[], away, &[]);

        b.switch_to_block(named);
        b.seal_block(named);
        let got = b.ins().load(types::I64, flags, cell, CELL_RAISED);
        let want = self.literal(name);
        let caught_here = b.create_block();
        let mine = b.ins().icmp_imm_s(IntCC::Equal, got, want as i64);
        b.ins().brif(mine, caught_here, &[], away, &[]);

        b.switch_to_block(away);
        b.seal_block(away);
        self.escape(b);

        b.switch_to_block(caught_here);
        b.seal_block(caught_here);
        // Handled: the whole first word is cleared — the code *and* the span — because the worker's
        // loop reads it as one `i64` to decide whether the call answered, so a caught failure that
        // left the span behind would come back looking like a trap.
        let zero = b.ins().iconst(types::I64, 0);
        b.ins().store(flags, zero, cell, 0);
        let pair = b.ins().load(types::I64, flags, cell, CELL_PAYLOAD);
        let held = self.load_field(pair, 1, err_ty, b, m);
        let bad = self.alloc(bytes, span, b, m);
        let tag = b.ins().iconst(types::I64, i64::from(err_tag));
        self.store_word(bad, 0, tag, b, m);
        self.store_field(bad, err_slot, &held, b, m);
        b.ins().jump(join, &[bad.into()]);

        b.switch_to_block(join);
        b.seal_block(join);
        Ok(Val {
            v: b.block_params(join)[0],
            ty: repr,
        })
    }

    /// One of [`Builds`]'s functions: the error cell, the arguments, the span.
    fn build_call(
        &mut self,
        id: FuncId,
        args: &[IrValue],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Val {
        let idx = self.span(span);
        let span = b.ins().iconst(types::I32, i64::from(idx));
        let mut operands = vec![self.err()];
        operands.extend_from_slice(args);
        operands.push(span);
        let f = m.declare_func_in_func(id, b.func);
        let call = b.ins().call(f, &operands);
        let v = b.inst_results(call)[0];
        self.check_call(b);
        Val { v, ty: Repr::Str }
    }

    /// The `Some` tag, the slot its payload is in, and what that payload is.
    ///
    /// For *consuming* an `Option`, where [`Body::option_of`] is for answering with one. The
    /// evaluator reads these by **name**, so this does too.
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

    /// Insist an argument is a map, and say which map it is.
    fn map_arg(&self, v: &Val, op: Prim) -> Result<u32, String> {
        match v.ty {
            Repr::Map(at) => Ok(at),
            _ => Err(format!("`{}` on something that is not a Map", op.name())),
        }
    }

    /// `map_get` — an `Option[V]` from the index a search answered, and **no branch**.
    ///
    /// [`Body::list_get`]'s trick with the value's address: the values start `n` words after the
    /// keys, so entry `i`'s value is word `1 + n + i`. A miss loads the header instead, which is
    /// always there, and the `None` tag means nobody reads it.
    fn map_get(
        &mut self,
        ty: &beck_core::ty::Ty,
        map: &Val,
        found: IrValue,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let Repr::Map(at) = map.ty else {
            return Err("`map_get` on something that is not a Map".into());
        };
        let (_, v) = self.heap.entry(at);
        let value = self.heap.element(v);
        let (option, some, none, slot, bytes) = self.option_of(ty, value)?;
        let maps = self.maps();
        let arena = self.arena();
        let _ = map;
        // The search answers a node or `0`, and reading the value word of node `0` reads the
        // arena's first bytes rather than past its end — `list_get`'s trick, one type over.
        let inside = b.ins().icmp_imm_s(IntCC::NotEqual, found, 0);
        let w = maps.field(arena, found, heap::NODE_VALUE, b, m);

        let out = self.alloc(bytes, span, b, m);
        let some = b.ins().iconst(types::I64, i64::from(some));
        let none = b.ins().iconst(types::I64, i64::from(none));
        let tag = b.ins().select(inside, some, none);
        self.store_word(out, 0, tag, b, m);
        self.store_word(out, slot, w, b, m);
        Ok(Val { v: out, ty: option })
    }

    /// `map_keys` and `map_values`: one run of the data area copied into a fresh list.
    fn map_run(
        &mut self,
        op: Prim,
        ty: &beck_core::ty::Ty,
        map: &Val,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let repr = self
            .repr(ty)
            .map_err(|why| format!("answers with a value that is {why}"))?;
        if !matches!(repr, Repr::List(_)) {
            return Err(format!(
                "`{}` answers with `{ty}`, which is not a list",
                op.name()
            ));
        }
        let maps = self.maps();
        // Which word of a node to take: the key or the value, which is the only thing the two
        // walks differ by.
        let which = if op == Prim::MapKeys {
            heap::NODE_KEY
        } else {
            heap::NODE_VALUE
        };
        let slot = b.ins().iconst(types::I64, which as i64);
        let idx = self.span(span);
        let span = b.ins().iconst(types::I32, i64::from(idx));
        let err = self.err();
        let f = m.declare_func_in_func(maps.run, b.func);
        let call = b.ins().call(f, &[err, map.v, slot, span]);
        let r = b.inst_results(call)[0];
        self.check_call(b);
        Ok(Val { v: r, ty: repr })
    }

    /// `{}` — and only `{}`. See `beck_llvm::emit`'s `map_lit` for why.
    fn map_lit(
        &mut self,
        ty: &beck_core::ty::Ty,
        kvs: &[(Core, Core)],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        _m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let repr = self
            .repr(ty)
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
        // An empty map is the offset `0`, which is the one offset no live object has — so `{}`
        // allocates nothing at all.
        let _ = span;
        let off = b.ins().iconst(types::I64, 0);
        Ok(Val { v: off, ty: repr })
    }

    fn maps(&self) -> Maps {
        self.runtime
            .maps
            .expect("a body with a map in it is a module with the map runtime")
    }

    /// Insist an argument is a list, and say which list it is.
    fn list_arg(&self, v: &Val, op: Prim) -> Result<u32, String> {
        match v.ty {
            Repr::List(at) => Ok(at),
            _ => Err(format!("`{}` on something that is not a list", op.name())),
        }
    }

    /// `list_get(xs, i)` — an `Option[T]`, and **no branch**.
    ///
    /// The trick is the address rather than the value: an index outside the list would read a word
    /// that may be past the end of the arena, so the address is a `select` between the element's
    /// and the list's own **header**, which is always there. Out of bounds therefore loads the
    /// length, stores it where `Some`'s payload would go, and tags the answer `None` — and the host
    /// reads a variant's own fields and nothing else, so that word is never looked at.
    fn list_get(
        &mut self,
        ty: &beck_core::ty::Ty,
        xs: &Val,
        index: &Val,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let Repr::List(at) = xs.ty else {
            return Err("`list_get` on something that is not a list".into());
        };
        let element = self.heap.element(at);
        let (option, some, none, slot, bytes) = self.option_of(ty, element)?;
        let lists = self.lists();
        let arena = self.arena();
        let n = lists.count(arena, xs.v, b, m);
        let low = b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, index.v, 0);
        let high = b.ins().icmp(IntCC::SignedLessThan, index.v, n);
        let inside = b.ins().band(low, high);
        let zero = b.ins().iconst(types::I64, 0);
        let safe = b.ins().select(inside, index.v, zero);
        let data = lists.data(arena, xs.v, b, m);
        let off = b.ins().imul_imm_s(safe, heap::WORD as i64);
        let element_at = b.ins().iadd(data, off);
        let base = arena.base(b, m);
        let header = b.ins().iadd(base, xs.v);
        let p = b.ins().select(inside, element_at, header);
        let w = b.ins().load(types::I64, MemFlagsData::trusted(), p, 0);

        let cell = self.alloc(bytes, span, b, m);
        let some = b.ins().iconst(types::I64, i64::from(some));
        let none = b.ins().iconst(types::I64, i64::from(none));
        let tag = b.ins().select(inside, some, none);
        self.store_word(cell, 0, tag, b, m);
        self.store_word(cell, slot, w, b, m);
        Ok(Val {
            v: cell,
            ty: option,
        })
    }

    /// `list_slice`, `list_take` and `list_drop`: one clamped range, one copy.
    ///
    /// Clamped exactly where the evaluator clamps — a negative start or count is zero, and a range
    /// past the end stops at the end — so the three differ only in the arithmetic above the copy.
    fn list_range(
        &mut self,
        op: Prim,
        vals: &[Val],
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let lists = self.lists();
        let arena = self.arena();
        let n = lists.count(arena, vals[0].v, b, m);
        let zero = b.ins().iconst(types::I64, 0);
        let floor = |b: &mut FunctionBuilder<'_>, v: IrValue| {
            let neg = b.ins().icmp_imm_s(IntCC::SignedLessThan, v, 0);
            b.ins().select(neg, zero, v)
        };
        let (from, count) = match op {
            Prim::ListSlice => (floor(b, vals[1].v), floor(b, vals[2].v)),
            Prim::ListTake => (zero, floor(b, vals[1].v)),
            _ => (floor(b, vals[1].v), n),
        };
        let over = b.ins().icmp(IntCC::UnsignedGreaterThan, from, n);
        let start = b.ins().select(over, n, from);
        let left = b.ins().isub(n, start);
        let too_many = b.ins().icmp(IntCC::UnsignedGreaterThan, count, left);
        let take = b.ins().select(too_many, left, count);

        let idx = self.span(span);
        let span = b.ins().iconst(types::I32, i64::from(idx));
        let err = self.err();
        let f = m.declare_func_in_func(lists.copy, b.func);
        let call = b.ins().call(f, &[err, vals[0].v, start, take, span]);
        let r = b.inst_results(call)[0];
        self.check_call(b);
        Ok(Val {
            v: r,
            ty: vals[0].ty,
        })
    }

    /// `Some(value = i)` when `found` is not `-1`, and `None()` when it is.
    fn some_or_none(
        &mut self,
        ty: &beck_core::ty::Ty,
        found: IrValue,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        let (option, some, none, slot, bytes) = self.option_of(ty, Repr::Int)?;
        let missing = b.ins().icmp_imm_s(IntCC::SignedLessThan, found, 0);
        let cell = self.alloc(bytes, span, b, m);
        let some = b.ins().iconst(types::I64, i64::from(some));
        let none = b.ins().iconst(types::I64, i64::from(none));
        let tag = b.ins().select(missing, none, some);
        self.store_word(cell, 0, tag, b, m);
        self.store_word(cell, slot, found, b, m);
        Ok(Val {
            v: cell,
            ty: option,
        })
    }

    /// The layout of the `Option[T]` a primitive answers with: the repr, its two tags, which word
    /// `Some`'s payload goes in, and how much to allocate for either.
    fn option_of(
        &mut self,
        ty: &beck_core::ty::Ty,
        want: Repr,
    ) -> Result<(Repr, u32, u32, usize, u64), String> {
        let repr = self
            .repr(ty)
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

    /// Insist an argument is text, so a message names the primitive rather than the operand.
    fn text_arg(&self, v: &Val, op: Prim) -> Result<(), String> {
        if v.ty == Repr::Str {
            Ok(())
        } else {
            Err(format!("`{}` on something that is not a Str", op.name()))
        }
    }

    /// One of [`Text`]'s allocating functions: the error cell, the arguments, the span.
    fn text_call(
        &mut self,
        which: Which,
        args: &[IrValue],
        ty: Repr,
        span: Span,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Val {
        let text = self.text();
        let idx = self.span(span);
        let span = b.ins().iconst(types::I32, i64::from(idx));
        let mut operands = vec![self.err()];
        operands.extend_from_slice(args);
        operands.push(span);
        let f = m.declare_func_in_func(text.id(which), b.func);
        let call = b.ins().call(f, &operands);
        let v = b.inst_results(call)[0];
        // Allocating means it can exhaust the arena, and a caller that ignored that would carry a
        // `0` offset into the next load.
        self.check_call(b);
        Val { v, ty }
    }

    /// `contains`, `starts_with` and `ends_with`, which are one search and two length tests.
    fn text_search(
        &mut self,
        op: Prim,
        hay: IrValue,
        needle: IrValue,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Val {
        let text = self.text();
        let arena = self.arena();
        if op == Prim::StrContains {
            let f = m.declare_func_in_func(text.find, b.func);
            let call = b.ins().call(f, &[hay, needle]);
            let at = b.inst_results(call)[0];
            return Val {
                v: b.ins().icmp_imm_s(IntCC::SignedGreaterThanOrEqual, at, 0),
                ty: Repr::Bool,
            };
        }
        let lh = text.header(arena, hay, 0, b, m);
        let ln = text.header(arena, needle, 0, b, m);
        let fits = b.ins().icmp(IntCC::UnsignedLessThanOrEqual, ln, lh);
        // The comparison runs either way and its operands are clamped to a length that fits, so
        // there is no branch: `memcmp` of zero bytes is zero, and a needle longer than the haystack
        // is refused by `fits` rather than by not being looked at.
        let zero = b.ins().iconst(types::I64, 0);
        let start = if op == Prim::StrStartsWith {
            zero
        } else {
            let d = b.ins().isub(lh, ln);
            b.ins().select(fits, d, zero)
        };
        let n = b.ins().select(fits, ln, zero);
        let ph = text.data(arena, hay, b, m);
        let at = b.ins().iadd(ph, start);
        let pn = text.data(arena, needle, b, m);
        let f = m.declare_func_in_func(text.memcmp, b.func);
        let call = b.ins().call(f, &[at, pn, n]);
        let c = b.inst_results(call)[0];
        let same = b.ins().icmp_imm_s(IntCC::Equal, c, 0);
        Val {
            v: b.ins().band(fits, same),
            ty: Repr::Bool,
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
        Val { v, ty: Repr::Int }
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
        Val { v, ty: Repr::Int }
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
    /// (`docs/93` §93.3).
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

    fn compare(
        &mut self,
        op: Prim,
        a: &Val,
        c: &Val,
        b: &mut FunctionBuilder<'_>,
        m: &mut ObjectModule,
    ) -> Result<Val, String> {
        // Reals compare through the order key and Bools compare unsigned, so `false < true`. Both
        // are the ordering `Value`'s derived `Ord` gives, which is the one the evaluator uses.
        // An object compares through the function `compare_function` defines for its layout, which
        // answers -1, 0 or 1 — so the six operators are one call and one integer test.
        let (lhs, rhs, signed) = match a.ty.order() {
            heap::Order::Key => (self.order_key(a, b), self.order_key(c, b), false),
            heap::Order::Words { signed } => (a.v, c.v, signed),
            // A reference decides through the three-way comparison for whatever it refers to, and
            // `Repr::order` is the only place that names one — see its own documentation for the
            // three times a `_` arm swallowed a reference kind instead.
            // Nothing to compare with: `Repr::order` names the reason and this is where a program
            // that asked hears it.
            heap::Order::Absent(why) => {
                return Err(format!("compares {}, which is {why}", self.heap.show(a.ty)))
            }
            heap::Order::Call(symbol) => {
                self.wants(a.ty)
                    .map_err(|why| format!("compares {}, which is {why}", self.heap.show(a.ty)))?;
                let sig = compare_signature(m);
                let id = m
                    .declare_function(&symbol, Linkage::Local, &sig)
                    .map_err(|e| format!("declaring a comparison: {e}"))?;
                let f = m.declare_func_in_func(id, b.func);
                let call = b.ins().call(f, &[a.v, c.v]);
                let r = b.inst_results(call)[0];
                let zero = b.ins().iconst(types::I64, 0);
                (r, zero, true)
            }
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
        Ok(Val {
            v: b.ins().icmp(cc, lhs, rhs),
            ty: Repr::Bool,
        })
    }

    /// The value as an `I64`, which is how it crosses the worker's protocol and how a trap carries
    /// the scrutinee that matched nothing.
    fn widen(&mut self, v: &Val, b: &mut FunctionBuilder<'_>) -> IrValue {
        match v.ty {
            Repr::Int => v.v,
            // Normalised, because the one thing that reads this is a message: a scrutinee printed
            // as `-0` where the evaluator prints `0` is a divergence in the differential.
            Repr::Float => {
                let n = self.normalise(v.v, b);
                b.ins().bitcast(types::I64, MemFlagsData::new(), n)
            }
            Repr::Bool => b.ins().uextend(types::I64, v.v),
            // Its offset, which is what a reference *is* here.
            Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => v.v,
        }
    }
}

/// Why a primitive this backend does not compile is not compiled.
///
/// The string half is spelled out one at a time rather than swept into "not a scalar primitive",
/// because since `docs/93` a `Str` *is* a value here and "text is not on this heap" would be
/// false. The wording is this emitter's own — `cranelift.rs` holds the two to refusing the same
/// **set** of definitions and not to saying the same words about them.
fn refusal(op: Prim) -> String {
    let why = match op {
        // The one that is a decision rather than a gap. `docs/46` §46.14 and `docs/93` §93.15 both
        // name shipping this as the mistake: the tree-walker pushes in place when `liveness` proves
        // the accumulator is a last use, and an arena with no ownership in it cannot.
        Prim::ListZip => "answers with a list of pairs, and there is no pair type to lay out",
        // The same rule `list_append` gets, one type over: the evaluator's `PMap` shares everything
        // it did not touch and rebuilds one path, and a sorted run in an arena has to copy all of
        // it.
        // The higher-order half compiles, `sort_by` included — so what is left of it is the one
        // that grows a list.
        Prim::ListFlatMap => {
            "answers a list whose length is the sum of the lists its function answers, which is \
             growing a list under another name"
        }
        // `str_upper`, `str_lower`, `str_to_int` and `str_replace` were here, refused for being a
        // table and somebody else's parser. They are the runtime library's now
        // (`beck_llvm::prim`), which is what those two sentences were describing without naming.
        Prim::JsonParse => {
            "answers a `Json`, whose object variant is a `Map` this module lays out — so the \
             library would have to build a balanced tree in a shape only the emitter knows"
        }
        Prim::JsonRender => {
            "reads a `Json`, and what a value of a declared type looks like in the arena is this \
             module's layout rather than the library's"
        }
        _ => return format!("`{}` is not one of the scalar primitives", op.name()),
    };
    format!("`{}` {why}", op.name())
}

// -------------------------------------------------------------------------------------------
// Asking the host
// -------------------------------------------------------------------------------------------

/// `beck.host`: write a question, block, and take the answer back.
///
/// The second direction of [`beck_llvm::Worker`]'s protocol, written a second time for this
/// module's stated reason — the two emitters are held to agreeing, and one implementation shared
/// between them would make the agreement true by construction. What is *not* written twice is the
/// frame: [`beck_llvm::Upcall`] is the codes and the field order, because that is a contract with
/// the host as much as with the other backend.
///
/// The signature is `(op, span, name, words, buf, copy, err) -> i64`, and the three that are not
/// obvious:
///
/// * `name` is the literal-pool offset of the error type a failed answer raises, written into the
///   cell by *this* function rather than by the host — a `try:` compares an interned offset, and
///   only the module knows which one.
/// * `words` is how many words of `buf` to send: the shapes and the argument words the caller has
///   already stored.
/// * `copy` says whether the arena travels, which it does exactly when an argument can point into
///   it.
#[derive(Clone, Copy, Debug)]
struct Host {
    call: FuncId,
}

impl Host {
    fn declare(m: &mut ObjectModule, ptr: Type) -> Result<Host, String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut sig = cranelift_codegen::ir::Signature::new(conv);
        for p in [types::I32, types::I32] {
            sig.params.push(AbiParam::new(p));
        }
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(ptr));
        sig.returns.push(AbiParam::new(types::I64));
        let call = m
            .declare_function("beck.host", Linkage::Local, &sig)
            .map_err(|e| format!("declaring the host call: {e}"))?;
        Ok(Host { call })
    }

    /// The pipe's two movers and `exit`, declared with the signatures [`driver`] gives them.
    ///
    /// Declared twice on purpose: `cranelift_module` answers with the same [`FuncId`] for a second
    /// declaration of one name at one signature, so this and the driver name the same functions
    /// without either having to be built first.
    fn pipe(m: &mut ObjectModule, ptr: Type) -> Result<(FuncId, FuncId, FuncId), String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut movesig = cranelift_codegen::ir::Signature::new(conv);
        movesig.params.push(AbiParam::new(ptr));
        movesig.params.push(AbiParam::new(types::I64));
        movesig.returns.push(AbiParam::new(types::I64));
        let read = m
            .declare_function("beck.read_exact", Linkage::Local, &movesig)
            .map_err(|e| format!("declaring the reader: {e}"))?;
        let write = m
            .declare_function("beck.write_all", Linkage::Local, &movesig)
            .map_err(|e| format!("declaring the writer: {e}"))?;
        let mut exitsig = cranelift_codegen::ir::Signature::new(conv);
        exitsig.params.push(AbiParam::new(types::I32));
        let exit = m
            .declare_function("exit", Linkage::Import, &exitsig)
            .map_err(|e| format!("declaring `exit`: {e}"))?;
        Ok((read, write, exit))
    }

    fn define(
        self,
        arena: Arena,
        m: &mut ObjectModule,
        ctx: &mut cranelift_codegen::Context,
        fctx: &mut FunctionBuilderContext,
        ptr: Type,
    ) -> Result<(), String> {
        let (read, write, exit) = Host::pipe(m, ptr)?;
        let conv = CallConv::triple_default(m.isa().triple());
        let mut sig = cranelift_codegen::ir::Signature::new(conv);
        for p in [types::I32, types::I32] {
            sig.params.push(AbiParam::new(p));
        }
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(ptr));
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(ptr));
        sig.returns.push(AbiParam::new(types::I64));

        ctx.func = Function::with_name_signature(UserFuncName::user(9, 0), sig);
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            b.seal_block(entry);
            let flags = MemFlagsData::trusted();
            let op = b.block_params(entry)[0];
            let span = b.block_params(entry)[1];
            let name = b.block_params(entry)[2];
            let words = b.block_params(entry)[3];
            let buf = b.block_params(entry)[4];
            let copy = b.block_params(entry)[5];
            let err = b.block_params(entry)[6];

            let frame = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                FRAME_BYTES,
                3,
            ));
            let frame = b.ins().stack_addr(ptr, frame, 0);
            let base = arena.base(&mut b, m);
            let next_at = arena.addr(arena.next, &mut b, m);
            let used = b.ins().load(types::I64, flags, next_at, 0);

            let marker = b.ins().iconst(types::I32, i64::from(Upcall::MARKER));
            b.ins().store(flags, marker, frame, 0);
            b.ins().store(flags, span, frame, CELL_SPAN);
            let wide = b.ins().uextend(types::I64, op);
            b.ins().store(flags, wide, frame, CELL_PAYLOAD);
            b.ins().store(flags, used, frame, FRAME_VALUE);
            let sends = b.ins().icmp_imm_s(IntCC::NotEqual, copy, 0);
            let none = b.ins().iconst(types::I64, 0);
            let blen = b.ins().select(sends, used, none);
            b.ins().store(flags, blen, frame, FRAME_BYTES_AT);

            let writer = m.declare_func_in_func(write, b.func);
            let reader = m.declare_func_in_func(read, b.func);
            let thirty_two = b.ins().iconst(types::I64, i64::from(FRAME_BYTES));
            b.ins().call(writer, &[frame, thirty_two]);
            let wbytes = b.ins().imul_imm_s(words, heap::WORD as i64);
            b.ins().call(writer, &[buf, wbytes]);
            b.ins().call(writer, &[base, blen]);

            // The answer, into the same frame. A short read is the host gone: nothing to compute
            // and nothing to reply to, so the worker stops and the host's own next read is what
            // reports it.
            let call = b.ins().call(reader, &[frame, thirty_two]);
            let got = b.inst_results(call)[0];
            let heard = b.ins().icmp(IntCC::Equal, got, thirty_two);
            let (gone, answered) = (b.create_block(), b.create_block());
            b.ins().brif(heard, answered, &[], gone, &[]);

            b.switch_to_block(gone);
            let quit = m.declare_func_in_func(exit, b.func);
            let one = b.ins().iconst(types::I32, 1);
            b.ins().call(quit, &[one]);
            b.ins()
                .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

            b.switch_to_block(answered);
            b.seal_block(answered);
            let code = b.ins().load(types::I32, flags, frame, 0);
            let tail = b.ins().load(types::I64, flags, frame, FRAME_BYTES_AT);
            let end = b.ins().iadd(used, tail);
            let limit_at = arena.addr(arena.limit, &mut b, m);
            let limit = b.ins().load(types::I64, flags, limit_at, 0);
            let over = b.ins().icmp(IntCC::UnsignedGreaterThan, end, limit);
            let (full, room) = (b.create_block(), b.create_block());
            b.ins().brif(over, full, &[], room, &[]);

            // An answer that does not fit is the arena being full, which is the allocator's own
            // trap rather than a new one.
            b.switch_to_block(full);
            b.seal_block(full);
            let exhausted = b
                .ins()
                .iconst(types::I32, i64::from(Trap::HeapExhausted.code()));
            b.ins().store(flags, exhausted, err, 0);
            b.ins().store(flags, span, err, CELL_SPAN);
            let z = b.ins().iconst(types::I64, 0);
            b.ins().store(flags, z, err, CELL_PAYLOAD);
            b.ins().return_(&[z]);

            b.switch_to_block(room);
            b.seal_block(room);
            let at = b.ins().iadd(base, used);
            let call = b.ins().call(reader, &[at, tail]);
            let read_back = b.inst_results(call)[0];
            let whole = b.ins().icmp(IntCC::Equal, read_back, tail);
            let kept = b.create_block();
            b.ins().brif(whole, kept, &[], gone, &[]);
            b.seal_block(gone);

            b.switch_to_block(kept);
            b.seal_block(kept);
            b.ins().store(flags, end, next_at, 0);
            let fine = b.ins().icmp_imm_s(IntCC::Equal, code, 0);
            let (value, failed) = (b.create_block(), b.create_block());
            b.ins().brif(fine, value, &[], failed, &[]);

            b.switch_to_block(failed);
            b.seal_block(failed);
            b.ins().store(flags, code, err, 0);
            b.ins().store(flags, span, err, CELL_SPAN);
            let payload = b.ins().load(types::I64, flags, frame, CELL_PAYLOAD);
            b.ins().store(flags, payload, err, CELL_PAYLOAD);
            b.ins().store(flags, name, err, CELL_RAISED);
            let z = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[z]);

            b.switch_to_block(value);
            b.seal_block(value);
            let answer = b.ins().load(types::I64, flags, frame, FRAME_VALUE);
            b.ins().return_(&[answer]);
            b.finalize(m.target_config());
        }
        m.define_function(self.call, ctx)
            .map_err(|e| format!("defining the host call: {e}"))?;
        m.clear_context(ctx);
        Ok(())
    }
}

/// A protocol frame: five fields in 32 bytes, in either direction.
const FRAME_BYTES: u32 = 32;
/// The fourth field — the answer's word going one way, the arena's mark going the other.
const FRAME_VALUE: i32 = 16;
/// The fifth — how many bytes of heap follow.
const FRAME_BYTES_AT: i32 = 24;

/// How many words a question's buffer holds, which is [`beck_llvm::emit`]'s `QUESTION_WORDS`.
const QUESTION_WORDS: u32 = 8;

/// The runtime library's one entry point, as an **imported** symbol.
///
/// The difference from [`Host`] is the linkage and it is the whole point: `beck.host` is a
/// function this module defines, and `beck_prim` is a function in an archive the link step puts on
/// the line. [`beck_llvm::prim`] is where the ABI is written down, and `beck_prim::abi` is the
/// other side of it.
#[derive(Clone, Copy, Debug)]
struct Linked {
    call: FuncId,
    /// The arena's reservation, which the library owns for a program that links it.
    arena: FuncId,
}

impl Linked {
    fn declare(m: &mut ObjectModule, ptr: Type) -> Result<Linked, String> {
        let conv = CallConv::triple_default(m.isa().triple());
        let mut sig = cranelift_codegen::ir::Signature::new(conv);
        sig.params.push(AbiParam::new(types::I32));
        for _ in 0..4 {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.returns.push(AbiParam::new(types::I64));
        let call = m
            .declare_function(prim::CALL, Linkage::Import, &sig)
            .map_err(|e| format!("declaring the runtime library: {e}"))?;
        let mut reserve = cranelift_codegen::ir::Signature::new(conv);
        reserve.params.push(AbiParam::new(types::I64));
        reserve.returns.push(AbiParam::new(ptr));
        let arena = m
            .declare_function(prim::ARENA, Linkage::Import, &reserve)
            .map_err(|e| format!("declaring the runtime library's arena: {e}"))?;
        Ok(Linked { call, arena })
    }
}

/// Whether any definition of this program calls one of the runtime library's primitives.
///
/// [`asks_the_host`]'s walk, over the other list, and for the same reason: this backend declares
/// its runtime up front, and an imported symbol nothing calls is an undefined reference in every
/// object file with a heap in it.
fn calls_the_runtime_library(program: &Program, eligible: &BTreeSet<Arc<str>>) -> bool {
    fn calls(c: &Core) -> bool {
        if let CoreKind::Prim { op, .. } = &c.kind {
            if prim::op_of(*op).is_some() {
                return true;
            }
        }
        beck_core::core::children(c).into_iter().any(calls)
    }
    eligible
        .iter()
        .filter_map(|name| program.defs.get(name))
        .any(|def| calls(&def.body))
}

/// Whether any definition of this program reaches one of the four host primitives.
///
/// Asked before anything is declared, because this backend declares its runtime up front and a
/// `beck.host` nothing calls would be a function in every object file with a heap in it. The walk
/// is [`beck_core::core::children`]'s rather than a hand-written match over `CoreKind`, for the
/// reason that function exists: a variant that gains a child would make a hand-written one
/// silently incomplete.
fn asks_the_host(program: &Program, eligible: &BTreeSet<Arc<str>>) -> bool {
    fn asks(c: &Core) -> bool {
        if let CoreKind::Prim { op, .. } = &c.kind {
            if Upcall::of(*op).is_some() {
                return true;
            }
        }
        beck_core::core::children(c).into_iter().any(asks)
    }
    eligible
        .iter()
        .filter_map(|name| program.defs.get(name))
        .any(|def| asks(&def.body))
}

// -------------------------------------------------------------------------------------------
// The worker: the same protocol, emitted rather than written
// -------------------------------------------------------------------------------------------

/// The thunks, the dispatch table and the loop that reads a call and answers it.
///
/// The protocol is [`beck_llvm::Worker`]'s, to the byte, because the host is the same host: eight
/// bytes of header, eight per argument, and a 24-byte reply of trap code, span index, payload and
/// result. Two spellings of one wire would be the drift this workspace spends its gates on.
#[allow(clippy::too_many_arguments)]
fn driver(
    m: &mut ObjectModule,
    ctx: &mut cranelift_codegen::Context,
    fctx: &mut FunctionBuilderContext,
    functions: &[Signature],
    ids: &BTreeMap<Arc<str>, FuncId>,
    order: &[Arc<str>],
    arena: Option<Arena>,
    linked: Option<Linked>,
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
            // Only the thunk knows what the function returns, so this is where the worker learns
            // whether this call's answer is on the heap.
            if let Some(arena) = arena {
                let at = arena.addr(arena.reply, &mut b, m);
                let on = b.ins().iconst(types::I64, i64::from(sig.ret.is_ref()));
                b.ins().store(flags, on, at, 0);
            }
            let mut operands = vec![err];
            for (k, p) in sig.params.iter().enumerate() {
                let raw = b.ins().load(types::I64, flags, args, (k * 8) as i32);
                operands.push(match p.machine() {
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
            let bits = match sig.ret.machine() {
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

    // `malloc`, and only when there is an arena to reserve.
    // The runtime library owns the heap of a program that links it, because that is what lets
    // every call into it carry offsets rather than a pointer (`beck_prim::arena`). A program that
    // links none of it reserves its arena the way it always did.
    let malloc = match (arena, linked) {
        (Some(_), Some(linked)) => Some(linked.arena),
        (Some(_), None) => {
            let mut sig = cranelift_codegen::ir::Signature::new(conv);
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(ptr));
            Some(
                m.declare_function("malloc", Linkage::Import, &sig)
                    .map_err(|e| format!("declaring `malloc`: {e}"))?,
            )
        }
        (None, _) => None,
    };

    ctx.func = Function::with_name_signature(UserFuncName::user(4, 0), main_sig);
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, fctx);
        let entry = b.create_block();
        b.switch_to_block(entry);
        let slot = |b: &mut FunctionBuilder<'_>, bytes: u32| {
            b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3))
        };
        let req = slot(&mut b, 16);
        let argbuf = slot(&mut b, (MAX_PARAMS * 8) as u32);
        let cell = slot(&mut b, 24);
        let resp = slot(&mut b, 32);
        let req = b.ins().stack_addr(ptr, req, 0);
        let argbuf = b.ins().stack_addr(ptr, argbuf, 0);
        let cell = b.ins().stack_addr(ptr, cell, 0);
        let resp = b.ins().stack_addr(ptr, resp, 0);

        if let (Some(arena), Some(malloc)) = (arena, malloc) {
            let f = m.declare_func_in_func(malloc, b.func);
            let want = b.ins().iconst(types::I64, heap::ARENA_BYTES as i64);
            let call = b.ins().call(f, &[want]);
            let got = b.inst_results(call)[0];
            let at = arena.addr(arena.heap, &mut b, m);
            b.ins().store(flags, got, at, 0);
            // A `malloc` that failed leaves the limit at zero, so the first allocation traps with
            // a message instead of the first *store* faulting with a signal.
            let null = b.ins().iconst(ptr, 0);
            let failed = b.ins().icmp(IntCC::Equal, got, null);
            let none = b.ins().iconst(types::I64, 0);
            let all = b.ins().iconst(types::I64, heap::ARENA_BYTES as i64);
            let cap = b.ins().select(failed, none, all);
            let at = arena.addr(arena.limit, &mut b, m);
            b.ins().store(flags, cap, at, 0);
        }

        let loop_ = b.create_block();
        let done = b.create_block();
        b.ins().jump(loop_, &[]);
        b.seal_block(entry);

        b.switch_to_block(loop_);
        let rd = m.declare_func_in_func(read_exact, b.func);
        let sixteen = b.ins().iconst(types::I64, 16);
        let call = b.ins().call(rd, &[req, sixteen]);
        let head = b.inst_results(call)[0];
        let closed = b.ins().icmp_imm_s(IntCC::NotEqual, head, 16);
        let sized = b.create_block();
        b.ins().brif(closed, done, &[], sized, &[]);

        b.switch_to_block(sized);
        b.seal_block(sized);
        let idx = b.ins().load(types::I32, flags, req, 0);
        let count = b.ins().load(types::I32, flags, req, 4);
        let count = b.ins().uextend(types::I64, count);
        let blob = b.ins().load(types::I64, flags, req, 8);
        let bytes = b.ins().imul_imm_s(count, 8);
        let call = b.ins().call(rd, &[argbuf, bytes]);
        let got = b.inst_results(call)[0];
        let short = b.ins().icmp(IntCC::NotEqual, got, bytes);
        let run = b.create_block();

        if let Some(arena) = arena {
            let accept = b.create_block();
            b.ins().brif(short, done, &[], accept, &[]);
            b.switch_to_block(accept);
            b.seal_block(accept);
            // A blob bigger than the arena is a host that disagrees with this module about the
            // protocol, which is a bug rather than an input: it closes rather than writing past
            // the end.
            let at = arena.addr(arena.limit, &mut b, m);
            let limit = b.ins().load(types::I64, flags, at, 0);
            let huge = b.ins().icmp(IntCC::UnsignedGreaterThan, blob, limit);
            let copy = b.create_block();
            b.ins().brif(huge, done, &[], copy, &[]);

            b.switch_to_block(copy);
            b.seal_block(copy);
            let base = arena.base(&mut b, m);
            let call = b.ins().call(rd, &[base, blob]);
            let read = b.inst_results(call)[0];
            let truncated = b.ins().icmp(IntCC::NotEqual, read, blob);
            b.ins().brif(truncated, done, &[], run, &[]);

            b.switch_to_block(run);
            b.seal_block(run);
            // The arena is reset to just past whatever the arguments brought with them.
            let first = b.ins().iconst(types::I64, heap::FIRST as i64);
            let small = b.ins().icmp(IntCC::UnsignedLessThan, blob, first);
            let start = b.ins().select(small, first, blob);
            let at = arena.addr(arena.next, &mut b, m);
            b.ins().store(flags, start, at, 0);
            let at = arena.addr(arena.reply, &mut b, m);
            let no = b.ins().iconst(types::I64, 0);
            b.ins().store(flags, no, at, 0);
        } else {
            b.ins().brif(short, done, &[], run, &[]);
            b.switch_to_block(run);
            b.seal_block(run);
        }

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

        let send = match arena {
            None => b.ins().iconst(types::I64, 0),
            Some(arena) => {
                let ok = b.ins().icmp_imm_s(IntCC::Equal, code, 0);
                let at = arena.addr(arena.reply, &mut b, m);
                let wants = b.ins().load(types::I64, flags, at, 0);
                let on_heap = b.ins().icmp_imm_s(IntCC::NotEqual, wants, 0);
                let answered = b.ins().band(ok, on_heap);
                // A raise is the one failure whose arena travels: the value it carried is in there,
                // and the host builds the evaluator's own message out of it rather than out of the
                // fact that there was one. The code is the cell's low half.
                let what = b.ins().ireduce(types::I32, code);
                let raised = b
                    .ins()
                    .icmp_imm_s(IntCC::Equal, what, i64::from(Trap::Raised.code()));
                let both = b.ins().bor(answered, raised);
                let at = arena.addr(arena.next, &mut b, m);
                let used = b.ins().load(types::I64, flags, at, 0);
                let none = b.ins().iconst(types::I64, 0);
                b.ins().select(both, used, none)
            }
        };
        b.ins().store(flags, send, resp, 24);

        let wr = m.declare_func_in_func(write_all, b.func);
        let thirty2 = b.ins().iconst(types::I64, 32);
        let call = b.ins().call(wr, &[resp, thirty2]);
        let wrote = b.inst_results(call)[0];
        let gone = b.ins().icmp_imm_s(IntCC::NotEqual, wrote, 32);
        let round = b.create_block();

        if let Some(arena) = arena {
            let carry = b.create_block();
            b.ins().brif(gone, done, &[], carry, &[]);
            b.switch_to_block(carry);
            b.seal_block(carry);
            let base = arena.base(&mut b, m);
            let call = b.ins().call(wr, &[base, send]);
            let pushed = b.inst_results(call)[0];
            let stalled = b.ins().icmp(IntCC::NotEqual, pushed, send);
            b.ins().brif(stalled, done, &[], round, &[]);
        } else {
            b.ins().brif(gone, done, &[], round, &[]);
        }

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
