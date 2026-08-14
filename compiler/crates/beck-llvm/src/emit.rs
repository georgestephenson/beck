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
//! A definition whose parameters and result each have a [`crate::heap::Repr`] — `Int`, `Float`,
//! `Bool`, a `Str`, a `list`, a `Map`, an `Html`, an `Attr`, or a `model`, `union` or `newtype`
//! that [`crate::heap`] can lay out — and whose body is built from constants, variables, `let`,
//! `if`, `match`, direct calls to other compiled definitions, record and variant construction,
//! field reads, `with`, lambdas and applications, and the arithmetic, comparison, logical, text,
//! collection and view primitives.
//!
//! What is refused is refused **by name, with the reason**, in [`crate::Report`]: the signal
//! vocabulary, a bounded definition, and the handful of primitives that read a Unicode table or
//! grow a collection whose size needs a counting pass. Nothing else on this list is still here —
//! failure compiles on the error cell that was already an unwinder (`docs/93`), growing a list and
//! a map compile (`docs/93`, `docs/93`), and the four primitives that have to **ask the host**
//! compile by asking it ([`Upcall`], `docs/93`). Nothing is silently approximated: a definition
//! either compiles to machine code that agrees with the evaluator on every input, or it does not
//! compile.
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
//!   monotone transform of the bits so the derived `Ord` is the numeric one (`docs/27` §27.8), and
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
//! `Function::normalise` has the whole story, and `docs/93` §93.3 is where it is written down.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use beck_core::check::{Def, Program};
use beck_core::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use beck_core::ty::Ty;
use beck_diag::Span;

use crate::heap::{self, Heap, Repr};

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
}

/// What a compiled function looks like from the outside.
#[derive(Clone, Debug)]
pub struct Signature {
    pub name: Arc<str>,
    pub params: Vec<Repr>,
    pub ret: Repr,
    /// Which entry of the worker's dispatch table calls it.
    pub index: u32,
}

impl Signature {
    /// Whether calling this needs the heap on the wire in either direction.
    pub fn touches_heap(&self) -> bool {
        self.ret.is_ref() || self.params.iter().any(|p| p.is_ref())
    }
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
    /// A `match` on a record or a union matched nothing. Unreachable for the same reason the three
    /// above it are — the checker proves a `match` exhaustive — and here for the same reason: an
    /// LLVM `unreachable` would let the optimiser do anything at all with the path a wrong
    /// exhaustiveness check reached.
    NoMatchData,
    /// The arena is full. [`crate::heap::ARENA_BYTES`] is how much there is, and this is the one
    /// failure a compiled program has that the evaluator does not.
    HeapExhausted,
    /// A closure carried a rank no arm of its family answers to. Unreachable for a stronger reason
    /// than [`Trap::NoMatchData`]'s: a closure object is built by this module and by nothing else,
    /// so its rank came from the same table the switch was written from. It is here rather than an
    /// `unreachable` because a wrong rank should be a message naming this trap and not an
    /// optimiser's licence to delete the path that produced it.
    NoSuchLambda,
    /// A `raise` nothing caught. **The one code that is not a fault**: it is the program failing
    /// the way its own type says it can, and the value it failed with travels with it.
    ///
    /// The payload is the offset of a two-word pair — the raised value's shape and its word, which
    /// is [`crate::heap::Repr::Html`]'s deferred value one subsystem over — and the arena travels
    /// with the reply, which is the one place the protocol treats a failure like an answer. What
    /// the host does with it is build the evaluator's own message out of the decoded value; what a
    /// compiled `try:` does with it is compare the type name in the error cell's third word.
    Raised,
    /// The host could not answer a question the program asked ([`Upcall`]).
    ///
    /// Not a failure of the program and not one of the machine: it is this compiler failing to
    /// turn the arena's bytes into a value, or the value back into bytes. The payload says
    /// nothing, because the *reason* is a sentence and a trap carries a number —
    /// [`crate::Artifact`] keeps the sentence beside the call and substitutes it for this trap's
    /// message, which is why the message below reads like a placeholder.
    HostFailed,
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
            Trap::NoMatchData => 11,
            Trap::HeapExhausted => 12,
            Trap::NoSuchLambda => 13,
            Trap::Raised => 14,
            Trap::HostFailed => 15,
        }
    }

    pub fn from_code(code: u32) -> Option<Trap> {
        const ALL: [Trap; 15] = [
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
            Trap::NoMatchData,
            Trap::HeapExhausted,
            Trap::NoSuchLambda,
            Trap::Raised,
            Trap::HostFailed,
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
            // Without the value, because what is in the payload is an offset into a heap this
            // process cannot see. It is the one trap message that is not the evaluator's word for
            // word, and it is on the path the checker proves unreachable.
            Trap::NoMatchData => "no match arm applies to this value".into(),
            Trap::HeapExhausted => format!(
                "the compiled program used all {} MiB of its heap",
                crate::heap::ARENA_BYTES >> 20
            ),
            Trap::NoSuchLambda => format!("no lambda of this module has rank {payload}"),
            // Never seen by a reader: `Native::exchange` decodes the raised value and builds the
            // evaluator's own `raised \`…\`` out of it, because a message about a failure the
            // program declared should say what was raised rather than that something was.
            Trap::Raised => format!(
                "a raised value at offset {payload}, which the host did not \
                                     decode"
            ),
            Trap::HostFailed => "the host could not answer a call the program made".into(),
        }
    }
}

/// A question the compiled program cannot answer, asked of the host in the middle of a call.
///
/// # Why the protocol needs a second direction at all
///
/// Everything else this backend compiles is a computation: the host asks, the worker answers, and
/// the pipe carries one message each way. These four are not computations. `uuid()` and `now()`
/// are `nondet`, `secret_env` is `env`, `http_fetch` is `net.out(host)` — each is a question whose
/// answer is outside the program, and no amount of machine code produces one.
///
/// So the worker asks back. Between reading a call and answering it, it may write a **question**
/// frame and block until the answer arrives, any number of times. The host services each against
/// [`beck_core::host::Atoms`] — the same trait the tree-walker's `Host` extends, so the two
/// backends cannot disagree about what the host said.
///
/// # The frame
///
/// A question is told from an answer by its first word, which is [`Upcall::MARKER`] and is not any
/// [`Trap::code`]. The rest of the 32-byte header is the same five fields the reply header has,
/// carrying different things:
///
/// | Bytes | In a question | In the host's answer |
/// |---|---|---|
/// | 0..4 | [`Upcall::MARKER`] | `0` for a value, else a [`Trap`] code |
/// | 4..8 | the span index that asked | unused |
/// | 8..16 | [`Upcall::code`] | the trap payload, or a raise's pair offset |
/// | 16..24 | the arena's high-water mark | the answer's word |
/// | 24..32 | how many arena bytes follow | how many bytes follow, to append at the mark |
///
/// After a question's header come `1 + 2 × arity` words: the [`crate::Heap`] shape the answer is
/// expected to have, and then a shape and a word for each argument. The shapes are what let the
/// host decode and encode without a second table of what each primitive's types are — the same
/// trick [`crate::heap::Heap`] plays for a view's deferred leaves.
///
/// The answer's bytes are a **tail**, appended at the mark, never a whole arena: the host may add
/// to what the worker allocated and may not rewrite it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upcall {
    Now,
    NewUuid,
    SecretEnv,
    HttpFetch,
}

impl Upcall {
    /// The frame code that says "this is a question, not an answer".
    ///
    /// `u32::MAX` rather than the next number after [`Trap::Raised`], so that a host reading a
    /// frame from a worker built before this existed — or from one whose error cell held a code
    /// this version does not know — cannot mistake a trap for a question.
    pub const MARKER: u32 = u32::MAX;

    /// The primitive this is, or `None` for one that is computed rather than asked.
    pub fn of(op: Prim) -> Option<Upcall> {
        match op {
            Prim::Now => Some(Upcall::Now),
            Prim::NewUuid => Some(Upcall::NewUuid),
            Prim::SecretEnv => Some(Upcall::SecretEnv),
            Prim::HttpFetch => Some(Upcall::HttpFetch),
            _ => None,
        }
    }

    pub fn code(self) -> u32 {
        match self {
            Upcall::Now => 1,
            Upcall::NewUuid => 2,
            Upcall::SecretEnv => 3,
            Upcall::HttpFetch => 4,
        }
    }

    pub fn from_code(code: u32) -> Option<Upcall> {
        const ALL: [Upcall; 4] = [
            Upcall::Now,
            Upcall::NewUuid,
            Upcall::SecretEnv,
            Upcall::HttpFetch,
        ];
        ALL.into_iter().find(|u| u.code() == code)
    }

    /// How many arguments the frame carries, which is how the host knows where the words end.
    ///
    /// A property of the primitive rather than a field of the frame: an arity the host read out of
    /// the frame would be an arity the host could not check.
    pub fn arity(self) -> usize {
        match self {
            Upcall::Now | Upcall::NewUuid => 0,
            Upcall::SecretEnv => 1,
            Upcall::HttpFetch => 2,
        }
    }

    /// Whether an argument can point into the arena, and the arena therefore has to travel.
    ///
    /// The two `nondet` atoms take nothing, so their question is 32 bytes and a word however much
    /// the program has allocated. The other two are handed text and a record, and the host cannot
    /// read either without the bytes they point into.
    pub fn carries_arena(self) -> bool {
        self.arity() > 0
    }

    /// The error type a failed answer raises, for the handler in the compiled code to match on.
    ///
    /// The **worker** supplies this rather than the host, because which type a primitive can raise
    /// is a fact about the program and the name has to be the one `try:` compares against — an
    /// interned literal's offset, not a string the host wrote into the arena.
    pub fn raises(self) -> Option<&'static str> {
        match self {
            Upcall::HttpFetch => Some("HttpError"),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Upcall::Now => "now",
            Upcall::NewUuid => "uuid",
            Upcall::SecretEnv => "secret_env",
            Upcall::HttpFetch => "http_fetch",
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
    /// What every object in this module looks like — and therefore how the host writes one and
    /// reads one back. [`crate::heap`] is why this is one table rather than three.
    pub heap: Heap,
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
    // Specialised first, so everything below sees one definition per instantiation and never a type
    // parameter. [`crate::mono`] is why this is a backend pass and what it refuses; a program with
    // no generic definition in it comes back as itself.
    let mono = crate::mono::specialise(program);
    let program = &mono.program;
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

    // Round two, to a fixed point: emit each body, and drop whichever ones will not emit. A body
    // that calls a definition dropped in an earlier round fails in a later one, which is what
    // makes mutual recursion work — the pair survives together or is refused together.
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
    let mut compared: BTreeSet<u32> = BTreeSet::new();
    let mut list_compared: BTreeSet<u32> = BTreeSet::new();
    let mut map_compared: BTreeSet<u32> = BTreeSet::new();
    let mut compared_fns = false;
    let mut lambdas: BTreeMap<u32, String> = BTreeMap::new();
    let mut applied: BTreeSet<u32> = BTreeSet::new();
    let mut loops: BTreeSet<(Loop, u32)> = BTreeSet::new();
    let mut asks = false;
    for name in &order {
        let def = &program.defs[name];
        let mut fun = Function::new(&indexed, &eligible, program, &mut heap);
        fun.spans = std::mem::take(&mut spans);
        let text = fun
            .emit(def)
            .expect("the fixed point already proved this emits");
        spans = std::mem::take(&mut fun.spans);
        compared.append(&mut fun.compared);
        list_compared.append(&mut fun.list_compared);
        map_compared.append(&mut fun.map_compared);
        applied.append(&mut fun.applied);
        loops.append(&mut fun.loops);
        compared_fns |= fun.compared_fns;
        asks |= fun.asks;
        for (rank, lam) in std::mem::take(&mut fun.lambdas) {
            lambdas.entry(rank).or_insert(lam);
        }
        bodies.push_str(&text);
        bodies.push('\n');
    }
    // In rank order rather than in the order the bodies met them, for the reason the definitions
    // are in declaration order: the IR is the same bytes twice.
    for lam in lambdas.values() {
        bodies.push_str(lam);
        bodies.push('\n');
    }

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    let (layouts, elements, entries) = reachable(&compared, &list_compared, &map_compared, &heap);
    let ir = assemble(
        &bodies,
        &functions,
        &heap,
        &layouts,
        &elements,
        &entries,
        asks,
        &Closures {
            applied: &applied,
            emitted: &lambdas.keys().copied().collect(),
            compiled: &eligible,
            compared: compared_fns,
            loops: &loops,
        },
    );
    refusals.sort_by(|a, b| a.name.cmp(&b.name));
    Module {
        ir,
        functions,
        spans,
        refusals,
        heap,
    }
}

/// Every layout that needs a comparison function, given the ones a body asked to compare.
///
/// Transitive, because comparing a record compares its fields: asking for `Line` asks for `Point`
/// whether or not any body compares two points.
/// Every layout, word comparison and map this module has to generate a comparison for.
///
/// One fixed point over all three, because they reach each other: a record with a `Map[Str, Point]`
/// field needs that map's comparison, which needs `Str`'s and `Point`'s, and a `list[Point]` needs
/// `Point`'s. Three separate closures would each be right about its own edges and wrong about the
/// others'.
///
/// The **elements** set is what [`element_functions`] is generated for, and it is a superset of the
/// list reprs: a map's key and value are entries in the same table (see [`heap::Heap::entry`]), so
/// a `Map[Str, Int]` asks for `Str`'s word comparison without there being a `list[Str]` anywhere.
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

/// The signature, or the reason there is not one.
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
        // Assigned once the survivors are known — an index into a table that does not exist yet
        // would be a number that means nothing.
        index: u32::MAX,
    })
}

