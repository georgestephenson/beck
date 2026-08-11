//! `Core` — the load-bearing IR, and the evaluator that runs it.
//!
//! [`docs/04-compiler-architecture.md`](../../../../../docs/04-compiler-architecture.md) §4.2:
//! "Typed ANF/SSA hybrid; explicit closures, explicit effect operations, explicit tier annotation
//! per node; `Query` sub-language kept *symbolic*. Typechecked semantics, placement, splitting,
//! optimisation. The load-bearing IR."
//!
//! Two things stay symbolic here, exactly as §4.2 demands, and for the same stated reason —
//! lowering them early would foreclose Phase 3:
//!
//! * **Signal and stream operations** are [`Prim`]s ([`Prim::Fold`], [`Prim::Durable`],
//!   [`Prim::SignalMap`], …), not loops. A fold that has already become a loop cannot be compiled
//!   to an incremental dataflow plan.
//! * **UI trees** are `Html` *values* built by [`Prim::HtmlEl`] and friends, not DOM mutation
//!   calls, so the same value can be server-side rendered, diffed, or (Phase 3) compiled for the
//!   client.
//!
//! # On the backend
//!
//! The roadmap names Cranelift as Phase 1's server backend. What is here instead is a **`Core`
//! evaluator**: the "engine-in-Rust with the language as its configuration" route that
//! [`docs/00-original-idea.md`](../../../../../docs/00-original-idea.md) names as one of the three
//! that work for a GC'd functional language on a Rust host (Materialize's shape). It is the
//! deliberately-bad-but-complete option, and it keeps the `Core → Target` seam narrow, which §5.2
//! says is what lets a backend slot in later. The Phase 1 report says plainly that native codegen
//! is not done.

use std::fmt;
use std::sync::Arc;

use beck_diag::Span;

use crate::html::Html;
use crate::pmap::PMap;
use crate::ty::{Effect, Tier, Ty};

pub type VarId = u32;

/// A primitive operation. Everything the standard library provides in Phase 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Prim {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    /// `abs`, `sqrt` and `Int` → `Float`, which are the three the numeric tower needs before any
    /// of SICP §1.1.7 will run (`docs/27` §27.2). `Abs` is resolved from its operand the way the
    /// arithmetic operators are; the other two are monomorphic.
    Abs,
    Sqrt,
    Sin,
    Cos,
    Trunc,
    ToFloat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    ToStr,
    StrTrim,
    StrToInt,
    // ---- strings (docs/27 §? — Wave 2's string half). One primitive each, because a string is
    // where a language's host has to be asked and there is nothing to express in Beck itself.
    StrLen,
    StrSlice,
    StrSplit,
    StrJoin,
    StrContains,
    StrStartsWith,
    StrEndsWith,
    StrUpper,
    StrLower,
    StrReplace,
    StrIndexOf,
    StrRepeat,
    StrChars,
    StrIsEmpty,
    ListLen,
    ListIsEmpty,
    // ---- collections
    ListGet,
    ListSlice,
    ListReverse,
    ListTake,
    ListDrop,
    ListContains,
    ListIndexOf,
    ListFold,
    ListAll,
    ListAny,
    ListFlatMap,
    ListZip,
    ListAppend,
    MapKeys,
    MapMerge,
    // ---- JSON, and the standard library's first fallible function.
    //
    // `json_parse` raises rather than returning a `Result`, which is the shape
    // [`docs/27`](../../../../../docs/27-the-walls-come-down-report.md) settled and the reason §8.5.3's trap 2
    // said the library had to wait for it: a caller that wants a `Result` writes `try:`, and a
    // caller inside something already fallible writes nothing at all.
    JsonParse,
    JsonRender,
    // ---- time. `now()` gives milliseconds; these two are the civil calendar over them.
    TimeFormat,
    TimeParse,
    // ---- digests, encodings and identifiers (`crate::digest`). A hash is a table and base64 is a
    // grammar, so both are the host's half of `lib/README.md`'s division; a digest is also a *pure*
    // function, which is what separates this from the other things a crypto library offers.
    Digest,
    /// A message authentication code: the one primitive whose input is a `secret[Str]` and whose
    /// output is a `Str`. Charged `cap.sign` rather than left free, so a view cannot mint one —
    /// `docs/adr/0014` is the record of the decision and §3.5 is what it is measured against.
    DigestKeyed,
    /// Constant-time equality, for the caller comparing a digest against one that arrived.
    DigestEq,
    HexEncode,
    HexDecode,
    Base64Encode,
    Base64Decode,
    /// Validates and *normalises*: two spellings of one identifier must not be two map keys.
    UuidParse,
    UuidVersion,
    // ---- the outbound call. The *second* primitive whose row is a function of its argument
    // (`Raise` is the first): the host it is given is the `net.out(host)` atom it performs, which
    // is why that argument has to be written at the call site rather than computed.
    HttpFetch,
    MapList,
    FilterList,
    ConcatLists,
    SortBy,
    MapGet,
    MapInsert,
    MapRemove,
    MapValues,
    MapContains,
    MapLen,
    OptionIsSome,
    OptionUnwrapOr,
    HtmlEl,
    HtmlText,
    HtmlAttr,
    HtmlOn,
    HtmlKey,
    /// Mints a fresh id. Nondeterministic, so §3.7 forbids it inside a fold; the client mints
    /// entity ids instead, which is "the small tell that browsers here are replicas, not
    /// terminals".
    NewUuid,
    /// Reads the wall clock. The other half of §3.7's rule: "time is data on the envelope".
    Now,
    /// Reads a secret from the process environment, yielding a `secret[Str]` (§3.5).
    SecretEnv,
    /// `raise e` — fail with a value.
    ///
    /// The atom it performs is `raises(T)`, which depends on the *type* of its argument, so the
    /// checker attaches it where that type is known rather than [`Prim::effects`] declaring it.
    /// This is the first primitive whose row is not a constant, and it is why that table's doc
    /// says "the atoms this primitive performs *itself*".
    Raise,
    /// `try: block` — run a thunk, and turn a raise of the named type into an `Err`.
    ///
    /// Two arguments: the thunk, and the name of the error type this handler catches. The name is
    /// what stops a handler from catching a failure it cannot type — a caller's function may raise
    /// something this `try` never heard of, and that has to keep travelling.
    Try,
    /// `parallel: block` — run the scope's children, then its tail with their results bound.
    ///
    /// The arguments are the children's thunks followed by the continuation, so a child cannot
    /// outlive the scope: there is no handle, and the only thing that can read a child's result is
    /// the one lambda the scope built. That is [`docs/38`](../../../../../docs/38-literature-survey.md)
    /// §38.4's "spawn/await as effect operations, the scope as their handler" with the handler as
    /// the *only* form — the operations are not separately reachable.
    ///
    /// The children are independent by construction (none of them can name another) and no child
    /// may perform an effect another child could observe, so the scope's answer does not depend on
    /// the order they ran in. A backend may therefore run them together; running them in the order
    /// they are written is a correct implementation of that, and is what the tree-walker does.
    Parallel,
    /// Wraps a value as `internal[T]`: storable, never Sendable.
    InternalOf,
    /// Unwraps one. Performs `cap.internal`, so only the authority chokepoint can do it.
    Reveal,
    // ---- the symbolic signal vocabulary (§3.7) ----
    MergeClients,
    /// `presence()` — who is connected now, as a signal that is not a function of the log.
    ///
    /// D6's last row. It performs `cap.presence` rather than an atom of its own, which is F16
    /// ([`docs/14`](../../../../../docs/14-review-findings.md)) taken literally: "presence signals
    /// leak who-is-online; gate behind a capability like any other view". The capability is also
    /// what places it — no tier below the server discharges a `cap.*` — so a fold cannot read the
    /// roster and a view reaches it across a declared edge.
    Presence,
    /// `freshness()` — whether the page being rendered is the confirmed state or a guess.
    ///
    /// §3.7's "`Signal[T]` carries a freshness dimension (`confirmed | pending(n)`) that UI code
    /// can render (\"saving…\") — staleness is typed, not pretended away". It is the mirror image
    /// of [`Prim::Presence`]: presence is a fact the *server* holds about its sockets and cannot
    /// reach a Mode B client, and freshness is a fact the *client* holds about its own guesses and
    /// is `Confirmed` everywhere else. No capability, because nothing is disclosed by it — a client
    /// counting its own unacknowledged commands is reading itself.
    Freshness,
    StreamFilterMap,
    Fold,
    Durable,
    SignalMap,
    SignalMap2,
    /// §3.8's per-session view: `todos.map(filter_by(session.user))`. First-class because "the
    /// fanout cost becomes a first-class engineering concern".
    PerSession,
    /// The authority chokepoint: the sole consumer of ingress, holding the accumulator so that
    /// first-writer-wins and ownership can be decided (§3.7, F2).
    Decide,
}

