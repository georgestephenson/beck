//! Query fusion: a plan rewritten into a smaller plan that computes the same thing.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.3:
//!
//! > Query fusion still matters (a `for` over a view of a view should become one plan, not N+1
//! > lookups); it is a plan-rewrite on symbolic `Query` nodes, kept symbolic in `Core` precisely
//! > for this.
//!
//! [`crate::plan`] decomposes a view into operators one construct at a time, so it produces the
//! operators the *source* names: `concat_lists(map_list(xs, f))` is a `map_list` whose arrangement
//! holds one list per element and a `flatten` that takes them apart again. Nothing reads the
//! arrangement in between. This module is the pass that says so.
//!
//! # What a rewrite has to preserve, and it is not only the values
//!
//! An arrangement's **key** is what makes iteration order a consequence of the plan rather than of
//! a sort at the end ([`crate::plan`]), and the order reaches the rendered page and the replay
//! digest. So a rewrite here has three obligations, not one: the same values, in the same order,
//! and the same *deltas* — a fused operator has to move exactly the entries the pair moved, or a
//! subscriber woken late updates by a delta that does not describe what happened.
//!
//! Every rule below is stated with the property that makes it sound, and each is a *local* rewrite
//! of one consumer and its producer. [`38`](../../../../../docs/38-literature-survey.md) §38.2
//! points at equality saturation (egg, egglog) as the machinery for this, and it is not used:
//! equality saturation earns its keep when rewrites conflict and the phase order would decide the
//! answer. None of these conflict — every one *removes* an operator and none adds one — so the
//! extraction that an e-graph would do by cost is done here by applying rules to a fixed point.
//! §89.6 names what would need one.
//!
//! # The three conditions, and the second is the interesting one
//!
//! A producer may be fused into its consumer only when:
//!
//! 1. **nothing else reads it** — `consumers == 1`. An arrangement read twice is
//!    [`26`](../../../../../docs/26-arrangement-sharing-report.md)'s shared prefix, and fusing it
//!    into one consumer computes it twice;
//! 2. **the fusion does not cross §5.3's session cut.** A shared operator fused into a per-session
//!    one stops being shared: its work moves from *once per event* to *once per event per
//!    subscriber*, which on a 256-subscriber feed is the 55× that report measured, spent rather
//!    than saved. A local rewrite that is an improvement everywhere else is a pessimisation here,
//!    and nothing but the cut can tell;
//! 3. **no name points at it.** A declared signal is projected as a read-model table
//!    ([`88`](../../../../../docs/88-read-models-and-pgwire-report.md)), so an operator a developer
//!    named is observable to a SQL client even when the page does not read it.

use std::collections::BTreeMap;

use crate::core::{Const, Core, CoreKind, VarId};
use crate::plan::{Fun, Op, OpId, Plan};
use crate::ty::{Tier, Ty};

/// Every rule this pass has, by name.
///
/// Published so that `fusion.rs` can hold the set to the programs that exercise it: a rule no
/// program reaches is a rule the differential harness says nothing about, and it would sit here
/// looking like coverage.
pub const RULES: &[&str] = &[
    "map_list over map_list",
    "filter_list over filter_list",
    "flatten over map_list",
    "a count over a cardinality-preserving operator",
    "concat_lists of one list",
];

/// One rewrite that fired.
#[derive(Clone, Debug)]
pub struct Fusion {
    /// The rule's name, which is the shape it matched: `"flatten over map_list"`.
    pub rule: &'static str,
    /// The operator that remains, in the fused plan's numbering.
    pub at: OpId,
    /// What the operator became.
    pub became: &'static str,
    /// The property that makes it sound.
    pub why: &'static str,
}

/// One rewrite that matched a shape and was refused, with the condition that refused it.
///
/// Printed by `beck explain query`, because "this could have fused and here is what stopped it" is
/// the sentence a developer can act on — usually by moving where the program reads the session.
#[derive(Clone, Debug)]
pub struct Refusal {
    pub rule: &'static str,
    /// The consumer, in the fused plan's numbering.
    pub at: OpId,
    /// The producer that stayed.
    pub kept: OpId,
    pub why: String,
}

/// What one run of the pass did.
#[derive(Clone, Debug, Default)]
pub struct Fusions {
    pub fired: Vec<Fusion>,
    pub refused: Vec<Refusal>,
    /// Operators before and after, and the arrangements among them — the two numbers the pass is
    /// for, since an arrangement removed is memory per subscriber as well as work per event.
    pub operators: (usize, usize),
    pub arrangements: (usize, usize),
}