/// An SSA value: the LLVM operand, and what it is.
///
/// An object is an `i64` holding its offset into the arena, so `ty` is what the *language* says it
/// is and [`Repr::machine`] is what LLVM sees.
#[derive(Clone, Debug)]
struct Val {
    text: String,
    ty: Repr,
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
    program: &'a Program,
    heap: &'a mut Heap,
    /// Emitted blocks, complete with their terminators.
    out: String,
    /// The block being written.
    body: String,
    label: String,
    next: u32,
    env: BTreeMap<VarId, Val>,
    spans: Vec<Span>,
    /// What this function returns, which is what its trap exit has to return too.
    ret: Repr,
    /// Whether anything branched to the trap exit, so an unused block is not emitted.
    trapped: bool,
    /// The layouts this body compares two of, so the module emits a comparison for them.
    compared: BTreeSet<u32>,
    /// The list reprs this body compares two of, so the module generates their functions.
    list_compared: BTreeSet<u32>,
    /// And the map reprs.
    map_compared: BTreeSet<u32>,
    /// Whether this body compares two closures, which needs one function for the whole module.
    compared_fns: bool,
    /// The lambdas this body built, by rank, each already emitted as its own function.
    ///
    /// Collected upwards rather than written where they are met, because a `lam` is an expression
    /// *inside* a function and its body is a function of its own: LLVM has no nested definitions,
    /// so what a closure compiles to is one object here and one `define` beside the module's.
    lambdas: BTreeMap<u32, String>,
    /// The closure families this body applies, so the module writes an application for those and
    /// not for every shape a type mentions.
    applied: BTreeSet<u32>,
    /// The higher-order list primitives this body reaches, by shape, so the module writes those
    /// loops and no others.
    loops: BTreeSet<(Loop, u32)>,
    /// Whether this body reaches the arena, and therefore needs its base in a register.
    uses_heap: bool,
    /// Whether this body asks the host anything, and therefore needs a question buffer.
    ///
    /// One buffer per *function* rather than one per call: a question is answered before the next
    /// one is asked, and an `alloca` at the call site would grow the stack once per iteration of a
    /// loop that calls `now()`.
    asks: bool,
    /// Where a failure inside the block being emitted goes, innermost last.
    ///
    /// Empty means the function's own trap exit. A `try:` pushes a label while its block is
    /// emitted, so every check a call makes and every trap a primitive stores lands in the handler
    /// rather than leaving the function — which is the whole of what makes a handler lexical
    /// ([`docs/38`](../../../../../docs/38-literature-survey.md) §38.4): there is no dynamic search
    /// for who handles what, because the label is decided where the block is written.
    handlers: Vec<String>,
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
            out: String::new(),
            body: String::new(),
            label: String::new(),
            next: 0,
            env: BTreeMap::new(),
            spans: Vec::new(),
            ret: Repr::Int,
            trapped: false,
            compared: BTreeSet::new(),
            list_compared: BTreeSet::new(),
            map_compared: BTreeSet::new(),
            compared_fns: false,
            lambdas: BTreeMap::new(),
            applied: BTreeSet::new(),
            loops: BTreeSet::new(),
            uses_heap: false,
            asks: false,
            handlers: Vec::new(),
        }
    }

    /// What `ty` looks like at the machine, or the reason this body cannot be compiled.
    fn repr(&mut self, ty: &Ty) -> Result<Repr, String> {
        self.heap.repr(ty, self.program)
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
        // The arena's base, hoisted to the top of the entry block: it is written once before any
        // compiled code runs, so one load dominates every use and none of the loads in between
        // have to be proved redundant.
        if self.uses_heap {
            let at = self.out.find('\n').map_or(0, |i| i + 1);
            self.out
                .insert_str(at, "  %hp = load ptr, ptr @\"beck.heap\"\n");
        }
        if self.asks {
            let at = self.out.find('\n').map_or(0, |i| i + 1);
            self.out
                .insert_str(at, &format!("  %q = alloca [{QUESTION_WORDS} x i64]\n"));
        }
        text.push_str(&self.out);
        if self.trapped {
            let _ = write!(
                text,
                "trap:\n  ret {} {}\n",
                sig.ret.llvm(),
                sig.ret.machine().zero()
            );
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

    /// Where a failure goes from here: the innermost `try:`'s handler, or the function's exit.
    ///
    /// One method rather than the literal `trap` at three sites, because a handler that only some
    /// of them honoured would catch a raise and miss an overflow — or worse, leave a function
    /// through a block a `try:` had already decided to protect.
    fn escape(&mut self) -> String {
        match self.handlers.last() {
            Some(label) => label.clone(),
            None => {
                self.trapped = true;
                "trap".to_string()
            }
        }
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
        let out = self.escape();
        self.terminate(format!("br label %{out}"));

        self.start(cont);
    }

    /// Leave for the exit block if a callee trapped.
    fn check_call(&mut self) {
        let code = self.fresh();
        self.line(format!("{code} = load i32, ptr %err"));
        let ok = self.fresh();
        self.line(format!("{ok} = icmp eq i32 {code}, 0"));
        let cont = self.label("call.ok");
        let out = self.escape();
        self.terminate(format!("br i1 {ok}, label %{cont}, label %{out}"));
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
    /// `Dest::Return` is not an optimisation: `docs/27-the-walls-come-down-report.md` makes "a call in tail
    /// position is free" a property of the *language*, so a backend that spent a frame on one
    /// would be a backend on which a Beck loop overflows the stack. It is threaded through `if`,
    /// `let` and `match` rather than pattern-matched at the top of a body because tail position
    /// travels through all three: the interesting call is almost never the outermost node.
    fn expr(&mut self, c: &Core, dest: Dest) -> Result<Option<Val>, String> {
        let value = match &c.kind {
            CoreKind::Const(k) => self.constant(k)?,
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
            CoreKind::Prim { op, args } => self.prim(*op, args, &c.ty, c.span)?,
            CoreKind::Lam { params, body } => self.closure(params, body, &c.ty, c.span)?,
            CoreKind::Global(name) => self.named(name, &c.ty, c.span)?,
            CoreKind::Make {
                variant, fields, ..
            } => self.make(&c.ty, variant.as_deref(), fields, c.span)?,
            CoreKind::Field { base, name } => self.field(base, name)?,
            CoreKind::With { base, fields } => self.with(base, fields, c.span)?,
            CoreKind::ListLit(xs) => self.list_lit(&c.ty, xs, c.span)?,
            CoreKind::MapLit(kvs) => self.map_lit(&c.ty, kvs, c.span)?,
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
        if c.ty != Repr::Bool {
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

    /// A `match`: a chain of tests, each falling through to the next.
    ///
    /// Falling through is what makes a guard a guard — an arm whose pattern matched but whose
    /// guard was false has to reach the arm after it, which is the evaluator's `continue`.
    ///
    /// An arm whose pattern takes a value apart is emitted **once per alternative**
    /// ([`alternatives`]), because two alternatives of one or-pattern bind the same names to
    /// different words — `Circle(r) | Square(r)` reads `r` out of two different objects — and a
    /// single block reached from both would need a `phi` per binder. Duplicating the arm is the
    /// same behaviour with no join to get wrong, and the count is bounded.
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
        let mut ty: Option<Repr> = None;

        for arm in arms {
            for pattern in alternatives(&arm.pattern)? {
                let next = self.label("match.next");
                let mut undo: Vec<(VarId, Option<Val>)> = Vec::new();
                let probed = self.probe(&pattern, &v, &next, &mut undo);
                if let Err(e) = probed {
                    self.unbind(undo);
                    return Err(e);
                }
                if let Some(guard) = &arm.guard {
                    let g = self.value(guard)?;
                    if g.ty != Repr::Bool {
                        return Err("a match guard is not a Bool".into());
                    }
                    let run = self.label("match.guarded");
                    self.terminate(format!("br i1 {}, label %{run}, label %{next}", g.text));
                    self.start(run);
                }
                if let Some(av) = self.expr(&arm.body, dest)? {
                    match ty {
                        Some(t) if t != av.ty => {
                            return Err("match arms have different types".into())
                        }
                        _ => ty = Some(av.ty),
                    }
                    incoming.push((av.text.clone(), self.label.clone()));
                    self.terminate(format!("br label %{join}"));
                }
                self.unbind(undo);

                self.start(next);
            }
        }

        // Nothing matched. The checker proves a `match` exhaustive, so this is unreachable for a
        // program that compiled — but "unreachable" in LLVM means the optimiser may do anything at
        // all with the path that reaches it, and a wrong exhaustiveness check would then be
        // undefined behaviour rather than a message. It traps instead.
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
        let payload = self.widen(&v);
        self.trap(trap, span, &payload, "true");
        // `trap` with a constant condition already left for the handler; this terminates the block
        // it carried on into, which nothing can reach.
        let out = self.escape();
        self.terminate(format!("br label %{out}"));

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

    /// Test `pat` against `v`, binding what it names: fall through on a match, branch to `fail`
    /// otherwise.
    ///
    /// Control flow rather than one `i1`, and that is a memory-safety requirement rather than a
    /// tidiness one: `Some(Circle(r))` cannot read the field it matches on until the tag says
    /// there is one there, and a conjunction that evaluated both sides would read a word of a
    /// variant that is not present and follow it as an offset.
    fn probe(
        &mut self,
        pat: &Pattern,
        v: &Val,
        fail: &str,
        undo: &mut Vec<(VarId, Option<Val>)>,
    ) -> Result<(), String> {
        match pat {
            Pattern::Wildcard => Ok(()),
            Pattern::Bind(var) => {
                undo.push((*var, self.env.insert(*var, v.clone())));
                Ok(())
            }
            Pattern::At { var, inner } => {
                undo.push((*var, self.env.insert(*var, v.clone())));
                self.probe(inner, v, fail, undo)
            }
            Pattern::Const(k) => {
                let want = self.constant(k)?;
                if want.ty != v.ty {
                    return Err("a match arm compares against a constant of another type".into());
                }
                let cond = self.equals(v, &want)?;
                self.branch(&cond, fail);
                Ok(())
            }
            // Only the alternatives `alternatives` leaves whole reach here: every one is a test
            // and none of them binds, so the disjunction is one `i1` and one branch.
            Pattern::Or(alts) => {
                let mut acc: Option<String> = None;
                for alt in alts {
                    let Pattern::Const(k) = alt else {
                        return Err("an or-pattern that was not split".into());
                    };
                    let want = self.constant(k)?;
                    if want.ty != v.ty {
                        return Err(
                            "a match arm compares against a constant of another type".into()
                        );
                    }
                    let t = self.equals(v, &want)?;
                    acc = Some(match acc {
                        None => t,
                        Some(prev) => {
                            let r = self.fresh();
                            self.line(format!("{r} = or i1 {prev}, {t}"));
                            r
                        }
                    });
                }
                let cond = acc.ok_or_else(|| "an or-pattern with no alternatives".to_string())?;
                self.branch(&cond, fail);
                Ok(())
            }
            Pattern::Ctor { variant, binds } => {
                let Repr::Obj(at) = v.ty else {
                    return Err(format!(
                        "matches the constructor `{variant}` against something that is not a record"
                    ));
                };
                let (tag, fields) = {
                    let layout = self.heap.layout(at);
                    let tag = layout.tag_of(Some(variant)).ok_or_else(|| {
                        format!("`{variant}` is not a variant of `{}`", layout.shown)
                    })?;
                    (tag, layout.variants[tag as usize].clone())
                };
                // A record has one variant, so its tag is known and there is nothing to test. A
                // union's is a word to load and compare.
                if self.heap.layout(at).tagged {
                    let got = self.load_word(&v.text, 0);
                    let ok = self.fresh();
                    self.line(format!("{ok} = icmp eq i64 {got}, {tag}"));
                    self.branch(&ok, fail);
                }
                for (name, sub) in binds {
                    let (slot, repr) = fields.slot(name).ok_or_else(|| {
                        format!("`{variant}` has no field `{name}` in this layout")
                    })?;
                    let field = self.load_field(&v.text, slot, repr);
                    self.probe(sub, &field, fail, undo)?;
                }
                Ok(())
            }
            // `[]`, `[a, b]`, `[first, *rest]` — the length, then the fixed elements, then the
            // tail. The order matters: an element is read only after the length test has proved it
            // is there, so nothing here can load past the end of the block.
            Pattern::List { items, rest } => {
                let Repr::List(at) = v.ty else {
                    return Err(format!(
                        "matches a list pattern against {}",
                        self.heap.show(v.ty)
                    ));
                };
                let element = self.heap.element(at);
                let n = self.list_len(v);
                // No tail binder means an exact length; a tail binder means "at least this many",
                // which is the evaluator's own rule.
                let long = self.fresh();
                let test = if rest.is_some() { "sge" } else { "eq" };
                self.line(format!("{long} = icmp {test} i64 {n}, {}", items.len()));
                self.branch(&long, fail);

                for (i, sub) in items.iter().enumerate() {
                    let addr = self.element_addr(v, &i.to_string());
                    let x = self.load_at(&addr, element);
                    self.probe(sub, &x, fail, undo)?;
                }

                // The tail is a **fresh list**, copied, which is what the evaluator does — an
                // `Arc<Vec<_>>` cannot share a suffix either (`docs/27` §27.3), so this is `O(n)`
                // per step on both backends rather than one being quietly quadratic against the
                // other. A borrowed suffix would also be unsound here for a reason the evaluator
                // does not have: a list's header points at a data block whose own header carries
                // `used`, and an offset header would read an element as that count.
                if let Some(Some(var)) = rest {
                    let left = self.fresh();
                    self.line(format!("{left} = sub i64 {n}, {}", items.len()));
                    self.uses_heap = true;
                    let idx = self.span(Span::NONE);
                    let tail = self.fresh();
                    self.line(format!(
                        "{tail} = call i64 @\"beck.list.copy\"(ptr %err, i64 {}, i64 {}, i64 {left}, i32 {idx})",
                        v.text,
                        items.len()
                    ));
                    self.check_call();
                    let tail = Val {
                        text: tail,
                        ty: v.ty,
                    };
                    undo.push((*var, self.env.insert(*var, tail)));
                }
                Ok(())
            }
        }
    }

    /// A word at an address, as the value its [`Repr`] says it is.
    ///
    /// [`Function::load_field`]'s twin for something that is already a pointer — a list's element,
    /// where that one starts from an object's offset and a slot.
    fn load_at(&mut self, p: &str, repr: Repr) -> Val {
        let r = self.fresh();
        match repr {
            Repr::Float => self.line(format!("{r} = load double, ptr {p}")),
            Repr::Bool => {
                let raw = self.fresh();
                self.line(format!("{raw} = load i64, ptr {p}"));
                self.line(format!("{r} = icmp ne i64 {raw}, 0"));
            }
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => self.line(format!("{r} = load i64, ptr {p}")),
        }
        Val { text: r, ty: repr }
    }

    /// Carry on if `cond`, and go to `fail` if not.
    fn branch(&mut self, cond: &str, fail: &str) {
        let cont = self.label("pat.ok");
        self.terminate(format!("br i1 {cond}, label %{cont}, label %{fail}"));
        self.start(cont);
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

    /// The arena's base, loaded once at the top of this function.
    ///
    /// Once, and not per access: the pointer is written by `main` before any compiled code runs and
    /// never again, so one load dominates every use of it. [`Function::emit`] is what puts the load
    /// in the entry block, and this is what tells it to.
    fn base(&mut self) -> &'static str {
        self.uses_heap = true;
        "%hp"
    }

    /// The address of word `slot` of the object at offset `off`.
    fn word_addr(&mut self, off: &str, slot: usize) -> String {
        let base = self.base();
        let p = self.fresh();
        self.line(format!(
            "{p} = getelementptr inbounds i8, ptr {base}, i64 {off}"
        ));
        if slot == 0 {
            return p;
        }
        let q = self.fresh();
        let bytes = slot as u64 * heap::WORD;
        self.line(format!(
            "{q} = getelementptr inbounds i8, ptr {p}, i64 {bytes}"
        ));
        q
    }

    /// One raw word of an object — the tag, or a field read for copying rather than for using.
    fn load_word(&mut self, off: &str, slot: usize) -> String {
        let p = self.word_addr(off, slot);
        let r = self.fresh();
        self.line(format!("{r} = load i64, ptr {p}"));
        r
    }

    fn store_word(&mut self, off: &str, slot: usize, word: &str) {
        let p = self.word_addr(off, slot);
        self.line(format!("store i64 {word}, ptr {p}"));
    }

    /// A field, as the value its [`Repr`] says it is.
    fn load_field(&mut self, off: &str, slot: usize, repr: Repr) -> Val {
        let p = self.word_addr(off, slot);
        let r = self.fresh();
        match repr {
            Repr::Float => self.line(format!("{r} = load double, ptr {p}")),
            Repr::Bool => {
                let raw = self.fresh();
                self.line(format!("{raw} = load i64, ptr {p}"));
                self.line(format!("{r} = icmp ne i64 {raw}, 0"));
            }
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => self.line(format!("{r} = load i64, ptr {p}")),
        }
        Val { text: r, ty: repr }
    }

    /// Put a value in a field.
    ///
    /// A real is **normalised** on the way in, which is the one place this backend's invariant
    /// about zeros and NaNs is paid for rather than argued away: a stored real is compared with
    /// another stored real by [`compare_functions`], is read back by the host's
    /// [`heap::Heap::decode`], and is part of what a record's `==` answers. Normalising here means
    /// every real on the heap is the one the evaluator would have built, so nothing downstream has
    /// to remember.
    fn store_field(&mut self, off: &str, slot: usize, v: &Val) {
        let p = self.word_addr(off, slot);
        match v.ty {
            Repr::Float => {
                let n = self.normalise(&v.text);
                self.line(format!("store double {}, ptr {p}", n.text));
            }
            Repr::Bool => {
                let w = self.fresh();
                self.line(format!("{w} = zext i1 {} to i64", v.text));
                self.line(format!("store i64 {w}, ptr {p}"));
            }
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => self.line(format!("store i64 {}, ptr {p}", v.text)),
        }
    }

    /// Ask the host one of the four questions compiled code cannot answer.
    ///
    /// The shapes are the point. What goes on the wire is a word per argument and a word saying
    /// what each word *is*, so the host decodes and encodes through [`crate::heap::Heap`] without a
    /// second table of what `secret_env` takes and what `http_fetch` answers — the same trick a
    /// view's deferred leaves play, one subsystem over.
    ///
    /// The name of the error type is written here rather than by the host, for
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
        let ret = self
            .repr(ty)
            .map_err(|why| format!("`{}` answers {why}", op.name()))?;
        let ret_shape = self.heap.word_of(ret);
        let (raises, named) = match op.raises() {
            Some(name) => {
                let repr = self
                    .repr(&Ty::con(name))
                    .map_err(|why| format!("`{}` raises {why}", op.name()))?;
                (self.heap.word_of(repr), self.literal(name))
            }
            None => (0, 0),
        };
        self.asks = true;
        let idx = self.span(span);

        let mut words = vec![
            u64::from(ret_shape).to_string(),
            u64::from(raises).to_string(),
        ];
        for v in vals {
            let shape = self.heap.word_of(v.ty);
            words.push(u64::from(shape).to_string());
            words.push(self.widen(v));
        }
        for (i, word) in words.iter().enumerate() {
            let p = self.fresh();
            self.line(format!(
                "{p} = getelementptr inbounds i8, ptr %q, i64 {}",
                i as u64 * heap::WORD
            ));
            self.line(format!("store i64 {word}, ptr {p}"));
        }
        let got = self.fresh();
        self.line(format!(
            "{got} = call i64 @\"beck.host\"(i32 {}, i32 {idx}, i64 {named}, i64 {}, ptr %q, i64 {}, ptr %err)",
            op.code(),
            words.len(),
            u32::from(op.carries_arena()),
        ));
        self.check_call();
        Ok(self.narrow(&got, ret))
    }

    /// The eight bytes the protocol carries, as the value its [`Repr`] says it is.
    fn narrow(&mut self, word: &str, ty: Repr) -> Val {
        let text = match ty {
            Repr::Float => {
                let r = self.fresh();
                self.line(format!("{r} = bitcast i64 {word} to double"));
                r
            }
            Repr::Bool => {
                let r = self.fresh();
                self.line(format!("{r} = icmp ne i64 {word}, 0"));
                r
            }
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => word.to_string(),
        };
        Val { text, ty }
    }

    /// Reserve `bytes` in the arena and answer the offset, or trap if there is no room.
    fn alloc(&mut self, bytes: u64, span: Span) -> String {
        self.uses_heap = true;
        let idx = self.span(span);
        let off = self.fresh();
        self.line(format!(
            "{off} = call i64 @\"beck.alloc\"(ptr %err, i64 {bytes}, i32 {idx})"
        ));
        self.check_call();
        off
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

        let off = self.alloc(layout.bytes(), span);
        self.store_word(&off, 0, &tag.to_string());
        for (slot, v) in &placed {
            self.store_field(&off, *slot, v);
        }
        Ok(Val {
            text: off,
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
        Ok(self.load_field(&b.text, slot, repr))
    }

    /// `p.with(x = 3)` — a new object with the old one's other fields.
    ///
    /// Always a fresh object. The evaluator rebuilds in place when the base is held by nobody else
    /// ([`docs/70`](../../../../../docs/70-the-evaluator-gets-fast-report.md)), and this cannot: an arena
    /// with no ownership in it cannot prove nobody else holds an offset. What that costs is
    /// [`docs/93`] §93.12's first row, and it is a cost rather than a difference — the answer is
    /// the same one.
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

        let off = self.alloc(layout.bytes(), span);
        // Word for word, because a copy does not care what a field means — and then the named ones
        // are written over.
        for slot in 0..=layout.fields.len() {
            let w = self.load_word(&b.text, slot);
            self.store_word(&off, slot, &w);
        }
        for (slot, v) in &placed {
            self.store_field(&off, *slot, v);
        }
        Ok(Val {
            text: off,
            ty: b.ty,
        })
    }

    /// `lambda x: …` — an object holding the lambda's rank and everything its body reads from here.
    ///
    /// The captures are the [`heap::Lambda`] record's, in that record's order, because the code that
    /// *reads* them back is [`Function::lam_body`] and the two must agree about which word is which.
    /// A closure is the one value this backend builds that the host never sees: it is refused in a
    /// signature, a field, an element and a map, so it lives and dies inside one compiled call.
    fn closure(
        &mut self,
        params: &Arc<[VarId]>,
        body: &Arc<Core>,
        ty: &Ty,
        span: Span,
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
        let captures = self.heap.lam(rank).captures.clone();
        let mut vals = Vec::with_capacity(captures.len());
        for (var, ty) in &captures {
            let want = self
                .repr(ty)
                .map_err(|why| format!("captures a variable that is {why}"))?;
            let v = self
                .env
                .get(var)
                .cloned()
                .ok_or("captures a variable that is not bound here")?;
            if v.ty != want {
                return Err("captures a variable at a type this backend reads two ways".into());
            }
            vals.push(v);
        }
        self.lambda(rank, params, body, family)?;
        let off = self.alloc(heap::closure_bytes(captures.len() as u64), span);
        self.store_word(&off, 0, &rank.to_string());
        for (i, v) in vals.iter().enumerate() {
            self.store_field(&off, i + 1, v);
        }
        Ok(Val {
            text: off,
            ty: repr,
        })
    }

    /// A definition named where a value is expected — `map_list(xs, double)`.
    ///
    /// The closure carries nothing, because a definition closes over nothing, and the arm the
    /// application switches into calls the definition itself: there is no wrapper function and no
    /// second copy of the body. Its rank is the one [`heap::survey`] gave the definition's own
    /// outermost `lam`, which is why that lambda is ranked along with every other.
    fn named(&mut self, name: &str, ty: &Ty, span: Span) -> Result<Val, String> {
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
        let off = self.alloc(heap::closure_bytes(0), span);
        self.store_word(&off, 0, &rank.to_string());
        Ok(Val {
            text: off,
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
        // Reserved before the body is emitted, so that a `lam` reaching itself would not recurse
        // here forever — and overwritten with the text below.
        self.lambdas.insert(rank, String::new());
        let mut inner = Function::new(self.sigs, self.eligible, self.program, self.heap);
        inner.spans = std::mem::take(&mut self.spans);
        let emitted = inner.lam_body(rank, params, body, &fam, &captures);
        self.spans = std::mem::take(&mut inner.spans);
        self.compared.append(&mut inner.compared);
        self.list_compared.append(&mut inner.list_compared);
        self.map_compared.append(&mut inner.map_compared);
        self.applied.append(&mut inner.applied);
        self.loops.append(&mut inner.loops);
        self.compared_fns |= inner.compared_fns;
        let nested = std::mem::take(&mut inner.lambdas);
        for (r, text) in nested {
            self.lambdas.entry(r).or_insert(text);
        }
        self.lambdas.insert(rank, emitted?);
        Ok(())
    }

    /// The body of one lambda, as its own `define`.
    ///
    /// Two things separate it from [`Function::emit`]: the closure itself is the first parameter,
    /// and the captures are loaded off it into the environment before the body is compiled — so the
    /// body reads a capture exactly as it reads a parameter, and nothing below this knows the
    /// difference.
    fn lam_body(
        &mut self,
        rank: u32,
        params: &[VarId],
        body: &Core,
        fam: &heap::Family,
        captures: &[(VarId, Ty)],
    ) -> Result<String, String> {
        self.ret = fam.ret;
        let mut head = format!(
            "define internal tailcc {} @\"beck.lam.{rank}\"(ptr noalias %err, i64 %clo",
            fam.ret.llvm()
        );
        for (i, ty) in fam.params.iter().enumerate() {
            let _ = write!(head, ", {} %a{i}", ty.llvm());
            self.env.insert(
                params[i],
                Val {
                    text: format!("%a{i}"),
                    ty: *ty,
                },
            );
        }
        head.push_str(") {\n");
        self.label = "entry".into();
        for (i, (var, ty)) in captures.iter().enumerate() {
            let want = self
                .repr(ty)
                .map_err(|why| format!("captures a variable that is {why}"))?;
            let v = self.load_field("%clo", i + 1, want);
            self.env.insert(*var, v);
        }
        self.expr(body, Dest::Return)?;

        let mut text = head;
        if self.uses_heap {
            let at = self.out.find('\n').map_or(0, |i| i + 1);
            self.out
                .insert_str(at, "  %hp = load ptr, ptr @\"beck.heap\"\n");
        }
        if self.asks {
            let at = self.out.find('\n').map_or(0, |i| i + 1);
            self.out
                .insert_str(at, &format!("  %q = alloca [{QUESTION_WORDS} x i64]\n"));
        }
        text.push_str(&self.out);
        if self.trapped {
            let _ = write!(
                text,
                "trap:\n  ret {} {}\n",
                fam.ret.llvm(),
                fam.ret.machine().zero()
            );
        }
        text.push_str("}\n");
        Ok(text)
    }

    /// Applying a value rather than calling a name: one switch, one direct call per rank.
    fn apply(&mut self, func: &Core, args: &[Core], dest: Dest) -> Result<Option<Val>, String> {
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
        let v = self.apply_call(family, &fam, &f.text, &vals, dest == Dest::Return);
        match v {
            // The application was a tail call and has already returned.
            None => Ok(None),
            Some(v) => self.finish(v, dest),
        }
    }

    /// The call into a family's application, tail or not.
    ///
    /// `tail` is honoured rather than advisory: `docs/27` makes a call in tail position free, and an
    /// application is a call — so a loop written as a closure calling itself must not grow the
    /// stack. Both hops are `musttail` (here, and the arm inside the application), which is what
    /// makes the whole path a jump.
    fn apply_call(
        &mut self,
        family: u32,
        fam: &heap::Family,
        closure: &str,
        args: &[Val],
        tail: bool,
    ) -> Option<Val> {
        self.applied.insert(family);
        self.uses_heap = true;
        let mut operands = format!("ptr %err, i64 {closure}");
        for v in args {
            let _ = write!(operands, ", {} {}", v.ty.llvm(), v.text);
        }
        let r = self.fresh();
        let kind = if tail && fam.ret == self.ret {
            "musttail call"
        } else {
            "call"
        };
        self.line(format!(
            "{r} = {kind} tailcc {} @\"beck.apply.{family}\"({operands})",
            fam.ret.llvm()
        ));
        if kind == "musttail call" {
            self.terminate(format!("ret {} {r}", fam.ret.llvm()));
            return None;
        }
        self.check_call();
        Some(Val {
            text: r,
            ty: fam.ret,
        })
    }

    /// and would have been a stack frame here.
    /// A direct call of a named definition — and in tail position, a jump.
    ///
    /// The tail case is `musttail`, which LLVM *guarantees* rather than attempts: if it could not
    /// discard the frame it refuses the module, so a build that succeeds is a build in which every
    /// tail call is a jump. That is stronger than the usual `-O2` sibling-call heuristic and it is
    /// the point — `docs/27` §27.2 says 1,500 and 60,000 tail calls spend the same host stack, and
    /// an optimisation that "usually" fires cannot be what a language guarantee rests on.
    ///
    /// It is `tailcc` rather than the C convention because the C convention's `musttail` demands
    /// that the caller and callee prototypes match, which is a rule about arity and not about
    /// tails: `def loop(n, acc)` calling `def done(acc)` is an ordinary tail call in the language
    /// and would have been a stack frame here.
    ///
    /// Anything that is not a name is a closure being applied, which is [`Function::apply`].
    fn call(&mut self, func: &Core, args: &[Core], dest: Dest) -> Result<Option<Val>, String> {
        let CoreKind::Global(name) = &func.kind else {
            return self.apply(func, args, dest);
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

    fn prim(&mut self, op: Prim, args: &[Core], ty: &Ty, span: Span) -> Result<Val, String> {
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
                Err(format!("`{}` mixes two scalar types", op.name()))
            }
        };

        // The four that are questions rather than computations. Before the rest, because what
        // separates them is not what they do with their arguments.
        if let Some(ask) = Upcall::of(op) {
            return self.upcall(ask, &vals, ty, span);
        }

        match op {
            Prim::Add | Prim::Sub | Prim::Mul => {
                arity(2)?;
                let ty = same(&vals)?;
                match ty {
                    // `+` on two strings is the one arithmetic operator text has, and it is the
                    // only place this backend allocates for something that is not a constructor.
                    Repr::Str if op == Prim::Add => {
                        Ok(self.text_call("concat", &[&vals[0], &vals[1]], Repr::Str, span))
                    }
                    Repr::Int => {
                        let (intrinsic, trap) = match op {
                            Prim::Add => ("sadd", Trap::AddOverflow),
                            Prim::Sub => ("ssub", Trap::SubOverflow),
                            _ => ("smul", Trap::MulOverflow),
                        };
                        Ok(self.checked_int(intrinsic, trap, &vals[0], &vals[1], span))
                    }
                    Repr::Float => {
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
                            ty: Repr::Float,
                        })
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
                arity(2)?;
                let ty = same(&vals)?;
                match ty {
                    Repr::Int => {
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
                    Repr::Float if op == Prim::Div => {
                        let divisor = self.normalise(&vals[1].text);
                        let r = self.fresh();
                        self.line(format!(
                            "{r} = fdiv double {}, {}",
                            vals[0].text, divisor.text
                        ));
                        Ok(Val {
                            text: r,
                            ty: Repr::Float,
                        })
                    }
                    _ => Err(format!("`{}` on this type", op.name())),
                }
            }
            Prim::Neg => {
                arity(1)?;
                match vals[0].ty {
                    Repr::Int => {
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
                            ty: Repr::Int,
                        })
                    }
                    Repr::Float => {
                        let r = self.fresh();
                        self.line(format!("{r} = fneg double {}", vals[0].text));
                        Ok(Val {
                            text: r,
                            ty: Repr::Float,
                        })
                    }
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
                arity(1)?;
                match vals[0].ty {
                    Repr::Int => {
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
                            ty: Repr::Int,
                        })
                    }
                    Repr::Float => {
                        let r = self.intrinsic_f64("llvm.fabs.f64", &vals[0])?;
                        Ok(Val {
                            text: r,
                            ty: Repr::Float,
                        })
                    }
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
                    ty: Repr::Float,
                })
            }
            Prim::Trunc => {
                arity(1)?;
                if vals[0].ty != Repr::Float {
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
                    ty: Repr::Int,
                })
            }
            Prim::ToFloat => {
                arity(1)?;
                if vals[0].ty != Repr::Int {
                    return Err("`float` of something that is not an Int".into());
                }
                let r = self.fresh();
                self.line(format!("{r} = sitofp i64 {} to double", vals[0].text));
                // No normalisation: an integer converts to neither a negative zero nor a NaN.
                Ok(Val {
                    text: r,
                    ty: Repr::Float,
                })
            }
            Prim::Eq | Prim::Ne | Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                arity(2)?;
                same(&vals)?;
                self.compare(op, &vals[0], &vals[1])
            }
            Prim::And | Prim::Or | Prim::Not => {
                let want = if op == Prim::Not { 1 } else { 2 };
                arity(want)?;
                if vals.iter().any(|v| v.ty != Repr::Bool) {
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
                    ty: Repr::Bool,
                })
            }
            Prim::StrLen | Prim::StrIsEmpty => {
                arity(1)?;
                self.text_arg(&vals[0], op)?;
                // Both counts are in the header, so both of these are a load: `str_len` is `O(1)`
                // in the evaluator since `docs/70`, and a backend that counted here would make the
                // loop that walks a string by index quadratic in one implementation and not the
                // other.
                let n = self.text_word(&vals[0], if op == Prim::StrLen { 8 } else { 0 });
                if op == Prim::StrLen {
                    return Ok(Val {
                        text: n,
                        ty: Repr::Int,
                    });
                }
                let r = self.fresh();
                self.line(format!("{r} = icmp eq i64 {n}, 0"));
                Ok(Val {
                    text: r,
                    ty: Repr::Bool,
                })
            }
            Prim::StrSlice => {
                arity(3)?;
                self.text_arg(&vals[0], op)?;
                for v in &vals[1..] {
                    if v.ty != Repr::Int {
                        return Err("`str_slice` takes two Int positions".into());
                    }
                }
                Ok(self.text_call("slice", &[&vals[0], &vals[1], &vals[2]], Repr::Str, span))
            }
            Prim::StrTrim => {
                arity(1)?;
                self.text_arg(&vals[0], op)?;
                Ok(self.text_call("trim", &[&vals[0]], Repr::Str, span))
            }
            Prim::StrSplit | Prim::StrChars => {
                // One function, because the evaluator answers characters for an empty separator —
                // so `str_chars(s)` *is* `str_split(s, "")`, and the two share a body as well as a
                // fixture.
                let sep = if op == Prim::StrChars {
                    arity(1)?;
                    self.text_arg(&vals[0], op)?;
                    // The offset `0`, which is never a live object — so `str_chars` needs no
                    // literal, and the pool stays a function of the program's own text.
                    Val {
                        text: "0".to_string(),
                        ty: Repr::Str,
                    }
                } else {
                    arity(2)?;
                    self.text_arg(&vals[0], op)?;
                    self.text_arg(&vals[1], op)?;
                    vals[1].clone()
                };
                // Interning the element is what puts the list runtime in the module: the answer is
                // a `list[Str]` no program in the module need have written down.
                let at = self.heap.word_of(Repr::Str);
                Ok(self.text_call("split", &[&vals[0], &sep], Repr::List(at), span))
            }
            Prim::StrContains | Prim::StrStartsWith | Prim::StrEndsWith => {
                arity(2)?;
                self.text_arg(&vals[0], op)?;
                self.text_arg(&vals[1], op)?;
                Ok(self.text_search(op, &vals[0], &vals[1]))
            }
            Prim::StrIndexOf => {
                arity(2)?;
                self.text_arg(&vals[0], op)?;
                self.text_arg(&vals[1], op)?;
                self.index_of(ty, &vals[0], &vals[1], span)
            }
            // `list_append` — a new header, and a slot in the block when there is one.
            //
            // Refused until `docs/93` for a reason that was true of the *layout* rather than of the
            // operation: with the count in front of the elements, an append could copy or it could
            // overwrite what other holders see. Separating the two made a third answer available.
            Prim::ListAppend => {
                arity(2)?;
                let at = self.list_arg(&vals[0], op)?;
                let element = self.heap.element(at);
                if vals[1].ty != element {
                    return Err("`list_append` of an element of another type".into());
                }
                let word = self.widen(&vals[1].clone());
                let idx = self.span(span);
                let r = self.fresh();
                self.line(format!(
                    "{r} = call i64 @\"beck.list.append\"(ptr %err, i64 {}, i64 {word}, i32 {idx})",
                    vals[0].text
                ));
                self.check_call();
                Ok(Val {
                    text: r,
                    ty: vals[0].ty,
                })
            }
            Prim::ListLen | Prim::ListIsEmpty => {
                arity(1)?;
                self.list_arg(&vals[0], op)?;
                let n = self.list_len(&vals[0]);
                if op == Prim::ListLen {
                    return Ok(Val {
                        text: n,
                        ty: Repr::Int,
                    });
                }
                let r = self.fresh();
                self.line(format!("{r} = icmp eq i64 {n}, 0"));
                Ok(Val {
                    text: r,
                    ty: Repr::Bool,
                })
            }
            Prim::ListGet => {
                arity(2)?;
                let at = self.list_arg(&vals[0], op)?;
                if vals[1].ty != Repr::Int {
                    return Err("`list_get` takes an Int index".into());
                }
                self.list_get(ty, at, &vals[0], &vals[1], span)
            }
            Prim::ListContains | Prim::ListIndexOf => {
                arity(2)?;
                let at = self.list_arg(&vals[0], op)?;
                let element = self.heap.element(at);
                if vals[1].ty != element {
                    return Err(format!(
                        "`{}` against an element of another type",
                        op.name()
                    ));
                }
                let word = self.widen(&vals[1]);
                let found = self.fresh();
                self.line(format!(
                    "{found} = call i64 @\"beck.list.find.{at}\"(i64 {}, i64 {word})",
                    vals[0].text
                ));
                self.wants(Repr::List(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                if op == Prim::ListContains {
                    let r = self.fresh();
                    self.line(format!("{r} = icmp sge i64 {found}, 0"));
                    return Ok(Val {
                        text: r,
                        ty: Repr::Bool,
                    });
                }
                self.some_or_none(ty, &found, span)
            }
            Prim::ListSlice | Prim::ListTake | Prim::ListDrop => {
                let want = if op == Prim::ListSlice { 3 } else { 2 };
                arity(want)?;
                self.list_arg(&vals[0], op)?;
                for v in &vals[1..] {
                    if v.ty != Repr::Int {
                        return Err(format!("`{}` takes Int positions", op.name()));
                    }
                }
                self.list_range(op, &vals, span)
            }
            Prim::ToStr => {
                arity(1)?;
                match vals[0].ty {
                    Repr::Str => Ok(vals[0].clone()),
                    Repr::Int => Ok(self.text_call("from_int", &[&vals[0]], Repr::Str, span)),
                    // Two literals from the pool, which is what `Value::display` answers.
                    Repr::Bool => {
                        let (t, f) = (self.literal("true"), self.literal("false"));
                        let r = self.fresh();
                        self.line(format!(
                            "{r} = select i1 {}, i64 {t}, i64 {f}",
                            vals[0].text
                        ));
                        Ok(Val {
                            text: r,
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
                arity(2)?;
                self.text_arg(&vals[0], op)?;
                if vals[1].ty != Repr::Int {
                    return Err("`str_repeat` takes an Int count".into());
                }
                Ok(self.text_call("repeat", &[&vals[0], &vals[1]], Repr::Str, span))
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
                self.text_arg(&vals[1], op)?;
                Ok(self.text_call("join", &[&vals[0], &vals[1]], Repr::Str, span))
            }
            Prim::OptionIsSome => {
                arity(1)?;
                let (some, ..) = self.option_taken(vals[0].ty)?;
                let tag = self.load_word(&vals[0].text, 0);
                let r = self.fresh();
                self.line(format!("{r} = icmp eq i64 {tag}, {some}"));
                Ok(Val {
                    text: r,
                    ty: Repr::Bool,
                })
            }
            Prim::OptionUnwrapOr => {
                arity(2)?;
                let (some, slot, payload) = self.option_taken(vals[0].ty)?;
                if vals[1].ty != payload {
                    return Err("`unwrap_or`'s fallback is not what the `Option` carries".into());
                }
                let tag = self.load_word(&vals[0].text, 0);
                let is_some = self.fresh();
                self.line(format!("{is_some} = icmp eq i64 {tag}, {some}"));
                // The address, not the value: a `None` the *host* wrote is one word long, because
                // `encode_object` allocates the variant's own size — so reading the payload slot
                // unconditionally can read past the end of the arena. The same `select` between the
                // field's address and the object's header that `list_get` and `map_get` use.
                let field = self.word_addr(&vals[0].text, slot);
                let header = self.word_addr(&vals[0].text, 0);
                let p = self.fresh();
                self.line(format!(
                    "{p} = select i1 {is_some}, ptr {field}, ptr {header}"
                ));
                let w = self.fresh();
                self.line(format!("{w} = load i64, ptr {p}"));
                let held = self.word_as(&w, payload);
                let r = self.fresh();
                self.line(format!(
                    "{r} = select i1 {is_some}, {ty} {held}, {ty} {}",
                    vals[1].text,
                    ty = payload.llvm()
                ));
                Ok(Val {
                    text: r,
                    ty: payload,
                })
            }
            Prim::MapLen => {
                arity(1)?;
                self.map_arg(&vals[0], op)?;
                let n = self.map_len(&vals[0]);
                Ok(Val {
                    text: n,
                    ty: Repr::Int,
                })
            }
            Prim::MapGet | Prim::MapContains => {
                arity(2)?;
                let at = self.map_arg(&vals[0], op)?;
                let (k, v) = self.heap.entry(at);
                let key = self.heap.element(k);
                if vals[1].ty != key {
                    return Err(format!("`{}` with a key of another type", op.name()));
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                let word = self.widen(&vals[1]);
                let found = self.fresh();
                self.line(format!(
                    "{found} = call i64 @\"beck.map.find.{at}\"(i64 {}, i64 {word})",
                    vals[0].text
                ));
                if op == Prim::MapContains {
                    let r = self.fresh();
                    self.line(format!("{r} = icmp ne i64 {found}, 0"));
                    return Ok(Val {
                        text: r,
                        ty: Repr::Bool,
                    });
                }
                self.map_get(ty, &vals[0], &found, self.heap.element(v), span)
            }
            // The three that grow a map. Refused until `docs/93` because a sorted run has to be
            // copied whole; a tree rebuilds the path and shares the rest, which is
            // `beck_core::pmap`'s own cost.
            Prim::MapInsert | Prim::MapRemove => {
                arity(if op == Prim::MapInsert { 3 } else { 2 })?;
                let at = self.map_arg(&vals[0], op)?;
                let (k, v) = self.heap.entry(at);
                let key = self.heap.element(k);
                if vals[1].ty != key {
                    return Err(format!("`{}` with a key of another type", op.name()));
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                let kw = self.widen(&vals[1].clone());
                let idx = self.span(span);
                let r = self.fresh();
                if op == Prim::MapInsert {
                    if vals[2].ty != self.heap.element(v) {
                        return Err("`map_insert` with a value of another type".into());
                    }
                    let vw = self.widen(&vals[2].clone());
                    self.line(format!(
                        "{r} = call i64 @\"beck.map.ins.{at}\"(ptr %err, i64 {}, i64 {kw}, i64 \
                         {vw}, i32 {idx})",
                        vals[0].text
                    ));
                } else {
                    self.line(format!(
                        "{r} = call i64 @\"beck.map.del.{at}\"(ptr %err, i64 {}, i64 {kw}, i32 \
                         {idx})",
                        vals[0].text
                    ));
                }
                self.check_call();
                Ok(Val {
                    text: r,
                    ty: vals[0].ty,
                })
            }
            Prim::MapMerge => {
                arity(2)?;
                let at = self.map_arg(&vals[0], op)?;
                if vals[1].ty != vals[0].ty {
                    return Err("`map_merge` of two maps of different types".into());
                }
                self.wants(Repr::Map(at))
                    .map_err(|why| format!("`{}` over {why}", op.name()))?;
                let idx = self.span(span);
                let r = self.fresh();
                self.line(format!(
                    "{r} = call i64 @\"beck.map.merge.{at}\"(ptr %err, i64 {}, i64 {}, i32 {idx})",
                    vals[0].text, vals[1].text
                ));
                self.check_call();
                Ok(Val {
                    text: r,
                    ty: vals[0].ty,
                })
            }
            Prim::MapKeys | Prim::MapValues => {
                arity(1)?;
                self.map_arg(&vals[0], op)?;
                self.map_run(op, ty, &vals[0], span)
            }
            // A list of lists into one list. Not a growth: the total is a sum over the outer list's
            // header words, so the allocation happens once and after it — the same argument
            // `str_join` is compiled under, and the reason `docs/93` §93.9 corrects this
            // primitive's own refusal.
            Prim::ConcatLists => {
                arity(1)?;
                let outer = self.list_arg(&vals[0], op)?;
                let Repr::List(inner) = self.heap.element(outer) else {
                    return Err("`concat_lists` on something that is not a list of lists".into());
                };
                self.uses_heap = true;
                let idx = self.span(span);
                let r = self.fresh();
                self.line(format!(
                    "{r} = call i64 @\"beck.list.concat\"(ptr %err, i64 {}, i32 {idx})",
                    vals[0].text
                ));
                self.check_call();
                Ok(Val {
                    text: r,
                    ty: Repr::List(inner),
                })
            }
            Prim::ListReverse => {
                arity(1)?;
                let at = self.list_arg(&vals[0], op)?;
                self.uses_heap = true;
                let idx = self.span(span);
                let r = self.fresh();
                self.line(format!(
                    "{r} = call i64 @\"beck.list.reverse\"(ptr %err, i64 {}, i32 {idx})",
                    vals[0].text
                ));
                self.check_call();
                Ok(Val {
                    text: r,
                    ty: Repr::List(at),
                })
            }
            Prim::MapList | Prim::FilterList => {
                arity(2)?;
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
                let r = self.list_loop(
                    which,
                    fam,
                    &vals[0].text.clone(),
                    None,
                    &vals[1].text.clone(),
                    None,
                    span,
                );
                Ok(Val { text: r, ty: out })
            }
            Prim::ListFold => {
                arity(3)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let acc = vals[1].ty;
                let fam = self.function_arg(&vals[2], op, &[acc, element])?;
                let family = self.heap.family(fam).clone();
                if family.ret != acc {
                    return Err(
                        "`list_fold`'s function answers something other than the accumulator it \
                         is given"
                            .into(),
                    );
                }
                let r = self.list_loop(
                    Loop::Fold,
                    fam,
                    &vals[0].text.clone(),
                    Some(&vals[1].clone()),
                    &vals[2].text.clone(),
                    None,
                    span,
                );
                Ok(Val { text: r, ty: acc })
            }
            // Decorate, sort, undecorate — and the keys are words like any others, so what compares
            // two of them is the function a list's element comparison already is.
            Prim::SortBy => {
                arity(2)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let fam = self.function_arg(&vals[1], op, &[element])?;
                let key = self.heap.family(fam).ret;
                // Interned here rather than in the survey: the keys are not a list any program
                // wrote, so nothing else would have asked for their comparison — and recording the
                // index is what makes the module generate it.
                let at = self.heap.word_of(key);
                self.wants(Repr::List(at))
                    .map_err(|why| format!("`{}` by a key that is {why}", op.name()))?;
                let r = self.list_loop(
                    Loop::Sort,
                    fam,
                    &vals[0].text.clone(),
                    None,
                    &vals[1].text.clone(),
                    None,
                    span,
                );
                Ok(Val {
                    text: r,
                    ty: vals[0].ty,
                })
            }
            Prim::ListAll | Prim::ListAny => {
                arity(2)?;
                let element = self.heap.element(self.list_arg(&vals[0], op)?);
                let fam = self.function_arg(&vals[1], op, &[element])?;
                if self.heap.family(fam).ret != Repr::Bool {
                    return Err(format!("`{}`'s function does not answer a Bool", op.name()));
                }
                let r = self.list_loop(
                    Loop::Every,
                    fam,
                    &vals[0].text.clone(),
                    None,
                    &vals[1].text.clone(),
                    Some(op == Prim::ListAny),
                    span,
                );
                Ok(Val {
                    text: r,
                    ty: Repr::Bool,
                })
            }
            // `raise e` — the one failure that is not a fault, so it carries a value.
            //
            // Two words in the arena — the value's shape and its word, which is a view node's
            // deferred value one subsystem over — and the error cell's third word holds the type
            // *name*, as the offset of that name in the literal pool. A name and not the shape,
            // because two instantiations of one generic type are two layouts and one name, and it
            // is the name `try:` compares (`beck_core`'s own rule: the atom is `raises(T)`).
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
                let pair = self.alloc(heap::RAISED_WORDS * heap::WORD, span);
                self.store_word(&pair, 0, &shape.to_string());
                let v = vals[0].clone();
                self.store_field(&pair, 1, &v);
                let named = self.literal(&name);
                let slot = self.fresh();
                self.line(format!(
                    "{slot} = getelementptr inbounds i8, ptr %err, i64 16"
                ));
                self.line(format!("store i64 {named}, ptr {slot}"));
                self.trap(Trap::Raised, span, &pair, "true");
                // Unreachable: the trap above left for the handler with a constant condition. The
                // block it carried on into still needs a terminator, and the value this answers
                // with is never read — `raise` has no type of its own, so the checker gave the
                // expression whatever the context wanted.
                let out = self.escape();
                self.terminate(format!("br label %{out}"));
                let gone = self.label("raise.gone");
                self.start(gone);
                let want = self.repr(ty).unwrap_or(Repr::Int);
                Ok(Val {
                    text: want.machine().zero().to_string(),
                    ty: want,
                })
            }
            // The five that build a page. Every one of them is an allocation and some stores:
            // nothing is rendered, nothing is hashed, and no attribute is dropped, because what
            // goes in the arena is the *call* rather than the tree it makes — see
            // [`heap::Repr::Html`] and `beck_core::html::element`, which is where the host bakes
            // it and where the evaluator has always baked it.
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
                let off = self.alloc(heap::NODE_WORDS * heap::WORD, span);
                self.store_word(&off, 0, &heap::HTML_ELEMENT.to_string());
                for (slot, v) in vals.iter().enumerate() {
                    self.store_field(&off, slot + 1, v);
                }
                Ok(Val {
                    text: off,
                    ty: Repr::Html,
                })
            }
            Prim::HtmlText => {
                arity(1)?;
                // A child that is already a tree is spliced rather than rendered, which is the
                // evaluator's own arm — and here it needs no node at all, because the value it
                // would defer is the answer.
                if vals[0].ty == Repr::Html {
                    return Ok(vals[0].clone());
                }
                self.node(
                    Repr::Html,
                    heap::HTML_TEXT,
                    None,
                    Some(&vals[0].clone()),
                    span,
                )
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
                let (name, value) = (vals[0].clone(), vals[1].clone());
                self.node(Repr::Attr, tag, Some(&name), Some(&value), span)
            }
            Prim::HtmlKey => {
                arity(1)?;
                self.node(
                    Repr::Attr,
                    heap::ATTR_KEY,
                    None,
                    Some(&vals[0].clone()),
                    span,
                )
            }
            other => Err(refusal(other)),
        }
    }

    /// The `list[Attr]` and `list[Html]` reprs, resolving `Html` first if nothing has.
    fn view_lists(&mut self) -> Result<(u32, u32), String> {
        self.repr(&Ty::html())?;
        self.heap
            .html_lists()
            .ok_or_else(|| "a view node in a module with no view in it".to_string())
    }

    /// One view node or attribute: four words, a tag, a name for the two shapes that have one, and
    /// a deferred value for the four that have one.
    ///
    /// The unused words are **written**, not left as they were found: the whole used arena goes
    /// back down the pipe with the reply, and a word nobody reads is still a byte that would differ
    /// between two runs of the same program.
    fn node(
        &mut self,
        ty: Repr,
        tag: u64,
        name: Option<&Val>,
        deferred: Option<&Val>,
        span: Span,
    ) -> Result<Val, String> {
        if let Some(v) = deferred {
            // The host reads this word back as a `Value`, so what a closure gets everywhere else it
            // gets here: a shape with no form the host can read is not one to defer.
            heap::Heap::crossing(v.ty)
                .map_err(|why| format!("puts {why} in a page, and a page is read by the host"))?;
        }
        self.view_lists()?;
        let off = self.alloc(heap::NODE_WORDS * heap::WORD, span);
        self.store_word(&off, 0, &tag.to_string());
        match name {
            Some(v) => self.store_field(&off, 1, v),
            None => self.store_word(&off, 1, "0"),
        }
        match deferred {
            Some(v) => {
                let at = self.heap.word_of(v.ty);
                self.store_word(&off, heap::DEFERRED, &at.to_string());
                self.store_field(&off, heap::DEFERRED + 1, v);
            }
            None => {
                self.store_word(&off, heap::DEFERRED, "0");
                self.store_word(&off, heap::DEFERRED + 1, "0");
            }
        }
        Ok(Val { text: off, ty })
    }

    /// Insist an argument is a closure of the shape this primitive applies it at.
    ///
    /// The parameters are checked here rather than left to the application, because a mismatch is a
    /// *refusal* about a primitive — `map_list` over a list whose element is not what its function
    /// takes — and the message should name the primitive the program wrote.
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
    #[allow(clippy::too_many_arguments)] // the four signatures are four shapes; see `loop_function`
    ///
    /// The four signatures are [`loop_function`]'s and the four call sites are these, so a change to
    /// one and not the other is a module `clang` refuses rather than a wrong answer.
    fn list_loop(
        &mut self,
        which: Loop,
        fam: u32,
        xs: &str,
        init: Option<&Val>,
        closure: &str,
        want: Option<bool>,
        span: Span,
    ) -> String {
        self.applied.insert(fam);
        self.loops.insert((which, fam));
        self.uses_heap = true;
        let family = self.heap.family(fam).clone();
        let ret = match which {
            Loop::Map | Loop::Filter | Loop::Sort => "i64".to_string(),
            Loop::Fold => family.ret.llvm().to_string(),
            Loop::Every => "i1".to_string(),
        };
        let mut operands = format!("ptr %err, i64 {xs}");
        if let Some(v) = init {
            let _ = write!(operands, ", {} {}", v.ty.llvm(), v.text);
        }
        let _ = write!(operands, ", i64 {closure}");
        if let Some(want) = want {
            let _ = write!(operands, ", i1 {want}");
        }
        let idx = self.span(span);
        let r = self.fresh();
        self.line(format!(
            "{r} = call {ret} @\"{}\"({operands}, i32 {idx})",
            which.symbol(fam)
        ));
        self.check_call();
        r
    }

    /// A literal, as the operand that carries it.
    ///
    /// A string literal is the one that is not written into the instruction: it is an offset into
    /// the pool the host wrote at the front of the request's heap, decided when the module was
    /// emitted. See [`crate::heap`] for why it cannot be allocated where it is written and cannot
    /// be a global either.
    fn constant(&mut self, k: &Const) -> Result<Val, String> {
        match k {
            Const::Int(i) => Ok(Val {
                text: i.to_string(),
                ty: Repr::Int,
            }),
            Const::Bool(b) => Ok(Val {
                text: b.to_string(),
                ty: Repr::Bool,
            }),
            // Written as the bit pattern, so what the assembler reads back is the double the
            // compiler held rather than whatever a decimal rendering happened to round to.
            Const::Float(f) => Ok(Val {
                text: format!("0x{:016X}", f.to_bits()),
                ty: Repr::Float,
            }),
            Const::Str(s) => {
                self.uses_heap = true;
                let at = self.heap.intern(s);
                Ok(Val {
                    text: self.heap.string_offset(at).to_string(),
                    ty: Repr::Str,
                })
            }
            Const::Unit => Err("the unit value, which has no machine representation here".into()),
        }
    }

    /// `[a, b, c]` — one allocation, filled left to right.
    ///
    /// Left to right because an element expression can trap and *which* trap the caller sees is
    /// part of what the evaluator answers, which is the same reason a record's fields are.
    fn list_lit(&mut self, ty: &Ty, xs: &[Core], span: Span) -> Result<Val, String> {
        let repr = self
            .repr(ty)
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
        let data = self.alloc(heap::DATA_HEADER + n * heap::WORD, span);
        self.store_word(&data, 0, &n.to_string());
        self.store_word(&data, 1, &n.to_string());
        for (i, v) in vals.iter().enumerate() {
            self.store_field(&data, i + 2, v);
        }
        let off = self.alloc(heap::LIST_HEADER, span);
        self.store_word(&off, 0, &n.to_string());
        self.store_word(&off, 1, &data);
        Ok(Val {
            text: off,
            ty: repr,
        })
    }

    /// The address of element `i` of `xs`, where `i` is a value rather than a constant.
    fn element_addr(&mut self, xs: &Val, index: &str) -> String {
        // Through the block, which is the one load `heap::LIST_HEADER`'s indirection costs.
        let q = self.fresh();
        self.line(format!(
            "{q} = call ptr @\"beck.list.data\"(i64 {})",
            xs.text
        ));
        let r = self.fresh();
        self.line(format!(
            "{r} = getelementptr inbounds i64, ptr {q}, i64 {index}"
        ));
        r
    }

    /// `str_index_of` — a byte search, a byte-to-character conversion, and an `Option`.
    ///
    /// The interesting half is that there is **no branch**. `Some(value=i)` is two words and
    /// `None()` is one, so allocating the larger of the two and choosing the tag with a `select`
    /// answers both: the host reads a variant's own fields and nothing else, so the word a `None`
    /// leaves behind is never looked at. A branch would be an `if` over two allocations, two
    /// arena bumps and a join — correct, and three times the code for a value that fits in a
    /// register either way.
    ///
    /// The tags are read off the layout rather than written down. `Option`'s variants sort to
    /// `None` then `Some`, which is a fact about two strings and not one to hardcode here.
    fn index_of(&mut self, ty: &Ty, hay: &Val, needle: &Val, span: Span) -> Result<Val, String> {
        let found = self.fresh();
        self.line(format!(
            "{found} = call i64 @\"beck.str.find\"(i64 {}, i64 {})",
            hay.text, needle.text
        ));
        let missing = self.fresh();
        self.line(format!("{missing} = icmp slt i64 {found}, 0"));
        // Clamped before the conversion rather than after it: `-1` is not a byte offset, and a
        // walk that started there would read backwards off the front of the string.
        let safe = self.fresh();
        self.line(format!("{safe} = select i1 {missing}, i64 0, i64 {found}"));
        let index = self.fresh();
        self.line(format!(
            "{index} = call i64 @\"beck.str.charat\"(i64 {}, i64 {safe})",
            hay.text
        ));
        // `-1` when it is missing, so the shape `some_or_none` answers is the shape here too.
        let answer = self.fresh();
        self.line(format!(
            "{answer} = select i1 {missing}, i64 -1, i64 {index}"
        ));
        self.some_or_none(ty, &answer, span)
    }

    /// The layout of the `Option[T]` a primitive answers with: the repr, its two tags, which word
    /// `Some`'s payload goes in, and how much to allocate for either.
    ///
    /// Resolved from the node's own type, which is the prelude's `Option` instantiated by the
    /// checker — so this needs no special case and would answer for a program's own union of the
    /// same shape. `want` is what the payload has to be, which is the element type for a list and
    /// an `Int` for a search.
    fn option_of(&mut self, ty: &Ty, want: Repr) -> Result<(Repr, u32, u32, usize, u64), String> {
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

    /// `try: block` — run the block under a handler, and reify one failure as a `Result[T, E]`.
    ///
    /// The block is emitted **inline**, not as a closure applied. The checker wraps it in a `lam`
    /// of no parameters so that the evaluator can delay it; here there is nothing to delay, and
    /// inlining is what puts the block's own calls under the handler — a `beck.lam.N` called
    /// through an application would check the cell inside *its* frame and leave through its own
    /// exit.
    ///
    /// Emitting it for a **value** is load-bearing and not a style: a call in tail position is a
    /// `musttail` that does not check the error cell (there is no frame left to check in), which is
    /// correct at the top of a function and would walk straight through a handler. `Dest::Value`
    /// is what guarantees no such call is emitted inside one.
    ///
    /// What the handler does is the evaluator's `Prim::Try`, word for word: a raise of the caught
    /// type becomes `Err(value)`, and **everything else keeps travelling** — a fault is not a
    /// failure, and a different error type belongs to a handler further out.
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
            .repr(ty)
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
            .map(super::heap::Variant::bytes)
            .max()
            .unwrap_or(heap::WORD);

        let handler = self.label("try.handler");
        let join = self.label("try.join");
        self.handlers.push(handler.clone());
        let value = self.expr(body, Dest::Value);
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
        let good = self.alloc(bytes, span);
        self.store_word(&good, 0, &ok.to_string());
        self.store_field(&good, ok_slot, &value);
        let from_ok = self.label.clone();
        self.terminate(format!("br label %{join}"));

        // The handler. Two tests and no search: is this failure a raise at all, and is it the one
        // this `try:` names. Anything else leaves for the *enclosing* handler with the cell
        // untouched, which is what "a different error type belongs to a handler further out" is.
        self.start(handler);
        let out = self.escape();
        let code = self.fresh();
        self.line(format!("{code} = load i32, ptr %err"));
        let raised = self.fresh();
        self.line(format!(
            "{raised} = icmp eq i32 {code}, {}",
            Trap::Raised.code()
        ));
        let named = self.label("try.named");
        self.terminate(format!("br i1 {raised}, label %{named}, label %{out}"));

        self.start(named);
        let slot = self.fresh();
        self.line(format!(
            "{slot} = getelementptr inbounds i8, ptr %err, i64 16"
        ));
        let got = self.fresh();
        self.line(format!("{got} = load i64, ptr {slot}"));
        let want = self.literal(name);
        let mine = self.fresh();
        self.line(format!("{mine} = icmp eq i64 {got}, {want}"));
        let caught = self.label("try.caught");
        self.terminate(format!("br i1 {mine}, label %{caught}, label %{out}"));

        self.start(caught);
        // Handled: the cell is cleared, because the failure stops here and whatever this function
        // does next must not look like it is still failing.
        //
        // The **whole word**, which is the code and the span together. Clearing the code alone
        // leaves the raise's span behind, and the worker's loop reads that word as one `i64` to
        // decide whether the call answered — so a caught failure would come back with a stale span
        // in the high half and be treated as a trap by everything downstream.
        self.line("store i64 0, ptr %err");
        let pl = self.fresh();
        self.line(format!("{pl} = getelementptr inbounds i8, ptr %err, i64 8"));
        let pair = self.fresh();
        self.line(format!("{pair} = load i64, ptr {pl}"));
        let held = self.load_field(&pair, 1, err_ty);
        let bad = self.alloc(bytes, span);
        self.store_word(&bad, 0, &err.to_string());
        self.store_field(&bad, err_slot, &held);
        let from_err = self.label.clone();
        self.terminate(format!("br label %{join}"));

        self.start(join);
        let r = self.fresh();
        self.line(format!(
            "{r} = phi i64 [ {good}, %{from_ok} ], [ {bad}, %{from_err} ]"
        ));
        Ok(Val { text: r, ty: repr })
    }

    /// `Some(value = i)` when `found` is not `-1`, and `None()` when it is.
    ///
    /// The shape both searches answer with, and the reason neither needs a branch: see
    /// [`Function::list_get`].
    fn some_or_none(&mut self, ty: &Ty, found: &str, span: Span) -> Result<Val, String> {
        let (repr, some, none, slot, bytes) = self.option_of(ty, Repr::Int)?;
        let missing = self.fresh();
        self.line(format!("{missing} = icmp slt i64 {found}, 0"));
        let off = self.alloc(bytes, span);
        let tag = self.fresh();
        self.line(format!(
            "{tag} = select i1 {missing}, i64 {none}, i64 {some}"
        ));
        self.store_word(&off, 0, &tag);
        self.store_word(&off, slot, found);
        Ok(Val {
            text: off,
            ty: repr,
        })
    }

    /// `{}` — and only `{}`.
    ///
    /// A map's keys are laid out **in key order**, and a literal's keys are expressions, so
    /// building a non-empty one means sorting at run time. That is a sort in emitted code for a
    /// form that is almost always written empty — every `durable` fold in this tree starts at `{}`
    /// — so the empty one compiles and the rest is refused by name until something needs it.
    fn map_lit(&mut self, ty: &Ty, kvs: &[(Core, Core)], span: Span) -> Result<Val, String> {
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
        Ok(Val {
            text: "0".to_string(),
            ty: repr,
        })
    }

    /// A string literal's offset in the pool, interned on the spot.
    fn literal(&mut self, s: &str) -> u64 {
        self.uses_heap = true;
        let at = self.heap.intern(s);
        self.heap.string_offset(at)
    }

    /// The `Some` tag, the slot its payload is in, and what that payload is.
    ///
    /// For *consuming* an `Option`, where [`Function::option_of`] is for answering with one. The
    /// evaluator reads these by **name** — `variant() == Some("Some")` and `field("value")` — so
    /// this does too, and a union of a program's own with the same two names is answered for
    /// exactly as the tree-walker answers for it.
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

    /// A raw word read back as the value its [`Repr`] says it is.
    fn word_as(&mut self, w: &str, repr: Repr) -> String {
        match repr {
            Repr::Bool => {
                let r = self.fresh();
                self.line(format!("{r} = icmp ne i64 {w}, 0"));
                r
            }
            Repr::Float => {
                let r = self.fresh();
                self.line(format!("{r} = bitcast i64 {w} to double"));
                r
            }
            _ => w.to_string(),
        }
    }

    /// Insist an argument is a map, and say which map it is.
    fn map_arg(&self, v: &Val, op: Prim) -> Result<u32, String> {
        match v.ty {
            Repr::Map(at) => Ok(at),
            _ => Err(format!("`{}` on something that is not a Map", op.name())),
        }
    }

    /// How many entries, which is the root's size word — or zero, for the empty map.
    fn map_len(&mut self, m: &Val) -> String {
        self.uses_heap = true;
        let r = self.fresh();
        self.line(format!("{r} = call i64 @\"beck.map.size\"(i64 {})", m.text));
        r
    }

    /// `map_get` — an `Option[V]` from the node a search answered, and **no branch**.
    ///
    /// [`Function::list_get`]'s trick, one type over: the search answers a node or `0`, and reading
    /// the value word of node `0` reads the arena's first bytes rather than past its end. The `None`
    /// tag means nobody looks at what that was.
    fn map_get(
        &mut self,
        ty: &Ty,
        m: &Val,
        found: &str,
        value: Repr,
        span: Span,
    ) -> Result<Val, String> {
        let (option, some, none, slot, bytes) = self.option_of(ty, value)?;
        let _ = m;
        let inside = self.fresh();
        self.line(format!("{inside} = icmp ne i64 {found}, 0"));
        let w = self.fresh();
        self.line(format!("{w} = call i64 @\"beck.map.value\"(i64 {found})"));

        let off = self.alloc(bytes, span);
        let tag = self.fresh();
        self.line(format!(
            "{tag} = select i1 {inside}, i64 {some}, i64 {none}"
        ));
        self.store_word(&off, 0, &tag);
        self.store_word(&off, slot, &w);
        Ok(Val {
            text: off,
            ty: option,
        })
    }

    /// `map_keys` and `map_values`: one run of the data area copied into a fresh list.
    fn map_run(&mut self, op: Prim, ty: &Ty, m: &Val, span: Span) -> Result<Val, String> {
        let repr = self
            .repr(ty)
            .map_err(|why| format!("answers with a value that is {why}"))?;
        if !matches!(repr, Repr::List(_)) {
            return Err(format!(
                "`{}` answers with `{ty}`, which is not a list",
                op.name()
            ));
        }
        // Which word of a node to take: the key or the value, which is the only thing the two
        // walks differ by.
        let slot = if op == Prim::MapKeys {
            heap::NODE_KEY
        } else {
            heap::NODE_VALUE
        };
        self.uses_heap = true;
        let idx = self.span(span);
        let r = self.fresh();
        self.line(format!(
            "{r} = call i64 @\"beck.map.run\"(ptr %err, i64 {}, i64 {slot}, i32 {idx})",
            m.text
        ));
        self.check_call();
        Ok(Val { text: r, ty: repr })
    }

    /// Insist an argument is a list, and say which list it is.
    fn list_arg(&self, v: &Val, op: Prim) -> Result<u32, String> {
        match v.ty {
            Repr::List(at) => Ok(at),
            _ => Err(format!("`{}` on something that is not a list", op.name())),
        }
    }

    /// How many elements, which is the header word.
    fn list_len(&mut self, xs: &Val) -> String {
        self.uses_heap = true;
        let base = self.base();
        let p = self.fresh();
        self.line(format!(
            "{p} = getelementptr inbounds i8, ptr {base}, i64 {}",
            xs.text
        ));
        let r = self.fresh();
        self.line(format!("{r} = load i64, ptr {p}"));
        r
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
        ty: &Ty,
        at: u32,
        xs: &Val,
        index: &Val,
        span: Span,
    ) -> Result<Val, String> {
        let element = self.heap.element(at);
        let (option, some, none, slot, bytes) = self.option_of(ty, element)?;
        let n = self.list_len(xs);
        let low = self.fresh();
        self.line(format!("{low} = icmp sge i64 {}, 0", index.text));
        let high = self.fresh();
        self.line(format!("{high} = icmp slt i64 {}, {n}", index.text));
        let inside = self.fresh();
        self.line(format!("{inside} = and i1 {low}, {high}"));
        let safe = self.fresh();
        self.line(format!(
            "{safe} = select i1 {inside}, i64 {}, i64 0",
            index.text
        ));
        let element_at = self.element_addr(xs, &safe);
        let header = self.fresh();
        let base = self.base();
        self.line(format!(
            "{header} = getelementptr inbounds i8, ptr {base}, i64 {}",
            xs.text
        ));
        let p = self.fresh();
        self.line(format!(
            "{p} = select i1 {inside}, ptr {element_at}, ptr {header}"
        ));
        let w = self.fresh();
        self.line(format!("{w} = load i64, ptr {p}"));

        let off = self.alloc(bytes, span);
        let tag = self.fresh();
        self.line(format!(
            "{tag} = select i1 {inside}, i64 {some}, i64 {none}"
        ));
        self.store_word(&off, 0, &tag);
        self.store_word(&off, slot, &w);
        Ok(Val {
            text: off,
            ty: option,
        })
    }

    /// `list_slice`, `list_take` and `list_drop`: one clamped range, one copy.
    ///
    /// Clamped exactly where the evaluator clamps — a negative start or count is zero, and a range
    /// past the end stops at the end — so the three differ only in the arithmetic above the copy.
    fn list_range(&mut self, op: Prim, vals: &[Val], span: Span) -> Result<Val, String> {
        let n = self.list_len(&vals[0]);
        let zero = |me: &mut Self, v: &str| {
            let neg = me.fresh();
            me.line(format!("{neg} = icmp slt i64 {v}, 0"));
            let r = me.fresh();
            me.line(format!("{r} = select i1 {neg}, i64 0, i64 {v}"));
            r
        };
        let (from, count) = match op {
            Prim::ListSlice => {
                let from = zero(self, &vals[1].text);
                let want = zero(self, &vals[2].text);
                (from, want)
            }
            Prim::ListTake => ("0".to_string(), zero(self, &vals[1].text)),
            _ => {
                let from = zero(self, &vals[1].text);
                (from, n.clone())
            }
        };
        // `from` first, then how many are left after it, then how many were asked for.
        let over = self.fresh();
        self.line(format!("{over} = icmp ugt i64 {from}, {n}"));
        let start = self.fresh();
        self.line(format!("{start} = select i1 {over}, i64 {n}, i64 {from}"));
        let left = self.fresh();
        self.line(format!("{left} = sub i64 {n}, {start}"));
        let too_many = self.fresh();
        self.line(format!("{too_many} = icmp ugt i64 {count}, {left}"));
        let take = self.fresh();
        self.line(format!(
            "{take} = select i1 {too_many}, i64 {left}, i64 {count}"
        ));

        self.uses_heap = true;
        let idx = self.span(span);
        let r = self.fresh();
        self.line(format!(
            "{r} = call i64 @\"beck.list.copy\"(ptr %err, i64 {}, i64 {start}, i64 {take}, i32 {idx})",
            vals[0].text
        ));
        self.check_call();
        Ok(Val {
            text: r,
            ty: vals[0].ty,
        })
    }

    /// Insist an argument is text, so a message names the primitive rather than the operand.
    fn text_arg(&self, v: &Val, op: Prim) -> Result<(), String> {
        if v.ty == Repr::Str {
            Ok(())
        } else {
            Err(format!("`{}` on something that is not a Str", op.name()))
        }
    }

    /// One word of a `Str`'s header: `0` is its bytes and `8` is its characters.
    fn text_word(&mut self, s: &Val, at: u64) -> String {
        self.uses_heap = true;
        let base = self.fresh();
        self.line(format!("{base} = load ptr, ptr @\"beck.heap\""));
        let off = self.fresh();
        self.line(format!("{off} = add i64 {}, {at}", s.text));
        let p = self.fresh();
        self.line(format!(
            "{p} = getelementptr inbounds i8, ptr {base}, i64 {off}"
        ));
        let r = self.fresh();
        self.line(format!("{r} = load i64, ptr {p}"));
        r
    }

    /// One of [`TEXT`]'s allocating functions: the error cell, the arguments, the span.
    fn text_call(&mut self, which: &str, args: &[&Val], ty: Repr, span: Span) -> Val {
        self.uses_heap = true;
        let idx = self.span(span);
        let mut operands = String::from("ptr %err");
        for a in args {
            let _ = write!(operands, ", i64 {}", a.text);
        }
        let r = self.fresh();
        self.line(format!(
            "{r} = call i64 @\"beck.str.{which}\"({operands}, i32 {idx})"
        ));
        // Allocating means it can exhaust the arena, and a caller that ignored that would carry a
        // `0` offset into the next load.
        self.check_call();
        Val { text: r, ty }
    }

    /// `contains`, `starts_with` and `ends_with`, which are one search and two length tests.
    fn text_search(&mut self, op: Prim, hay: &Val, needle: &Val) -> Val {
        self.uses_heap = true;
        let r = self.fresh();
        if op == Prim::StrContains {
            self.line(format!(
                "{r} = call i64 @\"beck.str.find\"(i64 {}, i64 {})",
                hay.text, needle.text
            ));
            let out = self.fresh();
            self.line(format!("{out} = icmp sge i64 {r}, 0"));
            return Val {
                text: out,
                ty: Repr::Bool,
            };
        }
        let lh = self.text_word(hay, 0);
        let ln = self.text_word(needle, 0);
        let fits = self.fresh();
        self.line(format!("{fits} = icmp ule i64 {ln}, {lh}"));
        // The comparison runs either way and its operands are clamped to a length that fits, so
        // there is no branch: `memcmp` of zero bytes is zero, and a needle longer than the haystack
        // is refused by `%fits` rather than by not being looked at.
        let start = if op == Prim::StrStartsWith {
            "0".to_string()
        } else {
            let d = self.fresh();
            self.line(format!("{d} = sub i64 {lh}, {ln}"));
            let ok = self.fresh();
            self.line(format!("{ok} = select i1 {fits}, i64 {d}, i64 0"));
            ok
        };
        let n = self.fresh();
        self.line(format!("{n} = select i1 {fits}, i64 {ln}, i64 0"));
        let ph = self.fresh();
        self.line(format!(
            "{ph} = call ptr @\"beck.str.data\"(i64 {})",
            hay.text
        ));
        let at = self.fresh();
        self.line(format!(
            "{at} = getelementptr inbounds i8, ptr {ph}, i64 {start}"
        ));
        let pn = self.fresh();
        self.line(format!(
            "{pn} = call ptr @\"beck.str.data\"(i64 {})",
            needle.text
        ));
        let c = self.fresh();
        self.line(format!(
            "{c} = call i32 @memcmp(ptr {at}, ptr {pn}, i64 {n})"
        ));
        let same = self.fresh();
        self.line(format!("{same} = icmp eq i32 {c}, 0"));
        let out = self.fresh();
        self.line(format!("{out} = and i1 {fits}, {same}"));
        Val {
            text: out,
            ty: Repr::Bool,
        }
    }

    fn intrinsic_f64(&mut self, name: &str, v: &Val) -> Result<String, String> {
        if v.ty != Repr::Float {
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
            ty: Repr::Int,
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
            ty: Repr::Int,
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
    /// `(0.0 * inf) > 0.0` answered `true` in the evaluator and `false` here (`docs/93` §93.3).
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
            ty: Repr::Float,
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

    fn equals(&mut self, a: &Val, b: &Val) -> Result<String, String> {
        Ok(self.compare(Prim::Eq, a, b)?.text)
    }

    /// Record that this repr's comparison has to exist, or refuse because it cannot.
    ///
    /// One method rather than three call sites, so that adding a reference kind means teaching
    /// [`heap::Repr::order`] and this — and `reachable` closes over whatever they name.
    ///
    /// It asks [`heap::Heap::ordered`] first, which is what keeps a repr with no order out of a
    /// generated comparison: the refusal names the definition that wanted one, where the same
    /// question asked while the module was being assembled would name nothing.
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
            // A closure's comparison is one word — `beck.fn.cmp` is written once for the module
            // rather than per family, because every closure's rank is in the same table.
            Repr::Fn(_) => self.compared_fns = true,
        }
        Ok(())
    }

    fn compare(&mut self, op: Prim, a: &Val, b: &Val) -> Result<Val, String> {
        // Reals compare through the order key and Bools compare unsigned, so `false < true`. Both
        // are the ordering `Value`'s derived `Ord` gives, which is the one the evaluator uses.
        // An object compares through the function `compare_functions` emitted for its layout,
        // which answers -1, 0 or 1 — so the six operators are one call and one integer test.
        let (lhs, rhs, signed) = match a.ty.order() {
            heap::Order::Key => (self.order_key(a), self.order_key(b), false),
            heap::Order::Words { signed } => (a.text.clone(), b.text.clone(), signed),
            // Nothing to compare with: `Repr::order` names the reason and this is where a program
            // that asked hears it.
            heap::Order::Absent(why) => {
                return Err(format!("compares {}, which is {why}", self.heap.show(a.ty)))
            }
            // A reference decides through the three-way comparison for whatever it refers to. The
            // symbol is `Repr::order`'s, which is the only place that names one.
            heap::Order::Call(symbol) => {
                self.wants(a.ty)
                    .map_err(|why| format!("compares {}, which is {why}", self.heap.show(a.ty)))?;
                let r = self.fresh();
                self.line(format!(
                    "{r} = call i64 @\"{symbol}\"(i64 {}, i64 {})",
                    a.text, b.text
                ));
                (r, "0".to_string(), true)
            }
        };
        let width = if a.ty == Repr::Bool { "i1" } else { "i64" };
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
        Ok(Val {
            text: r,
            ty: Repr::Bool,
        })
    }

    /// The value as an `i64`, which is how it crosses the worker's protocol and how a trap carries
    /// the scrutinee that matched nothing.
    fn widen(&mut self, v: &Val) -> String {
        match v.ty {
            Repr::Int => v.text.clone(),
            // Normalised, because the one thing that reads this is a message: `Trap::message`
            // renders it, and a scrutinee printed as `-0` where the evaluator prints `0` is a
            // divergence in the differential.
            Repr::Float => {
                let v = self.normalise(&v.text);
                let r = self.fresh();
                self.line(format!("{r} = bitcast double {} to i64", v.text));
                r
            }
            Repr::Bool => {
                let r = self.fresh();
                self.line(format!("{r} = zext i1 {} to i64", v.text));
                r
            }
            // Its offset, which is what an object *is* here. Only a trap payload reads this, and
            // the one trap that can carry an object says nothing about the value it carries —
            // `Trap::NoMatchData` is the message, and this is what makes it honest.
            Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => v.text.clone(),
        }
    }
}

/// Why a primitive this backend does not compile is not compiled.
///
/// The string half is spelled out one at a time rather than swept into "not a scalar primitive",
/// because since `docs/93` a `Str` *is* a value here and "text is not on this heap" would be
/// false. Each reason names the thing that is missing rather than the primitive that wanted it —
/// which is the difference between a refusal a reader can act on and one they can only observe.
fn refusal(op: Prim) -> String {
    let why = match op {
        Prim::ListZip => "answers with a list of pairs, and there is no pair type to lay out",
        // The higher-order half compiles — `map_list`, `filter_list`, `list_fold`, `list_all`,
        // `list_any` and `sort_by` — so what is left of it is the one that grows a list.
        Prim::ListFlatMap => {
            "answers a list whose length is the sum of the lists its function answers, which is \
             growing a list under another name"
        }

        Prim::StrUpper | Prim::StrLower => {
            "is Unicode case mapping, which is a table rather than an operation — and a compiled \
             half-answer that folded ASCII only would disagree with the evaluator on the first \
             letter that is not"
        }
        Prim::StrReplace => {
            "builds text whose size is the number of occurrences of one string in another, which \
             needs a pass to count before there is anything to allocate"
        }
        Prim::StrToInt => {
            "reads a number out of text, and has to agree with Rust's parser about every input \
             that is not one"
        }
        _ => return format!("`{}` is not one of the scalar primitives", op.name()),
    };
    format!("`{}` {why}", op.name())
}

/// A Beck name as an LLVM symbol.
///
/// Quoted and escaped rather than transliterated: identifiers are Unicode (`docs/44` §44.4), and a
/// scheme that dropped or folded characters could give two definitions one symbol.
///
/// **`beck.def.` and not `beck.`**, which is a namespace rather than a prefix. Everything this
/// module generates for itself is `beck.<something>` — `beck.dispatch`, `beck.alloc`, `beck.map.*`
/// — so a definition *called* `dispatch` used to take the dispatcher's own symbol, and the
/// assembler answered "invalid redefinition" for a program that had done nothing wrong.
/// `awfy/richards.beck` has one, and it was invisible until `docs/93` made that definition
/// compile: a collision needs both halves to exist, and one of them never had.
fn mangle(name: &str) -> String {
    let mut out = String::from("\"beck.def.");
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
/// The most patterns one arm may be split into.
///
/// A bound rather than a judgement: splitting an or-pattern copies the arm, and a pattern of
/// nested ors multiplies. Nothing in this tree needs three.
const MAX_ALTERNATIVES: usize = 16;

/// One arm's pattern as the patterns that have to be tried in turn.
///
/// An or-pattern of plain constants is left whole, because [`Function::probe`] can test it with one
/// `or` and it binds nothing. Anything else is **split**: two alternatives that take a value apart
/// bind the same names to different words, so one block reached from both would need a `phi` per
/// binder, and copying the arm is the same behaviour with no join to get wrong. `docs/91`
/// §91.1 is the bug that is not available to make here — an alternative that reaches the body
/// unexpanded is a wildcard — because there is no matrix and no lazy split: every alternative is
/// its own test.
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
                    for s in &subs {
                        let mut row = row.clone();
                        row.push((name.clone(), s.clone()));
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

// -------------------------------------------------------------------------------------------
// The module around the functions
// -------------------------------------------------------------------------------------------

/// The declarations, the arena, the compiled bodies, the dispatch table and the worker loop.
/// What the module has to write for the closures its bodies turned out to build.
///
/// Four facts rather than one set, because an application's arms are the *intersection* of what a
/// family could hold and what was emitted: a rank whose definition was refused has no function to
/// call, and an arm calling one would be a link error rather than a refusal.
struct Closures<'a> {
    /// The families a body applies. An application is written for these and no others.
    applied: &'a BTreeSet<u32>,
    /// The ranks that became a `beck.lam.N`.
    emitted: &'a BTreeSet<u32>,
    /// The definitions that compiled, for the ranks that are a definition's own lambda.
    compiled: &'a BTreeSet<Arc<str>>,
    /// Whether anything compares two closures.
    compared: bool,
    /// The higher-order list primitives that were reached, by shape.
    loops: &'a BTreeSet<(Loop, u32)>,
}

// One argument per table the module has to write, and they are seven different tables. Grouping
// them into a struct would name a thing that does not exist.
#[allow(clippy::too_many_arguments)]
fn assemble(
    bodies: &str,
    functions: &[Signature],
    heap: &Heap,
    compared: &BTreeSet<u32>,
    lists: &BTreeSet<u32>,
    maps: &BTreeSet<u32>,
    asks: bool,
    closures: &Closures<'_>,
) -> String {
    let arena = !heap.is_empty();
    let mut m = String::new();
    m.push_str(HEADER);
    if arena {
        let _ = write!(m, "{}", arena_prelude());
    }
    // A question needs the arena, because an answer is written into it — and it has one, because
    // asking interns the shape of what it answers with and a module with an interned shape is not
    // an empty heap. `debug_assert` rather than a branch: the two facts are one fact.
    debug_assert!(arena || !asks, "a module that asks the host has an arena");
    if asks {
        m.push_str(&host_call());
    }
    // The two C library functions both runtimes reach for, declared once: `LISTS` copies words and
    // `TEXT` copies and compares bytes, and a second `declare` of either would be a redefinition.
    if heap.uses_text() || heap.uses_lists() {
        m.push_str(LIBC);
    }
    if heap.uses_text() {
        m.push_str(TEXT);
    }
    // A map turns into a list — `map_keys` and `map_values` allocate one — so its runtime needs
    // lists' even when the program never writes one down.
    if heap.uses_lists() || heap.uses_maps() {
        m.push_str(LISTS);
    }
    if heap.uses_maps() {
        m.push_str(MAPS);
    }
    if heap.uses_text() {
        m.push_str(BUILDS);
    }
    // `beck.str.join` reads a list, so it needs both runtimes present.
    if heap.uses_text() && (heap.uses_lists() || heap.uses_maps()) {
        m.push_str(JOINS);
    }
    m.push_str(bodies);
    for at in compared {
        m.push_str(&compare_function(*at, heap));
    }
    for at in lists {
        m.push_str(&element_functions(*at, heap));
    }
    for at in maps {
        m.push_str(&map_functions(*at, heap));
    }
    for at in closures.applied {
        m.push_str(&apply_function(*at, heap, closures));
    }
    for (which, at) in closures.loops {
        m.push_str(&loop_function(*which, *at, heap));
    }
    // One merge sort per key repr, over the families that sort. Deduplicated here for the reason
    // every generated function is: the module has one definition of each.
    let sorted: BTreeSet<u32> = closures
        .loops
        .iter()
        .filter(|(which, _)| *which == Loop::Sort)
        .filter_map(|(_, fam)| heap.word_at(heap.family(*fam).ret))
        .collect();
    for at in sorted {
        m.push_str(&merge_sort(at, heap));
    }
    if closures.compared {
        m.push_str(FN_CMP);
    }

    // One thunk per function: the protocol carries every argument as eight bytes, so this is where
    // an `i64` becomes a `double` or an `i1` and the result becomes eight bytes again. It is also
    // where the worker learns whether this call's answer is on the heap, since only the thunk knows
    // what the function returns.
    for sig in functions {
        let _ = writeln!(
            m,
            "define internal i64 @\"beck.thunk.{}\"(ptr noalias %err, ptr %args) {{\nentry:",
            sig.index
        );
        if arena {
            let _ = writeln!(
                m,
                "  store i64 {}, ptr @\"beck.reply\"",
                u32::from(sig.ret.is_ref())
            );
        }
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
            match ty.machine() {
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
        match sig.ret.machine() {
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

    m.push_str(PIPE);
    m.push_str(&main_loop(arena));
    m
}

/// How many words a question's buffer holds.
///
/// A bound rather than a judgement: the widest question is `http_fetch`, at two words of header
/// and two per argument, and [`Upcall`] has no way to grow past this without a new primitive.
const QUESTION_WORDS: usize = 8;

/// `beck.host`: write a question, block, and take the answer back.
///
/// The whole of the second direction on the worker's side, in one function that knows nothing
/// about which primitive it is asking for — the caller has already put the shapes and the words in
/// the buffer, and what comes back is eight bytes and, when the answer is an object, some bytes to
/// append.
///
/// Three things are worth reading twice:
///
/// * **The answer is appended, never assigned.** The host is told the arena's high-water mark and
///   sends bytes to put at it, so nothing a compiled value already points at can be rewritten by
///   an answer. That is what makes servicing a question safe while the arena is live, and it is
///   why the reply carries a tail rather than a heap.
/// * **The room is checked before the bytes are read**, against the same limit `beck.alloc` uses
///   and with the same trap, because an answer that does not fit is the arena being full and not
///   the host being wrong.
/// * **A host that stops answering ends the process.** There is nothing to compute and nothing to
///   reply to: the caller's next read fails and [`crate::Worker`] says the program stopped, which
///   is the message that path already had. Storing a trap instead would mean writing a reply to a
///   pipe nobody is reading.
fn host_call() -> String {
    format!(
        r#"declare void @exit(i32)

define internal i64 @"beck.host"(i32 %op, i32 %span, i64 %name, i64 %words, ptr %buf, i64 %copy, ptr noalias %err) {{
entry:
  %hdr = alloca [4 x i64]
  %hp = load ptr, ptr @"beck.heap"
  %used = load i64, ptr @"beck.next"
  store i32 {marker}, ptr %hdr
  %sp = getelementptr inbounds i8, ptr %hdr, i64 4
  store i32 %span, ptr %sp
  %opp = getelementptr inbounds i8, ptr %hdr, i64 8
  %opw = zext i32 %op to i64
  store i64 %opw, ptr %opp
  %up = getelementptr inbounds i8, ptr %hdr, i64 16
  store i64 %used, ptr %up
  %sends = icmp ne i64 %copy, 0
  %blen = select i1 %sends, i64 %used, i64 0
  %bp = getelementptr inbounds i8, ptr %hdr, i64 24
  store i64 %blen, ptr %bp
  %w1 = call i64 @"beck.write_all"(ptr %hdr, i64 32)
  %wbytes = mul i64 %words, 8
  %w2 = call i64 @"beck.write_all"(ptr %buf, i64 %wbytes)
  %w3 = call i64 @"beck.write_all"(ptr %hp, i64 %blen)
  %r1 = call i64 @"beck.read_exact"(ptr %hdr, i64 32)
  %heard = icmp eq i64 %r1, 32
  br i1 %heard, label %answered, label %gone
gone:
  call void @exit(i32 1)
  unreachable
answered:
  %code = load i32, ptr %hdr
  %tp = getelementptr inbounds i8, ptr %hdr, i64 24
  %tail = load i64, ptr %tp
  %end = add i64 %used, %tail
  %lim = load i64, ptr @"beck.limit"
  %over = icmp ugt i64 %end, %lim
  br i1 %over, label %full, label %room
full:
  store i32 {exhausted}, ptr %err
  %fs = getelementptr inbounds i8, ptr %err, i64 4
  store i32 %span, ptr %fs
  %fp = getelementptr inbounds i8, ptr %err, i64 8
  store i64 0, ptr %fp
  ret i64 0
room:
  %at = getelementptr inbounds i8, ptr %hp, i64 %used
  %r2 = call i64 @"beck.read_exact"(ptr %at, i64 %tail)
  %whole = icmp eq i64 %r2, %tail
  br i1 %whole, label %kept, label %gone
kept:
  store i64 %end, ptr @"beck.next"
  %fine = icmp eq i32 %code, 0
  br i1 %fine, label %value, label %failed
failed:
  store i32 %code, ptr %err
  %es = getelementptr inbounds i8, ptr %err, i64 4
  store i32 %span, ptr %es
  %pp = getelementptr inbounds i8, ptr %hdr, i64 8
  %pl = load i64, ptr %pp
  %ep = getelementptr inbounds i8, ptr %err, i64 8
  store i64 %pl, ptr %ep
  ; The type name a `try:` compares against, written by this module because only this module knows
  ; which literal's offset it is.
  %en = getelementptr inbounds i8, ptr %err, i64 16
  store i64 %name, ptr %en
  ret i64 0
value:
  %vp = getelementptr inbounds i8, ptr %hdr, i64 16
  %v = load i64, ptr %vp
  ret i64 %v
}}

"#,
        marker = Upcall::MARKER as i32,
        exhausted = Trap::HeapExhausted.code(),
    )
}

/// The arena: two globals, a pointer, and the only allocator this backend has.
///
/// A bump pointer and no free. The whole of the reasoning is in
/// [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md): a call is
/// bounded, the arena is reset before every one, and a collector is a design with a cost that this
/// backend has not measured a need for. What it means for a program that allocates without bound
/// *within* one call is [`Trap::HeapExhausted`], which is a message rather than a crash.
fn arena_prelude() -> String {
    format!(
        r#"declare ptr @malloc(i64)

@"beck.heap" = internal global ptr null
@"beck.next" = internal global i64 {first}
@"beck.limit" = internal global i64 0
@"beck.reply" = internal global i64 0

define internal i64 @"beck.alloc"(ptr noalias %err, i64 %bytes, i32 %span) {{
entry:
  %n = load i64, ptr @"beck.next"
  %lim = load i64, ptr @"beck.limit"
  %new = add i64 %n, %bytes
  ; Unsigned, and against the sum rather than against the room left: a null arena has a limit of
  ; zero, and `limit - next` would underflow into "plenty".
  %over = icmp ugt i64 %new, %lim
  br i1 %over, label %full, label %ok
full:
  store i32 {code}, ptr %err
  %sp = getelementptr inbounds i8, ptr %err, i64 4
  store i32 %span, ptr %sp
  %pl = getelementptr inbounds i8, ptr %err, i64 8
  store i64 0, ptr %pl
  ret i64 0
ok:
  store i64 %new, ptr @"beck.next"
  ret i64 %n
}}

"#,
        first = heap::FIRST,
        code = Trap::HeapExhausted.code(),
    )
}

/// A stable merge sort over two parallel runs of words: the keys, and the elements they decorate.
///
/// Generated per **key** repr rather than per family, because what it needs to know is how to
/// compare two key words and nothing else — `beck.elem.cmp.{at}` is that, and it is the same
/// function a list's own comparison and search use.
///
/// Recursive rather than bottom-up, which is the difference between a function with one loop in it
/// and a function with three nested ones. The depth is `log n` on the host's stack, and `n` is
/// bounded by the arena.
///
/// **Stability is the property that matters**, and it is one `<=`: on equal keys the element from
/// the left run goes first. `beck-eval` says why it matters rather than being nice — the input order
/// is itself deterministic (a `Map`'s values come out in key order), so a stable sort is what makes
/// the answer total without a second key, and a replay reproduces it.
fn merge_sort(at: u32, heap: &Heap) -> String {
    let shown = heap.show(heap.element(at));
    format!(
        r#"; a stable merge sort keyed by {shown}
define internal void @"beck.list.msort.{at}"(ptr %keys, ptr %vals, ptr %tk, ptr %tv, i64 %lo, i64 %hi) {{
entry:
  %span = sub i64 %hi, %lo
  %tiny = icmp ule i64 %span, 1
  br i1 %tiny, label %done, label %split
split:
  %half = udiv i64 %span, 2
  %mid = add i64 %lo, %half
  call void @"beck.list.msort.{at}"(ptr %keys, ptr %vals, ptr %tk, ptr %tv, i64 %lo, i64 %mid)
  call void @"beck.list.msort.{at}"(ptr %keys, ptr %vals, ptr %tk, ptr %tv, i64 %mid, i64 %hi)
  br label %merge
merge:
  ; Three indices: where each run is up to, and where the answer is going.
  %i = phi i64 [ %lo, %split ], [ %i1, %took ]
  %j = phi i64 [ %mid, %split ], [ %j1, %took ]
  %k = phi i64 [ %lo, %split ], [ %k1, %took ]
  %filled = icmp uge i64 %k, %hi
  br i1 %filled, label %back, label %pick
pick:
  %left.left = icmp ult i64 %i, %mid
  br i1 %left.left, label %maybe, label %right
maybe:
  %right.gone = icmp uge i64 %j, %hi
  br i1 %right.gone, label %left, label %compare
compare:
  %ka.at = getelementptr inbounds i64, ptr %keys, i64 %i
  %kb.at = getelementptr inbounds i64, ptr %keys, i64 %j
  %ka = load i64, ptr %ka.at
  %kb = load i64, ptr %kb.at
  %c = call i64 @"beck.elem.cmp.{at}"(i64 %ka, i64 %kb)
  ; `<=`, which is the whole of the stability: on equal keys the left run goes first.
  %take.left = icmp sle i64 %c, 0
  br i1 %take.left, label %left, label %right
left:
  %lk.at = getelementptr inbounds i64, ptr %keys, i64 %i
  %lv.at = getelementptr inbounds i64, ptr %vals, i64 %i
  %lk = load i64, ptr %lk.at
  %lv = load i64, ptr %lv.at
  %il = add i64 %i, 1
  br label %took
right:
  %rk.at = getelementptr inbounds i64, ptr %keys, i64 %j
  %rv.at = getelementptr inbounds i64, ptr %vals, i64 %j
  %rk = load i64, ptr %rk.at
  %rv = load i64, ptr %rv.at
  %jr = add i64 %j, 1
  br label %took
took:
  %key = phi i64 [ %lk, %left ], [ %rk, %right ]
  %val = phi i64 [ %lv, %left ], [ %rv, %right ]
  %i1 = phi i64 [ %il, %left ], [ %i, %right ]
  %j1 = phi i64 [ %j, %left ], [ %jr, %right ]
  %ok.at = getelementptr inbounds i64, ptr %tk, i64 %k
  %ov.at = getelementptr inbounds i64, ptr %tv, i64 %k
  store i64 %key, ptr %ok.at
  store i64 %val, ptr %ov.at
  %k1 = add i64 %k, 1
  br label %merge
back:
  ; The merged run, back where the caller's half of it was.
  %bytes = mul i64 %span, 8
  %from.k = getelementptr inbounds i64, ptr %tk, i64 %lo
  %to.k = getelementptr inbounds i64, ptr %keys, i64 %lo
  %copied.k = call ptr @memcpy(ptr %to.k, ptr %from.k, i64 %bytes)
  %from.v = getelementptr inbounds i64, ptr %tv, i64 %lo
  %to.v = getelementptr inbounds i64, ptr %vals, i64 %lo
  %copied.v = call ptr @memcpy(ptr %to.v, ptr %from.v, i64 %bytes)
  br label %done
done:
  ret void
}}

"#
    )
}

/// The list primitives whose argument is a function, each one loop.
///
/// Generated per family rather than written inline, for the reason a list's comparison is: the
/// emitter's own output is straight-line, so a loop here would be the first place it needed `phi`
/// nodes of its own. One function per (primitive, family) is also one function per *shape* rather
/// than per call site, and the shape is what decides how a word becomes an argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Loop {
    /// `map_list` — one answer per element, into a list allocated up front.
    Map,
    /// `filter_list` — the answers that are `true`, into a list allocated for all of them.
    Filter,
    /// `list_fold` — the accumulator through every element.
    Fold,
    /// `list_all` and `list_any`, which are one loop and a flag.
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
}

/// One of [`Loop`]'s four, for one family.
///
/// What every one of them shares: the element is a **word** in the arena and the closure takes a
/// value, so each iteration converts one way and — for `map` — back again. And every call to
/// `beck.apply` is followed by a look at the error cell, because a closure can trap and a loop that
/// carried on would run the rest of the program's iterations after the failure the caller is about
/// to report.
fn loop_function(which: Loop, fam: u32, heap: &Heap) -> String {
    let family = heap.family(fam);
    let symbol = which.symbol(fam);
    let mut b = Text::new();
    let _ = writeln!(b.out, "; {} over {}", which.symbol(fam), family.shown);

    match which {
        Loop::Map => {
            let element = family.params[0];
            let _ = writeln!(
                b.out,
                "define internal i64 @\"{symbol}\"(ptr noalias %err, i64 %xs, i64 %clo, i32 %span) {{\nentry:"
            );
            b.line("%n = call i64 @\"beck.list.len\"(i64 %xs)".into());
            b.line("%src = call ptr @\"beck.list.data\"(i64 %xs)".into());
            b.line("%out = call i64 @\"beck.list.alloc\"(ptr %err, i64 %n, i32 %span)".into());
            b.checked("ready");
            b.block("ready");
            b.line("%dst = call ptr @\"beck.list.data\"(i64 %out)".into());
            b.line("br label %loop".into());
            b.block("loop");
            b.line("%i = phi i64 [ 0, %ready ], [ %j, %next ]".into());
            b.line("%past = icmp uge i64 %i, %n".into());
            b.line("br i1 %past, label %done, label %one".into());
            b.block("one");
            b.line("%at = getelementptr inbounds i64, ptr %src, i64 %i".into());
            b.line("%w = load i64, ptr %at".into());
            let arg = b.value_of("%w", element);
            let r = b.fresh();
            b.line(format!(
                "{r} = call tailcc {} @\"beck.apply.{fam}\"(ptr %err, i64 %clo, {} {arg})",
                family.ret.llvm(),
                element.llvm()
            ));
            b.checked("store");
            b.block("store");
            let word = b.word_of(&r, family.ret);
            b.line("%to = getelementptr inbounds i64, ptr %dst, i64 %i".into());
            b.line(format!("store i64 {word}, ptr %to"));
            b.line("br label %next".into());
            b.block("next");
            b.line("%j = add i64 %i, 1".into());
            b.line("br label %loop".into());
            b.block("done");
            b.line("ret i64 %out".into());
            b.block("failed");
            b.line("ret i64 0".into());
        }
        Loop::Filter => {
            let element = family.params[0];
            let _ = writeln!(
                b.out,
                "define internal i64 @\"{symbol}\"(ptr noalias %err, i64 %xs, i64 %clo, i32 %span) {{\nentry:"
            );
            b.line("%n = call i64 @\"beck.list.len\"(i64 %xs)".into());
            b.line("%src = call ptr @\"beck.list.data\"(i64 %xs)".into());
            // Room for every element, and the header written at the end says how many were kept.
            // One pass rather than a count and a fill: a predicate called twice per element would
            // double what a filter costs to save arena the next allocation does not need.
            b.line("%out = call i64 @\"beck.list.alloc\"(ptr %err, i64 %n, i32 %span)".into());
            b.checked("ready");
            b.block("ready");
            b.line("%dst = call ptr @\"beck.list.data\"(i64 %out)".into());
            b.line("br label %loop".into());
            b.block("loop");
            b.line("%i = phi i64 [ 0, %ready ], [ %j, %next ]".into());
            b.line("%k = phi i64 [ 0, %ready ], [ %kn, %next ]".into());
            b.line("%past = icmp uge i64 %i, %n".into());
            b.line("br i1 %past, label %done, label %one".into());
            b.block("one");
            b.line("%at = getelementptr inbounds i64, ptr %src, i64 %i".into());
            b.line("%w = load i64, ptr %at".into());
            let arg = b.value_of("%w", element);
            let r = b.fresh();
            b.line(format!(
                "{r} = call tailcc i1 @\"beck.apply.{fam}\"(ptr %err, i64 %clo, {} {arg})",
                element.llvm()
            ));
            b.checked("decide");
            b.block("decide");
            b.line(format!("br i1 {r}, label %take, label %skip"));
            b.block("take");
            b.line("%to = getelementptr inbounds i64, ptr %dst, i64 %k".into());
            b.line("store i64 %w, ptr %to".into());
            b.line("%k1 = add i64 %k, 1".into());
            b.line("br label %next".into());
            b.block("skip");
            b.line("br label %next".into());
            b.block("next");
            b.line("%kn = phi i64 [ %k1, %take ], [ %k, %skip ]".into());
            b.line("%j = add i64 %i, 1".into());
            b.line("br label %loop".into());
            b.block("done");
            // The count, at last: the list is as long as what was kept, and the words after it are
            // arena nobody reads — bounded by the input's length and given back when it is reset.
            b.line("%hp = load ptr, ptr @\"beck.heap\"".into());
            b.line("%hdr = getelementptr inbounds i8, ptr %hp, i64 %out".into());
            b.line("store i64 %k, ptr %hdr".into());
            b.line("ret i64 %out".into());
            b.block("failed");
            b.line("ret i64 0".into());
        }
        Loop::Fold => {
            let acc = family.ret;
            let element = family.params[1];
            let _ = writeln!(
                b.out,
                "define internal {} @\"{symbol}\"(ptr noalias %err, i64 %xs, {} %init, i64 %clo, i32 %span) {{\nentry:",
                acc.llvm(),
                acc.llvm()
            );
            b.line("%n = call i64 @\"beck.list.len\"(i64 %xs)".into());
            b.line("%src = call ptr @\"beck.list.data\"(i64 %xs)".into());
            b.line("br label %loop".into());
            b.block("loop");
            b.line("%i = phi i64 [ 0, %entry ], [ %j, %next ]".into());
            b.line(format!(
                "%acc = phi {} [ %init, %entry ], [ %new, %next ]",
                acc.llvm()
            ));
            b.line("%past = icmp uge i64 %i, %n".into());
            b.line("br i1 %past, label %done, label %one".into());
            b.block("one");
            b.line("%at = getelementptr inbounds i64, ptr %src, i64 %i".into());
            b.line("%w = load i64, ptr %at".into());
            let arg = b.value_of("%w", element);
            b.line(format!(
                "%new = call tailcc {} @\"beck.apply.{fam}\"(ptr %err, i64 %clo, {} %acc, {} {arg})",
                acc.llvm(),
                acc.llvm(),
                element.llvm()
            ));
            b.checked("next");
            b.block("next");
            b.line("%j = add i64 %i, 1".into());
            b.line("br label %loop".into());
            b.block("done");
            b.line(format!("ret {} %acc", acc.llvm()));
            b.block("failed");
            b.line(format!("ret {} {}", acc.llvm(), acc.machine().zero()));
        }
        Loop::Sort => {
            let element = family.params[0];
            let key = family.ret;
            let at = heap
                .word_at(key)
                .expect("the key's repr was interned when the sort was met");
            let _ = writeln!(
                b.out,
                "define internal i64 @\"{symbol}\"(ptr noalias %err, i64 %xs, i64 %clo, i32 %span) {{\nentry:"
            );
            b.line("%n = call i64 @\"beck.list.len\"(i64 %xs)".into());
            b.line("%src = call ptr @\"beck.list.data\"(i64 %xs)".into());
            // Four runs of `n` words: the keys, the elements, and a scratch pair the merge writes
            // into. `beck.list.copy` gives the elements as a fresh list, which is also the list this
            // answers with — so the sort is in place in something nobody else holds.
            b.line("%keys = call i64 @\"beck.list.alloc\"(ptr %err, i64 %n, i32 %span)".into());
            b.checked("two");
            b.block("two");
            b.line(
                "%vals = call i64 @\"beck.list.copy\"(ptr %err, i64 %xs, i64 0, i64 %n, i32 %span)"
                    .into(),
            );
            b.checked("three");
            b.block("three");
            b.line("%tk = call i64 @\"beck.list.alloc\"(ptr %err, i64 %n, i32 %span)".into());
            b.checked("four");
            b.block("four");
            b.line("%tv = call i64 @\"beck.list.alloc\"(ptr %err, i64 %n, i32 %span)".into());
            b.checked("ready");
            b.block("ready");
            b.line("%pk = call ptr @\"beck.list.data\"(i64 %keys)".into());
            b.line("%pv = call ptr @\"beck.list.data\"(i64 %vals)".into());
            b.line("%ptk = call ptr @\"beck.list.data\"(i64 %tk)".into());
            b.line("%ptv = call ptr @\"beck.list.data\"(i64 %tv)".into());
            b.line("br label %loop".into());

            // Decorate: one key per element, and the closure is applied exactly `n` times, which is
            // what the evaluator does — `beck-eval` decorates, sorts and undecorates too.
            b.block("loop");
            b.line("%i = phi i64 [ 0, %ready ], [ %j, %next ]".into());
            b.line("%past = icmp uge i64 %i, %n".into());
            b.line("br i1 %past, label %sort, label %one".into());
            b.block("one");
            b.line("%at = getelementptr inbounds i64, ptr %src, i64 %i".into());
            b.line("%w = load i64, ptr %at".into());
            let arg = b.value_of("%w", element);
            let r = b.fresh();
            b.line(format!(
                "{r} = call tailcc {} @\"beck.apply.{fam}\"(ptr %err, i64 %clo, {} {arg})",
                key.llvm(),
                element.llvm()
            ));
            b.checked("store");
            b.block("store");
            let word = b.word_of(&r, key);
            b.line("%to = getelementptr inbounds i64, ptr %pk, i64 %i".into());
            b.line(format!("store i64 {word}, ptr %to"));
            b.line("br label %next".into());
            b.block("next");
            b.line("%j = add i64 %i, 1".into());
            b.line("br label %loop".into());

            b.block("sort");
            b.line(format!(
                "call void @\"beck.list.msort.{at}\"(ptr %pk, ptr %pv, ptr %ptk, ptr %ptv, i64 0, i64 %n)"
            ));
            b.line("ret i64 %vals".into());
            b.block("failed");
            b.line("ret i64 0".into());
            let _ = writeln!(b.out, "}}\n");
            // The sort itself is generated once per **key** repr rather than here, because two
            // families can sort by the same kind of key and a second `define` of one function is a
            // module `clang` refuses.
            return b.out;
        }
        Loop::Every => {
            let element = family.params[0];
            let _ = writeln!(
                b.out,
                "define internal i1 @\"{symbol}\"(ptr noalias %err, i64 %xs, i64 %clo, i1 %want, i32 %span) {{\nentry:"
            );
            b.line("%n = call i64 @\"beck.list.len\"(i64 %xs)".into());
            b.line("%src = call ptr @\"beck.list.data\"(i64 %xs)".into());
            b.line("br label %loop".into());
            b.block("loop");
            b.line("%i = phi i64 [ 0, %entry ], [ %j, %next ]".into());
            b.line("%past = icmp uge i64 %i, %n".into());
            b.line("br i1 %past, label %exhausted, label %one".into());
            b.block("one");
            b.line("%at = getelementptr inbounds i64, ptr %src, i64 %i".into());
            b.line("%w = load i64, ptr %at".into());
            let arg = b.value_of("%w", element);
            let r = b.fresh();
            b.line(format!(
                "{r} = call tailcc i1 @\"beck.apply.{fam}\"(ptr %err, i64 %clo, {} {arg})",
                element.llvm()
            ));
            b.checked("decide");
            b.block("decide");
            // Short-circuiting, which `beck-eval` documents as a promise rather than an
            // optimisation: `list_any` stops at the first `true` and `list_all` at the first
            // `false`, and the flag is which of the two this call is.
            let hit = b.fresh();
            b.line(format!("{hit} = icmp eq i1 {r}, %want"));
            b.line(format!("br i1 {hit}, label %stopped, label %next"));
            b.block("next");
            b.line("%j = add i64 %i, 1".into());
            b.line("br label %loop".into());
            b.block("stopped");
            b.line("ret i1 %want".into());
            b.block("exhausted");
            b.line("%rest = xor i1 %want, true".into());
            b.line("ret i1 %rest".into());
            b.block("failed");
            b.line("ret i1 false".into());
        }
    }
    let _ = writeln!(b.out, "}}\n");
    b.out
}

/// The one function that compares two closures, whatever their family.
///
/// One rather than one per family, because a rank is unique across the module: the word at a
/// closure's start is its place in the program's lambdas, and [`heap::Repr::order`] says why
/// comparing two of those is comparing what the evaluator compares — the parameters and where the
/// body starts, with the captured frame deliberately not in it.
const FN_CMP: &str = r#"define internal i64 @"beck.fn.cmp"(i64 %a, i64 %b) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %pa = getelementptr inbounds i8, ptr %hp, i64 %a
  %pb = getelementptr inbounds i8, ptr %hp, i64 %b
  %ra = load i64, ptr %pa
  %rb = load i64, ptr %pb
  %lt = icmp ult i64 %ra, %rb
  br i1 %lt, label %less, label %test
test:
  %gt = icmp ugt i64 %ra, %rb
  br i1 %gt, label %greater, label %same
less:
  ret i64 -1
greater:
  ret i64 1
same:
  ret i64 0
}

"#;

/// Applying a closure of one family: the switch, and one direct call per rank.
///
/// This is where [`heap::CLOSURE_HEADER`]'s decision is spent. There is no indirect call and no
/// function pointer anywhere: the closure's first word is a rank, the ranks of a family are known
/// from the whole program, and every arm is a `musttail` into a function whose symbol was written at
/// compile time — so a closure crossing the arena as bytes stays a value with no relocation in it.
///
/// Two kinds of arm. A `lam` has its own `beck.lam.N`, which takes the closure so it can read its
/// captures; a *definition* named as a value is called directly, because it closes over nothing and
/// a wrapper would be a second copy of a body that already exists.
fn apply_function(at: u32, heap: &Heap, closures: &Closures<'_>) -> String {
    let fam = heap.family(at);
    let ret = fam.ret.llvm();
    let mut b = Text::new();
    let mut head = format!(
        "; applying {}\ndefine internal tailcc {ret} @\"beck.apply.{at}\"(ptr noalias %err, i64 %clo",
        fam.shown
    );
    for (i, p) in fam.params.iter().enumerate() {
        let _ = write!(head, ", {} %a{i}", p.llvm());
    }
    let _ = writeln!(b.out, "{head}) {{\nentry:");
    b.line("%hp = load ptr, ptr @\"beck.heap\"".into());
    b.line("%p = getelementptr inbounds i8, ptr %hp, i64 %clo".into());
    b.line("%rank = load i64, ptr %p".into());

    // Only the ranks that became code. A family is a set of shapes and this is the subset of it the
    // module actually contains — see [`Closures`].
    let arms: Vec<u32> = fam
        .ranks
        .iter()
        .copied()
        .filter(|r| match &heap.lam(*r).def {
            Some(name) => closures.compiled.contains(name),
            None => closures.emitted.contains(r),
        })
        .collect();
    b.line("switch i64 %rank, label %none [".into());
    for r in &arms {
        b.line(format!("  i64 {r}, label %rank{r}"));
    }
    b.line("]".into());

    for r in &arms {
        b.block(&format!("rank{r}"));
        let lam = heap.lam(*r);
        let mut operands = String::from("ptr %err");
        if lam.def.is_none() {
            operands.push_str(", i64 %clo");
        }
        for (i, p) in fam.params.iter().enumerate() {
            let _ = write!(operands, ", {} %a{i}", p.llvm());
        }
        let symbol = match &lam.def {
            Some(name) => mangle(name),
            None => format!("\"beck.lam.{r}\""),
        };
        let v = b.fresh();
        b.line(format!(
            "{v} = musttail call tailcc {ret} @{symbol}({operands})"
        ));
        b.line(format!("ret {ret} {v}"));
    }

    // A rank no arm answers to. Unreachable — this module built the closure — and a trap rather
    // than LLVM's `unreachable` for [`Trap::NoSuchLambda`]'s reason. The span index is past the end
    // of the table on purpose: there is no source position for a wrong rank, and the host reads one
    // it cannot find as `Span::NONE`.
    b.block("none");
    b.line(format!("store i32 {}, ptr %err", Trap::NoSuchLambda.code()));
    b.line("%sp = getelementptr inbounds i8, ptr %err, i64 4".into());
    b.line(format!("store i32 {}, ptr %sp", u32::MAX));
    b.line("%pl = getelementptr inbounds i8, ptr %err, i64 8".into());
    b.line("store i64 %rank, ptr %pl".into());
    b.line(format!("ret {ret} {}", fam.ret.machine().zero()));
    let _ = writeln!(b.out, "}}\n");
    b.out
}

/// A three-way comparison over one layout: `-1`, `0` or `1`, and the same answer `Value`'s derived
/// `Ord` gives.
///
/// Tag first, then fields in the order they are laid out — which is name order, which is the order
/// `Fields` iterates and therefore the order `Ord` reads a record in. A field that is itself an
/// object is a call to *its* layout's comparison, so the recursion in the type is the recursion in
/// the code.
fn compare_function(at: u32, heap: &Heap) -> String {
    let layout = heap.layout(at);
    let mut b = Text::new();
    let _ = writeln!(
        b.out,
        "; {}\ndefine internal i64 @\"beck.cmp.{at}\"(i64 %a, i64 %b) {{\nentry:",
        layout.shown
    );
    b.line("%hp = load ptr, ptr @\"beck.heap\"".into());
    b.line("%pa = getelementptr inbounds i8, ptr %hp, i64 %a".into());
    b.line("%pb = getelementptr inbounds i8, ptr %hp, i64 %b".into());

    if layout.tagged {
        b.line("%ta = load i64, ptr %pa".into());
        b.line("%tb = load i64, ptr %pb".into());
        let lt = b.fresh();
        b.line(format!("{lt} = icmp ult i64 %ta, %tb"));
        b.line(format!("br i1 {lt}, label %less, label %tagged"));
        b.block("tagged");
        let gt = b.fresh();
        b.line(format!("{gt} = icmp ugt i64 %ta, %tb"));
        b.line(format!("br i1 {gt}, label %greater, label %same"));
        b.block("less");
        b.line("ret i64 -1".into());
        b.block("greater");
        b.line("ret i64 1".into());
        b.block("same");
        b.line("switch i64 %ta, label %equal [".into());
        for (i, _) in layout.variants.iter().enumerate() {
            b.line(format!("  i64 {i}, label %v{i}"));
        }
        b.line("]".into());
    }

    for (i, variant) in layout.variants.iter().enumerate() {
        if layout.tagged {
            b.block(&format!("v{i}"));
        }
        for (slot, (_, repr)) in variant.fields.iter().enumerate() {
            let bytes = (slot as u64 + 1) * heap::WORD;
            let (fa, fb) = (b.fresh(), b.fresh());
            b.line(format!(
                "{fa} = getelementptr inbounds i8, ptr %pa, i64 {bytes}"
            ));
            b.line(format!(
                "{fb} = getelementptr inbounds i8, ptr %pb, i64 {bytes}"
            ));
            let (xa, xb) = (b.fresh(), b.fresh());
            b.line(format!("{xa} = load i64, ptr {fa}"));
            b.line(format!("{xb} = load i64, ptr {fb}"));
            match repr.order() {
                // A reference decides through the three-way comparison for whatever it refers to,
                // and `Repr::order` is the only place that names one — see its own documentation
                // for the three times a `_` arm here swallowed a reference kind instead.
                heap::Order::Call(symbol) => {
                    let r = b.fresh();
                    b.line(format!("{r} = call i64 @\"{symbol}\"(i64 {xa}, i64 {xb})"));
                    let done = b.fresh();
                    let (decided, next) = (b.label("cmp.decided"), b.label("cmp.next"));
                    b.line(format!("{done} = icmp ne i64 {r}, 0"));
                    b.line(format!("br i1 {done}, label %{decided}, label %{next}"));
                    b.block(&decided);
                    b.line(format!("ret i64 {r}"));
                    b.block(&next);
                }
                // A field with no order at all. Unreachable, and the reason is a rule rather than
                // an argument: `Function::wants` asks `Heap::ordered` before it records a demand,
                // and that walks a record's fields — so a layout holding one of these is never in
                // the set this function is generated for. Answering "equal" is what the tag arm
                // above answers for the case that cannot happen, and for the same reason: it is
                // the one answer that cannot make a comparison asymmetric.
                heap::Order::Absent(_) => {}
                order => {
                    // A real compares through its order key, and both are already normalised —
                    // `Function::store_field` is where that is paid for.
                    let (ka, kb) = match order {
                        heap::Order::Key => (b.order_key(&xa), b.order_key(&xb)),
                        _ => (xa.clone(), xb.clone()),
                    };
                    let pred = if order == (heap::Order::Words { signed: true }) {
                        "s"
                    } else {
                        "u"
                    };
                    let below = b.label("cmp.below");
                    let above = b.label("cmp.above");
                    let test = b.label("cmp.test");
                    let next = b.label("cmp.next");
                    let lt = b.fresh();
                    b.line(format!("{lt} = icmp {pred}lt i64 {ka}, {kb}"));
                    b.line(format!("br i1 {lt}, label %{below}, label %{test}"));
                    b.block(&test);
                    let gt = b.fresh();
                    b.line(format!("{gt} = icmp {pred}gt i64 {ka}, {kb}"));
                    b.line(format!("br i1 {gt}, label %{above}, label %{next}"));
                    b.block(&below);
                    b.line("ret i64 -1".into());
                    b.block(&above);
                    b.line("ret i64 1".into());
                    b.block(&next);
                }
            }
        }
        b.line("ret i64 0".into());
    }
    if layout.tagged {
        // A tag the switch does not name cannot happen — the host writes one this table produced —
        // and answering "equal" is the one answer that cannot make a comparison asymmetric.
        b.block("equal");
        b.line("ret i64 0".into());
    }
    b.out.push_str("}\n\n");
    b.out
}

/// A tiny SSA name supply, for the functions that are written without a [`Function`] around them.
struct Text {
    out: String,
    next: u32,
}

impl Text {
    fn new() -> Text {
        Text {
            out: String::new(),
            next: 0,
        }
    }

    fn fresh(&mut self) -> String {
        self.next += 1;
        format!("%w{}", self.next)
    }

    fn label(&mut self, hint: &str) -> String {
        self.next += 1;
        format!("{hint}{}", self.next)
    }

    fn line(&mut self, text: String) {
        let _ = writeln!(self.out, "  {text}");
    }

    /// Start a block. Written flush left, because that is where a label goes.
    fn block(&mut self, label: &str) {
        let _ = writeln!(self.out, "{label}:");
    }

    /// `Value::float`'s two rules on a `double` already in a register: one zero, one NaN.
    ///
    /// The same four instructions [`Function::normalise`] emits, here because a generated loop
    /// stores a real into a list and a real on this heap is the one the evaluator would have built.
    fn normalised(&mut self, raw: &str) -> String {
        let is_zero = self.fresh();
        self.line(format!(
            "{is_zero} = fcmp oeq double {raw}, 0x0000000000000000"
        ));
        let zeroed = self.fresh();
        self.line(format!(
            "{zeroed} = select i1 {is_zero}, double 0x0000000000000000, double {raw}"
        ));
        let is_nan = self.fresh();
        self.line(format!("{is_nan} = fcmp uno double {raw}, {raw}"));
        let r = self.fresh();
        self.line(format!(
            "{r} = select i1 {is_nan}, double 0x7FF8000000000000, double {zeroed}"
        ));
        r
    }

    /// One word of a list as the value a closure of this repr takes.
    fn value_of(&mut self, w: &str, repr: Repr) -> String {
        match repr {
            Repr::Float => {
                let r = self.fresh();
                self.line(format!("{r} = bitcast i64 {w} to double"));
                r
            }
            Repr::Bool => {
                let r = self.fresh();
                self.line(format!("{r} = icmp ne i64 {w}, 0"));
                r
            }
            _ => w.to_string(),
        }
    }

    /// A closure's answer as the word a list holds.
    fn word_of(&mut self, v: &str, repr: Repr) -> String {
        match repr {
            Repr::Float => {
                let n = self.normalised(v);
                let r = self.fresh();
                self.line(format!("{r} = bitcast double {n} to i64"));
                r
            }
            Repr::Bool => {
                let r = self.fresh();
                self.line(format!("{r} = zext i1 {v} to i64"));
                r
            }
            _ => v.to_string(),
        }
    }

    /// Leave for `failed` if the call that just returned stored a trap.
    fn checked(&mut self, cont: &str) {
        let code = self.fresh();
        self.line(format!("{code} = load i32, ptr %err"));
        let bad = self.fresh();
        self.line(format!("{bad} = icmp ne i32 {code}, 0"));
        self.line(format!("br i1 {bad}, label %failed, label %{cont}"));
    }

    /// `beck_core`'s order key over raw bits already in an `i64`.
    fn order_key(&mut self, bits: &str) -> String {
        let sign = self.fresh();
        self.line(format!("{sign} = ashr i64 {bits}, 63"));
        let mask = self.fresh();
        self.line(format!("{mask} = or i64 {sign}, -9223372036854775808"));
        let key = self.fresh();
        self.line(format!("{key} = xor i64 {bits}, {mask}"));
        key
    }
}

/// `main`: read a call, answer it, repeat until the host closes the pipe.
///
/// `arena` is what a module with a layout in it adds: one `malloc` at startup, the argument graph
/// copied in before the call and the whole used part copied back out after one that answers with an
/// object. A module of pure arithmetic gets neither, which is what keeps `docs/93` §93.5's round
/// trip the same round trip.
fn main_loop(arena: bool) -> String {
    let mut m = String::from(
        r#"define i32 @main() {
entry:
  %req = alloca [2 x i64]
  %args = alloca [16 x i64]
  %err = alloca [3 x i64]
  %resp = alloca [4 x i64]
"#,
    );
    if arena {
        let _ = write!(
            m,
            r#"  %arena = call ptr @malloc(i64 {bytes})
  store ptr %arena, ptr @"beck.heap"
  %failed = icmp eq ptr %arena, null
  %cap = select i1 %failed, i64 0, i64 {bytes}
  store i64 %cap, ptr @"beck.limit"
"#,
            bytes = heap::ARENA_BYTES
        );
    }
    m.push_str(
        r#"  br label %loop
loop:
  %head = call i64 @"beck.read_exact"(ptr %req, i64 16)
  %closed = icmp ne i64 %head, 16
  br i1 %closed, label %done, label %sized
sized:
  %idx = load i32, ptr %req
  %cntp = getelementptr inbounds i8, ptr %req, i64 4
  %cnt32 = load i32, ptr %cntp
  %cnt = zext i32 %cnt32 to i64
  %blenp = getelementptr inbounds i8, ptr %req, i64 8
  %blen = load i64, ptr %blenp
  %bytes = mul i64 %cnt, 8
  %read = call i64 @"beck.read_exact"(ptr %args, i64 %bytes)
  %short = icmp ne i64 %read, %bytes
"#,
    );
    if arena {
        m.push_str(
            r#"  br i1 %short, label %done, label %accept
accept:
  ; A blob bigger than the arena is a host that disagrees with this module about the protocol,
  ; which is a bug rather than an input: it closes rather than writing past the end.
  %limit = load i64, ptr @"beck.limit"
  %huge = icmp ugt i64 %blen, %limit
  br i1 %huge, label %done, label %copy
copy:
  %into = load ptr, ptr @"beck.heap"
  %got = call i64 @"beck.read_exact"(ptr %into, i64 %blen)
  %truncated = icmp ne i64 %got, %blen
  br i1 %truncated, label %done, label %run
run:
  ; The arena is reset to just past whatever the arguments brought with them.
  %small = icmp ult i64 %blen, 8
  %start = select i1 %small, i64 8, i64 %blen
  store i64 %start, ptr @"beck.next"
  store i64 0, ptr @"beck.reply"
"#,
        );
    } else {
        m.push_str(
            r#"  br i1 %short, label %done, label %run
run:
"#,
        );
    }
    m.push_str(
        r#"  store i64 0, ptr %err
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
  %rb = getelementptr inbounds i8, ptr %resp, i64 24
"#,
    );
    if arena {
        let _ = write!(
            m,
            r#"  %ok = icmp eq i64 %cell, 0
  %wants = load i64, ptr @"beck.reply"
  %onheap = icmp ne i64 %wants, 0
  %answered = and i1 %ok, %onheap
  ; A raise is the one failure whose arena travels: the value it carried is in there, and the host
  ; builds the evaluator's own message out of it rather than out of the fact that there was one.
  %what = trunc i64 %cell to i32
  %raised = icmp eq i32 %what, {raised}
  %both = or i1 %answered, %raised
  %used = load i64, ptr @"beck.next"
  %send = select i1 %both, i64 %used, i64 0
  store i64 %send, ptr %rb
  %wrote = call i64 @"beck.write_all"(ptr %resp, i64 32)
  %gone = icmp ne i64 %wrote, 32
  br i1 %gone, label %done, label %blob
blob:
  %from = load ptr, ptr @"beck.heap"
  %pushed = call i64 @"beck.write_all"(ptr %from, i64 %send)
  %stalled = icmp ne i64 %pushed, %send
  br i1 %stalled, label %done, label %loop
"#,
            raised = Trap::Raised.code()
        );
    } else {
        m.push_str(
            r#"  store i64 0, ptr %rb
  %wrote = call i64 @"beck.write_all"(ptr %resp, i64 32)
  %gone = icmp ne i64 %wrote, 32
  br i1 %gone, label %done, label %loop
"#,
        );
    }
    m.push_str(
        r#"done:
  ret i32 0
}
"#,
    );
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

/// Text: the six functions everything this backend does to a `Str` is built from.
///
/// The shape is [`crate::heap`]'s — two counts and the bytes — and every one of these is written
/// against it rather than against a `Str` as C would think of one: there is no terminator, the
/// length is a word, and a character index is a byte index exactly when the two counts agree.
///
/// `memcmp` and `memcpy` are the C library's. They are linked in anyway (`main` calls `read` and
/// `write`), and hand-rolling either would be slower and no more honest.
///
/// | Function | Answers |
/// |---|---|
/// | `beck.str.alloc` | a fresh, uninitialised `Str` of the given two counts, or `0` on a full arena |
/// | `beck.str.cmp` | `-1`, `0` or `1`, which is what `String`'s `Ord` gives — bytes first, then length |
/// | `beck.str.concat` | `a + b` |
/// | `beck.str.byteof` | which byte character `i` begins at, clamped to the end |
/// | `beck.str.slice` | `str_slice`, in characters and clamped, exactly as the evaluator clamps |
/// | `beck.str.find` | the byte offset of a substring, or `-1` |
/// | `beck.str.ws` | the byte width of the whitespace character at `i`, or `0` if there is not one |
/// | `beck.str.trim` | `str_trim` — `str::trim`, which is `char::is_whitespace` at both ends |
///
/// `beck.str.ws` is the one that is a **closed set rather than a table**, which is why `str_trim`
/// compiles where `str_upper` does not. `White_Space` is 25 code points, none of them four bytes
/// long, so the test is a switch over five lead bytes; case mapping is some fourteen hundred
/// mappings and a handful that change a string's length. That is why the two are not one refusal,
/// and `native.rs::the_whitespace_this_backend_knows_is_every_one_rust_does` is what holds the
/// difference: it walks all of Unicode and fails the day the set is not this one.
///
/// `beck.str.byteof` is the one with a cost worth naming: it is constant time when the text is
/// ASCII — every character one byte, which is what the two equal counts say — and a walk otherwise,
/// where the evaluator has a chunked index and answers in at most a stride
/// ([`beck_core::core::Text`]). `docs/93` §93.7 carries that as a difference rather than hiding
/// it, and the fix, if a program ever needs it, is the same index in the same header.
const TEXT: &str = r#"define internal i64 @"beck.str.alloc"(ptr noalias %err, i64 %bytes, i64 %chars, i32 %span) {
entry:
  %pad = add i64 %bytes, 7
  %body = and i64 %pad, -8
  %total = add i64 %body, 16
  %off = call i64 @"beck.alloc"(ptr %err, i64 %total, i32 %span)
  %failed = icmp eq i64 %off, 0
  br i1 %failed, label %out, label %fill
fill:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %off
  store i64 %bytes, ptr %p
  %pc = getelementptr inbounds i8, ptr %p, i64 8
  store i64 %chars, ptr %pc
  ; The padding is zeroed rather than left as whatever the arena held, so that two runs of one
  ; program leave the same bytes behind and a heap read back byte for byte is a fair comparison.
  %empty = icmp eq i64 %body, 0
  br i1 %empty, label %out, label %tail
tail:
  %last = add i64 %body, 8
  %pt = getelementptr inbounds i8, ptr %p, i64 %last
  store i64 0, ptr %pt
  br label %out
out:
  ret i64 %off
}

define internal ptr @"beck.str.data"(i64 %s) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %at = add i64 %s, 16
  %p = getelementptr inbounds i8, ptr %hp, i64 %at
  ret ptr %p
}

define internal i64 @"beck.str.bytes"(i64 %s) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %s
  %n = load i64, ptr %p
  ret i64 %n
}

define internal i64 @"beck.str.chars"(i64 %s) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %at = add i64 %s, 8
  %p = getelementptr inbounds i8, ptr %hp, i64 %at
  %n = load i64, ptr %p
  ret i64 %n
}

define internal i64 @"beck.str.cmp"(i64 %a, i64 %b) {
entry:
  %la = call i64 @"beck.str.bytes"(i64 %a)
  %lb = call i64 @"beck.str.bytes"(i64 %b)
  %pa = call ptr @"beck.str.data"(i64 %a)
  %pb = call ptr @"beck.str.data"(i64 %b)
  %shorter = icmp ult i64 %la, %lb
  %n = select i1 %shorter, i64 %la, i64 %lb
  %c = call i32 @memcmp(ptr %pa, ptr %pb, i64 %n)
  %decided = icmp ne i32 %c, 0
  br i1 %decided, label %bytes, label %lengths
bytes:
  ; `memcmp` may answer any negative or any positive number; the language wants one of three.
  %neg = icmp slt i32 %c, 0
  %sign = select i1 %neg, i64 -1, i64 1
  ret i64 %sign
lengths:
  %lt = icmp ult i64 %la, %lb
  br i1 %lt, label %less, label %maybe
maybe:
  %gt = icmp ugt i64 %la, %lb
  br i1 %gt, label %greater, label %equal
less:
  ret i64 -1
greater:
  ret i64 1
equal:
  ret i64 0
}

define internal i64 @"beck.str.concat"(ptr noalias %err, i64 %a, i64 %b, i32 %span) {
entry:
  %la = call i64 @"beck.str.bytes"(i64 %a)
  %lb = call i64 @"beck.str.bytes"(i64 %b)
  %ca = call i64 @"beck.str.chars"(i64 %a)
  %cb = call i64 @"beck.str.chars"(i64 %b)
  %lt = add i64 %la, %lb
  %ct = add i64 %ca, %cb
  %r = call i64 @"beck.str.alloc"(ptr %err, i64 %lt, i64 %ct, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %copy
copy:
  ; The three data pointers are taken *after* the allocation, because the arena never moves but a
  ; pointer taken before one is still only correct by that fact — this way nothing depends on it.
  %pr = call ptr @"beck.str.data"(i64 %r)
  %pa = call ptr @"beck.str.data"(i64 %a)
  %pb = call ptr @"beck.str.data"(i64 %b)
  %ignored = call ptr @memcpy(ptr %pr, ptr %pa, i64 %la)
  %pr2 = getelementptr inbounds i8, ptr %pr, i64 %la
  %ignored2 = call ptr @memcpy(ptr %pr2, ptr %pb, i64 %lb)
  br label %out
out:
  ret i64 %r
}

define internal i64 @"beck.str.byteof"(i64 %s, i64 %i) {
entry:
  %len = call i64 @"beck.str.bytes"(i64 %s)
  %chars = call i64 @"beck.str.chars"(i64 %s)
  %past = icmp sge i64 %i, %chars
  br i1 %past, label %end, label %inside
inside:
  %before = icmp sle i64 %i, 0
  br i1 %before, label %start, label %known
known:
  ; Every character is one byte exactly when there are as many bytes as characters, so the two
  ; counts the header already carries are the ASCII test and no flag is stored for it.
  %ascii = icmp eq i64 %len, %chars
  br i1 %ascii, label %direct, label %walk
direct:
  ret i64 %i
walk:
  %p = call ptr @"beck.str.data"(i64 %s)
  br label %step
step:
  %at = phi i64 [ 0, %walk ], [ %next, %skipped ]
  %seen = phi i64 [ 0, %walk ], [ %more, %skipped ]
  %done = icmp eq i64 %seen, %i
  br i1 %done, label %here, label %advance
advance:
  ; One character is its lead byte and every byte after it whose top two bits are `10`.
  %one = add i64 %at, 1
  br label %skip
skip:
  %k = phi i64 [ %one, %advance ], [ %k1, %again ]
  %over = icmp uge i64 %k, %len
  br i1 %over, label %skipped, label %look
look:
  %bp = getelementptr inbounds i8, ptr %p, i64 %k
  %byte = load i8, ptr %bp
  %top = and i8 %byte, -64
  %cont = icmp eq i8 %top, -128
  br i1 %cont, label %again, label %skipped
again:
  %k1 = add i64 %k, 1
  br label %skip
skipped:
  %next = phi i64 [ %k, %skip ], [ %k, %look ]
  %more = add i64 %seen, 1
  br label %step
here:
  ret i64 %at
start:
  ret i64 0
end:
  ret i64 %len
}

define internal i64 @"beck.str.charat"(i64 %s, i64 %byte) {
entry:
  %len = call i64 @"beck.str.bytes"(i64 %s)
  %chars = call i64 @"beck.str.chars"(i64 %s)
  %ascii = icmp eq i64 %len, %chars
  br i1 %ascii, label %direct, label %count
direct:
  ret i64 %byte
count:
  ; How many characters begin before this byte, which is how many bytes before it are not
  ; continuation bytes. The inverse of `beck.str.byteof`, and the reason it exists is that a search
  ; answers in bytes and the language indexes in characters.
  %p = call ptr @"beck.str.data"(i64 %s)
  br label %loop
loop:
  %k = phi i64 [ 0, %count ], [ %k1, %next ]
  %n = phi i64 [ 0, %count ], [ %n1, %next ]
  %done = icmp uge i64 %k, %byte
  br i1 %done, label %out, label %look
look:
  %bp = getelementptr inbounds i8, ptr %p, i64 %k
  %b = load i8, ptr %bp
  %top = and i8 %b, -64
  %cont = icmp eq i8 %top, -128
  %step = select i1 %cont, i64 0, i64 1
  br label %next
next:
  %k1 = add i64 %k, 1
  %n1 = add i64 %n, %step
  br label %loop
out:
  ret i64 %n
}

define internal i64 @"beck.str.slice"(ptr noalias %err, i64 %s, i64 %start, i64 %len, i32 %span) {
entry:
  ; A negative index or a negative length is zero, which is what `i64::max(0)` does in the
  ; evaluator; and `start + len` saturates rather than wrapping, which is its `saturating_add`.
  %sneg = icmp slt i64 %start, 0
  %from = select i1 %sneg, i64 0, i64 %start
  %lneg = icmp slt i64 %len, 0
  %take = select i1 %lneg, i64 0, i64 %len
  %sum = add i64 %from, %take
  %wrapped = icmp slt i64 %sum, 0
  %upto = select i1 %wrapped, i64 9223372036854775807, i64 %sum
  %chars = call i64 @"beck.str.chars"(i64 %s)
  %fromover = icmp ugt i64 %from, %chars
  %cstart = select i1 %fromover, i64 %chars, i64 %from
  %uptoover = icmp ugt i64 %upto, %chars
  %cend = select i1 %uptoover, i64 %chars, i64 %upto
  %count = sub i64 %cend, %cstart
  %a = call i64 @"beck.str.byteof"(i64 %s, i64 %cstart)
  %b = call i64 @"beck.str.byteof"(i64 %s, i64 %cend)
  %bytes = sub i64 %b, %a
  %r = call i64 @"beck.str.alloc"(ptr %err, i64 %bytes, i64 %count, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %copy
copy:
  %pr = call ptr @"beck.str.data"(i64 %r)
  %ps = call ptr @"beck.str.data"(i64 %s)
  %at = getelementptr inbounds i8, ptr %ps, i64 %a
  %ignored = call ptr @memcpy(ptr %pr, ptr %at, i64 %bytes)
  br label %out
out:
  ret i64 %r
}

; The byte width of the whitespace character beginning at %i, or 0 if the character there is not
; whitespace. Every one of `White_Space`'s 25 code points is one, two or three bytes, and no
; continuation byte can be 0xC2, 0xE1, 0xE2 or 0xE3 — continuations are 0x80..0xBF — so this may be
; asked at *any* byte of well-formed UTF-8 and never answers inside a character. That is what lets
; `beck.str.trim` walk a byte at a time without decoding what it is skipping over.
define internal i64 @"beck.str.ws"(ptr %p, i64 %i, i64 %len) {
entry:
  %b0p = getelementptr inbounds i8, ptr %p, i64 %i
  %b0 = load i8, ptr %b0p
  %b0z = zext i8 %b0 to i32
  switch i32 %b0z, label %no [ i32 9, label %yes1
                               i32 10, label %yes1
                               i32 11, label %yes1
                               i32 12, label %yes1
                               i32 13, label %yes1
                               i32 32, label %yes1
                               i32 194, label %two
                               i32 225, label %e1
                               i32 226, label %e2
                               i32 227, label %e3 ]
two:
  ; U+0085 NEL and U+00A0 NBSP.
  %room2 = add i64 %i, 1
  %fits2 = icmp ult i64 %room2, %len
  br i1 %fits2, label %read2, label %no
read2:
  %b1p = getelementptr inbounds i8, ptr %p, i64 %room2
  %b1 = load i8, ptr %b1p
  %nel = icmp eq i8 %b1, -123
  %nbsp = icmp eq i8 %b1, -96
  %ws2 = or i1 %nel, %nbsp
  br i1 %ws2, label %yes2, label %no
e1:
  ; U+1680 OGHAM SPACE MARK.
  %room3a = add i64 %i, 2
  %fits3a = icmp ult i64 %room3a, %len
  br i1 %fits3a, label %read3a, label %no
read3a:
  %a1p = getelementptr inbounds i8, ptr %p, i64 %room3a
  %a1 = load i8, ptr %a1p
  %a0i = add i64 %i, 1
  %a0q = getelementptr inbounds i8, ptr %p, i64 %a0i
  %a0 = load i8, ptr %a0q
  %oga = icmp eq i8 %a0, -102
  %ogb = icmp eq i8 %a1, -128
  %ogham = and i1 %oga, %ogb
  br i1 %ogham, label %yes3, label %no
e2:
  ; U+2000..U+200A, U+2028, U+2029, U+202F — all `E2 80 xx` — and U+205F, which is `E2 81 9F`.
  %room3b = add i64 %i, 2
  %fits3b = icmp ult i64 %room3b, %len
  br i1 %fits3b, label %read3b, label %no
read3b:
  %c1i = add i64 %i, 1
  %c1p = getelementptr inbounds i8, ptr %p, i64 %c1i
  %c1 = load i8, ptr %c1p
  %c2p = getelementptr inbounds i8, ptr %p, i64 %room3b
  %c2 = load i8, ptr %c2p
  %c2z = zext i8 %c2 to i32
  %is80 = icmp eq i8 %c1, -128
  br i1 %is80, label %tail80, label %maybe81
tail80:
  switch i32 %c2z, label %no [ i32 128, label %yes3
                               i32 129, label %yes3
                               i32 130, label %yes3
                               i32 131, label %yes3
                               i32 132, label %yes3
                               i32 133, label %yes3
                               i32 134, label %yes3
                               i32 135, label %yes3
                               i32 136, label %yes3
                               i32 137, label %yes3
                               i32 138, label %yes3
                               i32 168, label %yes3
                               i32 169, label %yes3
                               i32 175, label %yes3 ]
maybe81:
  %is81 = icmp eq i8 %c1, -127
  %mmsp = icmp eq i8 %c2, -97
  %ws81 = and i1 %is81, %mmsp
  br i1 %ws81, label %yes3, label %no
e3:
  ; U+3000 IDEOGRAPHIC SPACE.
  %room3c = add i64 %i, 2
  %fits3c = icmp ult i64 %room3c, %len
  br i1 %fits3c, label %read3c, label %no
read3c:
  %d1i = add i64 %i, 1
  %d1p = getelementptr inbounds i8, ptr %p, i64 %d1i
  %d1 = load i8, ptr %d1p
  %d2p = getelementptr inbounds i8, ptr %p, i64 %room3c
  %d2 = load i8, ptr %d2p
  %ida = icmp eq i8 %d1, -128
  %idb = icmp eq i8 %d2, -128
  %ideo = and i1 %ida, %idb
  br i1 %ideo, label %yes3, label %no
yes1:
  ret i64 1
yes2:
  ret i64 2
yes3:
  ret i64 3
no:
  ret i64 0
}

; `str_trim`, in one pass. The leading run is skipped whole; then every byte is either the start of
; a whitespace character — skipped, and *not* recorded — or one byte of something else, which moves
; the end. So %end finishes one past the last byte of the last non-whitespace character, which is
; what `str::trim` answers, and the character count is the bytes in `[%start, %end)` that are not
; continuations.
define internal i64 @"beck.str.trim"(ptr noalias %err, i64 %s, i32 %span) {
entry:
  %len = call i64 @"beck.str.bytes"(i64 %s)
  %p = call ptr @"beck.str.data"(i64 %s)
  br label %lead
lead:
  %l = phi i64 [ 0, %entry ], [ %lnext, %skipping ]
  %ldone = icmp uge i64 %l, %len
  br i1 %ldone, label %empty, label %ltest
ltest:
  %lw = call i64 @"beck.str.ws"(ptr %p, i64 %l, i64 %len)
  %lws = icmp sgt i64 %lw, 0
  br i1 %lws, label %skipping, label %body
skipping:
  %lnext = add i64 %l, %lw
  br label %lead
body:
  %start = phi i64 [ %l, %ltest ]
  br label %scan
scan:
  %i = phi i64 [ %start, %body ], [ %inext, %spaced ], [ %knext, %kept ]
  %end = phi i64 [ %start, %body ], [ %end, %spaced ], [ %knext, %kept ]
  %over = icmp uge i64 %i, %len
  br i1 %over, label %cut, label %test
test:
  %w = call i64 @"beck.str.ws"(ptr %p, i64 %i, i64 %len)
  %isws = icmp sgt i64 %w, 0
  br i1 %isws, label %spaced, label %kept
spaced:
  %inext = add i64 %i, %w
  br label %scan
kept:
  %knext = add i64 %i, 1
  br label %scan
cut:
  %r = call i64 @"beck.str.piece"(ptr %err, i64 %s, i64 %start, i64 %end, i32 %span)
  ret i64 %r
empty:
  %e = call i64 @"beck.str.alloc"(ptr %err, i64 0, i64 0, i32 %span)
  ret i64 %e
}

; The bytes of %s in `[%from, %to)`, as a `Str` of its own.
;
; The character count is the bytes in the range that are not continuations, which is the same test
; `beck.str.byteof` walks with — and the range is always a whole number of characters, because every
; caller cuts at a boundary a scan stopped on.
define internal i64 @"beck.str.piece"(ptr noalias %err, i64 %s, i64 %from, i64 %to, i32 %span) {
entry:
  %bytes = sub i64 %to, %from
  %p = call ptr @"beck.str.data"(i64 %s)
  br label %count
count:
  %k = phi i64 [ %from, %entry ], [ %k1, %counted ]
  %chars = phi i64 [ 0, %entry ], [ %chars1, %counted ]
  %cdone = icmp uge i64 %k, %to
  br i1 %cdone, label %make, label %counted
counted:
  %cbp = getelementptr inbounds i8, ptr %p, i64 %k
  %cb = load i8, ptr %cbp
  %ctop = and i8 %cb, -64
  %ccont = icmp eq i8 %ctop, -128
  %one = select i1 %ccont, i64 0, i64 1
  %chars1 = add i64 %chars, %one
  %k1 = add i64 %k, 1
  br label %count
make:
  %r = call i64 @"beck.str.alloc"(ptr %err, i64 %bytes, i64 %chars, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %copy
copy:
  ; Both pointers are taken again: `beck.str.alloc` can move the arena, and `%p` was read before it
  ; ran.
  %pr = call ptr @"beck.str.data"(i64 %r)
  %ps = call ptr @"beck.str.data"(i64 %s)
  %at = getelementptr inbounds i8, ptr %ps, i64 %from
  %ignored = call ptr @memcpy(ptr %pr, ptr %at, i64 %bytes)
  br label %out
out:
  ret i64 %r
}

; `beck.str.find`, starting at a byte offset rather than at zero — what a repeated search needs and
; the only thing `str_split` asks that `beck.str.find` does not answer.
define internal i64 @"beck.str.findat"(i64 %h, i64 %n, i64 %from) {
entry:
  %lh = call i64 @"beck.str.bytes"(i64 %h)
  %ln = call i64 @"beck.str.bytes"(i64 %n)
  %room = sub i64 %lh, %ln
  %too = icmp slt i64 %room, 0
  br i1 %too, label %missing, label %search
search:
  %ph = call ptr @"beck.str.data"(i64 %h)
  %pn = call ptr @"beck.str.data"(i64 %n)
  br label %loop
loop:
  %i = phi i64 [ %from, %search ], [ %j, %next ]
  %over = icmp sgt i64 %i, %room
  br i1 %over, label %missing, label %try
try:
  %at = getelementptr inbounds i8, ptr %ph, i64 %i
  %c = call i32 @memcmp(ptr %at, ptr %pn, i64 %ln)
  %hit = icmp eq i32 %c, 0
  br i1 %hit, label %found, label %next
next:
  %j = add i64 %i, 1
  br label %loop
found:
  ret i64 %i
missing:
  ret i64 -1
}

define internal i64 @"beck.str.find"(i64 %h, i64 %n) {
entry:
  %lh = call i64 @"beck.str.bytes"(i64 %h)
  %ln = call i64 @"beck.str.bytes"(i64 %n)
  %too = icmp ugt i64 %ln, %lh
  br i1 %too, label %missing, label %search
search:
  ; Naive, and correct on UTF-8 for the reason a byte search is: the encoding is
  ; self-synchronising, so a well-formed needle cannot match starting inside a character.
  %last = sub i64 %lh, %ln
  %ph = call ptr @"beck.str.data"(i64 %h)
  %pn = call ptr @"beck.str.data"(i64 %n)
  br label %loop
loop:
  %i = phi i64 [ 0, %search ], [ %j, %next ]
  %over = icmp ugt i64 %i, %last
  br i1 %over, label %missing, label %try
try:
  %at = getelementptr inbounds i8, ptr %ph, i64 %i
  %c = call i32 @memcmp(ptr %at, ptr %pn, i64 %ln)
  %hit = icmp eq i32 %c, 0
  br i1 %hit, label %found, label %next
next:
  %j = add i64 %i, 1
  br label %loop
found:
  ret i64 %i
missing:
  ret i64 -1
}

"#;

/// The two functions a map of a given key and value repr needs, generated per repr.
///
/// A **binary search** rather than the linear one a list gets, because the keys are in key order
/// and `map_get` is what a fold does on every event. And the lexicographic order over two maps,
/// which is `PMap`'s: pair by pair in key order — key first, then value — and then by length.
fn map_functions(at: u32, heap: &Heap) -> String {
    let (key, value) = heap.entry(at);
    format!(
        r#"; {shown}
; The search: down the tree, comparing keys. Answers the *node*, or 0 — a lookup and a containment
; test are the same walk, and so is the value, which is a word off the node it found.
define internal i64 @"beck.map.find.{at}"(i64 %m, i64 %k) {{
entry:
  br label %loop
loop:
  %n = phi i64 [ %m, %entry ], [ %next, %step ]
  %empty = icmp eq i64 %n, 0
  br i1 %empty, label %missing, label %probe
probe:
  %nk = call i64 @"beck.map.key"(i64 %n)
  %c = call i64 @"beck.elem.cmp.{key}"(i64 %k, i64 %nk)
  %hit = icmp eq i64 %c, 0
  br i1 %hit, label %found, label %step
step:
  %down = icmp slt i64 %c, 0
  %l = call i64 @"beck.map.left"(i64 %n)
  %r = call i64 @"beck.map.right"(i64 %n)
  %next = select i1 %down, i64 %l, i64 %r
  br label %loop
found:
  ret i64 %n
missing:
  ret i64 0
}}

; `map_insert`: rebuild the path, share everything off it, rebalance on the way out. `O(log n)`
; fresh nodes, which is `beck_core::pmap`'s own cost and the reason this can be compiled at all.
define internal i64 @"beck.map.ins.{at}"(ptr noalias %err, i64 %m, i64 %k, i64 %v, i32 %span) {{
entry:
  %empty = icmp eq i64 %m, 0
  br i1 %empty, label %fresh, label %walk
fresh:
  %leaf = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 0, i64 0, i32 %span)
  ret i64 %leaf
walk:
  %mk = call i64 @"beck.map.key"(i64 %m)
  %mv = call i64 @"beck.map.value"(i64 %m)
  %ml = call i64 @"beck.map.left"(i64 %m)
  %mr = call i64 @"beck.map.right"(i64 %m)
  %c = call i64 @"beck.elem.cmp.{key}"(i64 %k, i64 %mk)
  %lt = icmp slt i64 %c, 0
  br i1 %lt, label %go.left, label %not.less
go.left:
  %nl = call i64 @"beck.map.ins.{at}"(ptr %err, i64 %ml, i64 %k, i64 %v, i32 %span)
  %bl = call i64 @"beck.map.balance"(ptr %err, i64 %mk, i64 %mv, i64 %nl, i64 %mr, i32 %span)
  ret i64 %bl
not.less:
  %gt = icmp sgt i64 %c, 0
  br i1 %gt, label %go.right, label %replace
go.right:
  %nr = call i64 @"beck.map.ins.{at}"(ptr %err, i64 %mr, i64 %k, i64 %v, i32 %span)
  %br = call i64 @"beck.map.balance"(ptr %err, i64 %mk, i64 %mv, i64 %ml, i64 %nr, i32 %span)
  ret i64 %br
replace:
  ; The *new* key as well as the new value, which is what the evaluator's `Ordering::Equal` arm
  ; does — two keys that compare equal need not be the same value.
  %same = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %ml, i64 %mr, i32 %span)
  ret i64 %same
}}

; The smallest node of a subtree, and the subtree with it taken out. `map_remove` needs both, and
; a weight-balanced tree needs the rebalance on the way out of each.
define internal i64 @"beck.map.min.{at}"(i64 %m) {{
entry:
  br label %loop
loop:
  %n = phi i64 [ %m, %entry ], [ %l, %down ]
  %l = call i64 @"beck.map.left"(i64 %n)
  %none = icmp eq i64 %l, 0
  br i1 %none, label %found, label %down
down:
  br label %loop
found:
  ret i64 %n
}}

define internal i64 @"beck.map.pop.{at}"(ptr noalias %err, i64 %m, i32 %span) {{
entry:
  %l = call i64 @"beck.map.left"(i64 %m)
  %none = icmp eq i64 %l, 0
  br i1 %none, label %gone, label %deeper
gone:
  %r = call i64 @"beck.map.right"(i64 %m)
  ret i64 %r
deeper:
  %nl = call i64 @"beck.map.pop.{at}"(ptr %err, i64 %l, i32 %span)
  %k = call i64 @"beck.map.key"(i64 %m)
  %v = call i64 @"beck.map.value"(i64 %m)
  %rr = call i64 @"beck.map.right"(i64 %m)
  %b = call i64 @"beck.map.balance"(ptr %err, i64 %k, i64 %v, i64 %nl, i64 %rr, i32 %span)
  ret i64 %b
}}

; `map_remove`: the same path rebuild as an insert. A node with two children is replaced by the
; smallest of its right subtree, which is that subtree's leftmost — the textbook deletion, with the
; rebalance the weights need.
define internal i64 @"beck.map.del.{at}"(ptr noalias %err, i64 %m, i64 %k, i32 %span) {{
entry:
  %empty = icmp eq i64 %m, 0
  br i1 %empty, label %absent, label %walk
absent:
  ret i64 0
walk:
  %mk = call i64 @"beck.map.key"(i64 %m)
  %mv = call i64 @"beck.map.value"(i64 %m)
  %ml = call i64 @"beck.map.left"(i64 %m)
  %mr = call i64 @"beck.map.right"(i64 %m)
  %c = call i64 @"beck.elem.cmp.{key}"(i64 %k, i64 %mk)
  %lt = icmp slt i64 %c, 0
  br i1 %lt, label %go.left, label %not.less
go.left:
  %nl = call i64 @"beck.map.del.{at}"(ptr %err, i64 %ml, i64 %k, i32 %span)
  %bl = call i64 @"beck.map.balance"(ptr %err, i64 %mk, i64 %mv, i64 %nl, i64 %mr, i32 %span)
  ret i64 %bl
not.less:
  %gt = icmp sgt i64 %c, 0
  br i1 %gt, label %go.right, label %here
go.right:
  %nr = call i64 @"beck.map.del.{at}"(ptr %err, i64 %mr, i64 %k, i32 %span)
  %br = call i64 @"beck.map.balance"(ptr %err, i64 %mk, i64 %mv, i64 %ml, i64 %nr, i32 %span)
  ret i64 %br
here:
  %no.left = icmp eq i64 %ml, 0
  br i1 %no.left, label %lift.right, label %maybe
lift.right:
  ret i64 %mr
maybe:
  %no.right = icmp eq i64 %mr, 0
  br i1 %no.right, label %lift.left, label %join
lift.left:
  ret i64 %ml
join:
  %least = call i64 @"beck.map.min.{at}"(i64 %mr)
  %lk = call i64 @"beck.map.key"(i64 %least)
  %lv = call i64 @"beck.map.value"(i64 %least)
  %rest = call i64 @"beck.map.pop.{at}"(ptr %err, i64 %mr, i32 %span)
  %joined = call i64 @"beck.map.balance"(ptr %err, i64 %lk, i64 %lv, i64 %ml, i64 %rest, i32 %span)
  ret i64 %joined
}}

; `map_merge`: every entry of the second map inserted into the first, in key order, so the later
; map wins — which is what the evaluator's own merge does.
define internal i64 @"beck.map.merge.{at}"(ptr noalias %err, i64 %a, i64 %b, i32 %span) {{
entry:
  %n = call i64 @"beck.map.size"(i64 %b)
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %j, %step ]
  %acc = phi i64 [ %a, %entry ], [ %next, %step ]
  %past = icmp uge i64 %i, %n
  br i1 %past, label %done, label %step
step:
  %node = call i64 @"beck.map.nth"(i64 %b, i64 %i)
  %k = call i64 @"beck.map.key"(i64 %node)
  %v = call i64 @"beck.map.value"(i64 %node)
  %next = call i64 @"beck.map.ins.{at}"(ptr %err, i64 %acc, i64 %k, i64 %v, i32 %span)
  %j = add i64 %i, 1
  br label %loop
done:
  ret i64 %acc
}}

; Two maps in key order: the keys, then the values, entry by entry, then the sizes. The order is
; `beck_core`'s own — what a `PMap` iterates is what this walks.
define internal i64 @"beck.map.cmp.{at}"(i64 %a, i64 %b) {{
entry:
  %la = call i64 @"beck.map.size"(i64 %a)
  %lb = call i64 @"beck.map.size"(i64 %b)
  %shorter = icmp ult i64 %la, %lb
  %n = select i1 %shorter, i64 %la, i64 %lb
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %j, %next ]
  %past = icmp uge i64 %i, %n
  br i1 %past, label %lengths, label %keys
keys:
  %na = call i64 @"beck.map.nth"(i64 %a, i64 %i)
  %nb = call i64 @"beck.map.nth"(i64 %b, i64 %i)
  %wka = call i64 @"beck.map.key"(i64 %na)
  %wkb = call i64 @"beck.map.key"(i64 %nb)
  %ck = call i64 @"beck.elem.cmp.{key}"(i64 %wka, i64 %wkb)
  %kdecided = icmp ne i64 %ck, 0
  br i1 %kdecided, label %answerk, label %values
answerk:
  ret i64 %ck
values:
  %wva = call i64 @"beck.map.value"(i64 %na)
  %wvb = call i64 @"beck.map.value"(i64 %nb)
  %cv = call i64 @"beck.elem.cmp.{value}"(i64 %wva, i64 %wvb)
  %vdecided = icmp ne i64 %cv, 0
  br i1 %vdecided, label %answerv, label %next
answerv:
  ret i64 %cv
next:
  %j = add i64 %i, 1
  br label %loop
lengths:
  ; Equal as far as both go, so the smaller map is the smaller value.
  %lt = icmp ult i64 %la, %lb
  %gt = icmp ugt i64 %la, %lb
  %up = select i1 %gt, i64 1, i64 -1
  %same = icmp eq i64 %la, %lb
  %r = select i1 %same, i64 0, i64 %up
  ret i64 %r
}}

"#,
        shown = heap.show(Repr::Map(at))
    )
}

/// The three functions a list of a given element repr needs, generated per repr.
///
/// One three-way comparison over two **words**, and two functions built on it: the lexicographic
/// order over two lists, and a linear search. Written per repr rather than taking a function
/// pointer, because an indirect call is the one thing this backend does not have — it is what a
/// closure would need, and `docs/93` §93.14 lists it as unbuilt.
///
/// The order is `Vec<Value>`'s: element by element, and a list that is a prefix of another is less
/// than it. That is Rust's derived `Ord` on a slice, which is what [`beck_core::Value`] holds.
fn element_functions(at: u32, heap: &Heap) -> String {
    let element = heap.element(at);
    let mut b = Text::new();

    // The word-level comparison. Every element crosses this as an `i64`, which is what it is in the
    // arena — a real is the bits of a double the store already normalised.
    let _ = writeln!(
        b.out,
        "; element of {}\ndefine internal i64 @\"beck.elem.cmp.{at}\"(i64 %a, i64 %b) {{\nentry:",
        heap.show(Repr::List(at))
    );
    match element.order() {
        // An element with no order at all: the three functions below are a comparison, a
        // lexicographic one and a search, and every one of them is that element's comparison in a
        // loop. Nothing is emitted rather than something that compares offsets — `Function::wants`
        // refuses the demand before this is reached, and a bug in that rule is then a missing
        // symbol at link time rather than a list of views that sorts by where they were allocated.
        heap::Order::Absent(_) => return String::new(),
        // Whatever this element is, its comparison is `Repr::order`'s — one call and no case
        // analysis, which is the whole point of that accessor.
        heap::Order::Call(symbol) => {
            b.line(format!("%c = call i64 @\"{symbol}\"(i64 %a, i64 %b)"));
            b.line("ret i64 %c".into());
        }
        order => {
            // A real compares through `beck_core`'s order key; an `Int` is signed; a `Bool` is a 0
            // or a 1 in the low bit and is therefore either way round.
            let (ka, kb) = match order {
                heap::Order::Key => (b.order_key("%a"), b.order_key("%b")),
                _ => ("%a".to_string(), "%b".to_string()),
            };
            let pred = if order == (heap::Order::Words { signed: true }) {
                "s"
            } else {
                "u"
            };
            b.line(format!("%lt = icmp {pred}lt i64 {ka}, {kb}"));
            b.line("br i1 %lt, label %less, label %test".into());
            b.block("test");
            b.line(format!("%gt = icmp {pred}gt i64 {ka}, {kb}"));
            b.line("br i1 %gt, label %greater, label %same".into());
            b.block("less");
            b.line("ret i64 -1".into());
            b.block("greater");
            b.line("ret i64 1".into());
            b.block("same");
            b.line("ret i64 0".into());
        }
    }
    b.out.push_str("}\n\n");

    let _ = write!(
        b.out,
        r#"define internal i64 @"beck.list.cmp.{at}"(i64 %a, i64 %b) {{
entry:
  %la = call i64 @"beck.list.len"(i64 %a)
  %lb = call i64 @"beck.list.len"(i64 %b)
  %shorter = icmp ult i64 %la, %lb
  %n = select i1 %shorter, i64 %la, i64 %lb
  %pa = call ptr @"beck.list.data"(i64 %a)
  %pb = call ptr @"beck.list.data"(i64 %b)
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %j, %next ]
  %past = icmp uge i64 %i, %n
  br i1 %past, label %lengths, label %one
one:
  %xa = getelementptr inbounds i64, ptr %pa, i64 %i
  %xb = getelementptr inbounds i64, ptr %pb, i64 %i
  %wa = load i64, ptr %xa
  %wb = load i64, ptr %xb
  %c = call i64 @"beck.elem.cmp.{at}"(i64 %wa, i64 %wb)
  %decided = icmp ne i64 %c, 0
  br i1 %decided, label %answer, label %next
next:
  %j = add i64 %i, 1
  br label %loop
answer:
  ret i64 %c
lengths:
  ; Equal as far as both go, so the shorter one is the smaller: `[1] < [1, 2]`.
  %lt = icmp ult i64 %la, %lb
  %gt = icmp ugt i64 %la, %lb
  %up = select i1 %gt, i64 1, i64 -1
  %same = icmp eq i64 %la, %lb
  %r = select i1 %same, i64 0, i64 %up
  ret i64 %r
}}

define internal i64 @"beck.list.find.{at}"(i64 %xs, i64 %w) {{
entry:
  %n = call i64 @"beck.list.len"(i64 %xs)
  %p = call ptr @"beck.list.data"(i64 %xs)
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %j, %next ]
  %past = icmp uge i64 %i, %n
  br i1 %past, label %missing, label %one
one:
  %at = getelementptr inbounds i64, ptr %p, i64 %i
  %x = load i64, ptr %at
  %c = call i64 @"beck.elem.cmp.{at}"(i64 %x, i64 %w)
  %hit = icmp eq i64 %c, 0
  br i1 %hit, label %found, label %next
next:
  %j = add i64 %i, 1
  br label %loop
found:
  ret i64 %i
missing:
  ret i64 -1
}}

"#
    );
    b.out
}

