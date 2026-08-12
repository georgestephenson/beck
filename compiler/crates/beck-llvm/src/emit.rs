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
//! `Bool`, or a `model`, `union` or `newtype` that [`crate::heap`] can lay out — and whose body is
//! built from constants, variables, `let`, `if`, `match`, direct calls to other compiled
//! definitions, record and variant construction, field reads, `with`, and the arithmetic,
//! comparison and logical primitives.
//!
//! A list, a string, a map, a closure and every effect are **refused** — by name, with the reason,
//! in [`crate::Report`]. Nothing is silently approximated: a definition either compiles to machine
//! code that agrees with the evaluator on every input, or it does not compile.
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
//! `Function::normalise` has the whole story, and `docs/93` §93.2 is where it is written down.

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
        }
    }

    pub fn from_code(code: u32) -> Option<Trap> {
        const ALL: [Trap; 12] = [
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
        bodies.push_str(&text);
        bodies.push('\n');
    }

    let functions: Vec<Signature> = order.iter().map(|n| indexed[n].clone()).collect();
    let (layouts, elements, entries) = reachable(&compared, &list_compared, &map_compared, &heap);
    let ir = assemble(&bodies, &functions, &heap, &layouts, &elements, &entries);
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
    /// Whether this body reaches the arena, and therefore needs its base in a register.
    uses_heap: bool,
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
            uses_heap: false,
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
            Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => Trap::NoMatchData,
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
                let cond = self.equals(v, &want);
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
                    let t = self.equals(v, &want);
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
            Pattern::List { .. } => {
                Err("matches a list pattern, and a collection is not on this heap yet".into())
            }
        }
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
            Repr::Int | Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => {
                self.line(format!("{r} = load i64, ptr {p}"))
            }
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
            Repr::Int | Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => {
                self.line(format!("store i64 {}, ptr {p}", v.text))
            }
        }
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
    /// [`docs/101`] §101.6's first row, and it is a cost rather than a difference — the answer is
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

    fn prim(&mut self, op: Prim, args: &[Core], ty: &Ty, span: Span) -> Result<Val, String> {
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
                    Repr::Bool | Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => {
                        Err(format!("`{}` on a value that is not a number", op.name()))
                    }
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
                    Repr::Bool | Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => {
                        Err("`negate` on a value that is not a number".into())
                    }
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
                    Repr::Bool | Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => {
                        Err("`abs` on a value that is not a number".into())
                    }
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
                Ok(self.compare(op, &vals[0], &vals[1]))
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
                self.wants(Repr::List(at));
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
                self.wants(Repr::Map(at));
                let word = self.widen(&vals[1]);
                let found = self.fresh();
                self.line(format!(
                    "{found} = call i64 @\"beck.map.find.{at}\"(i64 {}, i64 {word})",
                    vals[0].text
                ));
                if op == Prim::MapContains {
                    let r = self.fresh();
                    self.line(format!("{r} = icmp sge i64 {found}, 0"));
                    return Ok(Val {
                        text: r,
                        ty: Repr::Bool,
                    });
                }
                self.map_get(ty, &vals[0], &found, self.heap.element(v), span)
            }
            Prim::MapKeys | Prim::MapValues => {
                arity(1)?;
                self.map_arg(&vals[0], op)?;
                self.map_run(op, ty, &vals[0], span)
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
            other => Err(refusal(other)),
        }
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
        let off = self.alloc(heap::list_bytes(xs.len() as u64), span);
        self.store_word(&off, 0, &xs.len().to_string());
        for (i, v) in vals.iter().enumerate() {
            self.store_field(&off, i + 1, v);
        }
        Ok(Val {
            text: off,
            ty: repr,
        })
    }

    /// The address of element `i` of `xs`, where `i` is a value rather than a constant.
    fn element_addr(&mut self, xs: &Val, index: &str) -> String {
        let base = self.base();
        let p = self.fresh();
        self.line(format!(
            "{p} = getelementptr inbounds i8, ptr {base}, i64 {}",
            xs.text
        ));
        let q = self.fresh();
        self.line(format!("{q} = getelementptr inbounds i8, ptr {p}, i64 8"));
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
        let off = self.alloc(heap::map_bytes(0), span);
        self.store_word(&off, 0, "0");
        Ok(Val {
            text: off,
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

    /// How many entries, which is the header word.
    fn map_len(&mut self, m: &Val) -> String {
        self.uses_heap = true;
        let base = self.base();
        let p = self.fresh();
        self.line(format!(
            "{p} = getelementptr inbounds i8, ptr {base}, i64 {}",
            m.text
        ));
        let r = self.fresh();
        self.line(format!("{r} = load i64, ptr {p}"));
        r
    }

    /// `map_get` — an `Option[V]` from the index a search answered, and **no branch**.
    ///
    /// [`Function::list_get`]'s trick, with the value's address rather than the element's: the
    /// values start `n` words after the keys, so entry `i`'s value is word `1 + n + i`. A miss
    /// loads the header instead, which is always there, and the `None` tag means nobody reads it.
    fn map_get(
        &mut self,
        ty: &Ty,
        m: &Val,
        found: &str,
        value: Repr,
        span: Span,
    ) -> Result<Val, String> {
        let (option, some, none, slot, bytes) = self.option_of(ty, value)?;
        let n = self.map_len(m);
        let inside = self.fresh();
        self.line(format!("{inside} = icmp sge i64 {found}, 0"));
        let safe = self.fresh();
        self.line(format!("{safe} = select i1 {inside}, i64 {found}, i64 0"));
        let at = self.fresh();
        self.line(format!("{at} = add i64 {safe}, {n}"));
        let base = self.base();
        let p = self.fresh();
        self.line(format!(
            "{p} = getelementptr inbounds i8, ptr {base}, i64 {}",
            m.text
        ));
        let data = self.fresh();
        self.line(format!(
            "{data} = getelementptr inbounds i8, ptr {p}, i64 8"
        ));
        let cell = self.fresh();
        self.line(format!(
            "{cell} = getelementptr inbounds i64, ptr {data}, i64 {at}"
        ));
        let addr = self.fresh();
        self.line(format!("{addr} = select i1 {inside}, ptr {cell}, ptr {p}"));
        let w = self.fresh();
        self.line(format!("{w} = load i64, ptr {addr}"));

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
        let n = self.map_len(m);
        let from = if op == Prim::MapKeys {
            "0".to_string()
        } else {
            n.clone()
        };
        self.uses_heap = true;
        let idx = self.span(span);
        let r = self.fresh();
        self.line(format!(
            "{r} = call i64 @\"beck.map.run\"(ptr %err, i64 {}, i64 {from}, i64 {n}, i32 {idx})",
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

    fn equals(&mut self, a: &Val, b: &Val) -> String {
        let cmp = self.compare(Prim::Eq, a, b);
        cmp.text
    }

    /// Record that this repr's comparison has to exist.
    ///
    /// One method rather than three call sites, so that adding a reference kind means teaching
    /// [`heap::Repr::order`] and this — and `reachable` closes over whatever they name.
    fn wants(&mut self, r: Repr) {
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
            Repr::Int | Repr::Float | Repr::Bool | Repr::Str => {}
        }
    }

    fn compare(&mut self, op: Prim, a: &Val, b: &Val) -> Val {
        // Reals compare through the order key and Bools compare unsigned, so `false < true`. Both
        // are the ordering `Value`'s derived `Ord` gives, which is the one the evaluator uses.
        // An object compares through the function `compare_functions` emitted for its layout,
        // which answers -1, 0 or 1 — so the six operators are one call and one integer test.
        let (lhs, rhs, signed) = match a.ty.order() {
            heap::Order::Key => (self.order_key(a), self.order_key(b), false),
            heap::Order::Words { signed } => (a.text.clone(), b.text.clone(), signed),
            // A reference decides through the three-way comparison for whatever it refers to. The
            // symbol is `Repr::order`'s, which is the only place that names one.
            heap::Order::Call(symbol) => {
                self.wants(a.ty);
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
        Val {
            text: r,
            ty: Repr::Bool,
        }
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
            Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => v.text.clone(),
        }
    }
}

/// Why a primitive this backend does not compile is not compiled.
///
/// The string half is spelled out one at a time rather than swept into "not a scalar primitive",
/// because since `docs/105` a `Str` *is* a value here and "text is not on this heap" would be
/// false. Each reason names the thing that is missing rather than the primitive that wanted it —
/// which is the difference between a refusal a reader can act on and one they can only observe.
fn refusal(op: Prim) -> String {
    let why = match op {
        Prim::StrSplit | Prim::StrChars => {
            "answers with a list whose elements it also allocates, which is two loops rather than \
             the one every list this backend builds has"
        }
        // The one that is a decision rather than a gap. `docs/69` §69.7 and `docs/101` §101.5 both
        // name shipping this as the mistake: the tree-walker pushes in place when `liveness` proves
        // the accumulator is a last use, and an arena with no ownership in it cannot, so every loop
        // in the language would be quadratic here and linear there.
        Prim::ListAppend | Prim::ConcatLists => {
            "grows a list, and this arena cannot prove nobody else holds the one it would push \
             into — so the accumulator every loop is written as would be quadratic here and linear \
             in the evaluator"
        }
        Prim::ListZip => "answers with a list of pairs, and there is no pair type to lay out",
        // The same rule `list_append` gets, one type over: the evaluator's `PMap` shares everything
        // it did not touch and rebuilds one path, and a sorted run in an arena has to copy all of
        // it. `docs/107` §107.4 is the argument.
        Prim::MapInsert | Prim::MapRemove | Prim::MapMerge => {
            "grows a map, and a sorted run in an arena has to be copied whole where the \
             evaluator's tree rebuilds one path"
        }
        // `map_list` and the rest of the higher-order half are deliberately absent. Their argument
        // is a function, `prim` evaluates its arguments before it looks at the operator, and
        // "`double_it` is used as a value rather than called, and a function value is a closure" is
        // both truer and more specific than anything this table could say. A reason that cannot be
        // produced is `docs/89` §89.5's unreachable rule, and the answer is the same: delete it.
        Prim::StrUpper | Prim::StrLower => {
            "is Unicode case mapping, which is a table rather than an operation — and a compiled \
             half-answer that folded ASCII only would disagree with the evaluator on the first \
             letter that is not"
        }
        Prim::StrTrim => {
            "trims Unicode whitespace, which is a table for the same reason case mapping is"
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
fn assemble(
    bodies: &str,
    functions: &[Signature],
    heap: &Heap,
    compared: &BTreeSet<u32>,
    lists: &BTreeSet<u32>,
    maps: &BTreeSet<u32>,
) -> String {
    let arena = !heap.is_empty();
    let mut m = String::new();
    m.push_str(HEADER);
    if arena {
        let _ = write!(m, "{}", arena_prelude());
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
        m.push_str(
            r#"  %ok = icmp eq i64 %cell, 0
  %wants = load i64, ptr @"beck.reply"
  %onheap = icmp ne i64 %wants, 0
  %both = and i1 %ok, %onheap
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
///
/// `beck.str.byteof` is the one with a cost worth naming: it is constant time when the text is
/// ASCII — every character one byte, which is what the two equal counts say — and a walk otherwise,
/// where the evaluator has a chunked index and answers in at most a stride
/// ([`beck_core::core::Text`]). `docs/105` §105.6 carries that as a difference rather than hiding
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
define internal i64 @"beck.map.find.{at}"(i64 %m, i64 %k) {{
entry:
  %n = call i64 @"beck.map.len"(i64 %m)
  %p = call ptr @"beck.map.data"(i64 %m)
  br label %loop
loop:
  ; The half-open window `[lo, hi)`. Unsigned throughout, because both ends are counts.
  %lo = phi i64 [ 0, %entry ], [ %lo1, %again ]
  %hi = phi i64 [ %n, %entry ], [ %hi1, %again ]
  %done = icmp uge i64 %lo, %hi
  br i1 %done, label %missing, label %probe
probe:
  %span = sub i64 %hi, %lo
  %half = lshr i64 %span, 1
  %mid = add i64 %lo, %half
  %at = getelementptr inbounds i64, ptr %p, i64 %mid
  %w = load i64, ptr %at
  %c = call i64 @"beck.elem.cmp.{key}"(i64 %w, i64 %k)
  %hit = icmp eq i64 %c, 0
  br i1 %hit, label %found, label %again
again:
  %less = icmp slt i64 %c, 0
  %mid1 = add i64 %mid, 1
  %lo1 = select i1 %less, i64 %mid1, i64 %lo
  %hi1 = select i1 %less, i64 %hi, i64 %mid
  br label %loop
found:
  ret i64 %mid
missing:
  ret i64 -1
}}

define internal i64 @"beck.map.cmp.{at}"(i64 %a, i64 %b) {{
entry:
  %la = call i64 @"beck.map.len"(i64 %a)
  %lb = call i64 @"beck.map.len"(i64 %b)
  %shorter = icmp ult i64 %la, %lb
  %n = select i1 %shorter, i64 %la, i64 %lb
  %pa = call ptr @"beck.map.data"(i64 %a)
  %pb = call ptr @"beck.map.data"(i64 %b)
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %j, %next ]
  %past = icmp uge i64 %i, %n
  br i1 %past, label %lengths, label %keys
keys:
  %ka = getelementptr inbounds i64, ptr %pa, i64 %i
  %kb = getelementptr inbounds i64, ptr %pb, i64 %i
  %wka = load i64, ptr %ka
  %wkb = load i64, ptr %kb
  %ck = call i64 @"beck.elem.cmp.{key}"(i64 %wka, i64 %wkb)
  %kdecided = icmp ne i64 %ck, 0
  br i1 %kdecided, label %answerk, label %values
answerk:
  ret i64 %ck
values:
  ; The values start `la` words after the keys in `a` and `lb` words after them in `b`.
  %ia = add i64 %i, %la
  %ib = add i64 %i, %lb
  %va = getelementptr inbounds i64, ptr %pa, i64 %ia
  %vb = getelementptr inbounds i64, ptr %pb, i64 %ib
  %wva = load i64, ptr %va
  %wvb = load i64, ptr %vb
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
/// closure would need, and `docs/101` §101.5 lists it as unbuilt.
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
/// **answer** rather than the list, which is `docs/105` §105.7's rule applied one type over.
const LISTS: &str = r#"define internal i64 @"beck.list.alloc"(ptr noalias %err, i64 %n, i32 %span) {
entry:
  %body = mul i64 %n, 8
  %total = add i64 %body, 8
  %off = call i64 @"beck.alloc"(ptr %err, i64 %total, i32 %span)
  %failed = icmp eq i64 %off, 0
  br i1 %failed, label %out, label %fill
fill:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %off
  store i64 %n, ptr %p
  br label %out
out:
  ret i64 %off
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
  %at = add i64 %xs, 8
  %p = getelementptr inbounds i8, ptr %hp, i64 %at
  ret ptr %p
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
/// `Value::display`, and `docs/105` §105.4 refused this whole primitive rather than answer a decimal
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

/// `str_join`, which is the one of the three that reads a **list** — so it is emitted only when
/// that runtime is there too.
const JOINS: &str = r#"define internal i64 @"beck.str.join"(ptr noalias %err, i64 %xs, i64 %sep, i32 %span) {
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
const MAPS: &str = r#"define internal i64 @"beck.map.alloc"(ptr noalias %err, i64 %n, i32 %span) {
entry:
  %pairs = mul i64 %n, 16
  %total = add i64 %pairs, 8
  %off = call i64 @"beck.alloc"(ptr %err, i64 %total, i32 %span)
  %failed = icmp eq i64 %off, 0
  br i1 %failed, label %out, label %fill
fill:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %off
  store i64 %n, ptr %p
  br label %out
out:
  ret i64 %off
}

define internal i64 @"beck.map.len"(i64 %m) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %p = getelementptr inbounds i8, ptr %hp, i64 %m
  %n = load i64, ptr %p
  ret i64 %n
}

define internal ptr @"beck.map.data"(i64 %m) {
entry:
  %hp = load ptr, ptr @"beck.heap"
  %at = add i64 %m, 8
  %p = getelementptr inbounds i8, ptr %hp, i64 %at
  ret ptr %p
}

; `map_keys` and `map_values`: a run of `count` words starting at word `from` of the data area,
; copied into a fresh list. The one place a map turns into a list.
define internal i64 @"beck.map.run"(ptr noalias %err, i64 %m, i64 %from, i64 %count, i32 %span) {
entry:
  %r = call i64 @"beck.list.alloc"(ptr %err, i64 %count, i32 %span)
  %failed = icmp eq i64 %r, 0
  br i1 %failed, label %out, label %move
move:
  %pr = call ptr @"beck.list.data"(i64 %r)
  %pm = call ptr @"beck.map.data"(i64 %m)
  %skip = mul i64 %from, 8
  %at = getelementptr inbounds i8, ptr %pm, i64 %skip
  %bytes = mul i64 %count, 8
  %ignored = call ptr @memcpy(ptr %pr, ptr %at, i64 %bytes)
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
