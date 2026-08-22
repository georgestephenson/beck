//! The compiled slice of one component, in the form a client can execute — Mode B's payload.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.1: a Mode-B component
//! is "the component's pure code compiled to WASM … fine-grained signal graph, local speculative
//! fold + `seq`-based reconciliation". This module is the *payload* half of that: what a browser
//! has to be given before it can render the page and guess the next state for itself.
//!
//! Four roles cross, and each is here for a reason a Mode-A client does not have:
//!
//! * **`view`** — because in Mode B the browser renders. This is the only per-component role; the
//!   other three belong to the application, which is why two components of one program share
//!   everything but this.
//! * **`validate`** and **`fold`** — because optimism is not a trick. "The browser applies the
//!   expected event to its local copy *speculatively* — legitimate because it runs the *same pure
//!   fold* the server runs" ([`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D5).
//!   The same `Core`, not a second implementation of it.
//! * **`init`** — so a client that has never spoken to the server has a state to render, which is
//!   what an offline cold start is (D7).
//!
//! Plus the **definitions those four reach**, transitively: `Core` refers to top-level definitions
//! by name ([`CoreKind::Global`]), so a role without them is not executable. That closure *is* the
//! component's slice, and it is why a bundle is smaller than a program.
//!
//! # What a bundle is not
//!
//! It is not a program. There is no `Placed`, no signal graph, no type table, no test — nothing
//! the client would need only in order to *check* something, because the client checks nothing.
//! The program was checked on the way in; the bundle is what is left when the only remaining
//! question is how to run it.
//!
//! # Why a mirror type rather than `#[derive(Serialize)]` on `Core`
//!
//! [`crate::repr`]'s argument, one layer up: a concrete type is where the decisions about what
//! crosses become *visible and reviewable* rather than implied by whatever fields a struct happens
//! to have. Two decisions are taken here, and neither should be able to change by accident:
//!
//! * **Types are dropped.** A `Core` node carries the [`Ty`] the checker inferred, and the
//!   evaluator never reads it — it dispatches on values. Carrying resolved types would roughly
//!   double the payload to say something the only consumer cannot use. A *compiling* client
//!   backend would need them, and that is a format version rather than a field somebody adds
//!   quietly: [`FORMAT`] is checked on load.
//! * **Spans are kept.** They are three integers and they are the difference between "the fold
//!   failed" and "the fold failed at `todo.beck:47`" in a browser console.
//!
//! # Why the bundle names the compiler that made it
//!
//! [`Prim`] is encoded as its *position* in the primitive table, which is the compact encoding —
//! and which silently means something different if the table changes. So a bundle carries
//! [`shape_id`]: a digest of every primitive's name and number. A kernel built from a different
//! compiler refuses the bundle instead of executing a `str_len` that used to be a `list_len`. It
//! is the same rule [`crate::repr`]'s `FORMAT` states for the log — "a misread log is worse than
//! an unreadable one" — applied to code rather than to data.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::{FileId, Span};
use serde::{Deserialize, Serialize};

use crate::command;
use crate::core::{Arm, Const, Core, CoreKind, Pattern, Prim, VarId};
use crate::split::Placed;
use crate::ty::{Tier, Ty};

/// The bundle format version, stamped into every bundle and checked on load.
///
/// * `1` — postcard over a mirror of `Core`, with types erased.
/// * `2` — plus `reads_freshness`: whether the view observes §3.7's freshness dimension. postcard
///   is not self-describing, so a field added is a format changed; the alternative was a kernel
///   deciding it by looking for a variable in the view's body, which is a guess about the
///   splitter's output rather than a fact from it.
pub const FORMAT: u32 = 2;