/// The C library, which is linked in anyway because `main` calls `read` and `write`.
const LIBC: &str = r#"declare i32 @memcmp(ptr, ptr, i64)
declare ptr @memcpy(ptr, ptr, i64)

"#;

/// Lists: the four functions that do not care what an element *is*.
///
/// A list is one header word — how many — and one word per element, so allocating one, taking a
/// range out of one and turning one around are word moves and nothing else. Everything that has to
/// know what a word means is generated per element repr instead: [`element_functions`] writes a
/// three-way comparison, a lexicographic one over two lists, and a linear search.
///
/// `beck.list.copy` is `list_slice`, `list_take` and `list_drop` at once, because all three are "a
/// range of the elements, clamped" and the clamping is arithmetic the caller does. It costs the
/// **answer** rather than the list, which is `docs/93` §93.5's rule applied one type over.
const LISTS: &str = r#"define internal i64 @"beck.list.block"(ptr noalias %err, i64 %cap, i64 %used, i32 %span) {
entry:
  %body = mul i64 %cap, 8
  %total = add i64 %body, 16
  %off = call i64 @"beck.alloc"(ptr %err, i64 %total, i32 %span)
  %failed = icmp eq i64 %off, 0
  br i1 %failed, label %out, label %fill
fill:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %off
  store i64 %cap, ptr %p
  %pu = getelementptr inbounds i8, ptr %p, i64 8
  store i64 %used, ptr %pu
  br label %out