/// Rewrite a plan to a fixed point.
pub fn fuse(mut plan: Plan) -> (Plan, Fusions) {
    let mut rec = Fusions {
        operators: (plan.nodes.len(), 0),
        arrangements: (arrangements(&plan), 0),
        ..Fusions::default()
    };
    // Recorded against the numbering of the round they fired in, then carried through each
    // compaction, so what a report prints is where the operator is *now*.
    let mut fired: Vec<(OpId, Fusion)> = Vec::new();
    let mut refused: Vec<(OpId, OpId, Refusal)> = Vec::new();

    // A round is bounded by the node count and each round removes at least one node, so this
    // terminates for the same reason the plan is finite.
    for _ in 0..plan.nodes.len() + 1 {
        let Some((absorbed, survivor)) = round(&mut plan, &mut fired, &mut refused) else {
            break;
        };
        // A refusal recorded against an operator that has since been absorbed is still a refusal —
        // it is the operator that absorbed it that now reads the thing it could not fuse. Carrying
        // it over rather than dropping it is what keeps `beck explain query` able to say why the
        // shared half of a view stayed shared, which is the one refusal a developer can act on.
        refused.retain(|(at, kept, _)| !(*at == survivor && *kept == absorbed));
        for (at, kept, _) in refused.iter_mut() {
            if *at == absorbed {
                *at = survivor;
            }
            if *kept == absorbed {
                *kept = survivor;
            }
        }
        let map = plan.prune();
        remap(&mut fired, &mut refused, &map);
    }

    rec.operators.1 = plan.nodes.len();
    rec.arrangements.1 = arrangements(&plan);
    rec.fired = fired
        .into_iter()
        .map(|(at, f)| Fusion { at, ..f })
        .collect();
    rec.refused = refused
        .into_iter()
        .map(|(at, kept, r)| Refusal { at, kept, ..r })
        .collect();
    rec.refused.sort_by_key(|r| (r.at, r.kept));
    rec.refused.dedup_by_key(|r| (r.at, r.kept, r.rule));
    (plan, rec)
}

fn arrangements(plan: &Plan) -> usize {
    plan.nodes.iter().filter(|n| n.op.is_arrangement()).count()
}

/// One pass over the plan, stopping at the first rewrite.
///
/// Returns the operator that was absorbed and the one that absorbed it, so that a refusal recorded
/// against the first can be carried to the second.
fn round(
    plan: &mut Plan,
    fired: &mut Vec<(OpId, Fusion)>,
    refused: &mut Vec<(OpId, OpId, Refusal)>,
) -> Option<(OpId, OpId)> {
    for i in 0..plan.nodes.len() {
        // A `concat_lists` of one list is that list. It is the only rewrite here that removes the
        // *consumer* rather than the producer, because what it removes is a re-keying — every
        // entry gains the same `[0]` prefix, so the order the prefix decides is the order it
        // already had.
        if matches!(plan.nodes[i].op, Op::Concat) && plan.nodes[i].inputs.len() == 1 {
            let input = plan.nodes[i].inputs[0];
            if plan.nodes[input].op.is_arrangement() && i != plan.state && i != plan.session {
                substitute(plan, i, input);
                fired.push((
                    input,
                    Fusion {
                        rule: "concat_lists of one list",
                        at: input,
                        became: plan.nodes[input].op.name(),
                        why: "a union of one delta stream is that delta stream, and every entry \
                              gained the same key prefix",
                    },
                ));
                return Some((i, input));
            }
        }

        let Some(&p) = plan.nodes[i].inputs.first() else {
            continue;
        };
        let Some(rule) = matching(&plan.nodes[i].op, &plan.nodes[p].op) else {
            continue;
        };
        if let Some(why) = refuses(plan, i, p, rule) {
            refused.push((
                i,
                p,
                Refusal {
                    rule: rule.name,
                    at: i,
                    kept: p,
                    why,
                },
            ));
            continue;
        }
        (rule.apply)(plan, i, p);
        fired.push((
            i,
            Fusion {
                rule: rule.name,
                at: i,
                became: plan.nodes[i].op.name(),
                why: rule.why,
            },
        ));
        return Some((p, i));
    }
    None
}

/// A rule: the shape it matches, why it is sound, and what it does.
struct Rule {
    name: &'static str,
    why: &'static str,
    /// True when this rule moves the producer's per-element work into the consumer, which is what
    /// makes crossing the session cut a pessimisation rather than a saving.
    carries_work: bool,
    apply: fn(&mut Plan, OpId, OpId),
}