/// A component's slice: the code, and nothing else.
#[derive(Clone, Debug)]
pub struct Bundle {
    /// The signal this bundle renders — the component's name in the program, and what
    /// `beck explain render` prints.
    pub component: Arc<str>,
    /// The program's content-derived command-channel id (§4.3). A client that reconnects to a
    /// server running a different program finds out here rather than by sending it a command it
    /// cannot decode.
    pub wire_id: String,
    /// `(state, session, presence, awareness, freshness, interface) -> Html`.
    pub view: Core,
    /// `(state, proposal) -> Result[list[Event], Rejection]`.
    pub validate: Core,
    /// `(state, Envelope[Event]) -> state`.
    pub fold: Core,
    /// The fold's initial accumulator, as an expression rather than a value: evaluating it is the
    /// kernel's first act, and a `Value` would have needed a second encoding for the same thing.
    pub init: Core,
    /// Every definition the four roles reach, transitively.
    pub defs: BTreeMap<Arc<str>, Core>,
    /// What the client may send, resolved: enough to turn a `data-b-click` attribute into the
    /// `Command` value `validate` expects, and to refuse anything else.
    pub command: command::Schema,
    /// Whether this component's client may run [`Bundle::validate`] and [`Bundle::fold`] on what it
    /// holds — see [`crate::render`] for what decides it, and why holding the state is the whole
    /// question.
    pub optimistic: bool,
    /// Whether this component's `view` reads `freshness()` — §3.7's freshness dimension.
    ///
    /// The client needs it for one reason and it is not rendering: a page that does *not* observe
    /// freshness must keep [`docs/94`](../../../../../docs/94-the-client-report.md) §94.12's
    /// shortcut, where a confirmation that does not move the state costs no render. Comparing the
    /// freshness unconditionally would make every confirmation move it — from `Pending(1)` to
    /// `Confirmed` — and hand every program in the tree back the second render that report
    /// removed.
    pub reads_freshness: bool,
    /// `gestures(step, init)`, when the page keeps client-local interface state — D30's
    /// non-durable fold.
    ///
    /// It travels in the bundle because **this is the only side that can run it**: a gesture never
    /// reaches the server, so a client that could not fold its own would have interface state
    /// nobody could advance. `None` for a page that keeps none, which is every page compiled
    /// before D30.
    pub gestures: Option<Gestures>,
}

/// What a client needs to fold its own gestures: the step, where to start, and what to decode.
#[derive(Clone, Debug)]
pub struct Gestures {
    /// `(S, G) -> S`.
    pub step: Core,
    /// `S` before any gesture — and what the server rendered against (`crate::render`, `B0522`),
    /// which is what makes hydration free for a page with interface state.
    pub init: Core,
    /// The gesture union, resolved to a decoder.
    ///
    /// A **second** schema beside [`Bundle::command`] rather than a widening of it, and the
    /// separation is the point: §3.5's "the client's entire write surface is `send(cmd)` into a
    /// typed `Command` union" survives D30 only if a gesture cannot be decoded as a command. Two
    /// decoders over two unions is what says so.
    pub schema: command::Schema,
}

/// Why a bundle could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadBundle {
    /// Written by a compiler whose format this kernel does not implement.
    Format {
        found: u32,
        expected: u32,
    },
    /// Written by a compiler whose primitives are numbered differently.
    Shape {
        found: String,
        expected: String,
    },
    Malformed(String),
}

impl std::fmt::Display for BadBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BadBundle::Format { found, expected } => write!(
                f,
                "this bundle is format {found} and this kernel reads format {expected}"
            ),
            BadBundle::Shape { found, expected } => write!(
                f,
                "this bundle was compiled by a different compiler \
                 (primitives {found}, this kernel {expected})"
            ),
            BadBundle::Malformed(why) => write!(f, "this bundle is malformed: {why}"),
        }
    }
}

impl std::error::Error for BadBundle {}

/// A digest of the primitive table: every primitive's name and the number this compiler gives it.
///
/// Recomputed rather than stored, so it cannot drift from the table it describes.
pub fn shape_id() -> String {
    let mut hasher = blake3::Hasher::new();
    for (_, prim, _) in crate::prelude::prims() {
        hasher.update(prim.name().as_bytes());
        hasher.update(b"=");
        hasher.update((prim as u32).to_le_bytes().as_slice());
        hasher.update(b";");
    }
    hasher.finalize().to_hex()[..16].to_string()
}

