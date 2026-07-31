//! `Core` — the load-bearing IR, and the evaluator that runs it.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.2:
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
//! [`docs/00-original-idea.md`](../../../../docs/00-original-idea.md) names as one of the three
//! that work for a GC'd functional language on a Rust host (Materialize's shape). It is the
//! deliberately-bad-but-complete option, and it keeps the `Core → Target` seam narrow, which §5.2
//! says is what lets a backend slot in later. The Phase 1 report says plainly that native codegen
//! is not done.

use std::collections::BTreeMap;
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
    StrIsEmpty,
    ListLen,
    ListIsEmpty,
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
    // ---- the symbolic signal vocabulary (§3.7) ----
    MergeClients,
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
            MergeClients => "merge_clients",
            StreamFilterMap => "filter_map",
            Fold => "fold",
            Durable => "durable",
            SignalMap => "signal_map",
            SignalMap2 => "map2",
            PerSession => "per_session",
            Decide => "decide",
        }
    }

    /// The effect row of this primitive. §3.2's table for the atoms Phase 1 carries.
    pub fn effects(self) -> &'static [Effect] {
        match self {
            // "Every connected client's send!s, interleaved. Arbitrary order — this is the
            // nondeterminism; there is exactly one of these."
            Prim::MergeClients => &[Effect::Ingress],
            Prim::Durable => &[Effect::Durable],
            Prim::NewUuid => &[Effect::Nondeterministic],
            _ => &[],
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
    pub body: Core,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    Wildcard,
    Bind(VarId),
    Const(Const),
    /// `Added(id, text)` — `variant` names the constructor, `binds` the fields it captures.
    Ctor {
        variant: Arc<str>,
        binds: Vec<(Arc<str>, VarId)>,
    },
}

#[derive(Clone, Debug)]
pub enum CoreKind {
    Const(Const),
    Var(VarId),
    /// A reference to a top-level definition.
    Global(Arc<str>),
    Lam {
        params: Vec<VarId>,
        body: Box<Core>,
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
}

impl Core {
    pub fn new(kind: CoreKind, ty: Ty, span: Span) -> Core {
        Core {
            kind,
            ty,
            tier: Tier::Any,
            span,
        }
    }

