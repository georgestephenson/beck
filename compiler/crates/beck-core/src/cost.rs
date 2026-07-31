//! The placement cost model — §3.4's node costs, edge costs and byte estimates.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.4:
//!
//! > **Node costs**: ∞ for forbidden tiers; tier-specific compute cost otherwise (client CPU is
//! > expensive and untrusted; fold-engine compute is cheap and adjacent to state).
//! > **Edge costs**: for each *signal edge or call* that crosses tiers, `latency + bytes × unit`,
//! > bytes estimated from row types.
//!
//! Everything here is an **integer**. Not for speed — the graphs are tiny — but because §3.4's
//! first guardrail is determinism, and a cost model in floating point makes "these two placements
//! cost the same" a question about rounding. Costs are in hundredths of a notional millisecond, so
//! a latency of 25 ms is `2_500`.
//!
//! # The numbers, and where they come from
//!
//! They are not measured; they are *ordered*, and the ordering is what a placement decision reads.
//! Every one is a ratio the design already states, so the model can be argued with rather than
//! tuned in the dark:
//!
//! | quantity | value | from |
//! |---|---|---|
//! | compute weight, data | 1 | "fold-engine compute is cheap and adjacent to state" (§3.4) |
//! | compute weight, server | 2 | the middle |
//! | compute weight, client | 8 | "client CPU is expensive and untrusted" (§3.4) |
//! | latency, data ↔ server | 1 ms | same pod network |
//! | latency, server ↔ client | 25 ms | Phase 0's realistic RTT ([`18`](../../../../docs/18-phase-0-report.md)) |
//! | latency, data ↔ client | 26 ms | it goes through the server |
//! | bytes → cost | ×5 per byte | so a 2 KB crossing (~100 ms) outweighs an RTT, which is the
//! |   |   | trade a placement decision is actually making |
//!
//! There is no constant for "a fold that is not at the data tier". [`node_cost`] charges such a
//! fold an *edge to the log*, sized from the accumulator, because that is what it physically pays:
//! the log is at the data tier, and an accumulator kept elsewhere crosses to it on every event. A
//! constant would have been a number to tune; an edge is a number to derive.
//!
//! # The one rule worth arguing with
//!
//! A crossing is charged the **smaller** of its two endpoints' values, and that is not a
//! simplification — it is §5.1's Mode A/B question, expressed as a cost instead of as a mode.
//! Between a `Signal[State]` at the data tier and a `Signal[Html]` at the browser, the compiler may
//! either send the state and render in the browser (Mode B: 2 KB of data patches) or render first
//! and send the document (Mode A: 1 KB of DOM patches). It will pick the cheaper, so the cheaper is
//! what the boundary costs. Phase 2 only *implements* Mode A, so today the minimum is a prediction
//! rather than a choice — and it is the right prediction to be making when Phase 3 makes the choice
//! real.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ty::{Effect, Row, Tier, Ty, TyDecl};

/// Costs are in hundredths of a millisecond.
pub type Cost = i64;

/// A placement that cannot be, kept finite so that sums never overflow and comparisons stay total.
pub const FORBIDDEN: Cost = 1_000_000_000;

/// §3.4: "client CPU is expensive and untrusted; fold-engine compute is cheap and adjacent to state".
pub fn compute_weight(t: Tier) -> Cost {
    match t {
        Tier::Data => 1,
        Tier::Server => 2,
        Tier::Client => 8,
        // Unplaced code is compiled into whichever tier needs it, so it is charged to that tier's
        // own work rather than to a placement of its own (§3.3).
        Tier::Any => 0,
    }
}

/// Round-trip latency between two tiers, in hundredths of a millisecond.
pub fn latency(a: Tier, b: Tier) -> Cost {
    use Tier::*;
    match (a, b) {
        // Unplaced code crosses nothing: it is duplicated to the tier that calls it.
        (Any, _) | (_, Any) => 0,
        (x, y) if x == y => 0,
        (Data, Server) | (Server, Data) => 100,
        (Server, Client) | (Client, Server) => 2_500,
        (Data, Client) | (Client, Data) => 2_600,
        _ => 0,
    }
}

/// The cost of moving one byte across a tier boundary.
pub const BYTE_UNIT: Cost = 5;