impl Prim {
    pub fn name(self) -> &'static str {
        use Prim::*;
        match self {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Rem => "%",
            Neg => "negate",
            Raise => "raise",
            Try => "try",
            Parallel => "parallel",
            Abs => "abs",
            Sqrt => "sqrt",
            Sin => "sin",
            Cos => "cos",
            Trunc => "trunc",
            ToFloat => "float",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            And => "and",
            Or => "or",
            Not => "not",
            ToStr => "str",
            StrTrim => "str_trim",
            StrToInt => "str_to_int",
            StrLen => "str_len",
            StrSlice => "str_slice",
            StrSplit => "str_split",
            StrJoin => "str_join",
            StrContains => "str_contains",
            StrStartsWith => "str_starts_with",
            StrEndsWith => "str_ends_with",
            StrUpper => "str_upper",
            StrLower => "str_lower",
            StrReplace => "str_replace",
            StrIndexOf => "str_index_of",
            StrRepeat => "str_repeat",
            StrChars => "str_chars",
            ListGet => "list_get",
            ListSlice => "list_slice",
            ListReverse => "list_reverse",
            ListTake => "list_take",
            ListDrop => "list_drop",
            ListContains => "list_contains",
            ListIndexOf => "list_index_of",
            ListFold => "list_fold",
            ListAll => "list_all",
            ListAny => "list_any",
            ListFlatMap => "list_flat_map",
            ListZip => "list_zip_with",
            ListAppend => "list_append",
            MapKeys => "map_keys",
            MapMerge => "map_merge",
            JsonParse => "json_parse",
            JsonRender => "json_render",
            TimeFormat => "time_format",
            TimeParse => "time_parse",
            Digest => "digest",
            DigestKeyed => "digest_keyed",
            DigestEq => "digest_eq",
            HexEncode => "hex_encode",
            HexDecode => "hex_decode",
            Base64Encode => "base64_encode",
            Base64Decode => "base64_decode",
            UuidParse => "uuid_parse",
            UuidVersion => "uuid_version",
            HttpFetch => "http_fetch",
            StrIsEmpty => "str_is_empty",
            ListLen => "list_len",
            ListIsEmpty => "list_is_empty",
            MapList => "map_list",
            FilterList => "filter_list",
            ConcatLists => "concat_lists",
            SortBy => "sort_by",
            MapGet => "map_get",
            MapInsert => "map_insert",
            MapRemove => "map_remove",
            MapValues => "map_values",
            MapContains => "map_contains",
            MapLen => "map_len",
            OptionIsSome => "is_some",
            OptionUnwrapOr => "unwrap_or",
            HtmlEl => "html_el",
            HtmlText => "html_text",
            HtmlAttr => "html_attr",
            HtmlOn => "html_on",
            HtmlKey => "html_key",
            NewUuid => "uuid",
            Now => "now",
            SecretEnv => "secret_env",
            InternalOf => "internal_of",
            Reveal => "reveal",
            MergeClients => "merge_clients",
            Presence => "presence",
            Freshness => "freshness",
            StreamFilterMap => "filter_map",
            Fold => "fold",
            Durable => "durable",
            SignalMap => "signal_map",
            SignalMap2 => "map2",
            PerSession => "per_session",
            Decide => "decide",
        }
    }

    /// The atoms this primitive performs *itself*.
    ///
    /// The polymorphic half of a primitive's row — `map_list`'s `e`, which is whatever its function
    /// argument does — lives in the scheme in [`crate::prelude`], because a row variable is not a
    /// constant. A test holds the two in agreement.
    pub fn effects(self) -> Vec<Effect> {
        match self {
            // "Every connected client's send!s, interleaved. Arbitrary order — this is the
            // nondeterminism; there is exactly one of these."
            Prim::MergeClients => vec![Effect::Ingress],
            // Who is connected is not a function of the log, so this is the second source of
            // nondeterminism in a Beck program — and the only one a *view* may read. The atom is a
            // capability rather than a new label: F16 asks for exactly that, and `cap.*` is what
            // keeps it off the tiers that would make it unreplayable (§3.3).
            Prim::Presence => vec![Effect::Cap(std::sync::Arc::from("presence"))],
            Prim::Durable => vec![Effect::Durable],
            Prim::NewUuid | Prim::Now => vec![Effect::Nondet],
            // The scope performs `spawn` itself; what its children perform is charged by the
            // checker, from each child's own row, because a thunk's effects belong to the thunk's
            // type and this is the form that calls them.
            Prim::Parallel => vec![Effect::Spawn],
            Prim::SecretEnv => vec![Effect::Env],
            // Wrapping is free; *reading* is the privileged half, and the capability is what stops
            // a view unwrapping one to render it.
            Prim::Reveal => vec![Effect::Cap(std::sync::Arc::from("internal"))],
            // The standard library's fallible pair. Unlike `Prim::Raise`, whose atom depends on
            // its argument's type, these two raise a type the prelude declares — so the row *is* a
            // constant and belongs here, where a test holds it against the scheme.
            Prim::JsonParse => vec![Effect::Raises(std::sync::Arc::from("JsonError"))],
            Prim::TimeParse => vec![Effect::Raises(std::sync::Arc::from("TimeError"))],
            Prim::HexDecode | Prim::Base64Decode => {
                vec![Effect::Raises(std::sync::Arc::from("EncodingError"))]
            }
            Prim::UuidParse | Prim::UuidVersion => {
                vec![Effect::Raises(std::sync::Arc::from("UuidError"))]
            }
            // The declassifier. `digest` and the encodings are pure; this one reads a
            // `secret[Str]`, so it is held where `reveal` is held — behind a capability no client
            // tier discharges, which is what stops a view minting a token (`docs/adr/0014`).
            Prim::DigestKeyed => vec![Effect::Cap(std::sync::Arc::from("sign"))],
            // Half of `http_fetch`'s row is a constant and half is its first argument. The
            // constant half is here; the `net.out(host)` half is added by [`Core::effects`],
            // which can see the argument, and by the checker, which is where it is charged.
            Prim::HttpFetch => vec![Effect::Raises(std::sync::Arc::from("HttpError"))],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Const {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<str>),
}

/// One arm of a `match`. Patterns are shallow — a constructor and its named field binders — which
/// is what §3.1's exhaustiveness check needs and no more.
#[derive(Clone, Debug)]
pub struct Arm {
    pub pattern: Pattern,
    /// `case Circle(r) if r > 0:` — a condition on the arm, in the scope of what the pattern bound.
    ///
    /// A guard that fails falls through to the next arm, which is what makes it a guard rather
    /// than an `if` in the body.
    pub guard: Option<Core>,
    pub body: Core,
    pub span: Span,
}

impl Arm {
    /// Every expression this arm holds, in the order they run.
    ///
    /// One method rather than `&a.body` at each of fourteen call sites, for
    /// [`Pattern::binders`]'s reason and with its history: a guard added as a field those sites
    /// did not know about would be a `Core` that liveness never marks, that `frames` never counts
    /// a slot for, and that the plan's free-variable analysis never sees — none of which is a
    /// compile error, and all of which are wrong on a program that uses one.
    pub fn exprs(&self) -> impl Iterator<Item = &Core> {
        self.guard.iter().chain(std::iter::once(&self.body))
    }

    pub fn exprs_mut(&mut self) -> impl Iterator<Item = &mut Core> {
        self.guard.iter_mut().chain(std::iter::once(&mut self.body))
    }
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Wildcard,
    Bind(VarId),
    Const(Const),
    /// `Added(id, text)` — `variant` names the constructor, and each field carries the pattern
    /// matched against it.
    ///
    /// A field's pattern is usually a [`Pattern::Bind`] or a [`Pattern::Wildcard`], which is what
    /// `Added(id, text)` and `Added(_)` mean. It may be any pattern: `Some(Added(id, text))` is a
    /// `Ctor` whose one field is a `Ctor`.
    Ctor {
        variant: Arc<str>,
        binds: Vec<(Arc<str>, Pattern)>,
    },
    /// `whole @ Circle(r)` — a name for the value, and a pattern that takes it apart.
    ///
    /// The binder is irrefutable, so whether this matches is entirely `inner`'s question.
    At {
        var: VarId,
        inner: Box<Pattern>,
    },
    /// `Circle(r) | Square(r)` — one of several, and every alternative binds the same names.
    ///
    /// The checker unifies the alternatives' binders onto one set of variables, so the body reads
    /// `r` without knowing which alternative matched. That is the rule that makes an or-pattern a
    /// pattern rather than two arms sharing a body.
    Or(Vec<Pattern>),
    /// `[]`, `[x]`, `[a, b]`, `[first, *rest]` — a list, taken apart.
    ///
    /// `items` is one pattern per fixed element and `rest` is the optional tail binder. A pattern
    /// with no `rest` matches a list of exactly `items.len()` elements; one with a `rest` matches
    /// any list at least that long.
    ///
    /// The tail is a binder rather than a pattern, and deliberately: `[a, *[b, c]]` is `[a, b, c]`
    /// written twice over, so what it would add is a second spelling rather than a shape.
    List {
        items: Vec<Pattern>,
        rest: Option<Option<VarId>>,
    },
}

impl Pattern {
    /// Every variable this pattern binds, at any depth.
    ///
    /// One method rather than a `match` at each of the three call sites, because those three were
    /// `Bind`/`Ctor`/`_ => {}` — and a new pattern kind falling into the `_` would have been a
    /// silent miscount in the splitter's variable supply and a false *free* variable in the plan's
    /// analysis. Neither would have failed a test until a program used one (`docs/27` §27.3).
    pub fn binders(&self) -> Vec<VarId> {
        let mut out = Vec::new();
        self.collect_binders(&mut out);
        out
    }

    fn collect_binders(&self, out: &mut Vec<VarId>) {
        match self {
            Pattern::Wildcard | Pattern::Const(_) => {}
            Pattern::Bind(v) => out.push(*v),
            Pattern::At { var, inner } => {
                out.push(*var);
                inner.collect_binders(out);
            }
            Pattern::Ctor { binds, .. } => {
                for (_, p) in binds {
                    p.collect_binders(out);
                }
            }
            Pattern::List { items, rest } => {
                for p in items {
                    p.collect_binders(out);
                }
                out.extend(rest.iter().filter_map(|b| *b));
            }
            // Every alternative binds the same variables, so one would do; all of them, deduped,
            // is what stays right if the checker's unification is ever wrong about that.
            Pattern::Or(alts) => {
                for p in alts {
                    p.collect_binders(out);
                }
                out.sort_unstable();
                out.dedup();
            }
        }
    }

    /// Whether this pattern matches every value of its type, so that no arm after it can run.
    ///
    /// Only a binder and a wildcard do — and a list pattern of nothing but a tail, `[*rest]`,
    /// which is the one refutable-looking shape that refuses nothing.
    pub fn irrefutable(&self) -> bool {
        match self {
            Pattern::Wildcard | Pattern::Bind(_) => true,
            Pattern::List { items, rest } => items.is_empty() && rest.is_some(),
            Pattern::Or(alts) => alts.iter().any(Pattern::irrefutable),
            Pattern::At { inner, .. } => inner.irrefutable(),
            Pattern::Const(_) | Pattern::Ctor { .. } => false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum CoreKind {
    Const(Const),
    Var(VarId),
    /// A reference to a top-level definition.
    Global(Arc<str>),
    Lam {
        /// Shared for the same reason `body` is: evaluating a `lam` hands the list to a closure,
        /// and a refcount bump is cheaper than copying it once per call.
        params: Arc<[VarId]>,
        /// Shared, not owned: a closure is built every time a `lam` node is *evaluated*, and a
        /// `Box` meant deep-copying the whole function body each time. `docs/70` §70.3 measured
        /// 20,000 calls to a function whose executed path never changed costing 42 ms, 227 ms and
        /// 606 ms as the *unexecuted* part of its body grew.
        body: Arc<Core>,
    },
    App {
        func: Box<Core>,
        args: Vec<Core>,
    },
    Prim {
        op: Prim,
        args: Vec<Core>,
    },
    Let {
        var: VarId,
        value: Box<Core>,
        body: Box<Core>,
    },
    If {
        cond: Box<Core>,
        then: Box<Core>,
        alt: Box<Core>,
    },
    Match {
        scrutinee: Box<Core>,
        arms: Vec<Arm>,
    },
    /// Construct a union variant or a model record.
    Make {
        ty: Arc<str>,
        variant: Option<Arc<str>>,
        fields: Vec<(Arc<str>, Core)>,
    },
    Field {
        base: Box<Core>,
        name: Arc<str>,
    },
    /// `t.with(done=not t.done)` — a functional record update. The sketch's
    /// `(set t :done (not t.done))`, and the reason `Todo` never needs a mutable binding.
    With {
        base: Box<Core>,
        fields: Vec<(Arc<str>, Core)>,
    },
    ListLit(Vec<Core>),
    MapLit(Vec<(Core, Core)>),
}

#[derive(Clone, Debug)]
pub struct Core {
    pub kind: CoreKind,
    pub ty: Ty,
    /// Which tier this node runs on. §4.2: "explicit tier annotation per node".
    pub tier: Tier,
    pub span: Span,
    /// Set on a [`CoreKind::Var`] whose value this expression is the **last** reader of, so a
    /// backend may move the binding rather than copy it. [`crate::liveness`] is what sets it and
    /// what the guarantee means; `false` is always safe.
    pub last_use: bool,
    /// Set on a [`CoreKind::Make`]: which written field belongs at each position of the record it
    /// builds, packed four bits per field. [`crate::fields`] is what sets it and what the packing
    /// means; [`crate::fields::UNORDERED`] is always safe and means "sort at run time".
    ///
    /// It costs nothing: a `u32` here fits in the padding `last_use` already leaves, so `Core` is
    /// 160 bytes either way.
    pub order: u32,
    /// Set on a [`CoreKind::Lam`]: how many bindings its body makes, so a call can reserve room
    /// for them in one frame instead of allocating a scope per `let`. [`crate::frames`] is what
    /// sets it and what the count means; `0` is always safe and means "chain a scope, as before".
    pub locals: u32,
}

/// The string this expression *is*, when it is written as one.
///
/// Deliberately not an evaluator and deliberately not a constant folder: the one caller is the
/// host of an outbound call, and "the host is written at the call site" is the rule that makes an
/// egress policy derivable. A `let` that happens to bind a literal is not written at the call
/// site, and reading through one would make the rule depend on how much folding the compiler
/// currently does.
pub fn literal_str(c: &Core) -> Option<Arc<str>> {
    match &c.kind {
        CoreKind::Const(Const::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

impl Core {
    pub fn new(kind: CoreKind, ty: Ty, span: Span) -> Core {
        Core {
            kind,
            ty,
            tier: Tier::Any,
            span,
            last_use: false,
            order: crate::fields::UNORDERED,
            locals: 0,
        }
    }

    /// Every effect this expression can perform, by walking what it calls.
    ///
    /// Phase 1 used this as *the* effect analysis. Phase 2 does not: rows are inferred during
    /// checking, where a call's latent row is known and a mere *reference* to a function performs
    /// nothing. What survives is this syntactic over-approximation, used in one place where that is
    /// the right answer — asking what a fold's function body could reach, including through a
    /// function value it was handed.
    pub fn effects(&self, globals: &dyn Fn(&str) -> Vec<Effect>, out: &mut Vec<Effect>) {
        match &self.kind {
            CoreKind::Prim { op, args } => {
                for e in op.effects() {
                    if !out.contains(&e) {
                        out.push(e);
                    }
                }
                // `http_fetch`'s host is its first argument and a literal, so this walk can read
                // it. It matters here rather than only in the checker because this is the oracle
                // `testing::performs_itself` asks — a definition that makes the call is the one a
                // `stub net.out(host)` replaces, and a definition that merely calls it is not.
                if let (Prim::HttpFetch, Some(host)) = (op, args.first().and_then(literal_str)) {
                    let atom = Effect::NetOut(host);
                    if !out.contains(&atom) {
                        out.push(atom);
                    }
                }
                for a in args {
                    a.effects(globals, out);
                }
            }
            CoreKind::Global(name) => {
                for e in globals(name) {
                    if !out.contains(&e) {
                        out.push(e);
                    }
                }
            }
            CoreKind::Const(_) | CoreKind::Var(_) => {}
            CoreKind::Lam { body, .. } => body.effects(globals, out),
            CoreKind::App { func, args } => {
                func.effects(globals, out);
                for a in args {
                    a.effects(globals, out);
                }
            }
            CoreKind::Let { value, body, .. } => {
                value.effects(globals, out);
                body.effects(globals, out);
            }
            CoreKind::If { cond, then, alt } => {
                cond.effects(globals, out);
                then.effects(globals, out);
                alt.effects(globals, out);
            }
            CoreKind::Match { scrutinee, arms } => {
                scrutinee.effects(globals, out);
                for e in arms.iter().flat_map(|a| a.exprs()) {
                    e.effects(globals, out);
                }
            }
            CoreKind::Make { fields, .. } => {
                for (_, f) in fields {
                    f.effects(globals, out);
                }
            }
            CoreKind::Field { base, .. } => base.effects(globals, out),
            CoreKind::With { base, fields } => {
                base.effects(globals, out);
                for (_, f) in fields {
                    f.effects(globals, out);
                }
            }
            CoreKind::ListLit(xs) => {
                for x in xs {
                    x.effects(globals, out);
                }
            }
            CoreKind::MapLit(kvs) => {
                for (k, v) in kvs {
                    k.effects(globals, out);
                    v.effects(globals, out);
                }
            }
        }
    }

    /// Set the tier on this node and everything under it.
    pub fn place(&mut self, tier: Tier) {
        self.tier = tier;
        match &mut self.kind {
            CoreKind::Lam { body, .. } => Arc::make_mut(body).place(tier),
            CoreKind::App { func, args } => {
                func.place(tier);
                for a in args {
                    a.place(tier);
                }
            }
            CoreKind::Prim { args, .. } => {
                for a in args {
                    a.place(tier);
                }
            }
            CoreKind::Let { value, body, .. } => {
                value.place(tier);
                body.place(tier);
            }
            CoreKind::If { cond, then, alt } => {
                cond.place(tier);
                then.place(tier);
                alt.place(tier);
            }
            CoreKind::Match { scrutinee, arms } => {
                scrutinee.place(tier);
                for e in arms.iter_mut().flat_map(|a| a.exprs_mut()) {
                    e.place(tier);
                }
            }
            CoreKind::Make { fields, .. } => {
                for (_, f) in fields {
                    f.place(tier);
                }
            }
            CoreKind::Field { base, .. } => base.place(tier),
            CoreKind::With { base, fields } => {
                base.place(tier);
                for (_, f) in fields {
                    f.place(tier);
                }
            }
            CoreKind::ListLit(xs) => {
                for x in xs {
                    x.place(tier);
                }
            }
            CoreKind::MapLit(kvs) => {
                for (k, v) in kvs {
                    k.place(tier);
                    v.place(tier);
                }
            }
            CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------------

/// A runtime value.
///
/// `Map` and a record's fields are both *ordered* on purpose and for the same reason Phase 0 chose
/// `BTreeMap`: iteration order is part of the rendered view, and replay must reproduce the *patch
/// stream* bit for bit, not merely the set of values.
///
/// `Map` is a [`PMap`], not an `Arc<BTreeMap>`, because it is the fold's accumulator: an update
/// must not copy it. See [`crate::pmap`] for why that structure and not another.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    /// A real, stored as an **order-preserving** key rather than as `f64::to_bits`, so that the
    /// derived `Ord` is the numeric one.
    ///
    /// A total order is not optional here — a map key and a component of the state digest need one
    /// — and `to_bits` supplies one that disagrees with arithmetic: `-1.0` has a larger bit pattern
    /// than `1.0`, so `<` answered backwards for every negative number and `sort_by` sorted the
    /// negatives in reverse. [`Value::float`] applies the standard monotone transform instead
    /// (flip the sign bit for a positive, invert every bit for a negative), which makes the two
    /// orders the same order. `docs/27` §27.8.
    Float(u64),
    /// Text, with the two facts a character-indexed language needs about it: how many characters
    /// there are, and whether a character index is a byte index. [`Text`] is why.
    Str(Arc<Text>),
    List(Arc<Vec<Value>>),
    Map(PMap<Value, Value>),
    /// A model instance or a union variant — see [`Record`].
    ///
    /// Behind one pointer rather than inline, and that is a size decision rather than a style one:
    /// the three fields inline made **every** `Value` 48 bytes, so a list of a million integers
    /// carried 32 bytes of nothing per element and a call frame paid for the widest variant it did
    /// not hold. One `Arc` makes a `Value` 16.
    Data(Arc<Record>),
    Html(Arc<Html>),
    /// An attribute waiting to be attached to an element.
    Attr(Arc<AttrValue>),
    Closure(Arc<Closure>),
}

/// A model instance or a union variant. `variant` is `None` for a plain record.
///
/// Split out of [`Value::Data`] so that a `Value` is a discriminant and a pointer. Records are the
/// widest thing the language has and the rarest thing in a hot loop, which is exactly the shape
/// that should be behind an indirection.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Record {
    pub ty: Arc<str>,
    pub variant: Option<Arc<str>>,
    pub fields: Fields,
}

/// A record's fields, sorted by name.
///
/// This was a `BTreeMap`, and a record is the wrong size for one: three to eight entries, built
/// once and read many times. A B-tree pays a node allocation and a pointer chase per level to buy
/// an asymptotic advantage that never arrives at that size, and profiling `awfy/havlak.beck` put
/// a fifth of the process inside its search, its insert and the `memcmp` underneath them.
///
/// Sorted by name and searched linearly: one allocation for the whole record, the names lie next
/// to each other in cache, and `get` compares lengths before bytes because it wants equality
/// rather than order. Iteration is in name order, so the value order, the state digest and the
/// wire format ([`crate::repr`]) are all bit-for-bit what the `BTreeMap` gave — which is what
/// makes this a representation change and not a semantic one.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fields(Vec<(Arc<str>, Value)>);

impl Fields {
    pub fn new() -> Fields {
        Fields(Vec::new())
    }

    pub fn with_capacity(n: usize) -> Fields {
        Fields(Vec::with_capacity(n))
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0
            .iter()
            .find(|(k, _)| same_name(k, name))
            .map(|(_, v)| v)
    }

    /// Set `name`, keeping the order by name. Answers the value that was there.
    ///
    /// The search is by **equality** and not by order, which is the whole difference: `==` on two
    /// `str`s compares their lengths first and reaches `memcmp` only for a pair that could match,
    /// where a binary search has to order every probe it makes. A record has three to eight
    /// fields, so a scan makes at most as many comparisons as a binary search and nearly all of
    /// them are an integer test. Only a field that is genuinely new pays for the ordered insert,
    /// and `with` — which is what calls this in a loop — never has one.
    pub fn insert(&mut self, name: Arc<str>, value: Value) -> Option<Value> {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| same_name(k, &name)) {
            return Some(std::mem::replace(&mut slot.1, value));
        }
        let at = self
            .0
            .partition_point(|(k, _)| cmp_name(k, &name) == std::cmp::Ordering::Less);
        self.0.insert(at, (name, value));
        None
    }

    /// Build from fields in **any** order, sorting once.
    ///
    /// This is how a record literal is built, and it is a separate entry point from `insert` in a
    /// loop because the two cost differently: `sort_unstable_by` on a handful of elements is an
    /// insertion sort, which makes `n - 1` comparisons and moves nothing when the fields already
    /// arrive in order — as a record literal's usually do.
    pub fn from_pairs(mut pairs: Vec<(Arc<str>, Value)>) -> Fields {
        pairs.sort_unstable_by(|(a, _), (b, _)| cmp_name(a, b));
        pairs.dedup_by(|(a, _), (b, _)| same_name(a, b));
        Fields(pairs)
    }

    /// Build from fields the caller has **already** put in order.
    ///
    /// The caller is [`crate::fields`], which decided the order once at compile time — a record
    /// literal's field names are written in the source, so sorting them once per record built is
    /// work with a known answer. Nothing else should use this: the order is the `Map` iteration,
    /// the state digest and the patch stream, so getting it wrong is a wire-format bug rather than
    /// a slow lookup.
    pub fn from_sorted(pairs: Vec<(Arc<str>, Value)>) -> Fields {
        debug_assert!(
            pairs
                .windows(2)
                .all(|w| cmp_name(&w[0].0, &w[1].0) == std::cmp::Ordering::Less),
            "from_sorted was given fields that are not sorted and distinct"
        );
        Fields(pairs)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, (Arc<str>, Value)> {
        self.0.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.0.iter().map(|(_, v)| v)
    }
}

impl FromIterator<(Arc<str>, Value)> for Fields {
    fn from_iter<I: IntoIterator<Item = (Arc<str>, Value)>>(it: I) -> Fields {
        Fields::from_pairs(it.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a Fields {
    type Item = &'a (Arc<str>, Value);
    type IntoIter = std::slice::Iter<'a, (Arc<str>, Value)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// A string that knows its own length in **characters**, and whether it is ASCII.
///
/// Beck indexes text by character everywhere or nowhere (`docs/50` §50.5), and a `String` counts
/// bytes — so `str_len` used to be `chars().count()` and `str_slice` used to `skip()` its way to
/// the start. Both are `O(n)` in the *string* rather than in the answer, which makes the ordinary
/// way to walk one — `while i < str_len(s)` reading `str_slice(s, i, 1)` — quadratic. Measured at
/// ×2.7 per doubling in [`70`](../../../../../docs/70-the-evaluator-gets-fast-report.md) §70.2.
///
/// Both facts are computed once, when the string is built, which is work the construction was
/// already doing: it had to copy the bytes, and `is_ascii` is a scan of the same bytes that answers
/// `chars` for free when it is true. Everything downstream is then `O(1)` or `O(answer)`.
///
/// The `String` rather than a `Box<str>` is the other half: it has spare capacity, so `a + b` can
/// push into `a` when the last-use analysis proves nobody else holds it ([`crate::liveness`]).
#[derive(Clone, Debug)]
pub struct Text {
    bytes: String,
    /// Characters, not bytes. `str_len`'s answer.
    chars: usize,
    /// Every character is one byte, so character index == byte index and a slice is a byte range.
    ascii: bool,
    /// For text that is *not* ASCII: the byte offset of every 32nd character.
    ///
    /// Chunked rather than one entry per character, because the point is to stop paying `O(n)` per
    /// slice and a jump to the nearest 32 does that for a thirty-second of the memory — `n / 8`
    /// bytes, and only for text that needs it, since an ASCII character index *is* a byte index.
    ///
    /// Built eagerly, in the pass that counts the characters, rather than cached on first use. A
    /// lazy one would be interior mutability inside a `Value`, and a `Value` is a `Map` key: the
    /// cache would be invisible to `Ord` and `Hash` and therefore harmless, but "harmless interior
    /// mutability in a key" is a sentence every reader and `clippy::mutable_key_type` would have to
    /// re-check. One pass and an eighth of the bytes is the cheaper answer.
    index: Box<[u32]>,
}

/// How many characters one entry of [`Text`]'s index skips.
const INDEX_STRIDE: usize = 32;

impl Text {
    pub fn new(bytes: String) -> Text {
        let ascii = bytes.is_ascii();
        // ASCII answers both questions from the scan `is_ascii` already did, and needs no index at
        // all. Anything else pays one more pass — once, here — rather than paying it on every
        // `str_len` and every `str_slice`.
        if ascii {
            let chars = bytes.len();
            return Text {
                bytes,
                chars,
                ascii,
                index: Box::new([]),
            };
        }
        let mut chars = 0usize;
        let mut index = Vec::with_capacity(bytes.len() / INDEX_STRIDE + 1);
        for (at, _) in bytes.char_indices() {
            if chars.is_multiple_of(INDEX_STRIDE) {
                index.push(at as u32);
            }
            chars += 1;
        }
        Text {
            bytes,
            chars,
            ascii,
            index: index.into_boxed_slice(),
        }
    }

    /// The byte offset of character `i`, in constant time for ASCII text and in at most
    /// one index stride for anything else. Past the end it answers the end, which is what a
    /// clamping slice wants.
    pub fn byte_offset(&self, i: usize) -> usize {
        if self.ascii {
            return i.min(self.bytes.len());
        }
        if i >= self.chars {
            return self.bytes.len();
        }
        let chunk = i / INDEX_STRIDE;
        let from = self.index.get(chunk).copied().unwrap_or(0) as usize;
        match self.bytes[from..]
            .char_indices()
            .nth(i - chunk * INDEX_STRIDE)
        {
            Some((at, _)) => from + at,
            None => self.bytes.len(),
        }
    }

    /// The length in characters, in constant time.
    pub fn chars_len(&self) -> usize {
        self.chars
    }

    pub fn is_ascii_text(&self) -> bool {
        self.ascii
    }

    pub fn as_str(&self) -> &str {
        &self.bytes
    }

    /// Append, consuming: the caller has established sole ownership, so this is a `push_str` and
    /// not a copy of both sides.
    pub fn appended(mut self, other: &str) -> Text {
        let before = self.chars;
        let start = self.bytes.len();
        let other_ascii = other.is_ascii();
        self.bytes.push_str(other);

        // Everything here is `O(other)`, never `O(self)`, which is the property the whole change
        // exists for: appending in a loop has to stay linear in the total.
        if self.ascii && other_ascii {
            self.chars = self.bytes.len();
            return self;
        }
        if self.ascii {
            // The left half was ASCII, so its character numbers *are* its byte offsets and its
            // share of the index can be written down rather than walked for.
            let mut index = Vec::with_capacity(self.bytes.len() / INDEX_STRIDE + 1);
            let mut at = 0;
            while at < before {
                index.push(at as u32);
                at += INDEX_STRIDE;
            }
            self.index = index.into_boxed_slice();
            self.ascii = false;
        }
        let mut index = std::mem::take(&mut self.index).into_vec();
        let mut chars = before;
        for (at, _) in self.bytes[start..].char_indices() {
            if chars.is_multiple_of(INDEX_STRIDE) {
                index.push((start + at) as u32);
            }
            chars += 1;
        }
        self.chars = chars;
        self.index = index.into_boxed_slice();
        self
    }
}

impl std::ops::Deref for Text {
    type Target = str;
    fn deref(&self) -> &str {
        &self.bytes
    }
}

impl From<&str> for Text {
    fn from(s: &str) -> Text {
        Text::new(s.to_string())
    }
}

/// Text compares, orders and hashes **as its characters**, so that adding the two cached facts
/// cannot change what a program means: a `Map` keyed by strings keeps its order, and so does the
/// state digest that a replay has to reproduce.
impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}
impl Eq for Text {}
impl PartialOrd for Text {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Text {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes.cmp(&other.bytes)
    }
}
impl std::hash::Hash for Text {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.bytes.hash(h)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttrValue {
    Plain(Arc<str>, Arc<str>),
    On(Arc<str>, Value),
    Key(Arc<str>),
}

#[derive(Debug)]
pub struct Closure {
    pub params: Arc<[VarId]>,
    /// The same `Arc` the [`CoreKind::Lam`] node holds, so building a closure is a refcount bump
    /// rather than a copy of the code.
    pub body: Arc<Core>,
    /// Behind an `Arc` so that *calling* the closure clones a pointer rather than the environment.
    pub env: Arc<Env>,
    /// How many bindings the body makes, copied off the [`CoreKind::Lam`] node so that a call can
    /// size one frame for the parameters and all of them. [`crate::frames`] is what counts it.
    pub locals: u32,
}

impl PartialEq for Closure {
    /// Closures compare by identity of their code position. Two closures are never equal unless
    /// they came from the same lambda with the same captured frame, which is all any program
    /// should rely on.
    fn eq(&self, other: &Self) -> bool {
        self.params == other.params
            && self.body.span == other.body.span
            && self.env.frame == other.env.frame
    }
}
impl Eq for Closure {}
impl PartialOrd for Closure {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Closure {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.params
            .cmp(&other.params)
            .then_with(|| self.body.span.start.cmp(&other.body.span.start))
    }
}

/// A lexical environment: a persistent chain of frames, so a closure can capture cheaply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env {
    /// `Arc<[T]>` rather than `Arc<Vec<T>>`: the second is two allocations and two hops to reach a
    /// binding.
    ///
    /// A call sizes this for the parameters **and** for every binding the body will make, so a
    /// `let` writes into a slot that is already there rather than allocating a scope of its own.
    /// The unwritten tail is filled with [`TOMBSTONE`], which no variable is named.
    frame: Arc<[(VarId, Value)]>,
    /// How much of `frame` holds a binding. Everything from here up is reserved and empty.
    used: u32,
    parent: Option<Arc<Env>>,
}

impl Env {
    pub fn new() -> Env {
        Env::default()
    }

    pub fn extend(&self, bindings: Vec<(VarId, Value)>) -> Env {
        Env {
            used: bindings.len() as u32,
            frame: bindings.into(),
            parent: Some(Arc::new(self.clone())),
        }
    }

    /// The frame a call runs in: the parameters, then `locals` reserved slots for the bindings the
    /// body is going to make.
    ///
    /// One allocation. `Map<Range, _>` has a length the compiler can trust, so collecting it into
    /// an `Arc<[_]>` sizes the allocation once — which is why the parameters and the reserved tail
    /// are produced by one iterator rather than a vector that is then converted.
    pub fn call_frame(
        parent: &Arc<Env>,
        params: &[VarId],
        args: impl Iterator<Item = Value>,
        locals: u32,
    ) -> Env {
        let n = params.len();
        let mut given = args;
        let frame = (0..n + locals as usize)
            .map(|i| match given.next() {
                Some(v) => (params[i], v),
                None => (TOMBSTONE, Value::Unit),
            })
            .collect();
        Env {
            frame,
            used: n as u32,
            parent: Some(Arc::clone(parent)),
        }
    }

    /// Bind `bindings` into this frame's reserved tail, if there is room for all of them and
    /// nobody else is holding the frame.
    ///
    /// Answers whether it did. `false` means the caller must fall back to [`Env::extend`] and
    /// chain a scope — which happens when a closure has captured this environment (its clone holds
    /// the frame, so `Arc::get_mut` refuses), when the reservation was too small, or when the
    /// program was built by something that never ran the reservation pass at all.
    ///
    /// The safety argument is the refusal: a closure that captured this environment can see the
    /// slots this would write, and `Arc::get_mut` is what proves that has not happened. Every
    /// binding gets a slot of its own, so nothing a closure captured is ever overwritten.
    pub fn put(&mut self, bindings: &mut Vec<(VarId, Value)>) -> bool {
        let at = self.used as usize;
        let n = bindings.len();
        if at + n > self.frame.len() {
            return false;
        }
        let Some(frame) = Arc::get_mut(&mut self.frame) else {
            return false;
        };
        for (i, b) in bindings.drain(..).enumerate() {
            frame[at + i] = b;
        }
        self.used = (at + n) as u32;
        true
    }

    /// [`Env::put`] for a single binding, which is what a `let` is — and a `let` is much the most
    /// common of the two, so it does not build a vector to hand over. Answers the value back when
    /// there is no room for it.
    pub fn put_one(&mut self, var: VarId, value: Value) -> Result<(), Value> {
        let at = self.used as usize;
        if at >= self.frame.len() {
            return Err(value);
        }
        match Arc::get_mut(&mut self.frame) {
            Some(frame) => {
                frame[at] = (var, value);
                self.used += 1;
                Ok(())
            }
            None => Err(value),
        }
    }

    /// The part of the frame that holds bindings, without the reserved tail.
    #[inline]
    fn bound(&self) -> &[(VarId, Value)] {
        &self.frame[..self.used as usize]
    }

    /// Extend a parent that is **already** behind an `Arc` — which a closure's captured environment
    /// is, so that a call clones a pointer instead of boxing a copy of the environment.
    ///
    /// This is the per-call path. `extend` above is the per-`let` path, where the parent is owned by
    /// the evaluator's loop and has to be boxed; there are far fewer of those.
    pub fn extend_shared(parent: &Arc<Env>, frame: Arc<[(VarId, Value)]>) -> Env {
        Env {
            used: frame.len() as u32,
            frame,
            parent: Some(Arc::clone(parent)),
        }
    }

    /// An environment with nothing in it, behind an `Arc`, shared by every top-level definition.
    pub fn empty_shared() -> Arc<Env> {
        Arc::new(Env::new())
    }

    pub fn get(&self, v: VarId) -> Option<&Value> {
        let mut env = self;
        loop {
            if let Some((_, value)) = env.bound().iter().rev().find(|(id, _)| *id == v) {
                return Some(value);
            }
            match &env.parent {
                Some(p) => env = p,
                None => return None,
            }
        }
    }

    /// Read `v`, and **move** it out of the frame when three things hold: the caller says no later
    /// evaluation reads it, this environment is the only holder of the frame it lives in, and the
    /// value is one whose copy costs something.
    ///
    /// The third condition is not an optimisation of an optimisation — it is what keeps the other
    /// two from costing more than they save. Moving is strictly more work than cloning at the point
    /// of the read: a clone of an `Int` is a copy of eight bytes and a clone of a container is one
    /// atomic increment, where a move has to find the slot, prove the frame is unshared and empty
    /// it. It pays only when somebody downstream can then *use* the sole ownership — which today is
    /// `list_append` pushing in place and `with` rebuilding a record's fields — and measuring it
    /// without this condition showed every benchmark in the tree 6–13% slower, because the reads
    /// that dominate a real program are of `Int`s and nothing was gained by moving one
    /// ([`69`](../../../../../docs/69-standard-library-imports-report.md) §69.7).
    ///
    /// The caller must have established that no later evaluation reads `v` — [`crate::liveness`]
    /// is what establishes it, and `last_use` is the flag. What this adds is the second half of the
    /// safety argument: a frame is emptied only when nothing else holds it, so an environment
    /// captured by a closure or shared with an inner scope is read from rather than emptied, and a
    /// caller that is wrong about liveness gets an unbound-variable error rather than somebody
    /// else's missing binding.
    pub fn read(&mut self, v: VarId, may_move: bool) -> Option<Value> {
        // The overwhelmingly common read is not a last use, and it needs none of the machinery
        // below: no scope on the way to the binding has to be proved unshared, because nothing is
        // going to be taken out of one. That proof costs **two atomic loads per scope level**
        // (`strong_count` and `weak_count`) plus an `Arc::get_mut`, and it was being paid on every
        // variable a program reads. `get` is a plain walk.
        if !may_move {
            return self.get(v).cloned();
        }
        let mut env = self;
        loop {
            if let Some(i) = env.bound().iter().rposition(|(id, _)| *id == v) {
                if may_move && worth_moving(&env.frame[i].1) {
                    if let Some(frame) = Arc::get_mut(&mut env.frame) {
                        // Tombstoned rather than removed: `Vec::remove` shifts every binding above
                        // it, and this runs on the hottest path there is. The slot keeps its place
                        // under a name no variable has, so a later read of `v` misses it and says
                        // so instead of finding a neighbour.
                        frame[i].0 = TOMBSTONE;
                        return Some(std::mem::replace(&mut frame[i].1, Value::Unit));
                    }
                }
                return Some(env.frame[i].1.clone());
            }
            let shared_parent = env
                .parent
                .as_ref()
                .is_some_and(|p| Arc::strong_count(p) > 1 || Arc::weak_count(p) > 0);
            if shared_parent {
                return env.parent.as_ref().and_then(|p| p.get(v)).cloned();
            }
            match env.parent.as_mut() {
                Some(p) => match Arc::get_mut(p) {
                    Some(p) => env = p,
                    None => return None,
                },
                None => return None,
            }
        }
    }
}

/// Every subexpression of this one.
///
/// The read-only twin of `children_mut`, and it exists for the same reason: a pass that walks
/// the whole tree should not restate the shape of `CoreKind`, because the day a variant gains a
/// child every hand-written walk is silently incomplete ([`docs/91`](../../../../../docs/91-guards-and-alternatives-report.md)
/// §91.3 is that failure, at fourteen sites).
pub fn children(c: &Core) -> Vec<&Core> {
    match &c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => Vec::new(),
        CoreKind::Lam { body, .. } => vec![body],
        CoreKind::App { func, args } => std::iter::once(&**func).chain(args).collect(),
        CoreKind::Let { value, body, .. } => vec![value, body],
        CoreKind::If { cond, then, alt } => vec![cond, then, alt],
        CoreKind::Match { scrutinee, arms } => std::iter::once(&**scrutinee)
            .chain(arms.iter().flat_map(|a| a.exprs()))
            .collect(),
        CoreKind::Prim { args, .. } => args.iter().collect(),
        CoreKind::Make { fields, .. } => fields.iter().map(|(_, f)| f).collect(),
        CoreKind::Field { base, .. } => vec![base],
        CoreKind::With { base, fields } => std::iter::once(&**base)
            .chain(fields.iter().map(|(_, f)| f))
            .collect(),
        CoreKind::ListLit(items) => items.iter().collect(),
        CoreKind::MapLit(kvs) => kvs.iter().flat_map(|(k, v)| [k, v]).collect(),
    }
}

/// Every subexpression of this one, to be rewritten in place.
///
/// The walk two passes over the finished program share — [`crate::frames`] and [`crate::fields`].
/// A lambda's body is behind an `Arc` because a closure shares it rather than copying it
/// (`docs/70`), so reaching into one is a `make_mut`: it runs once, on a program nothing else
/// holds yet, so nothing is actually cloned.
pub(crate) fn children_mut(c: &mut Core) -> Vec<&mut Core> {
    match &mut c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => Vec::new(),
        CoreKind::Lam { body, .. } => vec![Arc::make_mut(body)],
        CoreKind::App { func, args } => std::iter::once(&mut **func).chain(args).collect(),
        CoreKind::Let { value, body, .. } => vec![&mut **value, &mut **body],
        CoreKind::If { cond, then, alt } => vec![&mut **cond, &mut **then, &mut **alt],
        CoreKind::Match { scrutinee, arms } => std::iter::once(&mut **scrutinee)
            .chain(arms.iter_mut().flat_map(|a| a.exprs_mut()))
            .collect(),
        CoreKind::Prim { args, .. } => args.iter_mut().collect(),
        CoreKind::Make { fields, .. } => fields.iter_mut().map(|(_, f)| f).collect(),
        CoreKind::Field { base, .. } => vec![&mut **base],
        CoreKind::With { base, fields } => std::iter::once(&mut **base)
            .chain(fields.iter_mut().map(|(_, f)| f))
            .collect(),
        CoreKind::ListLit(items) => items.iter_mut().collect(),
        CoreKind::MapLit(kvs) => kvs.iter_mut().flat_map(|(k, v)| [k, v]).collect(),
    }
}

/// Are these the same field name?
///
/// Length, then first byte, then the rest. `str`'s own `==` checks the length and hands the bytes
/// to `memcmp`, and a `memcmp` call is dear next to what it decides here: field names are short,
/// there are three to eight of them in a record, and two that share a length almost never share a
/// first letter. Profiling `awfy/richards.beck` put 6% of the process inside `memcmp`, nearly all
/// of it deciding between `kind` and `link`.
#[inline]
fn same_name(a: &str, b: &str) -> bool {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    x.len() == y.len() && (x.is_empty() || x[0] == y[0]) && x == y
}

/// The order a record keeps its fields in.
///
/// [`Fields`] is sorted by name and that order is load-bearing far outside this module — it is a
/// `Map`'s iteration, the state digest, the patch stream and the order `Ord` compares two records
/// in. A backend that lays a record out in memory has to lay it out in *this* order or its `<`
/// disagrees with the evaluator's, so the order is published rather than reimplemented.
pub fn field_order(a: &str, b: &str) -> std::cmp::Ordering {
    cmp_name(a, b)
}

/// Order two field names, deciding on the first byte where it can.
///
/// `<[u8]>::cmp` calls `memcmp` over the common prefix before it looks at the lengths, so it pays
/// a call to distinguish `id` from `kind`. Sorting a record's fields is the other half of what
/// `same_name` documents.
#[inline]
pub(crate) fn cmp_name(a: &str, b: &str) -> std::cmp::Ordering {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    match (x.first(), y.first()) {
        (Some(p), Some(q)) if p != q => p.cmp(q),
        _ => x.cmp(y),
    }
}

/// The name a moved-out binding takes, which no variable has: `VarId`s are handed out from zero by
/// the checker, and a program with four billion of them has lost to `MAX_NESTING` long before.
const TOMBSTONE: VarId = VarId::MAX;

/// Whether moving this value out of a frame can save anything downstream.
///
/// A `List` can be pushed into by `list_append`, a `Str` by `+`, and a record's fields can be
/// rebuilt in place by `with` — each only when nobody else holds them. Everything else is either a copy of a few bytes
/// or an atomic increment, and moving it costs more than it saves — `Env::read` is the measurement.
fn worth_moving(v: &Value) -> bool {
    matches!(v, Value::List(_) | Value::Str(_) | Value::Data(_))
}

/// The monotone `f64` → `u64` transform: for a non-negative float flip the sign bit, for a
/// negative one invert every bit. `a < b` as reals iff `order_key(a) < order_key(b)` as integers,
/// with `-inf` at the bottom and NaN above `+inf`.
const SIGN: u64 = 1 << 63;

fn order_key(f: f64) -> u64 {
    let bits = f.to_bits();
    if bits & SIGN != 0 {
        !bits
    } else {
        bits ^ SIGN
    }
}

fn from_order_key(key: u64) -> f64 {
    f64::from_bits(if key & SIGN != 0 { key ^ SIGN } else { !key })
}

impl Value {
    /// Make a real, canonicalising the two IEEE values that would otherwise break `Eq`.
    ///
    /// `-0.0` becomes `0.0` and every NaN becomes one NaN, because [`Value`] is `Eq` and `Ord` and
    /// a fold's accumulator is compared, hashed and used as a map key. IEEE 754 says `NaN != NaN`
    /// and `-0.0 == 0.0`; both are irreconcilable with a total order, and the total order is the
    /// one §3.7 needs. So Beck's `==` on reals is *structural*, and `docs/27` §27.8 says so where
    /// somebody porting numeric code will read it.
    pub fn float(f: f64) -> Value {
        let f = if f.is_nan() {
            f64::NAN
        } else if f == 0.0 {
            0.0
        } else {
            f
        };
        Value::Float(order_key(f))
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(key) => Some(from_order_key(*key)),
            _ => None,
        }
    }

    pub fn str_(s: impl AsRef<str>) -> Value {
        Value::Str(Arc::new(Text::from(s.as_ref())))
    }

    /// The same from a `String` that is already owned, which is most of the string primitives:
    /// they build one and would otherwise copy it again on the way in.
    pub fn text(s: String) -> Value {
        Value::Str(Arc::new(Text::new(s)))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(xs) => Some(xs),
            _ => None,
        }
    }

    pub fn as_map(&self) -> Option<&PMap<Value, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_html(&self) -> Option<&Html> {
        match self {
            Value::Html(h) => Some(h),
            _ => None,
        }
    }

    /// Build a record or a union variant. The one constructor, so that the `Arc` and the map are
    /// allocated in one place rather than at every call site.
    pub fn data(ty: impl Into<Arc<str>>, variant: Option<Arc<str>>, fields: Fields) -> Value {
        Value::Data(Arc::new(Record {
            ty: ty.into(),
            variant,
            fields,
        }))
    }

    /// The same from a list of pairs, which is how most call sites have them.
    pub fn record<const N: usize>(
        ty: impl Into<Arc<str>>,
        variant: Option<&str>,
        fields: [(&str, Value); N],
    ) -> Value {
        Value::data(
            ty,
            variant.map(Arc::from),
            fields.into_iter().map(|(k, v)| (Arc::from(k), v)).collect(),
        )
    }

    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Data(d) => d.fields.get(name),
            _ => None,
        }
    }

    pub fn variant(&self) -> Option<&str> {
        match self {
            Value::Data(d) => d.variant.as_deref(),
            _ => None,
        }
    }

    pub fn some(v: Value) -> Value {
        Value::record(Ty::OPTION, Some("Some"), [("value", v)])
    }

    pub fn none() -> Value {
        Value::record(Ty::OPTION, Some("None"), [])
    }

    pub fn ok(v: Value) -> Value {
        Value::record(Ty::RESULT, Some("Ok"), [("value", v)])
    }

    pub fn err(v: Value) -> Value {
        Value::record(Ty::RESULT, Some("Err"), [("error", v)])
    }

    /// How `str(x)` renders a value, and how a value becomes a `Map` key's printed form.
    pub fn display(&self) -> String {
        match self {
            Value::Unit => "unit".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(_) => format!("{}", self.as_f64().unwrap_or(0.0)),
            Value::Str(s) => s.to_string(),
            Value::List(xs) => {
                let parts: Vec<String> = xs.iter().map(Value::display).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Map(m) => {
                let parts: Vec<String> = m
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.display(), v.display()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
            Value::Data(d) => {
                // A newtype wrapping one field prints as that field — `Id(uuid)` reads as the uuid,
                // which is what a key attribute and a rendered list want.
                if d.variant.is_none() && d.fields.len() == 1 {
                    if let Some(v) = d.fields.values().next() {
                        return v.display();
                    }
                }
                let name = d.variant.as_deref().unwrap_or(&d.ty);
                if d.fields.is_empty() {
                    return name.to_string();
                }
                let parts: Vec<String> = d
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{k}: {}", v.display()))
                    .collect();
                format!("{name}{{{}}}", parts.join(", "))
            }
            Value::Html(h) => h.render(),
            Value::Attr(_) => "<attr>".into(),
            Value::Closure(_) => "<fn>".into(),
        }
    }

    /// The wire form, used for command payloads carried by handlers and for the log.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map as JMap, Value as J};
        match self {
            Value::Unit => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::Number((*i).into()),
            Value::Float(_) => serde_json::Number::from_f64(self.as_f64().unwrap_or(0.0))
                .map(J::Number)
                .unwrap_or(J::Null),
            Value::Str(s) => J::String(s.to_string()),
            Value::List(xs) => J::Array(xs.iter().map(Value::to_json).collect()),
            Value::Map(m) => {
                let mut obj = JMap::new();
                for (k, v) in m.iter() {
                    obj.insert(k.display(), v.to_json());
                }
                J::Object(obj)
            }
            Value::Data(d) => {
                if d.variant.is_none() && d.fields.len() == 1 {
                    if let Some(v) = d.fields.values().next() {
                        return v.to_json();
                    }
                }
                let mut obj = JMap::new();
                if let Some(v) = &d.variant {
                    obj.insert("c".into(), J::String(v.to_string()));
                }
                for (k, v) in d.fields.iter() {
                    obj.insert(k.to_string(), v.to_json());
                }
                J::Object(obj)
            }
            Value::Html(h) => h.to_wire(),
            Value::Attr(_) | Value::Closure(_) => J::Null,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

/// A value that cannot be written to a log.
///
/// [`Value`] has four consumers with different requirements — the evaluator, the log, the wire, and
/// the digest — and three of its variants exist only for the first. Encoding one of those is not a
/// value to be lowered; it is a program that should not have compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotStorable {
    /// Which variant, by name.
    pub kind: &'static str,
}

impl fmt::Display for NotStorable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a {} cannot be written to the log: it is {}, not data",
            self.kind,
            match self.kind {
                "closure" => "code",
                "attribute" => "part of a view",
                _ => "a view",
            }
        )
    }
}

impl std::error::Error for NotStorable {}

/// A lossless encoding of a [`Value`], for the log and for snapshots.
///
/// [`Value::to_json`] is the *wire* form: it drops the type name of a record and unwraps a
/// newtype, because that is what a browser wants. The log needs the opposite — a record that can
/// be read back as exactly the value that was written, because replay compares digests. Hence two
/// encodings, and a test that says why.
///
/// # Why this returns a `Result`
///
/// It used to encode `Html`, `Attr` and `Closure` as `unit` on the grounds that "neither can appear
/// in a log". That grounds was an assumption, not a check: nothing stops a program declaring
/// `model State: cached: Html`, and the encoding would then write `unit` into the *durable* path,
/// silently, and replay would rebuild a different state. A system whose correctness argument is
/// "replay is exact" cannot have a lossy branch in the function that makes the log.
///
/// Until placement can prove such a type never reaches `durable` (Phase 2's effect rows), refusing
/// at the boundary is the honest position: the append fails, the process aborts by the same rule as
/// any other failed append (§18.5 item 6), and nothing unreadable is ever committed.
pub fn value_to_repr(v: &Value) -> Result<serde_json::Value, NotStorable> {
    use serde_json::{json, Map as JMap, Value as J};
    Ok(match v {
        Value::Unit => json!({"$": "unit"}),
        Value::Bool(b) => json!({"$": "bool", "v": b}),
        Value::Int(i) => json!({"$": "int", "v": i}),
        Value::Float(bits) => json!({"$": "float", "v": bits.to_string()}),
        Value::Str(s) => json!({"$": "str", "v": s.as_str()}),
        Value::List(xs) => {
            let items: Result<Vec<_>, _> = xs.iter().map(value_to_repr).collect();
            json!({"$": "list", "v": items?})
        }
        Value::Map(m) => {
            let mut pairs = Vec::with_capacity(m.len());
            for (k, val) in m.iter() {
                pairs.push(json!([value_to_repr(k)?, value_to_repr(val)?]));
            }
            json!({"$": "map", "v": pairs})
        }
        Value::Data(d) => {
            let mut f = JMap::new();
            for (k, val) in d.fields.iter() {
                f.insert(k.to_string(), value_to_repr(val)?);
            }
            json!({
                "$": "data",
                "t": d.ty.as_ref(),
                "c": d.variant.as_deref(),
                "f": J::Object(f)
            })
        }
        Value::Html(_) => return Err(NotStorable { kind: "view" }),
        Value::Attr(_) => return Err(NotStorable { kind: "attribute" }),
        Value::Closure(_) => return Err(NotStorable { kind: "closure" }),
    })
}

pub fn value_from_repr(j: &serde_json::Value) -> Option<Value> {
    let tag = j.get("$")?.as_str()?;
    Some(match tag {
        "unit" => Value::Unit,
        "bool" => Value::Bool(j.get("v")?.as_bool()?),
        "int" => Value::Int(j.get("v")?.as_i64()?),
        "float" => Value::Float(j.get("v")?.as_str()?.parse().ok()?),
        "str" => Value::str_(j.get("v")?.as_str()?),
        "list" => Value::List(Arc::new(
            j.get("v")?
                .as_array()?
                .iter()
                .map(value_from_repr)
                .collect::<Option<Vec<_>>>()?,
        )),
        "map" => {
            let mut m = PMap::new();
            for pair in j.get("v")?.as_array()? {
                let pair = pair.as_array()?;
                m = m.insert(
                    value_from_repr(pair.first()?)?,
                    value_from_repr(pair.get(1)?)?,
                );
            }
            Value::Map(m)
        }
        "data" => {
            let mut fields = Fields::new();
            for (k, val) in j.get("f")?.as_object()? {
                fields.insert(Arc::from(k.as_str()), value_from_repr(val)?);
            }
            Value::data(
                Arc::from(j.get("t")?.as_str()?),
                j.get("c").and_then(|c| c.as_str()).map(Arc::from),
                fields,
            )
        }
        _ => return None,
    })
}

/// Structural digest of a value — the replay-determinism oracle (§4.8).
///
/// A property of the *value*, not of whoever produced it: two backends that disagree are
/// detected by comparing digests, so the digest cannot live in either of them.
pub fn digest(v: &Value) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hash_into(v, &mut hasher);
    *hasher.finalize().as_bytes()
}

fn hash_into(v: &Value, h: &mut blake3::Hasher) {
    match v {
        Value::Unit => h.update(&[0]),
        Value::Bool(b) => h.update(&[1, *b as u8]),
        Value::Int(i) => {
            h.update(&[2]);
            h.update(&i.to_le_bytes())
        }
        Value::Float(bits) => {
            h.update(&[3]);
            h.update(&bits.to_le_bytes())
        }
        Value::Str(s) => {
            h.update(&[4]);
            h.update(&(s.len() as u64).to_le_bytes());
            h.update(s.as_bytes())
        }
        Value::List(xs) => {
            h.update(&[5]);
            h.update(&(xs.len() as u64).to_le_bytes());
            for x in xs.iter() {
                hash_into(x, h);
            }
            h
        }
        Value::Map(m) => {
            h.update(&[6]);
            h.update(&(m.len() as u64).to_le_bytes());
            for (k, val) in m.iter() {
                hash_into(k, h);
                hash_into(val, h);
            }
            h
        }
        Value::Data(d) => {
            h.update(&[7]);
            h.update(d.ty.as_bytes());
            h.update(d.variant.as_deref().unwrap_or("").as_bytes());
            h.update(&(d.fields.len() as u64).to_le_bytes());
            for (k, val) in d.fields.iter() {
                h.update(k.as_bytes());
                hash_into(val, h);
            }
            h
        }
        Value::Html(html) => {
            h.update(&[8]);
            h.update(html.render().as_bytes())
        }
        Value::Attr(_) => h.update(&[9]),
        Value::Closure(_) => h.update(&[10]),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property `Value::Float`'s representation exists for: the order the fold uses and the
    /// order arithmetic uses are one order.
    ///
    /// `f64::to_bits` does not have it — `(-1.0).to_bits()` is larger than `(1.0).to_bits()`
    /// because the sign bit is the top one — and that is the defect docs/27 §27.8 records. This is
    /// the test that would have caught it, checked across the sign, the zeroes and the infinities
    /// rather than on one example.
    #[test]
    fn reals_compare_as_reals_and_round_trip_through_their_key() {
        let ladder = [
            f64::NEG_INFINITY,
            -1e308,
            -1.5,
            -1.0,
            -f64::MIN_POSITIVE,
            0.0,
            f64::MIN_POSITIVE,
            1.0,
            1.5,
            1e308,
            f64::INFINITY,
        ];
        for w in ladder.windows(2) {
            let (a, b) = (Value::float(w[0]), Value::float(w[1]));
            assert!(a < b, "{} should order below {}", w[0], w[1]);
            assert_eq!(a.as_f64(), Some(w[0]), "and survive the round trip");
        }

        // The two IEEE values that would otherwise break `Eq`, canonicalised.
        assert_eq!(
            Value::float(-0.0),
            Value::float(0.0),
            "`-0.0` and `0.0` are one value, because `Ord` cannot have two of them"
        );
        assert_eq!(
            Value::float(f64::NAN),
            Value::float(-f64::NAN),
            "and every NaN is one NaN — including a negative one — for the same reason"
        );
        assert!(
            Value::float(f64::NAN) > Value::float(f64::INFINITY),
            "NaN has to go somewhere, and above every number is somewhere"
        );

        // The digest is a function of the value, and a different real is a different digest.
        assert_eq!(digest(&Value::float(1.5)), digest(&Value::float(1.5)));
        assert_ne!(digest(&Value::float(1.5)), digest(&Value::float(-1.5)));
    }

    #[test]
    fn the_log_encoding_round_trips_exactly() {
        // The wire encoding deliberately loses information the browser does not need; the log
        // encoding cannot, because replay compares digests of what it reads back.
        let v = Value::data(
            Arc::from("State"),
            None,
            Fields::from_iter([(
                Arc::from("todos"),
                Value::Map(PMap::from_iter([(
                    Value::str_("k"),
                    Value::data(
                        Arc::from("Todo"),
                        None,
                        Fields::from_iter([
                            (Arc::from("done"), Value::Bool(true)),
                            (Arc::from("n"), Value::Int(-3)),
                        ]),
                    ),
                )])),
            )]),
        );
        assert_eq!(
            value_from_repr(&value_to_repr(&v).unwrap()),
            Some(v.clone())
        );
        let evt = Value::data(
            Arc::from("Event"),
            Some(Arc::from("Toggled")),
            Fields::from_iter([(Arc::from("id"), Value::str_("x"))]),
        );
        assert_eq!(value_from_repr(&value_to_repr(&evt).unwrap()), Some(evt));
    }

    #[test]
    fn a_value_the_log_cannot_hold_is_refused_rather_than_flattened() {
        // This used to encode as `unit`, on the assumption that a view never reaches the log.
        // Nothing checked the assumption: `model State: cached: Html` compiles today, and the
        // durable path would have written `unit` and replayed a different state — silently, in the
        // one place this system's correctness argument does not permit silence.
        let view = Value::Html(Arc::new(crate::html::Html::text("hello")));
        let err = value_to_repr(&view).expect_err("a view is not data");
        assert!(
            err.to_string().contains("cannot be written to the log"),
            "{err}"
        );

        // …and nesting does not launder it: a record holding one is refused too.
        let state = Value::data(
            Arc::from("State"),
            None,
            Fields::from_iter([(Arc::from("cached"), view.clone())]),
        );
        assert!(
            value_to_repr(&state).is_err(),
            "a record holding a view is not data"
        );
        assert!(
            value_to_repr(&Value::List(Arc::new(vec![view.clone()]))).is_err(),
            "a list holding a view is not data"
        );
        assert!(
            value_to_repr(&Value::Map(PMap::new().insert(Value::str_("k"), view))).is_err(),
            "a map holding a view is not data"
        );
    }

    #[test]
    fn maps_order_by_key_so_rendering_is_deterministic() {
        let m = PMap::new()
            .insert(Value::str_("b"), Value::Int(2))
            .insert(Value::str_("a"), Value::Int(1));
        let v = Value::Map(m);
        assert_eq!(v.display(), "{a: 1, b: 2}");
    }

    #[test]
    fn a_newtype_renders_as_its_payload() {
        let id = Value::data(
            Arc::from("Id"),
            None,
            Fields::from_iter([(Arc::from("value"), Value::str_("u-1"))]),
        );
        assert_eq!(id.display(), "u-1");
        assert_eq!(id.to_json(), serde_json::json!("u-1"));
    }

    #[test]
    fn a_command_serialises_with_its_variant_tag() {
        let cmd = Value::data(
            Arc::from("Command"),
            Some(Arc::from("Toggle")),
            Fields::from_iter([(Arc::from("id"), Value::str_("x"))]),
        );
        assert_eq!(cmd.to_json(), serde_json::json!({"c": "Toggle", "id": "x"}));
    }

    #[test]
    fn environments_shadow_innermost_first() {
        let base = Env::new().extend(vec![(1, Value::Int(1))]);
        let inner = base.extend(vec![(1, Value::Int(2))]);
        assert_eq!(inner.get(1), Some(&Value::Int(2)));
        assert_eq!(base.get(1), Some(&Value::Int(1)));
        assert_eq!(inner.get(9), None);
    }

    #[test]
    fn only_the_impure_primitives_carry_atoms_of_their_own() {
        assert_eq!(Prim::MergeClients.effects(), vec![Effect::Ingress]);
        assert_eq!(Prim::Durable.effects(), vec![Effect::Durable]);
        assert_eq!(Prim::NewUuid.effects(), vec![Effect::Nondet]);
        assert_eq!(Prim::Now.effects(), vec![Effect::Nondet]);
        assert_eq!(Prim::SecretEnv.effects(), vec![Effect::Env]);
        assert!(Prim::Add.effects().is_empty());
        assert!(
            Prim::Fold.effects().is_empty(),
            "a fold is pure; `durable` is the effect"
        );
        assert!(
            Prim::MapList.effects().is_empty(),
            "`map_list` performs nothing of its own — it performs its argument's row, which is a \
             variable and lives in the scheme"
        );
    }
}