out:
  ret i64 %off
}

define internal i64 @"beck.list.head"(ptr noalias %err, i64 %n, i64 %data, i32 %span) {
entry:
  %off = call i64 @"beck.alloc"(ptr %err, i64 16, i32 %span)
  %failed = icmp eq i64 %off, 0
  br i1 %failed, label %out, label %fill
fill:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %off
  store i64 %n, ptr %p
  %pd = getelementptr inbounds i8, ptr %p, i64 8
  store i64 %data, ptr %pd
  br label %out
out:
  ret i64 %off
}

define internal i64 @"beck.list.alloc"(ptr noalias %err, i64 %n, i32 %span) {
entry:
  %d = call i64 @"beck.list.block"(ptr %err, i64 %n, i64 %n, i32 %span)
  %failed = icmp eq i64 %d, 0
  br i1 %failed, label %out, label %top
top:
  %h = call i64 @"beck.list.head"(ptr %err, i64 %n, i64 %d, i32 %span)
  br label %out
out:
  %r = phi i64 [ 0, %entry ], [ %h, %top ]
  ret i64 %r
}

define internal i64 @"beck.list.len"(i64 %xs) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %xs
  %n = load i64, ptr %p
  ret i64 %n
}

define internal ptr @"beck.list.data"(i64 %xs) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %ph = getelementptr inbounds i8, ptr %hp, i64 %xs
  %pd = getelementptr inbounds i8, ptr %ph, i64 8
  %d = load i64, ptr %pd
  %at = add i64 %d, 16
  %p = getelementptr inbounds i8, ptr %hp, i64 %at
  ret ptr %p
}

