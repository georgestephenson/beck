//! What everybody is doing now — the roster with a payload, and the rules that keep it out of the
//! log.
//!
//! `presence.rs` next door gates the count; this gates the value. The suite has the shape that one
//! does, because the claim has the same shape: awareness is an input to a *view* and to nothing
//! else, so what has to be tested is (1) that a page can read it, (2) that a page which reads it is
//! woken when it changes and (3) not otherwise, (4) that nothing else can read it, and (5) that the
//! roster is bounded — twice, because unlike presence it holds a value the client chose as well as
//! a name the client chose.
//!
//! Where a refusal is asserted, the program asserting it is written to make *that* condition bite
//! and nothing else — a program refused for two reasons proves neither.

use std::sync::Arc;

use beck_core::{Tier, Value};
use beck_rt::awareness::{Config, Registry};
use beck_rt::{App, AppConfig, MemoryLog};
use tokio::sync::mpsc::unbounded_channel;
use tokio_tungstenite::tungstenite::Message;

mod support;
use support::socket::{drain, Duplex};

/// The corpus program written for this feature.
const AWARE: &str = include_str!("../../../corpus/33-awareness.beck");

/// The roster without a payload — the control for everything this adds to it.
const HERE: &str = include_str!("../../../corpus/32-here.beck");

fn compile(name: &str, src: &str) -> beck_core::Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.expect("it slices")
}

fn runtime(name: &str, src: &str) -> beck_rt::Runtime {
    let placed = compile(name, src);
    let backend = beck_eval::backend(&placed);
    beck_rt::Runtime::new(placed, backend).expect("it prepares")
}

/// The codes a source refuses with.
fn codes(name: &str, src: &str) -> Vec<String> {
    let (_, diags, _) = beck_core::compile_str(name, src);
    diags
        .iter()
        .filter(|d| d.severity == beck_diag::Severity::Error)
        .map(|d| d.code.to_string())
        .collect()
}

/// A program whose only interesting line is the one the caller supplies.
///
/// Every refusal below needs a whole program around one wrong line, and writing five of them out
/// meant five chances for a second error to creep in and prove the wrong thing.
fn program(signals: &str, extra: &str) -> String {
    format!(
        "\
model State:
    n: Int

union Command:
    Bump

union Event:
    Bumped

union Rejection:
    Nope

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(n=s.n + 1)

def whereabouts(session: Session) -> Str:
    return session.path

{extra}
proposals: Stream[Proposal] = merge_clients()
{signals}
"
    )
}

// ---------------------------------------------------------------------------------------------
// The language: where a roster with a payload may be read, and where it may not
// ---------------------------------------------------------------------------------------------

/// `awareness(f)` performs `cap.presence`, and no tier below the server discharges a capability —
/// so it places itself, with nothing written down. The same derivation as `presence()`, asserted
/// separately because it is a different primitive with its own scheme.
#[test]
fn the_roster_places_itself_on_the_server() {
    let (program, diags, map) = beck_core::check_str("33-awareness.beck", AWARE);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let solution = beck_core::place::solve(&program, None);
    let tiers: std::collections::BTreeMap<String, Tier> = solution
        .tiers
        .into_iter()
        .map(|(k, t)| (k.to_string(), t))
        .collect();
    assert_eq!(
        tiers.get("signal/reading"),
        Some(&Tier::Server),
        "the awareness roster is the server's: {tiers:?}"
    );
    assert_eq!(
        tiers.get("signal/board"),
        Some(&Tier::Data),
        "the accumulator is still the data tier's: {tiers:?}"
    );
    assert_eq!(
        tiers.get("signal/page"),
        Some(&Tier::Client),
        "the page is still the browser's: {tiers:?}"
    );
}