/// The node cost of putting `row` on `tier`, given how much work the node does and — for a durable
/// fold — what its accumulator looks like.
pub fn node_cost(
    tier: Tier,
    row: &Row,
    work: i64,
    state: Option<&Ty>,
    types: &BTreeMap<Arc<str>, TyDecl>,
) -> Cost {
    if !row.atoms.iter().all(|e| tier.discharges(e)) {
        return FORBIDDEN;
    }
    let mut cost = compute_weight(tier) * work;
    // The log lives at the data tier. A fold placed anywhere else does not merely compute
    // elsewhere; it crosses to the log on every event, carrying its accumulator. That is an edge,
    // so it is priced as one.
    if row.atoms.contains(&Effect::Durable) && tier != Tier::Data {
        let bytes = state.map(|t| estimate_bytes(t, types)).unwrap_or(0);
        cost += latency(tier, Tier::Data) + bytes * BYTE_UNIT;
    }
    cost
}

/// The cost of an edge between two placed nodes.
///
/// The two types are what each *end* produces. The bytes charged are the smaller: see the module
/// docs — the compiler may compute on either side of a boundary and will send whichever is less, so
/// the boundary costs the lesser of the two.
pub fn edge_cost(
    a: Tier,
    b: Tier,
    ty_a: &Ty,
    ty_b: &Ty,
    types: &BTreeMap<Arc<str>, TyDecl>,
) -> Cost {
    if a == b || a == Tier::Any || b == Tier::Any {
        return 0;
    }
    let bytes = estimate_bytes(ty_a, types).min(estimate_bytes(ty_b, types));
    latency(a, b) + bytes * BYTE_UNIT
}