; `list_append` — a new header over the same block when the block has room and this list is the one
; standing at its end, and a doubled copy otherwise.
;
; The test is `count == used`, and it is the whole of what makes this sound: every header over a
; block has a count of at most `used`, so the slot at `used` is one no reader can see. Writing it
; and answering a *new* header leaves every existing list exactly as it was — no ownership analysis,
; no reference count, and no way for two holders to disagree about what a list contains.
define internal i64 @"beck.list.append"(ptr noalias %err, i64 %xs, i64 %w, i32 %span) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %ph = getelementptr inbounds i8, ptr %hp, i64 %xs
  %n = load i64, ptr %ph
  %pd = getelementptr inbounds i8, ptr %ph, i64 8
  %d = load i64, ptr %pd
  %pb = getelementptr inbounds i8, ptr %hp, i64 %d
  %cap = load i64, ptr %pb
  %pu = getelementptr inbounds i8, ptr %pb, i64 8
  %used = load i64, ptr %pu
  %at.end = icmp eq i64 %n, %used
  %room = icmp ult i64 %used, %cap
  %fits = and i1 %at.end, %room
  br i1 %fits, label %push, label %grow
push:
  %pe = getelementptr inbounds i8, ptr %pb, i64 16
  %slot = getelementptr inbounds i64, ptr %pe, i64 %n
  store i64 %w, ptr %slot
  %n1 = add i64 %n, 1
  store i64 %n1, ptr %pu
  br label %done