impl Bundle {
    /// The bundle for a placed program's component.
    pub fn of(placed: &Placed) -> Bundle {
        let roles = &placed.roles;
        let mut defs = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for role in [&roles.view, &roles.validate, &roles.fold, &roles.init] {
            reachable(role, placed, &mut seen, &mut defs);
        }
        // The gesture step is a fifth root, and missing it would ship a bundle whose interface
        // state could not be advanced: the step calls the program's own functions like any other
        // role, and nothing else in the bundle reaches them.
        if let Some(g) = &roles.gestures {
            reachable(&g.step, placed, &mut seen, &mut defs);
            reachable(&g.init, placed, &mut seen, &mut defs);
        }
        Bundle {
            component: roles.page_name.clone(),
            wire_id: placed.wire_id.clone(),
            view: roles.view.clone(),
            validate: roles.validate.clone(),
            fold: roles.fold.clone(),
            init: roles.init.clone(),
            defs,
            command: command::Schema::of(placed),
            // Read rather than taken as an argument: whether a client may guess is a question
            // about what crosses to it, answered once in `crate::render`.
            optimistic: placed.render.optimistic,
            reads_freshness: placed.render.reads_freshness,
            gestures: roles.gestures.as_ref().map(|g| Gestures {
                step: g.step.clone(),
                init: g.init.clone(),
                schema: command::Schema::of_union(
                    placed,
                    g.gesture_ty.con_name().unwrap_or_default(),
                ),
            }),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        // The only failure postcard has for an owned `Vec` sink is allocation, and a `Wire` built
        // from a bundle in memory cannot exceed it by construction.
        postcard::to_allocvec(&Wire::of(self)).expect("a bundle is encodable")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Bundle, BadBundle> {
        let wire: Wire =
            postcard::from_bytes(bytes).map_err(|e| BadBundle::Malformed(e.to_string()))?;
        if wire.format != FORMAT {
            return Err(BadBundle::Format {
                found: wire.format,
                expected: FORMAT,
            });
        }
        let expected = shape_id();
        if wire.shape != expected {
            return Err(BadBundle::Shape {
                found: wire.shape,
                expected,
            });
        }
        Ok(wire.to_bundle())
    }

    /// How many `Core` nodes this bundle carries — what `beck explain render` reports as its size
    /// in the compiler's own units, beside the bytes.
    pub fn nodes(&self) -> usize {
        let mut n = 0;
        for code in [&self.view, &self.validate, &self.fold, &self.init]
            .into_iter()
            .chain(self.defs.values())
        {
            count(code, &mut n);
        }
        n
    }
}

/// Everything `code` calls, transitively, added to `defs`.
fn reachable(
    code: &Core,
    placed: &Placed,
    seen: &mut BTreeSet<Arc<str>>,
    defs: &mut BTreeMap<Arc<str>, Core>,
) {
    let mut names = Vec::new();
    globals(code, &mut names);
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        // A global with no definition is a prelude name the checker resolved to a primitive, or a
        // trait method the desugaring turned into an ordinary definition. Either way there is
        // nothing to carry, and the evaluator resolves it the same way on both tiers.
        let Some(def) = placed.program.defs.get(&name) else {
            continue;
        };
        defs.insert(name, def.body.clone());
        reachable(&def.body, placed, seen, defs);
    }
}

fn globals(code: &Core, out: &mut Vec<Arc<str>>) {
    if let CoreKind::Global(name) = &code.kind {
        out.push(name.clone());
    }
    walk(code, &mut |c| globals(c, out));
}

fn count(code: &Core, n: &mut usize) {
    *n += 1;
    walk(code, &mut |c| count(c, n));
}

/// Apply `f` to each immediate sub-expression.
///
/// One walk rather than the same `match` written out at each traversal — and the exhaustive match
/// is what makes a new [`CoreKind`] variant a compile error here rather than a node the bundle
/// silently drops.
fn walk(code: &Core, f: &mut dyn FnMut(&Core)) {
    match &code.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
        CoreKind::Lam { body, .. } => f(body),
        CoreKind::App { func, args } => {
            f(func);
            args.iter().for_each(&mut *f);
        }
        CoreKind::Prim { args, .. } => args.iter().for_each(&mut *f),
        CoreKind::Let { value, body, .. } => {
            f(value);
            f(body);
        }
        CoreKind::If { cond, then, alt } => {
            f(cond);
            f(then);
            f(alt);
        }
        CoreKind::Match { scrutinee, arms } => {
            f(scrutinee);
            for arm in arms {
                arm.exprs().for_each(&mut *f);
            }
        }
        CoreKind::Make { fields, .. } => fields.iter().for_each(|(_, c)| f(c)),
        CoreKind::Field { base, .. } => f(base),
        CoreKind::With { base, fields } => {
            f(base);
            fields.iter().for_each(|(_, c)| f(c));
        }
        CoreKind::ListLit(xs) => xs.iter().for_each(&mut *f),
        CoreKind::MapLit(kvs) => kvs.iter().for_each(|(k, v)| {
            f(k);
            f(v);
        }),
    }
}

