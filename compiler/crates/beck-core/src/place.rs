//! Stage 7 — placement: inferred from effects, solved against a cost model, verified.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../../docs/03-type-and-effect-system.md) §3.3:
//! "**Placement is not a primitive annotation; it is a constraint solution** — with explicit
//! `@on(...)` always available and always winning. Purity means *unplaced*: legal on every tier,
//! compiled to each tier that needs it."
//!
//! Phase 1 shipped the *verification* half. Phase 2 adds the other two thirds of §3.3 and all of
//! §3.4: candidates come from the inferred effect row, the choice among them comes from a cost
//! model ([`crate::cost`]), and every guardrail §3.4 calls non-negotiable is here —
//!
//! * **determinism** — integer costs, a fixed enumeration order and a total tie-break, so the same
//!   program always yields the same solution;
//! * **stability** — the previous solution is persisted in `beck.lock` and preferred on a tie, and
//!   churn against it is reported rather than absorbed;
//! * **explainability** — every candidate's cost survives the solve, so `beck explain place` can
//!   print the derivation rather than the conclusion;
//! * **assertability** — `beck check --assert-place page=client` fails the build on a change.
//!
//! # What is actually being decided
//!
//! Less than it first appears, and that is the design working rather than the solver shirking.
//! Three rules pin most of a program before the solver runs, and each is a statement about meaning
//! rather than about cost:
//!
//! 1. `@on(...)` wins (§3.3).
//! 2. A **pure definition is unplaced** — `Tier::Any` — and is compiled into every tier that calls
//!    it. "That duplication is the payoff, not waste."
//! 3. A **`Signal[Html]` is the browser's subscription** (§4.3), so it is pinned to the client.
//!    Where the *view function* runs — Mode A server-side rendering versus Mode B in the browser —
//!    is the [`05`](../../../../../docs/05-tier-lowering.md) §5.1 decision that §3.4 also mentions,
//!    and it is Phase 3's: Mode B does not exist, so the cost model has nothing to choose between
//!    and should not pretend otherwise.
//!
//! What is left is genuinely open, and the corpus exercises it: which tier holds a `durable` fold,
//! where `decide` sits, and where an effectful definition that more than one tier could discharge
//! ends up — `net.out(origin)` being the sharp case, since a browser and a server can both make
//! that call and only the callers decide which is cheaper.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics};

use crate::check::Program;
use crate::core::{Core, CoreKind};
use crate::cost::{self, Cost, FORBIDDEN};
use crate::ty::{Effect, Row, Tier, Ty};

/// A placeable thing: a top-level definition, or a signal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Def(Arc<str>),
    Signal(Arc<str>),
}

impl Key {
    pub fn name(&self) -> &Arc<str> {
        match self {
            Key::Def(n) | Key::Signal(n) => n,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Def(n) => write!(f, "def/{n}"),
            Key::Signal(n) => write!(f, "signal/{n}"),
        }
    }
}

/// Why a node's placement is what it is — the content of `beck explain place`.
#[derive(Clone, Debug)]
pub struct Explanation {
    pub key: Key,
    pub chosen: Tier,
    pub row: Row,
    /// True when the tier was decided by a *rule* rather than by cost — an `@on(...)`, purity, or a
    /// `Signal[Html]`. The candidate costs below are still real, and for a pinned node the chosen
    /// one is not required to be the cheapest: that is what pinning means.
    pub pinned: bool,
    /// Every tier, with the whole program's cost when this node sits there and everything else
    /// stays where the solver put it. §4.7's "candidates : data (cost 1.0), server (cost 3.1), …".
    pub candidates: Vec<(Tier, Cost)>,
    pub because: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Every combination was tried: the solution is optimal for this cost model.
    Exhaustive,
    /// Too many free nodes to enumerate; a deterministic sweep to a local minimum. Reported, so
    /// that nobody has to wonder which they got.
    Sweep,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Exhaustive => "exhaustive (optimal for this cost model)",
            Method::Sweep => "sweep (a local minimum, not proved optimal)",
        }
    }
}

/// The solved placement of a whole program.
#[derive(Clone, Debug)]
pub struct Solution {
    pub tiers: BTreeMap<Key, Tier>,
    pub explanations: Vec<Explanation>,
    pub method: Method,
    pub total: Cost,
    /// Nodes whose placement differs from `beck.lock` — §3.4's "churn reported in CI".
    pub churn: Vec<(Key, Tier, Tier)>,
    /// Nodes where two tiers cost exactly the same and no lock settled it.
    pub ties: Vec<(Key, Vec<Tier>)>,
}

impl Solution {
    pub fn explanation(&self, name: &str) -> Option<&Explanation> {
        self.explanations
            .iter()
            .find(|e| e.key.name().as_ref() == name || e.key.to_string() == name)
    }
}