grow:
  ; Doubled, so the copies over a whole accumulator sum to a constant per element.
  %want = add i64 %n, 1
  %twice = mul i64 %want, 2
  %big = icmp ult i64 %twice, 4
  %cap2 = select i1 %big, i64 4, i64 %twice
  %d2 = call i64 @"beck.list.block"(ptr %err, i64 %cap2, i64 %want, i32 %span)
  %failed = icmp eq i64 %d2, 0
  br i1 %failed, label %out, label %move
move:
  %hp2 = load ptr, ptr @"beck.heap"
  %pb2 = getelementptr inbounds i8, ptr %hp2, i64 %d2
  %pe2 = getelementptr inbounds i8, ptr %pb2, i64 16
  %from = call ptr @"beck.list.data"(i64 %xs)
  %bytes = mul i64 %n, 8
  %ignored = call ptr @memcpy(ptr %pe2, ptr %from, i64 %bytes)
  %slot2 = getelementptr inbounds i64, ptr %pe2, i64 %n
  store i64 %w, ptr %slot2
  br label %done
done:
  %block = phi i64 [ %d, %push ], [ %d2, %move ]
  %len = phi i64 [ %n1, %push ], [ %want, %move ]
  %h = call i64 @"beck.list.head"(ptr %err, i64 %len, i64 %block, i32 %span)
  br label %out
