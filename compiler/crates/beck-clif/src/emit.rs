//! `Core` → Cranelift IR → an object file.
//!
//! # The same subset, a second time
//!
//! This compiles exactly what [`beck_llvm::emit`] compiles: a definition whose parameters and
//! result each have a [`beck_llvm::heap::Repr`] — `Int`, `Float`, `Bool`, or a `model`, `union` or
//! `newtype` — and whose body is built from constants, variables, `let`, `if`, `match`, direct
//! calls, record and variant construction, field reads, `with`, and the arithmetic, comparison and
//! logical primitives. Text, a collection, a closure and every effect are refused by name, with a
//! reason, exactly as they are there.
//!
//! Writing the selection a second time rather than importing it is deliberate. The two emitters
//! are held to *agreeing* — `cranelift.rs` asserts that they accept and refuse the same
//! definitions over every program in the tree — and a shared implementation would make that
//! agreement true by construction and therefore worth nothing. What *is* shared is the vocabulary
//! and the wire: [`beck_llvm::Scalar`], [`beck_llvm::Signature`], [`beck_llvm::Refusal`] and
//! [`beck_llvm::Trap`] are types, [`beck_llvm::Trap`]'s codes are a protocol the host decodes, and
//! [`beck_llvm::heap`] is the **layout** — which word a field is in, which rank a variant has —
//! because that one is a contract with the host too, and a contract with three spellings drifts.
//! A second copy of any of those would be two opinions about one thing.
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
use beck_llvm::{Refusal, Scalar, Signature, Trap, MAX_PARAMS};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Function, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Type,
    UserFuncName, Value as IrValue,
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
    let runtime = Runtime { arena, text };

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

    if let Some(arena) = arena {
        arena
            .define(&mut object, &mut ctx, &mut fctx, ptr)
            .map_err(Failure::Fatal)?;
        if let Some(text) = text {
            text.define(arena, &mut object, &mut ctx, &mut fctx, ptr)
                .map_err(Failure::Fatal)?;
        }
        for at in closure_of(&compared, heap) {
            compare_function(at, heap, arena, text, &mut object, &mut ctx, &mut fctx)
                .map_err(Failure::Fatal)?;
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
            Ok(r) => params.push(r),
            Err(why) => return Err(format!("parameter `{name}` is {why}")),
        }
    }
    let ret = heap
        .repr(&def.ret, program)
        .map_err(|why| format!("returns {why}"))?;
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

/// What a module carries besides its own definitions: the arena, and text's runtime.
///
/// One value rather than two parameters, because every body is handed both or neither and the two
/// are decided together — a module with text in it is a module with an arena by construction.
#[derive(Clone, Copy, Debug)]
struct Runtime {
    arena: Option<Arena>,
    text: Option<Text>,
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
    memcmp: FuncId,
}