fn matching(consumer: &Op, producer: &Op) -> Option<&'static Rule> {
    match (consumer, producer) {
        (Op::MapList { .. }, Op::MapList { .. }) => Some(&MAP_OVER_MAP),
        (Op::FilterList { .. }, Op::FilterList { .. }) => Some(&FILTER_OVER_FILTER),
        (Op::Flatten, Op::MapList { .. }) => Some(&FLATTEN_OVER_MAP),
        (Op::Count | Op::IsEmpty, Op::MapList { .. } | Op::SortBy { .. }) => Some(&COUNT_OVER),
        _ => None,
    }
}

static MAP_OVER_MAP: Rule = Rule {
    name: "map_list over map_list",
    why: "neither moves an element, so both arrangements have the input's key and the composition \
          has it too",
    carries_work: true,
    apply: |plan, i, p| {
        let inner = fun_of(&plan.nodes[p].op)
            .expect("the rule matched a map_list")
            .clone();
        let outer = fun_of(&plan.nodes[i].op)
            .expect("the rule matched a map_list")
            .clone();
        plan.nodes[i].op = Op::MapList {
            f: compose(&outer, &inner),
        };
        plan.nodes[i].inputs = plan.nodes[p].inputs.clone();
    },
};

static FILTER_OVER_FILTER: Rule = Rule {
    name: "filter_list over filter_list",
    why: "a conjunction, and it short-circuits — the outer predicate is applied to exactly the \
          elements the inner one kept, which is what the pair did",
    carries_work: true,
    apply: |plan, i, p| {
        let inner = fun_of(&plan.nodes[p].op)
            .expect("the rule matched a filter_list")
            .clone();
        let outer = fun_of(&plan.nodes[i].op)
            .expect("the rule matched a filter_list")
            .clone();
        plan.nodes[i].op = Op::FilterList {
            f: conjoin(&outer, &inner),
        };
        plan.nodes[i].inputs = plan.nodes[p].inputs.clone();
    },
};

static FLATTEN_OVER_MAP: Rule = Rule {
    name: "flatten over map_list",
    why: "the map's key is the input's and the flatten's is the map's followed by a position, so \
          one operator keyed by the input's key and a position is the same order",
    carries_work: true,
    apply: |plan, i, p| {
        let f = fun_of(&plan.nodes[p].op)
            .expect("the rule matched a map_list")
            .clone();
        plan.nodes[i].op = Op::FlatMap { f };
        plan.nodes[i].inputs = plan.nodes[p].inputs.clone();
    },
};

static COUNT_OVER: Rule = Rule {
    name: "a count over a cardinality-preserving operator",
    why: "`map_list` and `sort_by` produce one entry per entry, so how many there are is a \
          question about the input and the arrangement between them is never read",
    // A count does not apply the producer's function at all — it drops it — so this one is a
    // saving on whichever side of the cut it lands.
    carries_work: false,
    apply: |plan, i, p| {
        plan.nodes[i].inputs = vec![plan.nodes[p].inputs[0]];
    },
};

/// The three conditions, in the order a reader needs them.
fn refuses(plan: &Plan, i: OpId, p: OpId, rule: &Rule) -> Option<String> {
    if p == plan.state || p == plan.session || p == plan.root {
        return Some("it is the plan's root or one of its sources".to_string());
    }
    if plan.nodes[p].consumers > 1 {
        return Some(format!(
            "#{p} is read by {} operators, and fusing it into one of them would compute it {} \
             times (docs/26)",
            plan.nodes[p].consumers, plan.nodes[p].consumers
        ));
    }
    let names = plan.names_of(p);
    if !names.is_empty() {
        return Some(format!(
            "`{}` is a declared signal, so the read model projects it as a table (docs/88)",
            names.join("`, `")
        ));
    }
    if rule.carries_work && !plan.nodes[p].per_session && plan.nodes[i].per_session {
        return Some(format!(
            "#{p} is shared and #{i} is per session, so fusing would move work the process does \
             once per event to work it does once per event per subscriber (docs/26 §5.3)"
        ));
    }
    None
}

fn fun_of(op: &Op) -> Option<&Fun> {
    match op {
        Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } | Op::FlatMap { f } => Some(f),
        _ => None,
    }
}

// -------------------------------------------------------------------------------------------
// Composing two per-element functions
// -------------------------------------------------------------------------------------------