// ---------------------------------------------------------------- the encoding

#[derive(Serialize, Deserialize)]
struct Wire {
    format: u32,
    shape: String,
    component: String,
    wire_id: String,
    view: WCore,
    validate: WCore,
    fold: WCore,
    init: WCore,
    defs: Vec<(String, WCore)>,
    command: command::Schema,
    optimistic: bool,
    reads_freshness: bool,
    gestures: Option<(WCore, WCore, command::Schema)>,
}

impl Wire {
    fn of(b: &Bundle) -> Wire {
        Wire {
            format: FORMAT,
            shape: shape_id(),
            component: b.component.to_string(),
            wire_id: b.wire_id.clone(),
            view: WCore::of(&b.view),
            validate: WCore::of(&b.validate),
            fold: WCore::of(&b.fold),
            init: WCore::of(&b.init),
            defs: b
                .defs
                .iter()
                .map(|(name, code)| (name.to_string(), WCore::of(code)))
                .collect(),
            command: b.command.clone(),
            optimistic: b.optimistic,
            reads_freshness: b.reads_freshness,
            gestures: b
                .gestures
                .as_ref()
                .map(|g| (WCore::of(&g.step), WCore::of(&g.init), g.schema.clone())),
        }
    }

    fn to_bundle(&self) -> Bundle {
        Bundle {
            component: Arc::from(self.component.as_str()),
            wire_id: self.wire_id.clone(),
            view: self.view.to_core(),
            validate: self.validate.to_core(),
            fold: self.fold.to_core(),
            init: self.init.to_core(),
            defs: self
                .defs
                .iter()
                .map(|(name, code)| (Arc::from(name.as_str()), code.to_core()))
                .collect(),
            command: self.command.clone(),
            optimistic: self.optimistic,
            reads_freshness: self.reads_freshness,
            gestures: self.gestures.as_ref().map(|(step, init, schema)| Gestures {
                step: step.to_core(),
                init: init.to_core(),
                schema: schema.clone(),
            }),
        }
    }
}

/// A `Core` node, minus the type.
#[derive(Serialize, Deserialize)]
struct WCore {
    kind: WKind,
    /// `(file, start, end)`.
    span: (u32, u32, u32),
    tier: u8,
    /// The three annotations later passes set. Each has a safe default — `false`, `UNORDERED`, `0`
    /// — so carrying them is a performance decision rather than a correctness one; they are here
    /// because a browser is the tier least able to afford re-deriving them.
    last_use: bool,
    order: u32,
    locals: u32,
}