/// Which of [`Text`]'s allocating functions a call site wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Concat,
    Slice,
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
            memcmp,
        })
    }

    fn id(self, which: Which) -> FuncId {
        match which {
            Which::Concat => self.concat,
            Which::Slice => self.slice,
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
fn closure_of(asked: &BTreeSet<u32>, heap: &Heap) -> BTreeSet<u32> {
    let mut out: BTreeSet<u32> = BTreeSet::new();
    let mut todo: Vec<u32> = asked.iter().copied().collect();
    while let Some(at) = todo.pop() {
        if !out.insert(at) {
            continue;
        }
        for v in &heap.layout(at).variants {
            for (_, r) in &v.fields {
                if let Repr::Obj(inner) = r {
                    todo.push(*inner);
                }
            }
        }
    }
    out
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
    text: Option<Text>,
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
                match repr {
                    // A field that is itself a reference decides through the three-way comparison
                    // for whatever it refers to: a layout's own, or text's one. Comparing the
                    // *offsets* would answer that two equal strings differ whenever they were
                    // allocated at different places, which is almost always.
                    Repr::Obj(_) | Repr::Str => {
                        let inner_id = match repr {
                            Repr::Obj(inner) => {
                                let inner_sig = compare_signature(m);
                                m.declare_function(
                                    &format!("beck.cmp.{inner}"),
                                    Linkage::Local,
                                    &inner_sig,
                                )
                                .map_err(|e| format!("declaring a comparison: {e}"))?
                            }
                            _ => {
                                text.ok_or("a layout with a `Str` field in a module with no text")?
                                    .cmp
                            }
                        };
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
                    _ => {
                        // A real compares through its order key, and both are already normalised —
                        // `Body::store_field` is where that is paid for. An `Int` is signed, and a
                        // `Bool` is a 0 or a 1 and therefore either way round.
                        let (ka, kb) = match repr {
                            Repr::Float => (order_key_bits(xa, &mut b), order_key_bits(xb, &mut b)),
                            _ => (xa, xb),
                        };
                        let (lt, gt) = if matches!(repr, Repr::Int) {
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
            CoreKind::Prim { op, args } => self.prim(*op, args, c.span, b, m)?,
            CoreKind::Lam { .. } => {
                return Err(
                    "a nested function is a closure, and a closure is not on this heap".into(),
                )
            }
            CoreKind::Global(name) => {
                return Err(format!(
                    "`{name}` is used as a value rather than called, and a function value is a \
                     closure"
                ))
            }
            CoreKind::Make {
                variant, fields, ..
            } => self.make(&c.ty, variant.as_deref(), fields, c.span, b, m)?,
            CoreKind::Field { base, name } => self.field(base, name, b, m)?,
            CoreKind::With { base, fields } => self.with(base, fields, c.span, b, m)?,
            CoreKind::ListLit(_) => {
                return Err("builds a list, and a collection is not on this heap yet".into())
            }
            CoreKind::MapLit(_) => {
                return Err("builds a map, and a collection is not on this heap yet".into())
            }
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
            Repr::Str | Repr::Obj(_) => Trap::NoMatchData,
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
            Pattern::List { .. } => {
                Err("matches a list pattern, and a collection is not on this heap yet".into())
            }
        }
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
            Repr::Int | Repr::Str | Repr::Obj(_) => b.ins().load(types::I64, flags, at, 0),
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
            Repr::Int | Repr::Str | Repr::Obj(_) => v.v,
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

    /// A direct call of a named definition — and in tail position, a jump.
    ///
    /// `return_call` rather than a call and a return: Cranelift's verifier *requires* the frame to
    /// be discardable and refuses the function otherwise, which is the same guarantee `musttail`
    /// gives the other backend. `docs/27` §27.2 says 1,500 and 60,000 tail calls spend the same
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
                    Repr::Bool | Repr::Str | Repr::Obj(_) => {
                        Err(format!("`{}` on a value that is not a number", op.name()))
                    }
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
                    Repr::Bool | Repr::Str | Repr::Obj(_) => {
                        Err("`negate` on a value that is not a number".into())
                    }
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
                    Repr::Bool | Repr::Str | Repr::Obj(_) => {
                        Err("`abs` on a value that is not a number".into())
                    }
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
            Prim::StrContains | Prim::StrStartsWith | Prim::StrEndsWith => {
                arity(2, &vals)?;
                self.text_arg(&vals[0], op)?;
                self.text_arg(&vals[1], op)?;
                Ok(self.text_search(op, vals[0].v, vals[1].v, b, m))
            }
            other => Err(refusal(other)),
        }
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
        let (lhs, rhs, signed) = match a.ty {
            // **Unsigned**, which is the whole point of the key: the transform maps every real
            // onto the unsigned order, so a signed comparison here answers `-1.0 < 0.0` with
            // `false`. That was this backend's first bug, and the differential is what said so.
            Repr::Float => (self.order_key(a, b), self.order_key(c, b), false),
            Repr::Int => (a.v, c.v, true),
            Repr::Bool => (a.v, c.v, false),
            // Text compares as its bytes, which is what `Text`'s `Ord` does and is the same
            // order as its characters: UTF-8 sorts code points and bytes the same way.
            Repr::Str => {
                let f = m.declare_func_in_func(self.text().cmp, b.func);
                let call = b.ins().call(f, &[a.v, c.v]);
                let r = b.inst_results(call)[0];
                let zero = b.ins().iconst(types::I64, 0);
                (r, zero, true)
            }
            Repr::Obj(at) => {
                self.compared.insert(at);
                let sig = compare_signature(m);
                let id = m
                    .declare_function(&format!("beck.cmp.{at}"), Linkage::Local, &sig)
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
            Repr::Str | Repr::Obj(_) => v.v,
        }
    }
}

/// Why a primitive this backend does not compile is not compiled.
///
/// The string half is spelled out one at a time rather than swept into "not a scalar primitive",
/// because since `docs/104` a `Str` *is* a value here and "text is not on this heap" would be
/// false. The wording is this emitter's own — `cranelift.rs` holds the two to refusing the same
/// **set** of definitions and not to saying the same words about them.
fn refusal(op: Prim) -> String {
    let why = match op {
        Prim::StrSplit | Prim::StrChars => {
            "answers with a list, and a collection is not on this heap yet"
        }
        Prim::StrJoin => "reads a list, and a collection is not on this heap yet",
        Prim::StrIndexOf => {
            "answers with an `Option`, whose layout this backend resolves from a program's own \
             types and not from the prelude's"
        }
        Prim::StrUpper | Prim::StrLower => {
            "is Unicode case mapping, which is a table rather than an operation — and a compiled \
             half-answer that folded ASCII only would disagree with the evaluator on the first \
             letter that is not"
        }
        Prim::StrTrim => {
            "trims Unicode whitespace, which is a table for the same reason case mapping is"
        }
        Prim::StrReplace | Prim::StrRepeat => {
            "builds text whose size is not a function of its arguments' sizes, and this arena \
             cannot grow an allocation it has already made"
        }
        Prim::ToStr | Prim::StrToInt => {
            "converts between text and a number, and the rendering has to be Rust's to the digit"
        }
        _ => return format!("`{}` is not one of the scalar primitives", op.name()),
    };
    format!("`{}` {why}", op.name())
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
    arena: Option<Arena>,
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
    let malloc = if arena.is_some() {
        let mut sig = cranelift_codegen::ir::Signature::new(conv);
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(ptr));
        Some(
            m.declare_function("malloc", Linkage::Import, &sig)
                .map_err(|e| format!("declaring `malloc`: {e}"))?,
        )
    } else {
        None
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
                let both = b.ins().band(ok, on_heap);
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