/// Estimate the wire size of a value of this type — §3.4's "bytes estimated from row types".
///
/// An estimate, and deliberately a crude one: what a placement decision needs is the *ratio*
/// between a `Map[Id, Todo]` and an `Int`, not a byte count. `ASSUMED_CARDINALITY` is the one place
/// a guess is made about data the compiler cannot see, and it is named so that it can be argued
/// with rather than discovered.
pub fn estimate_bytes(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Cost {
    fn go(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>, depth: u32) -> Cost {
        if depth > 8 {
            // A recursive type: stop, and charge it as one more element rather than diverging.
            return 32;
        }
        match ty {
            Ty::Var(_) => 16,
            Ty::Fun(..) => 0, // a function does not cross; a closure is not Sendable
            Ty::Con(name, args) => match name.as_ref() {
                Ty::BOOL => 1,
                Ty::INT | Ty::FLOAT => 8,
                Ty::STR => 32,
                Ty::UNIT => 0,
                // A rendered document, or a patch derived from one. Big enough that placing a view
                // across a boundary is visible in the model, which is the point.
                Ty::HTML => 1_024,
                Ty::ATTR => 32,
                Ty::LIST | Ty::STREAM | Ty::SIGNAL | Ty::OPTION | Ty::ENVELOPE | Ty::SECRET => {
                    let elem = args.first().map(|a| go(a, types, depth + 1)).unwrap_or(16);
                    if name.as_ref() == Ty::OPTION
                        || name.as_ref() == Ty::ENVELOPE
                        || name.as_ref() == Ty::SECRET
                    {
                        elem + 8
                    } else {
                        elem * ASSUMED_CARDINALITY
                    }
                }
                Ty::MAP => {
                    let k = args.first().map(|a| go(a, types, depth + 1)).unwrap_or(16);
                    let v = args.get(1).map(|a| go(a, types, depth + 1)).unwrap_or(16);
                    (k + v) * ASSUMED_CARDINALITY
                }
                Ty::RESULT => args
                    .iter()
                    .map(|a| go(a, types, depth + 1))
                    .max()
                    .unwrap_or(16),
                other => match types.get(other) {
                    Some(TyDecl::Model { fields, .. }) => {
                        fields.iter().map(|(_, t)| go(t, types, depth + 1)).sum()
                    }
                    // A union costs its largest variant plus a tag.
                    Some(TyDecl::Union { variants, .. }) => {
                        1 + variants
                            .iter()
                            .map(|v| v.fields.iter().map(|(_, t)| go(t, types, depth + 1)).sum())
                            .max()
                            .unwrap_or(0)
                    }
                    Some(TyDecl::Newtype { inner, .. }) => go(inner, types, depth + 1),
                    Some(TyDecl::Alias { ty, .. }) => go(ty, types, depth + 1),
                    None => 16,
                },
            },
        }
    }
    go(ty, types, 0)
}

/// How many elements a collection is assumed to hold when nothing says otherwise.
///
/// The compiler cannot know, and the honest options are to guess visibly or to pretend a `Map` is
/// the size of a pointer. `beck tune` (Phase 4) is where a measured number would replace this one.
pub const ASSUMED_CARDINALITY: Cost = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Variant;

    fn types() -> BTreeMap<Arc<str>, TyDecl> {
        BTreeMap::from([
            (
                Arc::from("Todo"),
                TyDecl::Model {
                    name: Arc::from("Todo"),
                    fields: vec![
                        (Arc::from("text"), Ty::str_()),
                        (Arc::from("done"), Ty::bool_()),
                    ],
                },
            ),
            (
                Arc::from("Command"),
                TyDecl::Union {
                    name: Arc::from("Command"),
                    variants: vec![
                        Variant {
                            name: Arc::from("Toggle"),
                            fields: vec![(Arc::from("id"), Ty::str_())],
                        },
                        Variant {
                            name: Arc::from("Add"),
                            fields: vec![
                                (Arc::from("id"), Ty::str_()),
                                (Arc::from("text"), Ty::str_()),
                            ],
                        },
                    ],
                },
            ),
        ])
    }

    #[test]
    fn a_collection_costs_more_to_cross_than_a_scalar() {
        let t = types();
        let scalar = estimate_bytes(&Ty::int(), &t);
        let record = estimate_bytes(&Ty::con("Todo"), &t);
        let collection = estimate_bytes(&Ty::map(Ty::str_(), Ty::con("Todo")), &t);
        assert!(scalar < record, "{scalar} < {record}");
        assert!(record < collection, "{record} < {collection}");
        // A union is its largest variant plus a tag, not the sum of all of them.
        assert_eq!(estimate_bytes(&Ty::con("Command"), &t), 1 + 64);
    }

    #[test]
    fn a_function_type_has_no_wire_size_because_it_never_crosses() {
        assert_eq!(
            estimate_bytes(&Ty::fun(vec![Ty::int()], Ty::int()), &types()),
            0
        );
    }

    #[test]
    fn a_recursive_type_terminates() {
        // `model Tree: kids: list[Tree]` — the estimate must stop rather than diverge.
        let t = BTreeMap::from([(
            Arc::from("Tree"),
            TyDecl::Model {
                name: Arc::from("Tree"),
                fields: vec![(Arc::from("kids"), Ty::list(Ty::con("Tree")))],
            },
        )]);
        assert!(estimate_bytes(&Ty::con("Tree"), &t) > 0);
    }

    #[test]
    fn a_forbidden_tier_costs_more_than_any_reachable_placement() {
        let t = types();
        let durable = Row::of([Effect::Durable]);
        let state = Ty::map(Ty::str_(), Ty::con("Todo"));
        assert_eq!(
            node_cost(Tier::Client, &durable, 1, Some(&state), &t),
            FORBIDDEN
        );
        // …and the log's residency makes the data tier the cheap place for a real accumulator, by a
        // margin no amount of compute closes: the charge is the accumulator's own size.
        assert!(
            node_cost(Tier::Data, &durable, 1_000, Some(&state), &t)
                < node_cost(Tier::Server, &durable, 1, Some(&state), &t)
        );
        // A trivial accumulator is a different matter, and the model says so rather than pretending
        // otherwise — which is the difference between a derived number and a tuned one.
        assert!(
            node_cost(Tier::Server, &durable, 1, Some(&Ty::int()), &t)
                < node_cost(Tier::Server, &durable, 1, Some(&state), &t)
        );
    }

    #[test]
    fn crossing_to_a_browser_costs_an_rtt_and_crossing_within_a_pod_does_not() {
        let t = types();
        assert_eq!(
            edge_cost(Tier::Data, Tier::Data, &Ty::int(), &Ty::int(), &t),
            0
        );
        assert!(
            edge_cost(Tier::Server, Tier::Client, &Ty::int(), &Ty::int(), &t)
                > edge_cost(Tier::Data, Tier::Server, &Ty::int(), &Ty::int(), &t)
        );
        // Unplaced code is duplicated rather than called across a boundary, so it crosses nothing.
        assert_eq!(
            edge_cost(Tier::Any, Tier::Client, &Ty::html(), &Ty::html(), &t),
            0
        );
    }

    #[test]
    fn a_crossing_costs_the_smaller_of_its_two_ends() {
        // §5.1's Mode A/B decision as a cost: between a big state and a smaller rendered document,
        // the boundary carries the document, because a compiler free to render on either side will.
        let t = types();
        let state = Ty::map(Ty::str_(), Ty::con("Todo"));
        let both = edge_cost(Tier::Data, Tier::Client, &state, &Ty::html(), &t);
        let doc_only = edge_cost(Tier::Data, Tier::Client, &Ty::html(), &Ty::html(), &t);
        assert_eq!(both, doc_only);
        assert!(both < edge_cost(Tier::Data, Tier::Client, &state, &state, &t));
    }
}