    /// Every effect this expression's body can perform, by walking what it calls.
    ///
    /// This is *collection*, not §3.2's inference: there are no effect rows and no effect
    /// polymorphism, so a call to a user function contributes that function's declared effects
    /// rather than inferred ones. It is enough to decide the two things Phase 1 must decide —
    /// whether a placement is legal, and whether a fold is replay-pure — and the Phase 1 report
    /// says so.
    pub fn effects(&self, globals: &dyn Fn(&str) -> Vec<Effect>, out: &mut Vec<Effect>) {
        match &self.kind {
            CoreKind::Prim { op, args } => {
                for e in op.effects() {
                    if !out.contains(e) {
                        out.push(*e);
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
                for a in arms {
                    a.body.effects(globals, out);
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
            CoreKind::Lam { body, .. } => body.place(tier),
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
                for a in arms {
                    a.body.place(tier);
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
    /// Floats are stored as their bit pattern so that `Value` can be `Ord` — a map key, and a
    /// component of the state digest, must have a total order.
    Float(u64),
    Str(Arc<str>),
    List(Arc<Vec<Value>>),
    Map(PMap<Value, Value>),
    /// A model instance or a union variant. `variant` is `None` for a plain record.
    Data {
        ty: Arc<str>,
        variant: Option<Arc<str>>,
        fields: Arc<BTreeMap<Arc<str>, Value>>,
    },
    Html(Arc<Html>),
    /// An attribute waiting to be attached to an element.
    Attr(Arc<AttrValue>),
    Closure(Arc<Closure>),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttrValue {
    Plain(Arc<str>, Arc<str>),
    On(Arc<str>, Value),
    Key(Arc<str>),
}

#[derive(Debug)]
pub struct Closure {
    pub params: Vec<VarId>,
    pub body: Core,
    pub env: Env,
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
    frame: Arc<Vec<(VarId, Value)>>,
    parent: Option<Arc<Env>>,
}

impl Env {
    pub fn new() -> Env {
        Env::default()
    }

    pub fn extend(&self, bindings: Vec<(VarId, Value)>) -> Env {
        Env {
            frame: Arc::new(bindings),
            parent: Some(Arc::new(self.clone())),
        }
    }

    pub fn get(&self, v: VarId) -> Option<&Value> {
        let mut env = self;
        loop {
            if let Some((_, value)) = env.frame.iter().rev().find(|(id, _)| *id == v) {
                return Some(value);
            }
            match &env.parent {
                Some(p) => env = p,
                None => return None,
            }
        }
    }
}

impl Value {
    pub fn float(f: f64) -> Value {
        Value::Float(f.to_bits())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    pub fn str_(s: impl AsRef<str>) -> Value {
        Value::Str(Arc::from(s.as_ref()))
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

    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Data { fields, .. } => fields.get(name),
            _ => None,
        }
    }

    pub fn variant(&self) -> Option<&str> {
        match self {
            Value::Data { variant, .. } => variant.as_deref(),
            _ => None,
        }
    }

    pub fn some(v: Value) -> Value {
        Value::Data {
            ty: Arc::from(Ty::OPTION),
            variant: Some(Arc::from("Some")),
            fields: Arc::new(BTreeMap::from([(Arc::from("value"), v)])),
        }
    }

    pub fn none() -> Value {
        Value::Data {
            ty: Arc::from(Ty::OPTION),
            variant: Some(Arc::from("None")),
            fields: Arc::new(BTreeMap::new()),
        }
    }

    pub fn ok(v: Value) -> Value {
        Value::Data {
            ty: Arc::from(Ty::RESULT),
            variant: Some(Arc::from("Ok")),
            fields: Arc::new(BTreeMap::from([(Arc::from("value"), v)])),
        }
    }

    pub fn err(v: Value) -> Value {
        Value::Data {
            ty: Arc::from(Ty::RESULT),
            variant: Some(Arc::from("Err")),
            fields: Arc::new(BTreeMap::from([(Arc::from("error"), v)])),
        }
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
            Value::Data {
                ty,
                variant,
                fields,
            } => {
                // A newtype wrapping one field prints as that field — `Id(uuid)` reads as the uuid,
                // which is what a key attribute and a rendered list want.
                if variant.is_none() && fields.len() == 1 {
                    if let Some(v) = fields.values().next() {
                        return v.display();
                    }
                }
                let name = variant.as_deref().unwrap_or(ty);
                if fields.is_empty() {
                    return name.to_string();
                }
                let parts: Vec<String> = fields
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
            Value::Data {
                variant, fields, ..
            } => {
                if variant.is_none() && fields.len() == 1 {
                    if let Some(v) = fields.values().next() {
                        return v.to_json();
                    }
                }
                let mut obj = JMap::new();
                if let Some(v) = variant {
                    obj.insert("c".into(), J::String(v.to_string()));
                }
                for (k, v) in fields.iter() {
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
        Value::Str(s) => json!({"$": "str", "v": s.as_ref()}),
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
        Value::Data {
            ty,
            variant,
            fields,
        } => {
            let mut f = JMap::new();
            for (k, val) in fields.iter() {
                f.insert(k.to_string(), value_to_repr(val)?);
            }
            json!({
                "$": "data",
                "t": ty.as_ref(),
                "c": variant.as_deref(),
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
            let mut fields = BTreeMap::new();
            for (k, val) in j.get("f")?.as_object()? {
                fields.insert(Arc::from(k.as_str()), value_from_repr(val)?);
            }
            Value::Data {
                ty: Arc::from(j.get("t")?.as_str()?),
                variant: j.get("c").and_then(|c| c.as_str()).map(Arc::from),
                fields: Arc::new(fields),
            }
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
        Value::Data {
            ty,
            variant,
            fields,
        } => {
            h.update(&[7]);
            h.update(ty.as_bytes());
            h.update(variant.as_deref().unwrap_or("").as_bytes());
            h.update(&(fields.len() as u64).to_le_bytes());
            for (k, val) in fields.iter() {
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

    #[test]
    fn the_log_encoding_round_trips_exactly() {
        // The wire encoding deliberately loses information the browser does not need; the log
        // encoding cannot, because replay compares digests of what it reads back.
        let v = Value::Data {
            ty: Arc::from("State"),
            variant: None,
            fields: Arc::new(BTreeMap::from([(
                Arc::from("todos"),
                Value::Map(PMap::from_iter([(
                    Value::str_("k"),
                    Value::Data {
                        ty: Arc::from("Todo"),
                        variant: None,
                        fields: Arc::new(BTreeMap::from([
                            (Arc::from("done"), Value::Bool(true)),
                            (Arc::from("n"), Value::Int(-3)),
                        ])),
                    },
                )])),
            )])),
        };
        assert_eq!(
            value_from_repr(&value_to_repr(&v).unwrap()),
            Some(v.clone())
        );
        let evt = Value::Data {
            ty: Arc::from("Event"),
            variant: Some(Arc::from("Toggled")),
            fields: Arc::new(BTreeMap::from([(Arc::from("id"), Value::str_("x"))])),
        };
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
        let state = Value::Data {
            ty: Arc::from("State"),
            variant: None,
            fields: Arc::new(BTreeMap::from([(Arc::from("cached"), view.clone())])),
        };
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
        let id = Value::Data {
            ty: Arc::from("Id"),
            variant: None,
            fields: Arc::new(BTreeMap::from([(Arc::from("value"), Value::str_("u-1"))])),
        };
        assert_eq!(id.display(), "u-1");
        assert_eq!(id.to_json(), serde_json::json!("u-1"));
    }

    #[test]
    fn a_command_serialises_with_its_variant_tag() {
        let cmd = Value::Data {
            ty: Arc::from("Command"),
            variant: Some(Arc::from("Toggle")),
            fields: Arc::new(BTreeMap::from([(Arc::from("id"), Value::str_("x"))])),
        };
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
    fn only_the_three_impure_primitives_carry_effects() {
        assert_eq!(Prim::MergeClients.effects(), &[Effect::Ingress]);
        assert_eq!(Prim::Durable.effects(), &[Effect::Durable]);
        assert_eq!(Prim::NewUuid.effects(), &[Effect::Nondeterministic]);
        assert!(Prim::Add.effects().is_empty());
        assert!(
            Prim::Fold.effects().is_empty(),
            "a fold is pure; `durable` is the effect"
        );
    }
}
