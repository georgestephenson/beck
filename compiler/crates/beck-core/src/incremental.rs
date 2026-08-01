//! Which views can be maintained by delta, and which have to be recomputed — and why.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.8:
//!
//! > **Subscribed views** (anything feeding a live `page`, or marked `materialized`) compile to
//! > **incremental dataflow plans** … `remaining` updates by ±1 per event, never by recount. …
//! > Arbitrary pure code is incrementalized where analysis allows, recomputed where not —
//! > **`beck explain incremental <view>` shows which, and why**.
//!
//! [`20`](../../../../docs/20-phase-2-report.md) §20.6 item 3 said the input for this existed
//! ("a view whose row is empty is a pure function of the signal — which is §3.8's precondition")
//! and §20.5 said the command was not built, because until the general slicer there was no plan to
//! ask about: an inlined view is one expression, and "which vertices are incremental" is not a
//! question an expression can answer.
//!
//! # What this is, and firmly is not
//!
//! It is the **analysis**. There is no differential-dataflow engine, no arrangement, no delta
//! stream; every view in the compiler is still a full recompute per event, and
//! [`23`](../../../../docs/23-general-slicer-report.md) §23.9 says so first. What this produces is
//! the verdict a view engine would consume and the sentence a developer needs before writing a
//! view that quietly costs a recount per event over a million rows.
//!
//! Saying that plainly matters more here than elsewhere, because a command called `explain
//! incremental` that printed "incremental" about a full recompute would be the most misleading
//! output in the compiler. Every report this module produces leads with what is true today.
//!
//! # The rule, and where it comes from
//!
//! Three things have to hold before a vertex can be maintained by delta, and they are checked in
//! this order because that is the order in which the answers are useful:
//!
//! 1. **The row is empty.** §3.8's precondition, and the one Phase 2 already computes. A view that
//!    performs an effect is re-evaluated when the effect says so, not when its input changes.
//! 2. **Every operation it applies has a delta rule.** `list_len` after a `filter_list` updates by
//!    ±1; a `sort_by` maintains a sorted arrangement; arithmetic and record construction are
//!    pointwise. A `match` on the accumulator, or a function this analysis cannot see through, has
//!    no rule, and the honest answer is "recompute".
//! 3. **It is downstream of a `durable` fold and upstream of a sink.** A vertex nothing subscribes
//!    to is not a view; §3.8's scope is "anything feeding a live `page`, or marked `materialized`".
//!
//! [`RULES`] is the table for step 2. Like [`crate::cost`]'s numbers it is **stated, not
//! measured** — each entry is a delta rule the differential-dataflow literature already has, and it
//! is written down so that it can be argued with rather than discovered in a profiler. Nothing in
//! this module claims an implementation exists for any of them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::check::Program;
use crate::core::{Core, CoreKind, Prim};
use crate::signal::{Op, SigId};
use crate::split::Placed;
use crate::ty::Effect;

/// The operations with a known delta rule, and the rule.
///
/// "Known" means known to the literature, not implemented here. The second column is what a view
/// engine would have to do, and it is written out because a table of names would be a list of
/// opinions.
pub const RULES: &[(Prim, &str)] = &[
    (Prim::MapList, "a delta in, the same delta mapped out"),
    (
        Prim::FilterList,
        "a delta in, kept or dropped by the predicate",
    ),
    (
        Prim::ListLen,
        "±1 per delta — §3.8's `remaining`, never a recount",
    ),
    (Prim::ListIsEmpty, "a count, thresholded"),
    (Prim::MapValues, "the arrangement, read by value"),
    (Prim::MapLen, "±1 per insert or remove"),
    (Prim::MapGet, "a point lookup into the arrangement"),
    (Prim::MapContains, "a point lookup into the arrangement"),
    (
        Prim::SortBy,
        "an ordered arrangement, maintained by insertion",
    ),
    (Prim::ConcatLists, "a union of delta streams"),
    // Pointwise on a value, so a delta at the input is a delta at the output.
    (Prim::Add, "pointwise"),
    (Prim::Sub, "pointwise"),
    (Prim::Mul, "pointwise"),
    (Prim::Div, "pointwise"),
    (Prim::Rem, "pointwise"),
    (Prim::Neg, "pointwise"),
    (Prim::Eq, "pointwise"),
    (Prim::Ne, "pointwise"),
    (Prim::Lt, "pointwise"),
    (Prim::Le, "pointwise"),
    (Prim::Gt, "pointwise"),
    (Prim::Ge, "pointwise"),
    (Prim::And, "pointwise"),
    (Prim::Or, "pointwise"),
    (Prim::Not, "pointwise"),
    (Prim::ToStr, "pointwise"),
    (Prim::StrTrim, "pointwise"),
    (Prim::StrIsEmpty, "pointwise"),
    (Prim::OptionIsSome, "pointwise"),
    (Prim::OptionUnwrapOr, "pointwise"),
    // The `ui:` vocabulary is a tree constructor, and a tree of deltas is what the patch protocol
    // already carries (§5.1). This is the one row where the runtime half exists.
    (
        Prim::HtmlEl,
        "a subtree delta — what the patch protocol already streams",
    ),
    (Prim::HtmlText, "a text patch"),
    (Prim::HtmlAttr, "an attribute patch"),
    (Prim::HtmlOn, "an attribute patch"),
    (Prim::HtmlKey, "the key a keyed-children diff is by"),
];

