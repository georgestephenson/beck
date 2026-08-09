//! The signal graph a devtools panel draws, as JSON.
//!
//! [`docs/08-roadmap.md`](../../../../../docs/08-roadmap.md) §8.6 asks for a devtools view of "the
//! signal graph, patch traffic and pending state". Two of those three are the client's own and are
//! measured where they happen ([`crate::PATCH_CLIENT`]); this is the third, and it is the only one
//! the browser cannot know, because the signal graph is a fact about the *program* and a Mode A
//! client is never sent one.
//!
//! # It is a projection, not a second account
//!
//! Everything here is read off the running program: the declared signals and their edges come from
//! [`beck_core::graph`], the maintained/recomputed verdict from [`beck_core::incremental`] — which
//! is what `beck explain incremental` prints — and the operator counts from the plan the engine
//! executes. Nothing is recomputed for the panel and nothing is stored for it, so a panel cannot
//! describe a program this process is not running. That is
//! [`docs/88`](../../../../../docs/88-read-models-and-pgwire-report.md)'s argument for the read
//! models applied to a third kind of reader: the cheapest way for a view of a thing to be right is
//! for it to be the thing.
//!
//! # What it does not carry
//!
//! No source, no `Core`, no types, no state. A panel says what the shape of the program is and how
//! its view is maintained; it is not a debugger and it does not put the accumulator on the wire —
//! which is the one thing here that would be a disclosure, since a Mode A page is precisely the
//! part of the state its viewer is allowed to see.

use std::sync::OnceLock;

use beck_core::incremental;
use beck_core::plan::Plan;
use beck_core::Placed;
use serde_json::{json, Value};

/// The document the panel fetches, built once.
///
/// The program does not change while the process runs, so this is [`mod@crate::dash`]'s rule
/// applied to a second endpoint: every answer is `O(size of the answer)` and the assessment behind
/// it — which walks the program — happens once rather than once per panel.
pub fn document(placed: &Placed, plan: &Plan) -> &'static str {
    static ONCE: OnceLock<String> = OnceLock::new();
    ONCE.get_or_init(|| of(placed, plan).to_string())
}

/// The graph, the verdicts and the plan's shape, for one program.
pub fn of(placed: &Placed, plan: &Plan) -> Value {
    let graph = &placed.graph;
    let verdicts = incremental::verdicts(placed);
    let (maintained, recomputed) = plan.counts();

    let nodes: Vec<Value> = graph
        .order()
        .into_iter()
        .map(|id| {
            let node = graph.node(id);
            json!({
                "id": id,
                "label": node.label.as_ref(),
                "op": node.op.name(),
                "tier": node.tier.name(),
                "inputs": node.inputs,
                "verdict": verdicts.get(&node.label).map(incremental::Verdict::name),
            })
        })
        .collect();

    json!({
        "program": placed.program.name,
        "wire": placed.wire_id,
        "mode": placed.render.mode.letter(),
        "page": placed.roles.page_name.as_ref(),
        // What the page may be a function of, in the vocabulary `beck explain render` uses. A
        // panel showing a route that does not change the page should say which of the two it is.
        "reads": placed.render.uses.describe(),
        "nodes": nodes,
        "plan": {
            "operators": plan.nodes.len(),
            "maintained": maintained,
            "recomputed": recomputed,
            "per_session": plan.nodes.iter().filter(|n| n.per_session).count(),
        },
    })
}