#[derive(Serialize, Deserialize)]
enum WKind {
    Const(WConst),
    Var(VarId),
    Global(String),
    Lam {
        params: Vec<VarId>,
        body: Box<WCore>,
    },
    App {
        func: Box<WCore>,
        args: Vec<WCore>,
    },
    Prim {
        op: WPrim,
        args: Vec<WCore>,
    },
    Let {
        var: VarId,
        value: Box<WCore>,
        body: Box<WCore>,
    },
    If {
        cond: Box<WCore>,
        then: Box<WCore>,
        alt: Box<WCore>,
    },
    Match {
        scrutinee: Box<WCore>,
        arms: Vec<WArm>,
    },
    Make {
        ty: String,
        variant: Option<String>,
        fields: Vec<(String, WCore)>,
    },
    Field {
        base: Box<WCore>,
        name: String,
    },
    With {
        base: Box<WCore>,
        fields: Vec<(String, WCore)>,
    },
    ListLit(Vec<WCore>),
    MapLit(Vec<(WCore, WCore)>),
}

#[derive(Serialize, Deserialize)]
enum WConst {
    Unit,
    Bool(bool),
    Int(i64),
    /// The bit pattern, for [`crate::repr`]'s reason: a decimal rendering is not a round trip.
    Float(u64),
    Str(String),
}

#[derive(Serialize, Deserialize)]
struct WArm {
    pattern: WPattern,
    guard: Option<WCore>,
    body: WCore,
    span: (u32, u32, u32),
}

#[derive(Serialize, Deserialize)]
enum WPattern {
    Wildcard,
    Bind(VarId),
    Const(WConst),
    Ctor {
        variant: String,
        binds: Vec<(String, WPattern)>,
    },
    At {
        var: VarId,
        inner: Box<WPattern>,
    },
    Or(Vec<WPattern>),
    List {
        items: Vec<WPattern>,
        rest: Option<Option<VarId>>,
    },
}

fn span_of(s: Span) -> (u32, u32, u32) {
    (s.file.0, s.start, s.end)
}

fn to_span(s: (u32, u32, u32)) -> Span {
    Span {
        file: FileId(s.0),
        start: s.1,
        end: s.2,
    }
}

impl WConst {
    fn of(c: &Const) -> WConst {
        match c {
            Const::Unit => WConst::Unit,
            Const::Bool(b) => WConst::Bool(*b),
            Const::Int(i) => WConst::Int(*i),
            Const::Float(f) => WConst::Float(f.to_bits()),
            Const::Str(s) => WConst::Str(s.to_string()),
        }
    }

    fn to_const(&self) -> Const {
        match self {
            WConst::Unit => Const::Unit,
            WConst::Bool(b) => Const::Bool(*b),
            WConst::Int(i) => Const::Int(*i),
            WConst::Float(bits) => Const::Float(f64::from_bits(*bits)),
            WConst::Str(s) => Const::Str(Arc::from(s.as_str())),
        }
    }
}

impl WPattern {
    fn of(p: &Pattern) -> WPattern {
        match p {
            Pattern::Wildcard => WPattern::Wildcard,
            Pattern::Bind(v) => WPattern::Bind(*v),
            Pattern::Const(c) => WPattern::Const(WConst::of(c)),
            Pattern::Ctor { variant, binds } => WPattern::Ctor {
                variant: variant.to_string(),
                binds: binds
                    .iter()
                    .map(|(f, p)| (f.to_string(), WPattern::of(p)))
                    .collect(),
            },
            Pattern::At { var, inner } => WPattern::At {
                var: *var,
                inner: Box::new(WPattern::of(inner)),
            },
            Pattern::Or(alts) => WPattern::Or(alts.iter().map(WPattern::of).collect()),
            Pattern::List { items, rest } => WPattern::List {
                items: items.iter().map(WPattern::of).collect(),
                rest: *rest,
            },
        }
    }