/// §3.7's replay rule, as a refusal. What somebody was looking at when an event was recorded is
/// written down nowhere, so a `validate` that decided from it would decide one thing now and
/// another on replay.
#[test]
fn the_chokepoint_cannot_read_the_roster() {
    let src = program(
        "\
events: Stream[Event] = decide(proposals, reading, validate)
reading: Signal[Map[Str, Str]] = awareness(whereabouts)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(count, view)",
        "\
def validate(r: Map[Str, Str], p: Proposal) -> Result[list[Event], Rejection]:
    if map_len(r) == 0:
        return Err(error=Nope)
    return Ok(value=[Bumped])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.n)
",
    );
    let found = codes("chokepoint.beck", &src);
    assert!(
        found.contains(&"B0520".to_string()),
        "the chokepoint reading the awareness roster must be refused: {found:?}"
    );
}

/// The same rule one step away. The check is reachability through the graph rather than the shape
/// of `decide`'s second argument, and this is the program that says so — a `signal_map` in between
/// defeats a check written the other way, which is exactly how the four gates in
/// `docs/82` §82.10 could not have failed.
#[test]
fn the_rule_follows_the_graph_rather_than_the_argument() {
    let src = program(
        "\
events: Stream[Event] = decide(proposals, busy, validate)
reading: Signal[Map[Str, Str]] = awareness(whereabouts)
busy: Signal[Int] = signal_map(reading, map_len)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(count, view)",
        "\
def validate(busy: Int, p: Proposal) -> Result[list[Event], Rejection]:
    if busy == 0:
        return Err(error=Nope)
    return Ok(value=[Bumped])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.n)
",
    );
    let found = codes("derived.beck", &src);
    assert!(
        found.contains(&"B0520".to_string()),
        "a chokepoint one map away from the roster must still be refused: {found:?}"
    );
}