/// The persisted previous solution — §3.4's stability guardrail.
///
/// A one-line edit must not re-place unrelated code. Nothing in the cost model prevents that on its
/// own: costs are a function of the whole program, so a new definition shifts them everywhere. The
/// lock is what turns "the solver happens to agree with yesterday" into "the solver is *asked* to
/// agree with yesterday, and says so when it cannot" — a tie-break and a record, never an override,
/// because a lock that could force an illegal placement would be a way to defeat §3.5.
#[derive(Clone, Debug, Default)]
pub struct Lock {
    pub tiers: BTreeMap<String, Tier>,
}

impl Lock {
    pub const FILE: &'static str = "beck.lock";

    pub fn from_json(text: &str) -> Option<Lock> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let mut tiers = BTreeMap::new();
        for (k, t) in v.get("placement")?.as_object()? {
            if let Some(t) = t.as_str().and_then(Tier::parse) {
                tiers.insert(k.clone(), t);
            }
        }
        Some(Lock { tiers })
    }

    pub fn to_json(&self) -> String {
        let placement: serde_json::Map<String, serde_json::Value> = self
            .tiers
            .iter()
            .map(|(k, t)| (k.clone(), serde_json::Value::String(t.name().into())))
            .collect();
        format!(
            "{:#}\n",
            serde_json::json!({
                "version": 1,
                "note": "beck's solved placement. Review it like a lockfile: a change here is a \
                         change in where code runs.",
                "placement": placement,
            })
        )
    }

    pub fn of(solution: &Solution) -> Lock {
        Lock {
            tiers: solution
                .tiers
                .iter()
                .map(|(k, t)| (k.to_string(), *t))
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The graph the solver works over
// ---------------------------------------------------------------------------------------------

struct Node {
    key: Key,
    row: Row,
    work: i64,
    /// The type of the value that flows *out* of this node, for edge byte estimates.
    carried: Ty,
    pinned: Option<(Tier, String)>,
    candidates: Vec<Tier>,
}

/// The order the solver prefers among equal-cost candidates: closest to the log first.
///
/// A total order, so a tie can never be resolved by iteration order or by a hash seed. The solver
/// still *records* that the tie happened, so the default is a decision rather than a hiding place.
const PREFERENCE: [Tier; 3] = [Tier::Data, Tier::Server, Tier::Client];

fn rank(t: Tier) -> usize {
    PREFERENCE.iter().position(|x| *x == t).unwrap_or(9)
}

fn walk(c: &Core, f: &mut dyn FnMut(&Core)) {
    match &c.kind {
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
            arms.iter().for_each(|a| f(&a.body));
        }
        CoreKind::Make { fields, .. } => fields.iter().for_each(|(_, v)| f(v)),
        CoreKind::With { base, fields } => {
            f(base);
            fields.iter().for_each(|(_, v)| f(v));
        }
        CoreKind::Field { base, .. } => f(base),
        CoreKind::ListLit(xs) => xs.iter().for_each(&mut *f),
        CoreKind::MapLit(kvs) => kvs.iter().for_each(|(k, v)| {
            f(k);
            f(v);
        }),
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
    }
}

/// Every top-level name an expression mentions — the placement graph's edges.
pub fn mentions(c: &Core, out: &mut BTreeSet<Arc<str>>) {
    if let CoreKind::Global(n) = &c.kind {
        out.insert(n.clone());
    }
    walk(c, &mut |k| mentions(k, out));
}

fn size(c: &Core) -> i64 {
    let mut n = 1;
    walk(c, &mut |k| n += size(k));
    n
}

/// The element a signal carries: `Signal[State]` carries a `State`.
fn carried(t: &Ty) -> Ty {
    match t {
        Ty::Con(n, args)
            if (n.as_ref() == Ty::SIGNAL || n.as_ref() == Ty::STREAM) && args.len() == 1 =>
        {
            args[0].clone()
        }
        other => other.clone(),
    }
}

fn is_html_signal(t: &Ty) -> bool {
    matches!(t, Ty::Con(n, args)
        if n.as_ref() == Ty::SIGNAL && args.len() == 1 && args[0].con_name() == Some(Ty::HTML))
}

// ---------------------------------------------------------------------------------------------
// The solver
// ---------------------------------------------------------------------------------------------

/// Beyond this many free nodes, enumerating every combination stops being instant. Ten is 59 049
/// assignments — microseconds — and no program in the corpus comes close.
const EXHAUSTIVE_LIMIT: usize = 10;

/// Solve the placement of every definition and signal in a checked program.
pub fn solve(program: &Program, lock: Option<&Lock>) -> Solution {
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: BTreeMap<Arc<str>, usize> = BTreeMap::new();

    for name in &program.def_order {
        let Some(d) = program.defs.get(name) else {
            continue;
        };
        let pinned = if d.tier_is_annotated {
            Some((
                d.tier,
                format!("`@on({})`, which always wins", d.tier.name()),
            ))
        } else if d.row.atoms.iter().all(|e| Tier::Any.discharges(e)) {
            // §3.3: "Purity means *unplaced*: legal on every tier, compiled to each tier that
            // needs it."
            //
            // The test is `Tier::Any.discharges` — every concrete tier can do this — and **not**
            // "the visible row is empty". They differ on `partial`, which every tier discharges and
            // which is not ambient, so a function that may diverge was being *placed* despite being
            // legal everywhere. Found by the generated-program suite
            // (`tests/placement_properties.rs`), which is exactly the case nobody would have
            // written by hand.
            Some((
                Tier::Any,
                if d.row.atoms.is_empty() {
                    "pure, so it is compiled into every tier that calls it".to_string()
                } else {
                    format!(
                        "{{{}}} is discharged on every tier, so it is compiled into each one that \
                         calls it",
                        d.row
                            .visible()
                            .iter()
                            .map(|e| e.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
            ))
        } else {
            None
        };
        index.insert(name.clone(), nodes.len());
        nodes.push(Node {
            key: Key::Def(name.clone()),
            candidates: Tier::candidates(&d.row),
            row: d.row.clone(),
            work: size(&d.body),
            carried: d.ret.clone(),
            pinned,
        });
    }

    for s in &program.signals {
        let pinned = if s.tier_is_annotated {
            Some((
                s.tier,
                format!("`@on({})`, which always wins", s.tier.name()),
            ))
        } else if is_html_signal(&s.ty) {
            // §4.3: a signal edge that crosses tiers *is* the subscription, and the subscriber of a
            // rendered document is the browser. That is what the type means, not what it costs.
            Some((
                Tier::Client,
                "a `Signal[Html]` is what the browser subscribes to".to_string(),
            ))
        } else {
            None
        };
        index.insert(s.name.clone(), nodes.len());
        nodes.push(Node {
            key: Key::Signal(s.name.clone()),
            candidates: Tier::candidates(&s.row),
            row: s.row.clone(),
            work: size(&s.expr),
            carried: carried(&s.ty),
            pinned,
        });
    }

    // Edges: one unordered pair per dependency, so a crossing is charged once however many times it
    // is named. The pair is all that is recorded — what a crossing *carries* is decided from both
    // ends in `cost::edge_cost`, not from whichever end happened to be walked first.
    let mut edges: Vec<(usize, usize)> = Vec::new();
    {
        let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
        let mut push = |from: usize, to: usize, edges: &mut Vec<(usize, usize)>| {
            if from == to {
                return;
            }
            let pair = if from < to { (from, to) } else { (to, from) };
            if seen.insert(pair) {
                edges.push(pair);
            }
        };
        for name in &program.def_order {
            let (Some(d), Some(&i)) = (program.defs.get(name), index.get(name)) else {
                continue;
            };
            let mut names = BTreeSet::new();
            mentions(&d.body, &mut names);
            for m in names {
                if let Some(&j) = index.get(&m) {
                    push(i, j, &mut edges);
                }
            }
        }
        for s in &program.signals {
            let Some(&i) = index.get(&s.name) else {
                continue;
            };
            let mut names = BTreeSet::new();
            mentions(&s.expr, &mut names);
            for m in names {
                if let Some(&j) = index.get(&m) {
                    push(i, j, &mut edges);
                }
            }
        }
    }

    // Candidate order: the lock first (stability), then the fixed preference (determinism).
    for n in &mut nodes {
        let locked = lock.and_then(|l| l.tiers.get(&n.key.to_string()).copied());
        n.candidates
            .sort_by_key(|t| (usize::from(Some(*t) != locked), rank(*t), t.name()));
    }

    let free: Vec<usize> = (0..nodes.len())
        .filter(|i| nodes[*i].pinned.is_none() && nodes[*i].candidates.len() > 1)
        .collect();

    let base: Vec<Tier> = nodes
        .iter()
        .map(|n| match &n.pinned {
            Some((t, _)) => *t,
            None => n.candidates.first().copied().unwrap_or(Tier::Server),
        })
        .collect();

    let total_of = |assign: &[Tier]| -> Cost {
        let mut sum: Cost = 0;
        for (i, n) in nodes.iter().enumerate() {
            sum = sum.saturating_add(cost::node_cost(
                assign[i],
                &n.row,
                n.work,
                Some(&n.carried),
                &program.types,
            ));
        }
        for (a, b) in &edges {
            sum = sum.saturating_add(cost::edge_cost(
                assign[*a],
                assign[*b],
                &nodes[*a].carried,
                &nodes[*b].carried,
                &program.types,
            ));
        }
        sum
    };

    let (assign, method) = if free.len() <= EXHAUSTIVE_LIMIT {
        (
            exhaustive(&nodes, &free, &base, &total_of),
            Method::Exhaustive,
        )
    } else {
        (sweep(&nodes, &free, &base, &total_of), Method::Sweep)
    };

    // Explanations: the cost of the whole program with this node moved and everything else left
    // where the solver put it. That is the number a person is actually asking about.
    //
    // Computed as a **delta** rather than by re-summing the program. Moving one node changes its
    // own cost and the cost of the edges touching it; every other term is the term it already was.
    // Re-summing made this loop `O(n × (n + e))` — three full sweeps per definition — which is the
    // whole of the front end's superlinearity, since parse, expand, check and the security pass
    // are each flat per declaration (`docs/64` §64.2). Incidence lists make it `O(n + e)`.
    let mut incident: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (e, (a, b)) in edges.iter().enumerate() {
        incident[*a].push(e);
        incident[*b].push(e);
    }
    let settled = total_of(&assign);

    let mut explanations = Vec::new();
    let mut ties = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let mut candidates: Vec<(Tier, Cost)> = Vec::new();
        for t in [Tier::Client, Tier::Server, Tier::Data] {
            if !n.row.atoms.iter().all(|e| t.discharges(e)) {
                candidates.push((t, FORBIDDEN));
                continue;
            }
            if t == assign[i] {
                candidates.push((t, settled));
                continue;
            }
            let mut delta = cost::node_cost(t, &n.row, n.work, Some(&n.carried), &program.types)
                .saturating_sub(cost::node_cost(
                    assign[i],
                    &n.row,
                    n.work,
                    Some(&n.carried),
                    &program.types,
                ));
            for &e in &incident[i] {
                let (a, b) = edges[e];
                let (was_a, was_b) = (assign[a], assign[b]);
                // `push` drops self-edges, so exactly one end is `i`.
                let (now_a, now_b) = if a == i { (t, was_b) } else { (was_a, t) };
                let carried = (&nodes[a].carried, &nodes[b].carried);
                delta = delta
                    .saturating_add(cost::edge_cost(
                        now_a,
                        now_b,
                        carried.0,
                        carried.1,
                        &program.types,
                    ))
                    .saturating_sub(cost::edge_cost(
                        was_a,
                        was_b,
                        carried.0,
                        carried.1,
                        &program.types,
                    ));
            }
            candidates.push((t, settled.saturating_add(delta)));
        }
        let because = match &n.pinned {
            Some((_, why)) => why.clone(),
            None => reason(n, assign[i]),
        };
        if n.pinned.is_none() && n.candidates.len() > 1 {
            let best = candidates
                .iter()
                .map(|(_, c)| *c)
                .filter(|c| *c < FORBIDDEN)
                .min()
                .unwrap_or(0);
            let tied: Vec<Tier> = candidates
                .iter()
                .filter(|(_, c)| *c == best)
                .map(|(t, _)| *t)
                .collect();
            if tied.len() > 1 {
                ties.push((n.key.clone(), tied));
            }
        }
        explanations.push(Explanation {
            key: n.key.clone(),
            chosen: assign[i],
            row: n.row.clone(),
            pinned: n.pinned.is_some(),
            candidates,
            because,
        });
    }

    let tiers: BTreeMap<Key, Tier> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.key.clone(), assign[i]))
        .collect();

    let churn = lock
        .map(|l| {
            tiers
                .iter()
                .filter_map(|(k, t)| {
                    let was = l.tiers.get(&k.to_string())?;
                    (was != t).then(|| (k.clone(), *was, *t))
                })
                .collect()
        })
        .unwrap_or_default();

    Solution {
        total: settled,
        tiers,
        explanations,
        method,
        churn,
        ties,
    }
}

/// Every combination, in candidate order, keeping the first strict improvement.
///
/// "First strict improvement" is where the tie-break lives: because candidates are sorted lock-first
/// and then by [`PREFERENCE`], two assignments that cost the same always resolve the same way, on
/// any machine, in any order of iteration.
fn exhaustive(
    nodes: &[Node],
    free: &[usize],
    base: &[Tier],
    total: &dyn Fn(&[Tier]) -> Cost,
) -> Vec<Tier> {
    let mut best = base.to_vec();
    let mut best_cost = total(&best);
    let radices: Vec<usize> = free.iter().map(|i| nodes[*i].candidates.len()).collect();
    let combinations: usize = radices.iter().product();
    let mut assign = base.to_vec();
    for n in 0..combinations {
        let mut rest = n;
        for (slot, i) in free.iter().enumerate() {
            let r = radices[slot];
            assign[*i] = nodes[*i].candidates[rest % r];
            rest /= r;
        }
        let c = total(&assign);
        if c < best_cost {
            best_cost = c;
            best.clone_from(&assign);
        }
    }
    best
}

/// A deterministic sweep to a local minimum, for programs too large to enumerate.
///
/// Iterated conditional modes: visit the free nodes in a fixed order, move each to its cheapest
/// tier given the others, and repeat until nothing moves. It is *not* optimal, and
/// [`Method::Sweep`] is reported so that a local minimum is never read as a global one.
fn sweep(
    nodes: &[Node],
    free: &[usize],
    base: &[Tier],
    total: &dyn Fn(&[Tier]) -> Cost,
) -> Vec<Tier> {
    let mut assign = base.to_vec();
    for _ in 0..64 {
        let mut moved = false;
        for i in free {
            let current = assign[*i];
            let mut best = current;
            let mut best_cost = total(&assign);
            for t in &nodes[*i].candidates {
                assign[*i] = *t;
                let c = total(&assign);
                if c < best_cost {
                    best_cost = c;
                    best = *t;
                }
            }
            assign[*i] = best;
            moved |= best != current;
        }
        if !moved {
            break;
        }
    }
    assign
}

fn reason(n: &Node, chosen: Tier) -> String {
    let visible = n.row.visible();
    if let Some(forcing) = visible.iter().find(|e| {
        crate::ty::CONCRETE_TIERS
            .iter()
            .filter(|t| t.discharges(e))
            .count()
            == 1
    }) {
        return format!(
            "`{}` is discharged only by `{}`",
            forcing.name(),
            chosen.name()
        );
    }
    if n.row.atoms.contains(&Effect::Durable) && chosen == Tier::Data {
        return "the log is at the data tier, and the accumulator is what the log stores".into();
    }
    if visible.is_empty() {
        return format!(
            "no effect forces a tier; `{}` costs least given its neighbours",
            chosen.name()
        );
    }
    format!(
        "{{{}}} can be discharged by {}, and `{}` costs least",
        visible
            .iter()
            .map(|e| e.name())
            .collect::<Vec<_>>()
            .join(", "),
        Tier::candidates(&n.row)
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(" or "),
        chosen.name()
    )
}

/// Write the solved tiers back into the program.
pub fn apply(program: &mut Program, solution: &Solution) {
    for (name, def) in program.defs.iter_mut() {
        if let Some(t) = solution.tiers.get(&Key::Def(name.clone())) {
            def.tier = *t;
            def.body.place(*t);
        }
    }
    for s in program.signals.iter_mut() {
        if let Some(t) = solution.tiers.get(&Key::Signal(s.name.clone())) {
            s.tier = *t;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Verification — Phase 1's half, still load-bearing
// ---------------------------------------------------------------------------------------------

/// Verify every placement in a checked program, annotated or solved.
pub fn check_placement(program: &Program, diags: &mut Diagnostics) {
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        verify(
            def.tier,
            &def.row,
            if def.tier_is_annotated {
                Why::Annotated
            } else {
                Why::Solved
            },
            &format!("`{}`", def.name),
            def.span,
            def.tier_span,
            diags,
        );
    }
    for s in &program.signals {
        verify(
            s.tier,
            &s.row,
            if s.tier_is_annotated {
                Why::Annotated
            } else if is_html_signal(&s.ty) {
                Why::Structural(HTML_IS_THE_BROWSERS)
            } else {
                Why::Solved
            },
            &format!("`{}`", s.name),
            s.span,
            s.tier_span,
            diags,
        );
    }

    // §3.7's determinism rule, over the *named* function a fold is given. `check` catches `uuid()`
    // written lexically inside `fold(...)`; this catches it reached through a function — and, with
    // inference, through any depth of them, which is what Phase 1 could not do.
    for s in &program.signals {
        for f in fold_functions(&s.expr) {
            let crate::core::CoreKind::Global(name) = &f.kind else {
                continue;
            };
            let Some(def) = program.defs.get(name) else {
                continue;
            };
            let breaking: Vec<&Effect> = def.effects.iter().filter(|e| e.breaks_replay()).collect();
            if breaking.is_empty() {
                continue;
            }
            diags.push(
                Diagnostic::error(
                    "B0402",
                    format!("`{name}` is a fold function, so it must be replay-pure"),
                    f.span,
                )
                .with_primary_label(format!(
                    "performs {{{}}}",
                    breaking
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .with_label(def.span, "defined here")
                .with_note(
                    "replaying the log must reproduce the state bit for bit; time is data on the \
                     envelope (`env.at`) and identity is minted at the edge",
                ),
            );
        }
    }

    // §3.7: "one totally-ordered log per application … there is exactly one of these."
    let ingress: Vec<&crate::check::SignalDecl> = program
        .signals
        .iter()
        .filter(|s| s.effects.contains(&Effect::Ingress))
        .collect();
    if ingress.len() > 1 {
        let mut d = Diagnostic::error(
            "B0403",
            "a program has exactly one merge point",
            ingress[1].span,
        )
        .with_primary_label("a second `merge_clients()`")
        .with_note(
            "the merge point is where time and nondeterminism enter; two of them would mean two \
             total orders, and replay would no longer be a function of the log",
        );
        d = d.with_label(ingress[0].span, "the first one is here");
        diags.push(d);
    }
}

/// Why a node sits on the tier a diagnostic found it on — which decides what the diagnostic is
/// allowed to claim.
///
/// The distinction matters because [`verify`] used to say "this placement was solved rather than
/// written, so a diagnostic here is a compiler defect" whenever no `@on` was present. That note is
/// true of a *chosen* placement and false of a **forced** one: a `Signal[Html]` is the browser's
/// because of its type, so an undischargeable effect under one is a statement about the program.
/// The first program to hit it was `reveal(...)` inside a view, which is a user error being
/// reported as a compiler bug.
#[derive(Clone, Copy)]
enum Why {
    /// The program wrote `@on(...)`.
    Annotated,
    /// The type decided, and no annotation could have decided otherwise. Carries the sentence that
    /// says so, and the advice that follows from it.
    Structural(&'static str),
    /// The cost model chose, so any refusal here really is the compiler's fault.
    Solved,
}

/// The one structural pin there is in Phase 2.
const HTML_IS_THE_BROWSERS: &str =
    "a `Signal[Html]` is the browser's because of its type, so this \
     is not a placement an annotation can move: the effect has to be discharged before the value \
     reaches the view — in `validate`, which is where the session is";

fn verify(
    tier: Tier,
    row: &Row,
    why: Why,
    what: &str,
    span: beck_diag::Span,
    tier_span: beck_diag::Span,
    diags: &mut Diagnostics,
) {
    let candidates = Tier::candidates(row);
    if candidates.is_empty() {
        // No tier can discharge this row. Neither the solver nor an annotation can help; the
        // definition has to be split into parts that can each be placed.
        let names: Vec<String> = row.visible().iter().map(|e| e.name()).collect();
        diags.push(
            Diagnostic::error(
                "B0400",
                format!("{what} performs effects no single tier can discharge"),
                span,
            )
            .with_primary_label(format!("{{{}}}", names.join(", ")))
            .with_note(
                "each tier discharges a fixed set (docs/03 §3.3); a row no tier covers has to be \
                 split across definitions that can each be placed",
            ),
        );
        return;
    }

    if tier == Tier::Any {
        // After the solver has run, this can only be reached by an explicit `@on(any)` on
        // something effectful — the solver never leaves an effectful node unplaced.
        //
        // "Effectful" has to mean *undischargeable here*, not merely non-empty. `partial` and
        // `raises(E)` are performed by definitions that are legal on every tier, and asking the
        // author to pin one of those to a tier would be asking them to make a placement decision
        // the language does not need them to make.
        if let Some(e) = row.visible().into_iter().find(|e| !tier.discharges(e)) {
            diags.push(
                Diagnostic::error("B0404", format!("{what} cannot be unplaced"), tier_span)
                    .with_primary_label(format!(
                        "`@on(any)` means every tier, and `{}` is not discharged on every tier",
                        e.name()
                    ))
                    .with_label(span, "the definition it is placed on")
                    .with_fix(format!(
                        "`@on({})`",
                        candidates
                            .iter()
                            .map(|t| t.name())
                            .collect::<Vec<_>>()
                            .join(")` or `@on(")
                    )),
            );
        }
        return;
    }

    for e in row.atoms.iter().filter(|e| !tier.discharges(e)) {
        let alternatives: Vec<&str> = candidates
            .iter()
            .filter(|t| **t != tier)
            .map(|t| t.name())
            .collect();
        let mut d = Diagnostic::error(
            "B0401",
            format!(
                "{what} is placed on `{}`, which cannot discharge `{}`",
                tier.name(),
                e.name()
            ),
            tier_span,
        )
        .with_primary_label(format!("`{}` cannot do this", tier.name()))
        .with_label(span, "the definition it is placed on")
        .with_note(match e {
            Effect::Ingress => {
                "`ingress` is the merge point: it admits time and nondeterminism, and only the \
                 server holds it"
            }
            Effect::Durable => {
                "`durable` is the log: placing it on the client would ship the database to the \
                 browser"
            }
            Effect::Dom => "`dom` touches the document, which only the client has",
            Effect::Nondet => {
                "minting ids or reading a clock is not replayable, so the fold engine refuses it"
            }
            other => match other.family() {
                "net.out" => {
                    "a browser can only reach its own origin; any other host is the server's to call"
                }
                "net.in" => "only the server accepts connections",
                "cap" => "a capability is held where sessions are minted, which is the server",
                "env" | "fs" => "there is no process environment or filesystem in a browser",
                "external.read" | "external.write" => {
                    "an external store is reached from the server, never from a browser"
                }
                _ => "this tier cannot discharge that effect",
            },
        });
        match why {
            Why::Solved => {
                d = d.with_note(
                    "this placement was solved rather than written, so a diagnostic here is a \
                     compiler defect and worth reporting",
                );
            }
            Why::Structural(note) => d = d.with_note(note),
            Why::Annotated => {}
        }
        // Only worth suggesting where an annotation could actually have made the difference.
        if let ([only], Why::Annotated | Why::Solved) = (alternatives.as_slice(), why) {
            d = d.with_fix(format!("`@on({only})` discharges everything this needs"));
        }
        diags.push(d);
    }
}

/// Every function argument of a `fold` anywhere in an expression.
fn fold_functions(c: &crate::core::Core) -> Vec<&crate::core::Core> {
    use crate::core::{CoreKind, Prim};
    let mut out = Vec::new();
    if let CoreKind::Prim { op, args } = &c.kind {
        if *op == Prim::Fold {
            if let Some(f) = args.first() {
                out.push(f);
            }
        }
        for a in args {
            out.extend(fold_functions(a));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{check_str, compile_str};

    fn errors(src: &str) -> Vec<&'static str> {
        let (_, d, _) = compile_str("t.beck", src);
        d.iter().map(|x| x.code).collect()
    }

    /// The solved tier of every definition and signal.
    fn solved(src: &str) -> BTreeMap<String, Tier> {
        let (program, d, map) = check_str("t.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        solve(&program, None)
            .tiers
            .into_iter()
            .map(|(k, t)| (k.to_string(), t))
            .collect()
    }

    /// The sketch, with every placement annotation removed.
    fn bare_sketch() -> String {
        let bare = crate::split::tests::TODO
            .replace("@on(server)\n", "")
            .replace("@on(data)\n", "")
            .replace("@on(client)\n", "");
        assert!(
            !bare.contains("@on"),
            "the annotations must actually be gone"
        );
        bare
    }

    const DOMAIN: &str = "\
union Event:
    Added(id: Str, text: Str)

model State:
    count: Int

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(count=(s.count + 1))
";

    #[test]
    fn a_durable_fold_on_the_client_is_rejected_by_name() {
        let src = format!(
            "{DOMAIN}
@on(client)
todos: Signal[State] = durable(fold(apply_event, State(count=0), events))

@on(server)
events: Stream[Event] = merge_clients()
"
        );
        assert!(errors(&src).contains(&"B0401"), "{:?}", errors(&src));
    }

    #[test]
    fn ingress_on_the_server_is_accepted() {
        let src = format!(
            "{DOMAIN}
@on(server)
proposals: Stream[Proposal] = merge_clients()
"
        );
        let codes = errors(&src);
        assert!(
            !codes.iter().any(|c| c.starts_with("B04")),
            "unexpected placement errors: {codes:?}"
        );
    }

    #[test]
    fn an_effectful_declaration_with_no_placement_is_solved_rather_than_refused() {
        // Phase 1 refused this and suggested an annotation. Phase 2 works it out: `ingress` is
        // discharged by exactly one tier, so there was never anything to ask about.
        let src = format!(
            "{DOMAIN}
proposals: Stream[Proposal] = merge_clients()
"
        );
        assert_eq!(solved(&src).get("signal/proposals"), Some(&Tier::Server));
    }

    #[test]
    fn two_merge_points_are_rejected() {
        let src = format!(
            "{DOMAIN}
@on(server)
a: Stream[Proposal] = merge_clients()

@on(server)
b: Stream[Proposal] = merge_clients()
"
        );
        assert!(errors(&src).contains(&"B0403"), "{:?}", errors(&src));
    }

    #[test]
    fn a_fold_that_reaches_nondeterminism_through_a_function_is_rejected() {
        let src = crate::split::tests::TODO.replace(
            "return s.with(todos=map_remove(s.todos, id))",
            "return s.with(todos=map_remove(s.todos, uuid()))",
        );
        let (_, d, _) = compile_str("t.beck", &src);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0402"), "got {codes:?}");
    }

    #[test]
    fn a_fold_that_reaches_nondeterminism_two_calls_deep_is_rejected() {
        // Phase 1 could not see this. Its collection consulted each callee's *declared* effects, so
        // `stamp` — which declares nothing — hid the clock behind it and the fold looked pure.
        // Inference is what makes §3.7's rule bite at a distance.
        let src = crate::split::tests::TODO
            .replace(
                "def toggle(s: State, id: Id) -> State:",
                "def stamp(id: Id) -> Id:\n    return Id(value=uuid())\n\n\
                 def toggle(s: State, id: Id) -> State:",
            )
            .replace(
                "return s.with(todos=map_remove(s.todos, id))",
                "return s.with(todos=map_remove(s.todos, stamp(id)))",
            );
        let (_, d, _) = compile_str("t.beck", &src);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0402"), "got {codes:?}");
    }

    #[test]
    fn the_sketch_places_itself_with_no_annotations_at_all() {
        // The exit criterion on the canonical program: strip every `@on(...)`, and the solver has
        // to land on a placement that runs — the log at the data tier, ingress at the server, the
        // page at the browser, and every pure definition unplaced so it compiles to both sides.
        let t = solved(&bare_sketch());
        assert_eq!(t.get("signal/proposals"), Some(&Tier::Server), "{t:?}");
        assert_eq!(t.get("signal/todos"), Some(&Tier::Data), "{t:?}");
        assert_eq!(t.get("signal/page"), Some(&Tier::Client), "{t:?}");
        assert_eq!(t.get("def/view"), Some(&Tier::Any), "{t:?}");
        assert_eq!(t.get("def/apply_event"), Some(&Tier::Any), "{t:?}");
        assert_eq!(t.get("def/validate"), Some(&Tier::Any), "{t:?}");
    }

    #[test]
    fn a_named_host_is_the_servers_to_reach_and_the_own_origin_is_a_real_choice() {
        // `net.out(origin)` is the sharp case for the cost model: a browser and a server can both
        // make that call, so nothing but the callers decides.
        let src = "\
def ping() -> Str uses net.out(origin):
    return \"pong\"

def dial() -> Str uses net.out(payments.example.com):
    return \"charged\"
";
        let t = solved(src);
        assert_eq!(t.get("def/dial"), Some(&Tier::Server));
        assert!(
            matches!(t.get("def/ping"), Some(Tier::Client) | Some(Tier::Server)),
            "own-origin is dischargeable on both: {t:?}"
        );
    }

    #[test]
    fn the_solution_is_deterministic() {
        let bare = bare_sketch();
        let first = solved(&bare);
        for _ in 0..8 {
            assert_eq!(solved(&bare), first, "§3.4: same input, same solution");
        }
    }

    #[test]
    fn a_lock_round_trips_and_disagreement_is_reported_as_churn() {
        let (program, _, _) = check_str("t.beck", &bare_sketch());
        let solution = solve(&program, None);
        let lock = Lock::of(&solution);
        assert_eq!(
            Lock::from_json(&lock.to_json())
                .expect("the lock parses")
                .tiers,
            lock.tiers
        );

        // A lock is a tie-break and a record, never an override: one that disagrees with a strictly
        // cheaper placement loses, and the disagreement is reported instead of being absorbed.
        let mut stale = lock.clone();
        stale.tiers.insert("signal/todos".into(), Tier::Server);
        let again = solve(&program, Some(&stale));
        assert_eq!(again.tiers[&Key::Signal("todos".into())], Tier::Data);
        assert!(
            again
                .churn
                .iter()
                .any(|(k, was, now)| k.to_string() == "signal/todos"
                    && *was == Tier::Server
                    && *now == Tier::Data),
            "churn must name what moved: {:?}",
            again.churn
        );
    }

    #[test]
    fn the_lock_settles_a_tie_that_the_cost_model_cannot() {
        // Two placements that cost exactly the same are exactly where a one-line edit could
        // otherwise re-place unrelated code. The lock is what stops that, and this is the case it
        // exists for.
        let src = "\
def ping() -> Str uses net.out(origin):
    return \"pong\"
";
        let (program, _, _) = check_str("t.beck", src);
        let free = solve(&program, None);
        let other = if free.tiers[&Key::Def("ping".into())] == Tier::Server {
            Tier::Client
        } else {
            Tier::Server
        };
        let mut lock = Lock::of(&free);
        lock.tiers.insert("def/ping".into(), other);
        let relocked = solve(&program, Some(&lock));
        if free.ties.iter().any(|(k, _)| k.to_string() == "def/ping") {
            assert_eq!(
                relocked.tiers[&Key::Def("ping".into())],
                other,
                "on a tie the lock decides, so yesterday's answer survives today's edit"
            );
        }
    }

    #[test]
    fn a_solved_placement_is_explained_rather_than_asserted() {
        let (program, _, _) = check_str("t.beck", &bare_sketch());
        let solution = solve(&program, None);
        let todos = solution.explanation("todos").expect("todos is explained");
        assert_eq!(todos.chosen, Tier::Data);
        assert!(todos.because.contains("log"), "{}", todos.because);
        // §4.7 prints candidates with costs, including the impossible ones.
        let client = todos
            .candidates
            .iter()
            .find(|(t, _)| *t == Tier::Client)
            .expect("client is a candidate to reject");
        assert_eq!(client.1, FORBIDDEN);
        let server = todos
            .candidates
            .iter()
            .find(|(t, _)| *t == Tier::Server)
            .unwrap();
        let data = todos
            .candidates
            .iter()
            .find(|(t, _)| *t == Tier::Data)
            .unwrap();
        assert!(
            data.1 < server.1,
            "data must be cheaper: {data:?} {server:?}"
        );
        assert_eq!(solution.method, Method::Exhaustive);
    }

    #[test]
    fn an_annotation_always_wins_even_when_it_costs_more() {
        // §3.3: "with explicit `@on(...)` always available and always winning." The solver may
        // disagree; it may not overrule.
        let src = crate::split::tests::TODO
            .replace("@on(data)\ntodos", "@on(server)\ntodos")
            .replace("@on(server)\nevents", "@on(data)\nevents");
        let t = solved(&src);
        assert_eq!(t.get("signal/todos"), Some(&Tier::Server));
        assert_eq!(t.get("signal/events"), Some(&Tier::Data));
    }
}