    fn to_pattern(&self) -> Pattern {
        match self {
            WPattern::Wildcard => Pattern::Wildcard,
            WPattern::Bind(v) => Pattern::Bind(*v),
            WPattern::Const(c) => Pattern::Const(c.to_const()),
            WPattern::Ctor { variant, binds } => Pattern::Ctor {
                variant: Arc::from(variant.as_str()),
                binds: binds
                    .iter()
                    .map(|(f, p)| (Arc::from(f.as_str()), p.to_pattern()))
                    .collect(),
            },
            WPattern::At { var, inner } => Pattern::At {
                var: *var,
                inner: Box::new(inner.to_pattern()),
            },
            WPattern::Or(alts) => Pattern::Or(alts.iter().map(WPattern::to_pattern).collect()),
            WPattern::List { items, rest } => Pattern::List {
                items: items.iter().map(WPattern::to_pattern).collect(),
                rest: *rest,
            },
        }
    }
}

impl WCore {
    fn of(c: &Core) -> WCore {
        WCore {
            kind: WKind::of(&c.kind),
            span: span_of(c.span),
            tier: c.tier as u8,
            last_use: c.last_use,
            order: c.order,
            locals: c.locals,
        }
    }

    fn to_core(&self) -> Core {
        // The type the checker inferred is not carried (see the module docs), so every node is
        // rebuilt with `Unit`: the evaluator dispatches on values, and a placeholder that is
        // obviously a placeholder is better than one that looks like an inference.
        let mut core = Core::new(self.kind.to_kind(), Ty::unit(), to_span(self.span));
        core.tier = tier_of(self.tier);
        core.last_use = self.last_use;
        core.order = self.order;
        core.locals = self.locals;
        core
    }
}

fn tier_of(byte: u8) -> Tier {
    // Written out rather than transmuted: `Tier` is a `Copy` enum whose numbering is nobody's
    // contract, and a bundle from a compiler that reordered it should be refused by `shape_id`
    // rather than land on a different tier here.
    match byte {
        b if b == Tier::Client as u8 => Tier::Client,
        b if b == Tier::Server as u8 => Tier::Server,
        b if b == Tier::Data as u8 => Tier::Data,
        _ => Tier::Any,
    }
}

impl WKind {
    fn of(k: &CoreKind) -> WKind {
        let fields = |fs: &Vec<(Arc<str>, Core)>| {
            fs.iter()
                .map(|(n, c)| (n.to_string(), WCore::of(c)))
                .collect()
        };
        match k {
            CoreKind::Const(c) => WKind::Const(WConst::of(c)),
            CoreKind::Var(v) => WKind::Var(*v),
            CoreKind::Global(name) => WKind::Global(name.to_string()),
            CoreKind::Lam { params, body } => WKind::Lam {
                params: params.to_vec(),
                body: Box::new(WCore::of(body)),
            },
            CoreKind::App { func, args } => WKind::App {
                func: Box::new(WCore::of(func)),
                args: args.iter().map(WCore::of).collect(),
            },
            CoreKind::Prim { op, args } => WKind::Prim {
                op: WPrim(*op),
                args: args.iter().map(WCore::of).collect(),
            },
            CoreKind::Let { var, value, body } => WKind::Let {
                var: *var,
                value: Box::new(WCore::of(value)),
                body: Box::new(WCore::of(body)),
            },
            CoreKind::If { cond, then, alt } => WKind::If {
                cond: Box::new(WCore::of(cond)),
                then: Box::new(WCore::of(then)),
                alt: Box::new(WCore::of(alt)),
            },
            CoreKind::Match { scrutinee, arms } => WKind::Match {
                scrutinee: Box::new(WCore::of(scrutinee)),
                arms: arms
                    .iter()
                    .map(|a| WArm {
                        pattern: WPattern::of(&a.pattern),
                        guard: a.guard.as_ref().map(WCore::of),
                        body: WCore::of(&a.body),
                        span: span_of(a.span),
                    })
                    .collect(),
            },
            CoreKind::Make {
                ty,
                variant,
                fields: fs,
            } => WKind::Make {
                ty: ty.to_string(),
                variant: variant.as_ref().map(|v| v.to_string()),
                fields: fields(fs),
            },
            CoreKind::Field { base, name } => WKind::Field {
                base: Box::new(WCore::of(base)),
                name: name.to_string(),
            },
            CoreKind::With { base, fields: fs } => WKind::With {
                base: Box::new(WCore::of(base)),
                fields: fields(fs),
            },
            CoreKind::ListLit(xs) => WKind::ListLit(xs.iter().map(WCore::of).collect()),
            CoreKind::MapLit(kvs) => WKind::MapLit(
                kvs.iter()
                    .map(|(k, v)| (WCore::of(k), WCore::of(v)))
                    .collect(),
            ),
        }
    }