/// `@render(client)` sends the browser the accumulator, and this roster is in neither the
/// accumulator nor the log.
#[test]
fn a_mode_b_component_cannot_read_the_roster() {
    let src = program(
        "\
events: Stream[Event] = decide(proposals, count, validate)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
reading: Signal[Map[Str, Str]] = awareness(whereabouts)

@render(client)
page: Signal[Html] = map2(show, count, reading)",
        "\
def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Bumped])

def show(s: State, r: Map[Str, Str]) -> Html:
    return ui:
        main:
            p: (str(s.n) + \" bumps, \" + str(map_len(r)) + \" here\")
",
    );
    let found = codes("modeb.beck", &src);
    assert!(
        found.contains(&"B0521".to_string()),
        "a Mode B page reading the awareness roster must be refused: {found:?}"
    );
    assert!(
        !found.contains(&"B0514".to_string()),
        "this program must be refused for the roster and not for the session: {found:?}"
    );
}

/// `beck explain render`'s counterfactual has to name the reason that applies. A reader told "this
/// page is a function of the state alone, so `@render(client)` would move it" would be told
/// something that is not true of their program.
#[test]
fn explain_render_says_which_refusal_would_fire() {
    let placed = compile("33-awareness.beck", AWARE);
    let bundle = beck_core::bundle::Bundle::of(&placed);
    let text = placed.render.explain(&bundle);
    assert!(
        text.contains("B0521"),
        "the counterfactual does not name the refusal that would fire:\n{text}"
    );
}

// ---------------------------------------------------------------------------------------------
// The plan: which side of §5.3's cut a roster with a payload is on
// ---------------------------------------------------------------------------------------------

/// Everything downstream of the roster runs per subscriber, for the reason the roster next door
/// does: the shared dataflow is versioned by the log's `seq`, and this moves when the log does not.
#[test]
fn everything_below_the_roster_is_per_subscriber() {
    let placed = compile("33-awareness.beck", AWARE);
    let plan = beck_core::plan::Plan::compile(&placed);
    let aware = plan.awareness;
    assert!(
        matches!(plan.nodes[aware].op, beck_core::plan::Op::Awareness),
        "the plan's `awareness` field is the roster source"
    );
    assert!(
        plan.nodes[aware].per_session,
        "the roster source itself runs per subscriber"
    );
    assert!(
        plan.nodes[aware].consumers > 0,
        "this program's page reads the roster, so the source has a consumer"
    );
    assert!(
        !plan.nodes[plan.state].per_session,
        "the accumulator is still shared between subscribers"
    );
}

/// The differential the engine is held to, over the input the engine had never had: a maintained
/// render against a *changed awareness roster* is the page a full recompute produces.
///
/// The accumulator is held still and the roster is moved, which is the opposite of every other
/// differential in this tree — and the roster is moved in a way a count could not express, because
/// the interesting change here is a value under a key that was already there.
#[test]
fn a_maintained_page_agrees_with_a_recompute_as_the_roster_moves() {
    let rt = runtime("33-awareness.beck", AWARE);
    let state = rt.initial_state().expect("an initial state");
    let session = rt.session("ana");
    let here = beck_core::edge::presence([("ana", 1)]);
    let mut engine = rt.view_engine().expect("an engine");

    let text = |s: &str| Value::str_(s);
    let rosters = [
        beck_core::edge::awareness([("ana", text("/"))]),
        beck_core::edge::awareness([("ana", text("/")), ("bo", text("/done"))]),
        // The change a count cannot see: the same two actors, one of them somewhere else.
        beck_core::edge::awareness([("ana", text("/")), ("bo", text("/todos"))]),
        beck_core::edge::awareness([("ana", text("/archive"))]),
        beck_core::edge::no_awareness(),
    ];
    for aware in &rosters {
        let maintained = engine
            .render_all(&state, &session, &here, aware)
            .expect("a maintained render");
        let Value::Html(maintained) = maintained else {
            panic!("the engine produced something that is not a page")
        };
        let recomputed = rt
            .view_with_all(&state, "ana", &here, aware)
            .expect("a recompute");
        assert_eq!(
            maintained.render(),
            recomputed.render(),
            "the maintained page and the recomputed one disagree for {}",
            aware.display()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The runtime: a connection contributes, and a page that asks is told
// ---------------------------------------------------------------------------------------------

async fn connect(
    app: &Arc<App>,
    sub: &str,
    actor: &str,
    path: &str,
) -> (
    tokio::sync::mpsc::UnboundedSender<Message>,
    tokio::sync::mpsc::UnboundedReceiver<Message>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let (tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, rx) = unbounded_channel::<Message>();
    let task = tokio::spawn(beck_rt::session::run(
        app.clone(),
        Duplex {
            out: server_tx,
            inbox: server_rx,
        },
    ));
    tx.send(Message::Text(
        serde_json::json!({"t":"hello","sub":sub,"actor":actor,"path":path})
            .to_string()
            .into(),
    ))
    .expect("hello");
    (tx, rx, task)
}

/// The whole feature, end to end: a second connection's *route* is on the first one's page, and
/// changing it changes that page — with nothing appended to the log at any point.
///
/// This is the test that would have gone red at every stage of building this, and the one whose
/// shape is the claim: presence could have carried the second client's arrival, and nothing but
/// awareness can carry where it went afterwards.
#[tokio::test]
async fn a_second_connection_puts_its_route_on_the_first_ones_page() {
    let app = App::start(
        runtime("33-awareness.beck", AWARE),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("the app starts");

    let (ana_tx, mut ana_rx, ana) = connect(&app, "s1", "ana", "/").await;
    let opening = drain(&mut ana_rx).await;
    let first = opening
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a first frame")
        .to_string();
    assert!(
        first.contains("ana is at /"),
        "a connecting client sees its own contribution in its first page: {first}"
    );

    let head = app.head();
    let (bo_tx, mut bo_rx, bo) = connect(&app, "s2", "bo", "/done").await;
    let _ = drain(&mut bo_rx).await;

    let arrived = drain(&mut ana_rx).await;
    let patch = arrived
        .iter()
        .find(|m| m["t"] == "p")
        .unwrap_or_else(|| panic!("ana was not told that bo arrived: {arrived:?}"))
        .to_string();
    assert!(
        patch.contains("/done"),
        "the patch does not carry where bo is: {patch}"
    );

    // The half a count cannot express: nobody arrived, nobody left, and the page still moved.
    bo_tx
        .send(Message::Text(
            serde_json::json!({"t":"g","path":"/archive"})
                .to_string()
                .into(),
        ))
        .expect("nav");
    let moved = drain(&mut ana_rx).await;
    let patch = moved
        .iter()
        .find(|m| m["t"] == "p")
        .unwrap_or_else(|| panic!("ana was not told that bo navigated: {moved:?}"))
        .to_string();
    assert!(
        patch.contains("/archive"),
        "the patch does not carry bo's new route: {patch}"
    );
    assert_eq!(app.presence().here(), 2, "nobody arrived and nobody left");
    assert_eq!(app.head(), head, "nothing was appended to the log");

    // …and when bo's socket closes, the roster and ana's page follow it.
    drop(bo_tx);
    let _ = bo.await;
    let left = drain(&mut ana_rx).await;
    let patch = left
        .iter()
        .find(|m| m["t"] == "p")
        .unwrap_or_else(|| panic!("ana was not told that bo left: {left:?}"))
        .to_string();
    assert!(
        !patch.contains("/archive"),
        "bo is still on the page after leaving: {patch}"
    );
    assert_eq!(app.head(), head, "still nothing in the log");

    drop(ana_tx);
    let _ = ana.await;
}

/// The control: what a program that does **not** read the roster pays for this feature.
///
/// Three claims, and they are not equally strong — which is the point of writing them out. The
/// first is the one with teeth:
///
/// 1. **No row.** A client of such a program contributes nothing, so the registry holds nothing
///    for it. That is the per-connection memory this feature could have cost every program in the
///    tree, and a `subscribe` that joined unconditionally turns this red.
/// 2. **No frame.** Moving the roster under that subscription sends its client nothing.
/// 3. The subscription is not *woken* either — `Roles::awareness` is `None`, so it holds no
///    receiver. This is the one claim here that cannot be seen from outside, and saying so is
///    better than implying the assertion below covers it: a subscription woken for nothing still
///    renders a page identical to the last one and diffs to nothing, so claim 2 would hold even
///    with the receiver wrongly armed. What the guard saves is the render, not the frame.
#[tokio::test]
async fn a_program_that_reads_only_presence_holds_no_row_and_is_sent_nothing() {
    let app = App::start(
        runtime("32-here.beck", HERE),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("the app starts");
    assert!(
        app.runtime().placed().roles.awareness.is_none(),
        "the control program must not read the awareness roster"
    );

    let (ana_tx, mut ana_rx, ana) = connect(&app, "s1", "ana", "/").await;
    let _ = drain(&mut ana_rx).await;
    assert_eq!(
        app.presence().here(),
        1,
        "ana is in the roster this program does read"
    );
    assert_eq!(
        app.awareness().here(),
        0,
        "and holds no row in the one it does not"
    );

    // The roster moves under a subscription that does not read it: a row appears, changes, and
    // goes. Presence is untouched throughout — nobody connected and nobody left.
    let cy = app.awareness().join("cy", Value::str_("/todos"));
    assert_eq!(app.awareness().here(), 1, "the roster really did move");
    assert!(cy.publish(Value::str_("/done")));
    drop(cy);

    let after = drain(&mut ana_rx).await;
    assert!(
        after.is_empty(),
        "a program that never reads awareness must not be sent a frame when it moves: {after:?}"
    );

    drop(ana_tx);
    let _ = ana.await;
}

/// The compile-time half of the control, which is what the runtime's guard is derived from: the
/// role is present for exactly the program that reads the roster.
#[test]
fn only_a_program_that_reads_the_roster_carries_the_role() {
    assert!(
        compile("33-awareness.beck", AWARE)
            .roles
            .awareness
            .is_some(),
        "the program that reads the roster carries the function to apply"
    );
    assert!(
        compile("32-here.beck", HERE).roles.awareness.is_none(),
        "the program that reads only presence carries none"
    );
}

/// A recovered process has nobody connected to it, so its awareness roster is empty — the whole
/// difference between this and a fold, asserted rather than described.
#[tokio::test]
async fn a_recovered_process_starts_with_nobody_in_it() {
    let app = App::start(
        runtime("33-awareness.beck", AWARE),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("the app starts");
    assert_eq!(app.awareness().here(), 0, "nobody is connected yet");
    let guard = app
        .awareness()
        .join("ana", beck_core::Value::str_("/todos"));
    assert_eq!(app.awareness().here(), 1);
    drop(guard);
    assert_eq!(
        app.awareness().here(),
        0,
        "and the roster follows the socket"
    );
}

// ---------------------------------------------------------------------------------------------
// The bounds: a name the client chose, and now a value the client chose
// ---------------------------------------------------------------------------------------------

/// §82.5's finding, the half presence already has: the table is keyed by a string the client may
/// choose, so what stops it is a capacity rather than a hope.
#[test]
fn the_roster_is_bounded_and_says_what_it_refused() {
    let registry = Registry::new(Config {
        capacity: 3,
        each: 4096,
    });
    let mut held = Vec::new();
    for i in 0..10 {
        held.push(registry.join(&format!("client-{i}"), Value::str_("/")));
    }
    assert_eq!(registry.here(), 3, "the roster stops at its capacity");
    assert_eq!(registry.refused(), 7, "and counts what it turned away");
    assert_eq!(
        registry.value().as_map().expect("a map").len(),
        3,
        "what a page renders is the bounded roster"
    );
    drop(held);
    assert_eq!(registry.here(), 0, "and every guard still removes itself");
}

/// The half presence does not have, and the reason this registry is not a copy of that one: a
/// roster of *values* costs the capacity times whatever the program's `f` returns, and `f` is the
/// program's.
///
/// The gate is on the shape of the gap rather than of the fix: what would make it red is a
/// registry that accepts a contribution because it is *shaped* small — one field, one list — and
/// the value below is one field holding a megabyte.
#[test]
fn a_contribution_is_bounded_by_what_it_renders_to() {
    let registry = Registry::new(Config {
        capacity: 8,
        each: 1024,
    });
    let ana = registry.join("ana", Value::str_("/todos"));
    assert!(ana.recorded());

    // One field. A bound counting fields, or depth, or list length would let this through.
    let big = Value::List(Arc::new(vec![Value::str_("x".repeat(64)); 64]));
    assert!(
        !ana.publish(big),
        "a contribution past the size bound must be refused"
    );
    assert_eq!(registry.oversized(), 1, "and counted");
    let value = registry.value();
    let map = value.as_map().expect("a map");
    assert_eq!(
        map.get(&Value::str_("ana")).map(|v| v.display()),
        Some("/todos".to_string()),
        "the actor keeps what it last contributed, rather than losing it or gaining the big one"
    );
    assert_eq!(registry.here(), 1, "and is still connected");
}

/// A navigation to the route a client is already on republishes an equal value, and the registry
/// must not treat that as a change: it would wake every other subscriber in the process, for
/// nothing, on a message the client sends whenever a link is clicked twice.
#[test]
fn republishing_the_same_contribution_wakes_nobody() {
    let registry = Registry::new(Config::default());
    let ana = registry.join("ana", Value::str_("/todos"));
    let mut watcher = registry.watch();
    watcher.mark_unchanged();
    assert!(!ana.publish(Value::str_("/todos")), "the same route again");
    assert!(
        !watcher.has_changed().expect("live"),
        "an unchanged contribution must not wake a watcher"
    );
    assert!(ana.publish(Value::str_("/done")));
    assert!(watcher.has_changed().expect("live"));
}