fn rule(op: Prim) -> Option<&'static str> {
    RULES.iter().find(|(p, _)| *p == op).map(|(_, r)| *r)
}

/// What a view engine could do with one vertex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Every operation has a delta rule: this vertex could be maintained rather than recomputed.
    Incremental,
    /// Pure, but something in it has no delta rule. The reason names the first blocker found, in
    /// source order, because the first is the one to fix.
    Recompute { because: String },
    /// The row is not empty, so §3.8's precondition fails before the shape is even looked at.
    Effectful { effects: Vec<Effect> },
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Incremental => "incremental",
            Verdict::Recompute { .. } => "recompute",
            Verdict::Effectful { .. } => "not a candidate",
        }
    }
}

/// One vertex's assessment.
#[derive(Clone, Debug)]
pub struct Assessment {
    pub node: SigId,
    pub label: Arc<str>,
    pub verdict: Verdict,
    /// The operations found in this vertex's function, with the rule each would be maintained by.
    /// Empty for a vertex that applies nothing — a `durable`, an alias.
    pub ops: Vec<(Prim, &'static str)>,
    /// True when this vertex's value is read by more than one consumer, so an engine would share
    /// one arrangement rather than build two ([`05`](../../../../docs/05-tier-lowering.md) §5.3).
    pub shared: bool,
    /// True when this vertex is at or below a `per_session`, so an engine would run it *per
    /// subscriber* rather than once. §3.8: "per-session views are the norm, not the exception."
    pub per_session: bool,
}

/// Assess every vertex between the durable folds and the sinks.
///
/// Vertices that are not views — the ingress, the chokepoint, the folds themselves — are left out,
/// because §3.8's question is about views and answering it about a `merge_clients()` would be
/// filling a report with rows nobody asked for.
pub fn assess(placed: &Placed) -> Vec<Assessment> {
    let g = &placed.graph;
    let below = per_session_closure(placed);
    let mut out = Vec::new();
    for id in g.order() {
        let node = g.node(id);
        let f = match &node.op {
            Op::Map { f } | Op::Map2 { f } | Op::PerSession { f } => f,
            // A `filter_map` on the *stream* side is not a view: it decides which events a fold
            // sees, and a fold is not maintained by delta — it *is* the delta consumer.
            _ => continue,
        };
        let (verdict, ops) = judge(f, &placed.program);
        out.push(Assessment {
            node: id,
            label: node.label.clone(),
            verdict,
            ops,
            shared: g.consumers(id).len() > 1,
            per_session: below.contains(&id),
        });
    }
    out
}

/// Every vertex at or downstream of a `per_session`.
///
/// §5.3's shape is "one shared dataflow whose final per-session operators run per subscriber", so
/// the boundary is the thing a report has to be able to point at.
fn per_session_closure(placed: &Placed) -> BTreeSet<SigId> {
    let g = &placed.graph;
    let mut below = BTreeSet::new();
    // The order is dependencies-first, so a vertex's inputs are decided before it is.
    for id in g.order() {
        let node = g.node(id);
        if matches!(node.op, Op::PerSession { .. }) || node.inputs.iter().any(|i| below.contains(i))
        {
            below.insert(id);
        }
    }
    below
}

/// Judge one signal function: the thing `signal_map(s, f)` applies.
fn judge(f: &Core, program: &Program) -> (Verdict, Vec<(Prim, &'static str)>) {
    let mut found: Vec<(Prim, &'static str)> = Vec::new();
    let mut blocker: Option<String> = None;
    let mut seen: BTreeSet<Arc<str>> = BTreeSet::new();

    // §3.8's precondition, from the row Phase 2 already inferred.
    let mut effects = Vec::new();
    f.effects(&globals_of(program), &mut effects);
    effects.retain(|e| !e.is_ambient());
    if !effects.is_empty() {
        return (Verdict::Effectful { effects }, found);
    }

    walk_through(f, program, &mut seen, &mut |c| {
        if blocker.is_some() {
            return;
        }
        match &c.kind {
            CoreKind::Prim { op, .. } => match rule(*op) {
                Some(r) => {
                    if !found.iter().any(|(p, _)| p == op) {
                        found.push((*op, r));
                    }
                }
                None => {
                    blocker = Some(format!(
                        "`{}` has no delta rule: a change to its input can change all of its \
                         output",
                        op.name()
                    ))
                }
            },
            // A `match` chooses a *shape*, and a delta that changes which arm applies changes
            // everything downstream of it. Differential dataflow handles this by treating the
            // scrutinee as a collection and each arm as a branch of the plan; that is a real
            // technique and it is not this table.
            CoreKind::Match { .. } => {
                blocker = Some(
                    "a `match` on the input picks which computation runs, and a delta can move it \
                     between arms"
                        .to_string(),
                )
            }
            CoreKind::Global(name) => {
                if program.defs.contains_key(name) {
                    return;
                }
                blocker = Some(format!(
                    "`{name}` is not a definition this analysis can see into"
                ));
            }
            _ => {}
        }
    });

    match blocker {
        Some(because) => (Verdict::Recompute { because }, found),
        None => (Verdict::Incremental, found),
    }
}

fn globals_of(program: &Program) -> impl Fn(&str) -> Vec<Effect> + '_ {
    move |name: &str| {
        program
            .defs
            .get(name)
            .map(|d| d.effects.clone())
            .unwrap_or_default()
    }
}

/// Walk an expression, following calls into the definitions it names.
///
/// Recursion is cut by `seen`, and a recursive definition is *not* a blocker on its own: a
/// self-recursive pure function over a list is exactly what `map`/`filter` desugar from in most
/// languages. What blocks is an operation with no rule, wherever it is found.
fn walk_through(
    c: &Core,
    program: &Program,
    seen: &mut BTreeSet<Arc<str>>,
    f: &mut impl FnMut(&Core),
) {
    f(c);
    if let CoreKind::Global(name) = &c.kind {
        if seen.insert(name.clone()) {
            if let Some(def) = program.defs.get(name) {
                walk_through(&def.body, program, seen, f);
            }
        }
        return;
    }
    children(c, &mut |k| walk_through(k, program, seen, f));
}

fn children(c: &Core, f: &mut impl FnMut(&Core)) {
    match &c.kind {
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
        CoreKind::Lam { body, .. } => f(body),
        CoreKind::App { func, args } => {
            f(func);
            args.iter().for_each(f);
        }
        CoreKind::Prim { args, .. } => args.iter().for_each(f),
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
            arms.iter().for_each(|a| f(&a.body));
        }
        CoreKind::Make { fields, .. } => fields.iter().for_each(|(_, v)| f(v)),
        CoreKind::Field { base, .. } => f(base),
        CoreKind::With { base, fields } => {
            f(base);
            fields.iter().for_each(|(_, v)| f(v));
        }
        CoreKind::ListLit(items) => items.iter().for_each(f),
        CoreKind::MapLit(pairs) => pairs.iter().for_each(|(k, v)| {
            f(k);
            f(v);
        }),
    }
}

/// What `beck explain incremental` prints.
pub fn report(placed: &Placed, only: Option<&str>) -> String {
    use std::fmt::Write;
    let all = assess(placed);
    let rows: Vec<&Assessment> = match only {
        None => all.iter().collect(),
        Some(name) => all.iter().filter(|a| a.label.as_ref() == name).collect(),
    };
    let mut out = String::new();

    if let Some(name) = only {
        if rows.is_empty() {
            let known: Vec<&str> = all.iter().map(|a| a.label.as_ref()).collect();
            let _ = writeln!(
                out,
                "`{name}` is not a view in this program.\nviews: {}",
                if known.is_empty() {
                    "none — every signal is the fold, the chokepoint or the ingress".to_string()
                } else {
                    known.join(", ")
                }
            );
            return out;
        }
    }

    // The disclaimer is first because it is the thing a reader most needs and least expects.
    let _ = writeln!(
        out,
        "Every view below is a **full recompute per event** today. This is the analysis §3.8 asks\n\
         for — which views a differential-dataflow plan could maintain by delta, and why the rest\n\
         could not — and the engine that would maintain them is not built (docs/23 §23.9).\n"
    );

    if rows.is_empty() {
        let _ = writeln!(
            out,
            "This program has no views: the page reads the accumulator directly, so there is\n\
             nothing between the fold and the browser to maintain."
        );
        return out;
    }

    let w = rows
        .iter()
        .map(|a| a.label.chars().count())
        .max()
        .unwrap_or(0);
    for a in &rows {
        let mut tags = Vec::new();
        if a.shared {
            tags.push("shared");
        }
        if a.per_session {
            tags.push("per session");
        }
        let _ = writeln!(
            out,
            "  {:<w$}  {:<15}{}",
            a.label,
            a.verdict.name(),
            if tags.is_empty() {
                String::new()
            } else {
                format!("({})", tags.join(", "))
            },
        );
        match &a.verdict {
            Verdict::Incremental => {
                for (op, r) in &a.ops {
                    let _ = writeln!(out, "  {:w$}    {:<14} {r}", "", op.name());
                }
            }
            Verdict::Recompute { because } => {
                let _ = writeln!(out, "  {:w$}    {because}", "");
                if !a.ops.is_empty() {
                    let _ = writeln!(
                        out,
                        "  {:w$}    the rest would have been: {}",
                        "",
                        a.ops
                            .iter()
                            .map(|(p, _)| p.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            Verdict::Effectful { effects } => {
                let _ = writeln!(
                    out,
                    "  {:w$}    performs {{{}}}, so §3.8's precondition — an empty row — does not \
                     hold",
                    "",
                    effects
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }

    if only.is_none() {
        let shared: Vec<&str> = rows
            .iter()
            .filter(|a| a.shared)
            .map(|a| a.label.as_ref())
            .collect();
        let fanout: Vec<&str> = rows
            .iter()
            .filter(|a| a.per_session)
            .map(|a| a.label.as_ref())
            .collect();
        let _ = writeln!(out, "\nthe shape a plan would have (§5.3)");
        let _ = writeln!(
            out,
            "  shared arrangement: {}",
            if shared.is_empty() {
                "nothing is read twice, so there is no prefix to share".to_string()
            } else {
                shared.join(", ")
            }
        );
        let _ = writeln!(
            out,
            "  per subscriber:     {}",
            if fanout.is_empty() {
                "nothing — this program broadcasts one view to every connection".to_string()
            } else {
                format!(
                    "{}  (one plan, these operators per connected session)",
                    fanout.join(", ")
                )
            }
        );
        let n = rows.len();
        let inc = rows
            .iter()
            .filter(|a| a.verdict == Verdict::Incremental)
            .count();
        let eff = rows
            .iter()
            .filter(|a| matches!(a.verdict, Verdict::Effectful { .. }))
            .count();
        let _ = write!(
            out,
            "\n{inc} of {n} view{} could be maintained by delta",
            if n == 1 { "" } else { "s" },
        );
        if n - inc - eff > 0 {
            let _ = write!(out, "; {} would be recomputed", n - inc - eff);
        }
        if eff > 0 {
            let _ = write!(
                out,
                "; {eff} {} not a candidate, because an effect decides when it runs",
                if eff == 1 { "is" } else { "are" }
            );
        }
        let _ = writeln!(out, ".");
    }
    out
}

/// A map from vertex label to verdict, for a test that wants the answer rather than the prose.
pub fn verdicts(placed: &Placed) -> BTreeMap<Arc<str>, Verdict> {
    assess(placed)
        .into_iter()
        .map(|a| (a.label, a.verdict))
        .collect()
}