    fn to_kind(&self) -> CoreKind {
        let fields = |fs: &Vec<(String, WCore)>| {
            fs.iter()
                .map(|(n, c)| (Arc::from(n.as_str()), c.to_core()))
                .collect()
        };
        match self {
            WKind::Const(c) => CoreKind::Const(c.to_const()),
            WKind::Var(v) => CoreKind::Var(*v),
            WKind::Global(name) => CoreKind::Global(Arc::from(name.as_str())),
            WKind::Lam { params, body } => CoreKind::Lam {
                params: params.as_slice().into(),
                body: Arc::new(body.to_core()),
            },
            WKind::App { func, args } => CoreKind::App {
                func: Box::new(func.to_core()),
                args: args.iter().map(WCore::to_core).collect(),
            },
            WKind::Prim { op, args } => CoreKind::Prim {
                op: op.0,
                args: args.iter().map(WCore::to_core).collect(),
            },
            WKind::Let { var, value, body } => CoreKind::Let {
                var: *var,
                value: Box::new(value.to_core()),
                body: Box::new(body.to_core()),
            },
            WKind::If { cond, then, alt } => CoreKind::If {
                cond: Box::new(cond.to_core()),
                then: Box::new(then.to_core()),
                alt: Box::new(alt.to_core()),
            },
            WKind::Match { scrutinee, arms } => CoreKind::Match {
                scrutinee: Box::new(scrutinee.to_core()),
                arms: arms
                    .iter()
                    .map(|a| Arm {
                        pattern: a.pattern.to_pattern(),
                        guard: a.guard.as_ref().map(WCore::to_core),
                        body: a.body.to_core(),
                        span: to_span(a.span),
                    })
                    .collect(),
            },
            WKind::Make {
                ty,
                variant,
                fields: fs,
            } => CoreKind::Make {
                ty: Arc::from(ty.as_str()),
                variant: variant.as_ref().map(|v| Arc::from(v.as_str())),
                fields: fields(fs),
            },
            WKind::Field { base, name } => CoreKind::Field {
                base: Box::new(base.to_core()),
                name: Arc::from(name.as_str()),
            },
            WKind::With { base, fields: fs } => CoreKind::With {
                base: Box::new(base.to_core()),
                fields: fields(fs),
            },
            WKind::ListLit(xs) => CoreKind::ListLit(xs.iter().map(WCore::to_core).collect()),
            WKind::MapLit(kvs) => CoreKind::MapLit(
                kvs.iter()
                    .map(|(k, v)| (k.to_core(), v.to_core()))
                    .collect(),
            ),
        }
    }
}

/// A primitive, as its number in the table [`shape_id`] pins.
///
/// The number is checked while the bundle is being *decoded* rather than after: a primitive this
/// compiler does not have is a malformed bundle, and the alternative — substituting some other
/// primitive and running it — is the failure this whole module exists to prevent.
#[derive(Clone, Copy)]
struct WPrim(Prim);

impl Serialize for WPrim {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.0 as u32)
    }
}

impl<'de> Deserialize<'de> for WPrim {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<WPrim, D::Error> {
        let n = u32::deserialize(d)?;
        table()
            .get(n as usize)
            .copied()
            .flatten()
            .map(WPrim)
            .ok_or_else(|| serde::de::Error::custom(format!("no primitive is numbered {n}")))
    }
}