out:
  %r = phi i64 [ 0, %grow ], [ %h, %done ]
  ret i64 %r
}

define internal i64 @"beck.list.copy"(ptr noalias %err, i64 %xs, i64 %from, i64 %count, i32 %span) {
entry:
  %r = call i64 @"beck.list.alloc"(ptr %err, i64 %count, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %move
move:
  %pr = call ptr @"beck.list.data"(i64 %r)
  %px = call ptr @"beck.list.data"(i64 %xs)
  %skip = mul i64 %from, 8
  %at = getelementptr inbounds i8, ptr %px, i64 %skip
  %bytes = mul i64 %count, 8
  %ignored = call ptr @memcpy(ptr %pr, ptr %at, i64 %bytes)
  br label %out
out:
  ret i64 %r
}

define internal i64 @"beck.list.concat"(ptr noalias %err, i64 %xss, i32 %span) {
entry:
  %n = call i64 @"beck.list.len"(i64 %xss)
  %outer = call ptr @"beck.list.data"(i64 %xss)
  br label %sum
sum:
  ; One pass for the size, which is what makes this an allocation rather than a growth: the length
  ; of every inner list is a header word, so the total is known before a byte is reserved.
  %i = phi i64 [ 0, %entry ], [ %i1, %next ]
  %so.far = phi i64 [ 0, %entry ], [ %grown, %next ]
  %at.end = icmp uge i64 %i, %n
  br i1 %at.end, label %build, label %next
next:
  %at = getelementptr inbounds i64, ptr %outer, i64 %i
  %inner = load i64, ptr %at
  %m = call i64 @"beck.list.len"(i64 %inner)
  %grown = add i64 %so.far, %m
  %i1 = add i64 %i, 1
  br label %sum
build:
  %out = call i64 @"beck.list.alloc"(ptr %err, i64 %so.far, i32 %span)
  %failed = icmp eq i64 %out, 0
  br i1 %failed, label %finish, label %move
move:
  %dst = call ptr @"beck.list.data"(i64 %out)
  br label %loop
loop:
  ; One `memcpy` per inner list. An element is a word whatever it means, and an offset stays an
  ; offset — so nothing here has to know what the elements are.
  %j = phi i64 [ 0, %move ], [ %j1, %again ]
  %k = phi i64 [ 0, %move ], [ %k1, %again ]
  %past = icmp uge i64 %j, %n
  br i1 %past, label %finish, label %again
again:
  %src.at = getelementptr inbounds i64, ptr %outer, i64 %j
  %one = load i64, ptr %src.at
  %len = call i64 @"beck.list.len"(i64 %one)
  %src = call ptr @"beck.list.data"(i64 %one)
  %to = getelementptr inbounds i64, ptr %dst, i64 %k
  %bytes = mul i64 %len, 8
  %copied = call ptr @memcpy(ptr %to, ptr %src, i64 %bytes)
  %k1 = add i64 %k, %len
  %j1 = add i64 %j, 1
  br label %loop
finish:
  ret i64 %out
}

define internal i64 @"beck.list.reverse"(ptr noalias %err, i64 %xs, i32 %span) {
entry:
  %n = call i64 @"beck.list.len"(i64 %xs)
  %r = call i64 @"beck.list.alloc"(ptr %err, i64 %n, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %walk
walk:
  %pr = call ptr @"beck.list.data"(i64 %r)
  %px = call ptr @"beck.list.data"(i64 %xs)
  br label %loop
loop:
  %i = phi i64 [ 0, %walk ], [ %j, %step ]
  %done = icmp uge i64 %i, %n
  br i1 %done, label %out, label %step
step:
  %back = sub i64 %n, %i
  %k = sub i64 %back, 1
  %src = getelementptr inbounds i64, ptr %px, i64 %i
  %dst = getelementptr inbounds i64, ptr %pr, i64 %k
  %w = load i64, ptr %src
  store i64 %w, ptr %dst
  %j = add i64 %i, 1
  br label %loop
out:
  ret i64 %r
}

"#;

/// Building text: the three that make one out of something that is not text.
///
/// Emitted beside [`TEXT`] rather than in it because `beck.str.join` reads a **list**, so this half
/// needs `LISTS` as well — which is why [`assemble`] emits it only when both are there.
///
/// `beck.str.from_int` is Rust's `i64::to_string` and has to be, to the digit: `str(n)` is
/// `Value::display`, and `docs/93` §93.9 refused this whole primitive rather than answer a decimal
/// that might differ. An integer's decimal *is* reproducible — the one that is not is a real's,
/// whose shortest round-trip form is a whole algorithm — so this compiles for an `Int`, a `Bool` and
/// a `Str`, and a `Float` is still refused.
const BUILDS: &str = r#"define internal i64 @"beck.str.from_int"(ptr noalias %err, i64 %n, i32 %span) {
entry:
  %neg = icmp slt i64 %n, 0
  ; `0 - i64::MIN` wraps to 2^63, which read as unsigned is exactly its magnitude — the one input
  ; where negating in signed arithmetic has no answer.
  %flip = sub i64 0, %n
  %u = select i1 %neg, i64 %flip, i64 %n
  br label %count
count:
  %d = phi i64 [ 1, %entry ], [ %d1, %more ]
  %t = phi i64 [ %u, %entry ], [ %t1, %more ]
  %t1 = udiv i64 %t, 10
  %done = icmp eq i64 %t1, 0
  br i1 %done, label %sized, label %more
more:
  %d1 = add i64 %d, 1
  br label %count
sized:
  %sign = zext i1 %neg to i64
  %bytes = add i64 %d, %sign
  ; Every byte is a digit or a minus, so the character count is the byte count.
  %r = call i64 @"beck.str.alloc"(ptr %err, i64 %bytes, i64 %bytes, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %fill
fill:
  %p = call ptr @"beck.str.data"(i64 %r)
  br i1 %neg, label %minus, label %digits
minus:
  store i8 45, ptr %p
  br label %digits
digits:
  br label %loop
loop:
  ; Backwards from the last byte, which is the order division produces them in.
  %i = phi i64 [ %bytes, %digits ], [ %i1, %loop ]
  %v = phi i64 [ %u, %digits ], [ %v1, %loop ]
  %i1 = sub i64 %i, 1
  %rem = urem i64 %v, 10
  %v1 = udiv i64 %v, 10
  %ch = trunc i64 %rem to i8
  %byte = add i8 %ch, 48
  %at = getelementptr inbounds i8, ptr %p, i64 %i1
  store i8 %byte, ptr %at
  %left = icmp eq i64 %v1, 0
  br i1 %left, label %out, label %loop
out:
  ret i64 %r
}

define internal i64 @"beck.str.repeat"(ptr noalias %err, i64 %s, i64 %n, i32 %span) {
entry:
  ; The evaluator clamps to a million, "because `"x" * 10_000_000_000` is a request nobody makes on
  ; purpose". The same bound, so the same answer.
  %neg = icmp slt i64 %n, 0
  %low = select i1 %neg, i64 0, i64 %n
  %big = icmp sgt i64 %low, 1000000
  %k = select i1 %big, i64 1000000, i64 %low
  %lb = call i64 @"beck.str.bytes"(i64 %s)
  %lc = call i64 @"beck.str.chars"(i64 %s)
  %tb = mul i64 %lb, %k
  %tc = mul i64 %lc, %k
  %r = call i64 @"beck.str.alloc"(ptr %err, i64 %tb, i64 %tc, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %copy
copy:
  %pr = call ptr @"beck.str.data"(i64 %r)
  %ps = call ptr @"beck.str.data"(i64 %s)
  br label %loop
loop:
  %i = phi i64 [ 0, %copy ], [ %j, %step ]
  %past = icmp uge i64 %i, %k
  br i1 %past, label %out, label %step
step:
  %at = mul i64 %i, %lb
  %dst = getelementptr inbounds i8, ptr %pr, i64 %at
  %ignored = call ptr @memcpy(ptr %dst, ptr %ps, i64 %lb)
  %j = add i64 %i, 1
  br label %loop
out:
  ret i64 %r
}

"#;

/// The two text functions that also touch a **list** — so they are emitted only when that runtime is
/// there too: `str_join`, which reads one, and `str_split`, which writes one.
const JOINS: &str = r#"; `str_split`, and `str_chars` with it — the evaluator answers characters for an empty separator,
; so the two primitives are one function with two ways of cutting.
;
; Two passes, and the first one exists so the second allocates nothing it has to grow: count the
; pieces, take the list, then fill it. The list's block is at a fixed **offset**, so a piece moving
; the arena costs a reload of the data pointer per element and nothing else — which is `adr/0026`'s
; value-is-an-offset paying for itself.
define internal i64 @"beck.str.split"(ptr noalias %err, i64 %s, i64 %sep, i32 %span) {
entry:
  %len = call i64 @"beck.str.bytes"(i64 %s)
  ; `str_chars` passes the offset **0**, which is never a live object — `beck.alloc` answers it only
  ; on a full arena — so it costs no literal, and a program that writes `str_split(s, "")` reaches
  ; the same path through the length test below.
  %none = icmp eq i64 %sep, 0
  br i1 %none, label %chars, label %measure
measure:
  %seplen = call i64 @"beck.str.bytes"(i64 %sep)
  %bychar = icmp eq i64 %seplen, 0
  br i1 %bychar, label %chars, label %pieces
chars:
  ; Every character is a piece, and the header already knows how many there are.
  %n = call i64 @"beck.str.chars"(i64 %s)
  br label %take
pieces:
  ; One more piece than there are occurrences, which is what `str::split` answers — including for
  ; the empty string, where nothing is found and the one piece is the string itself.
  br label %tally
tally:
  %at = phi i64 [ 0, %pieces ], [ %past, %again ]
  %seen = phi i64 [ 0, %pieces ], [ %more, %again ]
  %hit = call i64 @"beck.str.findat"(i64 %s, i64 %sep, i64 %at)
  %gone = icmp slt i64 %hit, 0
  br i1 %gone, label %counted, label %again
again:
  %past = add i64 %hit, %seplen
  %more = add i64 %seen, 1
  br label %tally
counted:
  %parts = add i64 %seen, 1
  br label %take
take:
  %count = phi i64 [ %n, %chars ], [ %parts, %counted ]
  %onechar = phi i1 [ true, %chars ], [ false, %counted ]
  ; `%seplen` is measured on one of the two paths in, and the cutting loop below is reachable from
  ; both — so it travels as a phi rather than on a dominance that is not there.
  %width = phi i64 [ 0, %chars ], [ %seplen, %counted ]
  %xs = call i64 @"beck.list.alloc"(ptr %err, i64 %count, i32 %span)
  %nolist = icmp eq i64 %xs, 0
  br i1 %nolist, label %out, label %which
which:
  br i1 %onechar, label %walk, label %cutting
walk:
  ; A character is its lead byte and every continuation after it. Nothing here decodes: a piece is
  ; the byte range between two lead bytes.
  %ci = phi i64 [ 0, %which ], [ %cj, %wrote ]
  %cslot = phi i64 [ 0, %which ], [ %cnext, %wrote ]
  %cdone = icmp uge i64 %ci, %len
  br i1 %cdone, label %out, label %stretch
stretch:
  %cp = call ptr @"beck.str.data"(i64 %s)
  %c1 = add i64 %ci, 1
  br label %reach
reach:
  %ck = phi i64 [ %c1, %stretch ], [ %ck1, %continues ]
  %cover = icmp uge i64 %ck, %len
  br i1 %cover, label %cend, label %clook
clook:
  %cbp = getelementptr inbounds i8, ptr %cp, i64 %ck
  %cb = load i8, ptr %cbp
  %ctop = and i8 %cb, -64
  %ccont = icmp eq i8 %ctop, -128
  br i1 %ccont, label %continues, label %cend
continues:
  %ck1 = add i64 %ck, 1
  br label %reach
cend:
  %cj = phi i64 [ %ck, %reach ], [ %ck, %clook ]
  %cpiece = call i64 @"beck.str.piece"(ptr %err, i64 %s, i64 %ci, i64 %cj, i32 %span)
  %cbad = icmp eq i64 %cpiece, 0
  br i1 %cbad, label %out, label %wrote
wrote:
  ; The data pointer is taken here rather than before the loop: a piece allocates, and an allocation
  ; can move the arena under a pointer read before it.
  %cdata = call ptr @"beck.list.data"(i64 %xs)
  %cwp = getelementptr inbounds i64, ptr %cdata, i64 %cslot
  store i64 %cpiece, ptr %cwp
  %cnext = add i64 %cslot, 1
  br label %walk
cutting:
  %lo = phi i64 [ 0, %which ], [ %after, %stored ]
  %slot = phi i64 [ 0, %which ], [ %onwards, %stored ]
  %found = call i64 @"beck.str.findat"(i64 %s, i64 %sep, i64 %lo)
  %last = icmp slt i64 %found, 0
  %upto = select i1 %last, i64 %len, i64 %found
  %piece = call i64 @"beck.str.piece"(ptr %err, i64 %s, i64 %lo, i64 %upto, i32 %span)
  %bad = icmp eq i64 %piece, 0
  br i1 %bad, label %out, label %store
store:
  %data = call ptr @"beck.list.data"(i64 %xs)
  %wp = getelementptr inbounds i64, ptr %data, i64 %slot
  store i64 %piece, ptr %wp
  br i1 %last, label %out, label %stored
stored:
  %after = add i64 %found, %width
  %onwards = add i64 %slot, 1
  br label %cutting
out:
  %r = phi i64 [ 0, %take ], [ %xs, %walk ], [ %xs, %store ], [ 0, %cend ], [ 0, %cutting ]
  ret i64 %r
}

define internal i64 @"beck.str.join"(ptr noalias %err, i64 %xs, i64 %sep, i32 %span) {
entry:
  %n = call i64 @"beck.list.len"(i64 %xs)
  %p = call ptr @"beck.list.data"(i64 %xs)
  %sb = call i64 @"beck.str.bytes"(i64 %sep)
  %sc = call i64 @"beck.str.chars"(i64 %sep)
  %empty = icmp eq i64 %n, 0
  br i1 %empty, label %none, label %measure
none:
  %e = call i64 @"beck.str.alloc"(ptr %err, i64 0, i64 0, i32 %span)
  ret i64 %e
measure:
  br label %sum
sum:
  %i = phi i64 [ 0, %measure ], [ %i1, %add ]
  %b = phi i64 [ 0, %measure ], [ %b1, %add ]
  %c = phi i64 [ 0, %measure ], [ %c1, %add ]
  %past = icmp uge i64 %i, %n
  br i1 %past, label %sized, label %add
add:
  %cell = getelementptr inbounds i64, ptr %p, i64 %i
  %x = load i64, ptr %cell
  %xb = call i64 @"beck.str.bytes"(i64 %x)
  %xc = call i64 @"beck.str.chars"(i64 %x)
  %b1 = add i64 %b, %xb
  %c1 = add i64 %c, %xc
  %i1 = add i64 %i, 1
  br label %sum
sized:
  ; One separator between each pair, which is `n - 1` of them.
  %gaps = sub i64 %n, 1
  %sepb = mul i64 %gaps, %sb
  %sepc = mul i64 %gaps, %sc
  %tb = add i64 %b, %sepb
  %tc = add i64 %c, %sepc
  %r = call i64 @"beck.str.alloc"(ptr %err, i64 %tb, i64 %tc, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %write
write:
  %pr = call ptr @"beck.str.data"(i64 %r)
  %pv = call ptr @"beck.str.data"(i64 %sep)
  br label %walk
walk:
  %k = phi i64 [ 0, %write ], [ %k1, %part ]
  %at = phi i64 [ 0, %write ], [ %at2, %part ]
  %fin = icmp uge i64 %k, %n
  br i1 %fin, label %out, label %maybe
maybe:
  %first = icmp eq i64 %k, 0
  br i1 %first, label %part, label %gap
gap:
  %gd = getelementptr inbounds i8, ptr %pr, i64 %at
  %g = call ptr @memcpy(ptr %gd, ptr %pv, i64 %sb)
  %at1 = add i64 %at, %sb
  br label %part
part:
  %here = phi i64 [ %at, %maybe ], [ %at1, %gap ]
  %pc = getelementptr inbounds i64, ptr %p, i64 %k
  %px = load i64, ptr %pc
  %pb = call i64 @"beck.str.bytes"(i64 %px)
  %pd = call ptr @"beck.str.data"(i64 %px)
  %dst = getelementptr inbounds i8, ptr %pr, i64 %here
  %w = call ptr @memcpy(ptr %dst, ptr %pd, i64 %pb)
  %at2 = add i64 %here, %pb
  %k1 = add i64 %k, 1
  br label %walk
out:
  ret i64 %r
}

"#;

/// Maps: the two functions that do not care what a key or a value *is*.
///
/// A map is a count, then every key in key order, then every value in the same order. The keys
/// being one contiguous run is what makes the search a binary one; the values being another is what
/// makes `map_keys` and `map_values` one `memcpy` each into a fresh list.
///
/// What has to know what a word means is generated per map repr by [`map_functions`]: the binary
/// search, and the lexicographic order over two maps.
const MAPS: &str = r#"; A `Map` is a weight-balanced tree; see `beck_llvm::heap::MAP_NODE` for the shape and for why.
; An empty map is the offset 0, which is the one offset no live object has.
;
; Everything here is one function for the whole module: rebalancing moves *words* — sizes, keys,
; values and two children — and never looks at what a key is. Only the three functions that compare
; are generated per map repr.
define internal i64 @"beck.map.size"(i64 %n) {
entry:
  %empty = icmp eq i64 %n, 0
  br i1 %empty, label %none, label %some
none:
  ret i64 0
some:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %n
  %s = load i64, ptr %p
  ret i64 %s
}

define internal i64 @"beck.map.node"(ptr noalias %err, i64 %k, i64 %v, i64 %l, i64 %r, i32 %span) {
entry:
  %ls = call i64 @"beck.map.size"(i64 %l)
  %rs = call i64 @"beck.map.size"(i64 %r)
  %sub = add i64 %ls, %rs
  %s = add i64 %sub, 1
  %off = call i64 @"beck.alloc"(ptr %err, i64 40, i32 %span)
  %failed = icmp eq i64 %off, 0
  br i1 %failed, label %out, label %fill
fill:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %off
  store i64 %s, ptr %p
  %pk = getelementptr inbounds i8, ptr %p, i64 8
  store i64 %k, ptr %pk
  %pv = getelementptr inbounds i8, ptr %p, i64 16
  store i64 %v, ptr %pv
  %pl = getelementptr inbounds i8, ptr %p, i64 24
  store i64 %l, ptr %pl
  %pr = getelementptr inbounds i8, ptr %p, i64 32
  store i64 %r, ptr %pr
  br label %out
out:
  ret i64 %off
}

; The four fields, so the rotations below read like the algorithm rather than like arithmetic.
define internal i64 @"beck.map.key"(i64 %n) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %n
  %q = getelementptr inbounds i8, ptr %p, i64 8
  %w = load i64, ptr %q
  ret i64 %w
}

define internal i64 @"beck.map.value"(i64 %n) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %n
  %q = getelementptr inbounds i8, ptr %p, i64 16
  %w = load i64, ptr %q
  ret i64 %w
}

define internal i64 @"beck.map.left"(i64 %n) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %n
  %q = getelementptr inbounds i8, ptr %p, i64 24
  %w = load i64, ptr %q
  ret i64 %w
}

define internal i64 @"beck.map.right"(i64 %n) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %n
  %q = getelementptr inbounds i8, ptr %p, i64 32
  %w = load i64, ptr %q
  ret i64 %w
}