/// `g ∘ f`, as one [`Fun`].
///
/// A [`Fun`]'s code is a `Lam` over its captured operators followed by the element, and both of
/// these are closed — every variable either is a parameter or is bound inside. So the composition
/// does not substitute anything: it binds fresh parameters and *applies* both lambdas, which is
/// why nothing here has to reason about variable capture.
fn compose(outer: &Fun, inner: &Fun) -> Fun {
    let (params, inner_args, outer_caps, x) = frame(outer, inner);
    let applied = apply(&inner.code, inner_args);
    let mut outer_args: Vec<Core> = outer_caps;
    outer_args.push(applied);
    let _ = x;
    Fun {
        code: lam(params, apply(&outer.code, outer_args)),
        captures: inner
            .captures
            .iter()
            .chain(outer.captures.iter())
            .copied()
            .collect(),
    }
}

/// `λx. if f(x): g(x) else false` — the conjunction of two predicates, short-circuiting.
///
/// Written as an `If` rather than as `Prim::And` for the reason
/// [`53`](../../../../../docs/53-are-we-fast-yet-report.md) gives: `and` *is* an `If` in `Core`, and
/// building the strict primitive here would apply the outer predicate to elements the inner one
/// rejected — which the pair of operators never did.
fn conjoin(outer: &Fun, inner: &Fun) -> Fun {
    let (params, inner_args, outer_caps, x) = frame(outer, inner);
    let mut outer_args: Vec<Core> = outer_caps;
    outer_args.push(var(x));
    Fun {
        code: lam(
            params,
            Core {
                kind: CoreKind::If {
                    cond: Box::new(apply(&inner.code, inner_args)),
                    then: Box::new(apply(&outer.code, outer_args)),
                    alt: Box::new(Core {
                        kind: CoreKind::Const(Const::Bool(false)),
                        ty: Ty::bool_(),
                        tier: Tier::Any,
                        span: beck_diag::Span::NONE,
                        last_use: false,
                        order: crate::fields::UNORDERED,
                        locals: 0,
                    }),
                },
                ty: Ty::bool_(),
                tier: Tier::Any,
                span: beck_diag::Span::NONE,
                last_use: false,
                order: crate::fields::UNORDERED,
                locals: 0,
            },
        ),
        captures: inner
            .captures
            .iter()
            .chain(outer.captures.iter())
            .copied()
            .collect(),
    }
}

/// The parameter list both compositions share: the inner's captures, the outer's, then the
/// element — the order [`crate::engine`] supplies arguments in.
fn frame(outer: &Fun, inner: &Fun) -> (Vec<VarId>, Vec<Core>, Vec<Core>, VarId) {
    let n = inner.captures.len() + outer.captures.len() + 1;
    // Fresh, so a parameter cannot shadow a variable either body binds. Both bodies are closed
    // under their own parameters, so any distinct names would do; distinct *and above everything*
    // keeps a debug print readable.
    let base = 1 + max_var(&inner.code).max(max_var(&outer.code));
    let params: Vec<VarId> = (0..n as VarId).map(|k| base + k).collect();
    let inner_args: Vec<Core> = params[..inner.captures.len()]
        .iter()
        .copied()
        .chain(std::iter::once(params[n - 1]))
        .map(var)
        .collect();
    let outer_caps: Vec<Core> = params[inner.captures.len()..n - 1]
        .iter()
        .copied()
        .map(var)
        .collect();
    let x = params[n - 1];
    (params, inner_args, outer_caps, x)
}

fn max_var(c: &Core) -> VarId {
    let mut top = 0;
    walk(c, &mut |x| {
        if let CoreKind::Var(v) = &x.kind {
            top = top.max(*v);
        }
        if let CoreKind::Lam { params, .. } = &x.kind {
            for p in params.iter() {
                top = top.max(*p);
            }
        }
        if let CoreKind::Let { var, .. } = &x.kind {
            top = top.max(*var);
        }
    });
    top
}

fn walk(c: &Core, f: &mut impl FnMut(&Core)) {
    f(c);
    match &c.kind {
        CoreKind::Var(_) | CoreKind::Const(_) | CoreKind::Global(_) => {}
        CoreKind::Lam { body, .. } => walk(body, f),
        CoreKind::App { func, args } => {
            walk(func, f);
            args.iter().for_each(|a| walk(a, f));
        }
        CoreKind::Prim { args, .. } => args.iter().for_each(|a| walk(a, f)),
        CoreKind::Let { value, body, .. } => {
            walk(value, f);
            walk(body, f);
        }
        CoreKind::If { cond, then, alt } => {
            walk(cond, f);
            walk(then, f);
            walk(alt, f);
        }
        CoreKind::Match { scrutinee, arms } => {
            walk(scrutinee, f);
            arms.iter().for_each(|a| walk(&a.body, f));
        }
        CoreKind::Make { fields, .. } => fields.iter().for_each(|(_, v)| walk(v, f)),
        CoreKind::Field { base, .. } => walk(base, f),
        CoreKind::With { base, fields } => {
            walk(base, f);
            fields.iter().for_each(|(_, v)| walk(v, f));
        }
        CoreKind::ListLit(items) => items.iter().for_each(|i| walk(i, f)),
        CoreKind::MapLit(pairs) => pairs.iter().for_each(|(k, v)| {
            walk(k, f);
            walk(v, f);
        }),
    }
}