/// The primitive table, by number, built once.
///
/// A `Prim` is a fieldless enum with no written discriminants, so the numbers are dense from zero
/// and a `Vec` indexed by them is the lookup. `None` would mean a primitive the prelude declares no
/// signature for, which the checker could never have produced.
fn table() -> &'static [Option<Prim>] {
    static TABLE: std::sync::OnceLock<Vec<Option<Prim>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let prims: Vec<Prim> = crate::prelude::prims()
            .into_iter()
            .map(|(_, p, _)| p)
            .collect();
        let width = prims.iter().map(|p| *p as usize).max().map_or(0, |m| m + 1);
        let mut table = vec![None; width];
        for p in prims {
            table[p as usize] = Some(p);
        }
        table
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(src: &str) -> Placed {
        let (placed, diags, map) = crate::compile_str("t.beck", src);
        assert!(!diags.has_errors(), "{}", diags.render(&map));
        placed.expect("compiles")
    }

    const TODO: &str = r#"
model Todo:
    id: Str
    text: Str
    done: Bool

model State:
    todos: list[Todo]

union Command:
    Add(id: Str, text: Str)
    Toggle(id: Str)

union Event:
    Added(id: Str, text: Str)
    Toggled(id: Str)

union Rejection:
    Blank

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(id, text):
            return s.with(todos=list_append(s.todos, Todo(id=id, text=text, done=False)))
        case Toggled(id):
            return s

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(id, text):
            if str_is_empty(text):
                return Err(error=Blank)
            return Ok(value=[Added(id=id, text=text)])
        case Toggle(id):
            return Ok(value=[Toggled(id=id)])

def label(t: Todo) -> Str:
    return t.text

def render(s: State) -> Html:
    return ui:
        ul:
            for t in s.todos:
                li: label(t)

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, todos, validate)

@on(data)
todos: Signal[State] = durable(fold(apply_event, State(todos=[]), events))

@on(client)
page: Signal[Html] = signal_map(todos, render)
"#;

    #[test]
    fn a_bundle_round_trips_through_its_bytes() {
        let placed = placed(TODO);
        let bundle = Bundle::of(&placed);
        let bytes = bundle.to_bytes();
        let back = Bundle::from_bytes(&bytes).expect("reads back");

        assert_eq!(back.component, bundle.component);
        assert_eq!(back.wire_id, bundle.wire_id);
        assert_eq!(back.optimistic, bundle.optimistic);
        assert_eq!(back.nodes(), bundle.nodes());
        assert_eq!(
            back.defs.keys().collect::<Vec<_>>(),
            bundle.defs.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_bundle_carries_what_its_roles_reach_and_not_the_rest() {
        let placed = placed(TODO);
        let bundle = Bundle::of(&placed);
        // `label` is called by `render`, which is the view.
        assert!(
            bundle.defs.contains_key("label"),
            "{:?}",
            bundle.defs.keys()
        );
        // The roles themselves are carried as roles, not as definitions of the same name.
        assert!(!bundle.defs.contains_key("page"));
    }

    #[test]
    fn a_bundle_from_a_differently_numbered_compiler_is_refused() {
        let placed = placed(TODO);
        let bytes = Bundle::of(&placed).to_bytes();
        let mut wire: Wire = postcard::from_bytes(&bytes).expect("decodes");
        wire.shape = "0000000000000000".to_string();
        let forged = postcard::to_allocvec(&wire).expect("encodes");

        match Bundle::from_bytes(&forged) {
            Err(BadBundle::Shape { found, .. }) => assert_eq!(found, "0000000000000000"),
            other => panic!("expected a shape refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_bundle_from_a_later_format_is_refused() {
        let placed = placed(TODO);
        let bytes = Bundle::of(&placed).to_bytes();
        let mut wire: Wire = postcard::from_bytes(&bytes).expect("decodes");
        wire.format = FORMAT + 1;
        let forged = postcard::to_allocvec(&wire).expect("encodes");

        assert!(matches!(
            Bundle::from_bytes(&forged),
            Err(BadBundle::Format { .. })
        ));
    }
}