; Adams's rebalance, with `beck_core::pmap`'s own DELTA = 3 and RATIO = 2. Four cases and no loop:
; a subtree that grew by one is at most one rotation away from balanced.
define internal i64 @"beck.map.balance"(ptr noalias %err, i64 %k, i64 %v, i64 %l, i64 %r, i32 %span) {
entry:
  %ls = call i64 @"beck.map.size"(i64 %l)
  %rs = call i64 @"beck.map.size"(i64 %r)
  %tot = add i64 %ls, %rs
  %tiny = icmp ule i64 %tot, 1
  br i1 %tiny, label %plain, label %ask.right
plain:
  %flat = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %l, i64 %r, i32 %span)
  ret i64 %flat
ask.right:
  %ld = mul i64 %ls, 3
  %heavy.r = icmp ugt i64 %rs, %ld
  br i1 %heavy.r, label %left.rot, label %ask.left
left.rot:
  %rk = call i64 @"beck.map.key"(i64 %r)
  %rv = call i64 @"beck.map.value"(i64 %r)
  %rl = call i64 @"beck.map.left"(i64 %r)
  %rr = call i64 @"beck.map.right"(i64 %r)
  %rls = call i64 @"beck.map.size"(i64 %rl)
  %rrs = call i64 @"beck.map.size"(i64 %rr)
  %rrx = mul i64 %rrs, 2
  %single.l = icmp ult i64 %rls, %rrx
  br i1 %single.l, label %single.left, label %double.left
single.left:
  %sl.inner = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %l, i64 %rl, i32 %span)
  %sl.out = call i64 @"beck.map.node"(ptr %err, i64 %rk, i64 %rv, i64 %sl.inner, i64 %rr, i32 %span)
  ret i64 %sl.out
double.left:
  %rlk = call i64 @"beck.map.key"(i64 %rl)
  %rlv = call i64 @"beck.map.value"(i64 %rl)
  %rll = call i64 @"beck.map.left"(i64 %rl)
  %rlr = call i64 @"beck.map.right"(i64 %rl)
  %dl.a = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %l, i64 %rll, i32 %span)
  %dl.b = call i64 @"beck.map.node"(ptr %err, i64 %rk, i64 %rv, i64 %rlr, i64 %rr, i32 %span)
  %dl.out = call i64 @"beck.map.node"(ptr %err, i64 %rlk, i64 %rlv, i64 %dl.a, i64 %dl.b, i32 %span)
  ret i64 %dl.out
ask.left:
  %rd = mul i64 %rs, 3
  %heavy.l = icmp ugt i64 %ls, %rd
  br i1 %heavy.l, label %right.rot, label %settled
settled:
  %same = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %l, i64 %r, i32 %span)
  ret i64 %same
right.rot:
  %lk = call i64 @"beck.map.key"(i64 %l)
  %lv = call i64 @"beck.map.value"(i64 %l)
  %ll = call i64 @"beck.map.left"(i64 %l)
  %lr = call i64 @"beck.map.right"(i64 %l)
  %lls = call i64 @"beck.map.size"(i64 %ll)
  %lrs = call i64 @"beck.map.size"(i64 %lr)
  %llx = mul i64 %lls, 2
  %single.r = icmp ult i64 %lrs, %llx
  br i1 %single.r, label %single.right, label %double.right
single.right:
  %sr.inner = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %lr, i64 %r, i32 %span)
  %sr.out = call i64 @"beck.map.node"(ptr %err, i64 %lk, i64 %lv, i64 %ll, i64 %sr.inner, i32 %span)
  ret i64 %sr.out
double.right:
  %lrk = call i64 @"beck.map.key"(i64 %lr)
  %lrv = call i64 @"beck.map.value"(i64 %lr)
  %lrl = call i64 @"beck.map.left"(i64 %lr)
  %lrr = call i64 @"beck.map.right"(i64 %lr)
  %dr.a = call i64 @"beck.map.node"(ptr %err, i64 %lk, i64 %lv, i64 %ll, i64 %lrl, i32 %span)
  %dr.b = call i64 @"beck.map.node"(ptr %err, i64 %k, i64 %v, i64 %lrr, i64 %r, i32 %span)
  %dr.out = call i64 @"beck.map.node"(ptr %err, i64 %lrk, i64 %lrv, i64 %dr.a, i64 %dr.b, i32 %span)
  ret i64 %dr.out
}

; The `i`th entry in key order, by subtree size. `map_keys`, `map_values` and the comparison all
; walk in order; only the comparison needs to do it by index, and this is how.
define internal i64 @"beck.map.nth"(i64 %m, i64 %i) {
entry:
  br label %loop
loop:
  %n = phi i64 [ %m, %entry ], [ %next, %step ]
  %want = phi i64 [ %i, %entry ], [ %want1, %step ]
  %empty = icmp eq i64 %n, 0
  br i1 %empty, label %missing, label %probe
probe:
  %l = call i64 @"beck.map.left"(i64 %n)
  %ls = call i64 @"beck.map.size"(i64 %l)
  %here = icmp eq i64 %want, %ls
  br i1 %here, label %found, label %step
step:
  %down = icmp ult i64 %want, %ls
  %r = call i64 @"beck.map.right"(i64 %n)
  %next = select i1 %down, i64 %l, i64 %r
  %past = add i64 %ls, 1
  %rest = sub i64 %want, %past
  %want1 = select i1 %down, i64 %want, i64 %rest
  br label %loop
found:
  ret i64 %n
missing:
  ret i64 0
}

; The in-order walk that fills a list. One function for keys and values, told which word to take —
; the two differ by eight bytes and nothing else.
define internal i64 @"beck.map.into"(i64 %n, ptr %dst, i64 %i, i64 %slot) {
entry:
  %empty = icmp eq i64 %n, 0
  br i1 %empty, label %done, label %walk
walk:
  %l = call i64 @"beck.map.left"(i64 %n)
  %i1 = call i64 @"beck.map.into"(i64 %l, ptr %dst, i64 %i, i64 %slot)
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %n
  %q = getelementptr inbounds i64, ptr %p, i64 %slot
  %w = load i64, ptr %q
  %at = getelementptr inbounds i64, ptr %dst, i64 %i1
  store i64 %w, ptr %at
  %i2 = add i64 %i1, 1
  %r = call i64 @"beck.map.right"(i64 %n)
  %i3 = call i64 @"beck.map.into"(i64 %r, ptr %dst, i64 %i2, i64 %slot)
  ret i64 %i3
done:
  ret i64 %i
}

; `map_keys` and `map_values`: a fresh list of the map's size, filled by the walk above.
define internal i64 @"beck.map.run"(ptr noalias %err, i64 %m, i64 %slot, i32 %span) {
entry:
  %n = call i64 @"beck.map.size"(i64 %m)
  %r = call i64 @"beck.list.alloc"(ptr %err, i64 %n, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %fill
fill:
  %dst = call ptr @"beck.list.data"(i64 %r)
  %ignored = call i64 @"beck.map.into"(i64 %m, ptr %dst, i64 0, i64 %slot)
  br label %out
out:
  ret i64 %r
}
"#;

/// Moving bytes to and from the host, in whatever pieces the pipe hands over.
///
/// A short count means the pipe is closed, which is how the worker learns the host has gone.
const PIPE: &str = r#"define internal i64 @"beck.read_exact"(ptr %p, i64 %n) {
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

"#;