fn apply(f: &Core, args: Vec<Core>) -> Core {
    let ty = match &f.ty {
        Ty::Fun(_, ret, _) => (**ret).clone(),
        _ => Ty::unit(),
    };
    Core {
        kind: CoreKind::App {
            func: Box::new(f.clone()),
            args,
        },
        ty,
        tier: Tier::Any,
        span: f.span,
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

fn lam(params: Vec<VarId>, body: Core) -> Core {
    Core {
        ty: Ty::fun(params.iter().map(|_| Ty::unit()).collect(), body.ty.clone()),
        tier: body.tier,
        span: body.span,
        kind: CoreKind::Lam {
            params: params.into(),
            body: std::sync::Arc::new(body),
        },
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

fn var(v: VarId) -> Core {
    Core {
        kind: CoreKind::Var(v),
        ty: Ty::unit(),
        tier: Tier::Any,
        span: beck_diag::Span::NONE,
        last_use: false,
        order: crate::fields::UNORDERED,
        locals: 0,
    }
}

// -------------------------------------------------------------------------------------------
// Keeping the plan a plan
// -------------------------------------------------------------------------------------------

/// Point every reader of `from` at `to`, including the plan's own roots and names.
fn substitute(plan: &mut Plan, from: OpId, to: OpId) {
    let swap = |id: &mut OpId| {
        if *id == from {
            *id = to;
        }
    };
    for node in &mut plan.nodes {
        node.inputs.iter_mut().for_each(swap);
        if let Op::MapList { f } | Op::FilterList { f } | Op::SortBy { f } | Op::FlatMap { f } =
            &mut node.op
        {
            f.captures.iter_mut().for_each(swap);
        }
    }
    swap(&mut plan.root);
    for (_, id) in &mut plan.signals {
        swap(id);
    }
}

fn remap(
    fired: &mut [(OpId, Fusion)],
    refused: &mut Vec<(OpId, OpId, Refusal)>,
    map: &BTreeMap<OpId, OpId>,
) {
    for (at, _) in fired.iter_mut() {
        if let Some(&n) = map.get(at) {
            *at = n;
        }
    }
    // A refusal whose producer was removed by another rule is not a refusal any more.
    refused.retain(|(at, kept, _)| map.contains_key(at) && map.contains_key(kept));
    for (at, kept, _) in refused.iter_mut() {
        *at = map[at];
        *kept = map[kept];
    }
}

// -------------------------------------------------------------------------------------------
// The report
// -------------------------------------------------------------------------------------------

/// The fusion half of `beck explain query`.
pub fn report(f: &Fusions) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "\nwhat fused (§5.3)");
    if f.fired.is_empty() {
        let _ = writeln!(
            out,
            "  nothing.{}",
            if f.arrangements.0 == 0 {
                " This view holds no collection, so there is no pair of collection\n  \
                 operators for a rule to match."
            } else {
                " No operator here is read by exactly one operator that could absorb\n  it."
            }
        );
    }
    for fusion in &f.fired {
        let _ = writeln!(
            out,
            "  #{:<3} {:<38} → {}",
            fusion.at, fusion.rule, fusion.became
        );
        let _ = writeln!(out, "       {}", fusion.why);
    }
    if !f.refused.is_empty() {
        let _ = writeln!(out, "\nwhat matched a rule and did not fuse");
        for r in &f.refused {
            let _ = writeln!(out, "  #{:<3} {:<38} kept #{}", r.at, r.rule, r.kept);
            let _ = writeln!(out, "       {}", r.why);
        }
    }
    let _ = writeln!(
        out,
        "\n  {} operators before, {} after; {} arrangements before, {} after. An arrangement is \n  \
         memory per subscriber as well as work per event (docs/26 §26.7), which is why the second \n  \
         pair is the one to read.",
        f.operators.0, f.operators.1, f.arrangements.0, f.arrangements.1
    );
    out
}
