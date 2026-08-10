//! Who is connected now — D6's non-durable signal, and the rules that keep it out of the log.
//!
//! [`docs/96-presence-report.md`](../../../../docs/96-presence-report.md) is what this gates. The
//! shape of the suite follows the shape of the claim: presence is an input to a *view* and to
//! nothing else, so what has to be tested is (1) that a page can read it, (2) that a page which
//! reads it is woken when it changes, (3) that nothing else can read it, and (4) that the roster
//! is bounded — because it is keyed by a name the client chooses, which is
//! [`docs/84`](../../../../docs/84-a-quota-is-only-as-good-as-its-actor-report.md) §84.4 one
//! subsystem over.
//!
//! Where a refusal is asserted, the program asserting it is written to make *that* condition bite
//! and nothing else — a program refused for two reasons proves neither.

use std::sync::Arc;

use beck_core::{Tier, Value};
use beck_rt::presence::{Config, Registry};
use beck_rt::{App, AppConfig, MemoryLog};
use tokio::sync::mpsc::unbounded_channel;
use tokio_tungstenite::tungstenite::Message;

mod support;
use support::socket::{drain, Duplex};

/// The corpus program written for this feature.
const HERE: &str = include_str!("../../../corpus/32-here.beck");

/// A program whose page never mentions `presence` — the control, and the file every other harness
/// in this directory is already about.
const TODO: &str = include_str!("../../../examples/todo.beck");

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

// ---------------------------------------------------------------------------------------------
// The language: where a roster may be read, and where it may not
// ---------------------------------------------------------------------------------------------

/// `presence()` performs `cap.presence`, and no tier below the server discharges a capability —
/// so the roster places itself, with nothing written down.
#[test]
fn the_roster_places_itself_on_the_server() {
    let (program, diags, map) = beck_core::check_str("32-here.beck", HERE);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let solution = beck_core::place::solve(&program, None);
    let tiers: std::collections::BTreeMap<String, Tier> = solution
        .tiers
        .into_iter()
        .map(|(k, t)| (k.to_string(), t))
        .collect();
    assert_eq!(
        tiers.get("signal/here"),
        Some(&Tier::Server),
        "the roster is the server's: {tiers:?}"
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

/// §3.7's replay rule, as a refusal. A `validate` that decided from the roster would decide one
/// thing now and another on replay, because who was connected is written down nowhere.
#[test]
fn the_chokepoint_cannot_read_the_roster() {
    // Deliberately minimal: the *only* thing wrong here is the chokepoint's second argument.
    let src = "\
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

def validate(h: Map[Str, Int], p: Proposal) -> Result[list[Event], Rejection]:
    if map_len(h) == 0:
        return Err(error=Nope)
    return Ok(value=[Bumped])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.n)

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, here, validate)
here: Signal[Map[Str, Int]] = presence()
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(count, view)
";
    assert!(
        codes("chokepoint.beck", src).contains(&"B0515".to_string()),
        "the chokepoint reading the roster must be refused: {:?}",
        codes("chokepoint.beck", src)
    );
}

/// The same rule one step away: a chokepoint that reads a *derived* signal whose ancestor is the
/// roster. The check is reachability rather than an argument's shape, and this is the program that
/// says so — a `signal_map` in between defeats a check written the other way.
#[test]
fn the_rule_follows_the_graph_rather_than_the_argument() {
    let src = "\
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

def busy(h: Map[Str, Int]) -> Map[Str, Int]:
    return h

def validate(h: Map[Str, Int], p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Bumped])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.n)

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, crowd, validate)
here: Signal[Map[Str, Int]] = presence()
crowd: Signal[Map[Str, Int]] = signal_map(here, busy)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(count, view)
";
    assert!(
        codes("derived.beck", src).contains(&"B0515".to_string()),
        "a roster one map away from the chokepoint is still the roster: {:?}",
        codes("derived.beck", src)
    );
}

/// Mode B sends the browser the accumulator, and the roster is in neither the accumulator nor the
/// log. `B0516` is the refusal; `docs/94` §94.2's `B0514` is the other one, and this program is
/// written so that only the new one can fire — its page does not read the session.
#[test]
fn a_mode_b_component_cannot_read_the_roster() {
    let src = "\
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

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Bumped])

def show(s: State, h: Map[Str, Int]) -> Html:
    return ui:
        main:
            p: (str(s.n) + \" bumps, \" + str(map_len(h)) + \" here\")

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, count, validate)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
here: Signal[Map[Str, Int]] = presence()

@render(client)
page: Signal[Html] = map2(show, count, here)
";
    let found = codes("modeb.beck", src);
    assert!(
        found.contains(&"B0516".to_string()),
        "a Mode B page reading the roster must be refused: {found:?}"
    );
    assert!(
        !found.contains(&"B0514".to_string()),
        "this program must be refused for the roster and not for the session: {found:?}"
    );
}

/// `beck explain render`'s counterfactual has to name the reason that applies. A page reading the
/// roster cannot move to the browser whatever it does with the session, and a reader told "this
/// page is a function of the state alone, so `@render(client)` would move it" would be told
/// something that is not true of their program.
#[test]
fn explain_render_says_which_refusal_would_fire() {
    let placed = compile("32-here.beck", HERE);
    let bundle = beck_core::bundle::Bundle::of(&placed);
    let text = placed.render.explain(&bundle);
    assert!(
        text.contains("B0516"),
        "the counterfactual does not name the refusal that would fire:\n{text}"
    );
}

/// A `durable` roster is refused by the rule that was already there — only a fold has an
/// accumulator to persist. Asserted so that "presence cannot be made durable" is a test rather
/// than a sentence in a report.
#[test]
fn the_roster_cannot_be_made_durable() {
    let src = "\
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

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Bumped])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.n)

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, count, validate)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
kept: Signal[Map[Str, Int]] = durable(presence())
page: Signal[Html] = per_session(count, view)
";
    assert!(
        codes("durable.beck", src).contains(&"B0502".to_string()),
        "{:?}",
        codes("durable.beck", src)
    );
}

// ---------------------------------------------------------------------------------------------
// The plan: which side of §5.3's cut a roster is on
// ---------------------------------------------------------------------------------------------

/// Everything downstream of the roster runs per subscriber, and the reason is a clock rather than
/// a privacy rule: the shared dataflow is versioned by the log's `seq`, and the roster moves when
/// the log does not.
#[test]
fn everything_below_the_roster_is_per_subscriber() {
    let placed = compile("32-here.beck", HERE);
    let plan = beck_core::plan::Plan::compile(&placed);
    let presence = plan.presence;
    assert!(
        matches!(plan.nodes[presence].op, beck_core::plan::Op::Presence),
        "the plan's `presence` field is the roster source"
    );
    assert!(
        plan.nodes[presence].per_session,
        "the roster source itself runs per subscriber"
    );
    assert!(
        plan.nodes[presence].consumers > 0,
        "this program's page reads the roster, so the source has a consumer"
    );
    // …and the accumulator's own source is still shared, so this is a cut rather than a surrender.
    assert!(
        !plan.nodes[plan.state].per_session,
        "the accumulator is still shared between subscribers"
    );
}

/// The differential the engine is held to, over the input the engine had never had: a maintained
/// render against a *changed roster* is the page a full recompute produces.
///
/// Written as its own test rather than folded into `incremental_engine.rs` because the thing being
/// varied is not the state — every other differential in this tree moves the accumulator and holds
/// the session still, and this holds the accumulator still and moves the roster.
#[test]
fn a_maintained_page_agrees_with_a_recompute_as_the_roster_moves() {
    let rt = runtime("32-here.beck", HERE);
    let state = rt.initial_state().expect("an initial state");
    let session = rt.session("ana");
    let mut engine = rt.view_engine().expect("an engine");

    let rosters = [
        beck_core::edge::presence([("ana", 1)]),
        beck_core::edge::presence([("ana", 1), ("bo", 1)]),
        beck_core::edge::presence([("ana", 2), ("bo", 1), ("cy", 1)]),
        beck_core::edge::presence([("ana", 1)]),
        beck_core::edge::presence([]),
    ];
    for here in &rosters {
        let maintained = engine
            .render(&state, &session, here)
            .expect("a maintained render");
        let Value::Html(maintained) = maintained else {
            panic!("the engine produced something that is not a page")
        };
        let recomputed = rt.view_with(&state, "ana", here).expect("a recompute");
        assert_eq!(
            maintained.render(),
            recomputed.render(),
            "the maintained page and the recomputed one disagree for {}",
            here.display()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The runtime: a connection joins the roster, and a page that asks is told
// ---------------------------------------------------------------------------------------------

/// Two connections, and the first one's page changes when the second arrives and again when it
/// goes — with nothing appended to the log in between.
#[tokio::test]
async fn a_second_connection_moves_the_first_ones_page() {
    let app = App::start(
        runtime("32-here.beck", HERE),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("the app starts");

    let (ana_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut ana_rx) = unbounded_channel::<Message>();
    let ana = tokio::spawn(beck_rt::session::run(
        app.clone(),
        Duplex {
            out: server_tx,
            inbox: server_rx,
        },
    ));
    ana_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","actor":"ana"})
                .to_string()
                .into(),
        ))
        .expect("hello");

    let opening = drain(&mut ana_rx).await;
    let first = opening
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a first frame")
        .to_string();
    assert!(
        first.contains("1 here"),
        "a connecting client is in its own first page: {first}"
    );

    // A second connection. Nothing is proposed, so `seq` does not move — the only thing that has
    // happened is a socket.
    let head = app.head();
    let (bo_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut bo_rx) = unbounded_channel::<Message>();
    let bo = tokio::spawn(beck_rt::session::run(
        app.clone(),
        Duplex {
            out: server_tx,
            inbox: server_rx,
        },
    ));
    bo_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s2","actor":"bo"})
                .to_string()
                .into(),
        ))
        .expect("hello");
    let _ = drain(&mut bo_rx).await;

    let arrived = drain(&mut ana_rx).await;
    let patch = arrived
        .iter()
        .find(|m| m["t"] == "p")
        .unwrap_or_else(|| panic!("ana was not told that bo arrived: {arrived:?}"))
        .to_string();
    assert!(
        patch.contains("2 here"),
        "the patch does not carry the new count: {patch}"
    );
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
        patch.contains("1 here"),
        "the patch does not carry the roster after the leave: {patch}"
    );
    assert_eq!(app.head(), head, "still nothing in the log");

    drop(ana_tx);
    let _ = ana.await;
}

/// The control, and the property that keeps this feature from costing every program something: a
/// page that never mentions `presence` is not re-rendered when somebody connects.
///
/// It is a compile-time fact — `Roles::view_reads_presence` — so the subscription does not even
/// hold a receiver, and this asserts the consequence rather than the flag: no frame reaches a
/// connected client when a second one arrives.
#[tokio::test]
async fn a_program_that_never_asks_is_not_woken_by_a_connection() {
    let app = App::start(
        runtime("examples/todo.beck", TODO),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("the app starts");
    assert!(
        !app.runtime().placed().roles.view_reads_presence,
        "the control program must not read the roster"
    );

    let (ana_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut ana_rx) = unbounded_channel::<Message>();
    let ana = tokio::spawn(beck_rt::session::run(
        app.clone(),
        Duplex {
            out: server_tx,
            inbox: server_rx,
        },
    ));
    ana_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","actor":"ana"})
                .to_string()
                .into(),
        ))
        .expect("hello");
    let opening = drain(&mut ana_rx).await;
    assert!(opening.iter().any(|m| m["t"] == "w"), "no welcome");

    let (bo_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut bo_rx) = unbounded_channel::<Message>();
    let bo = tokio::spawn(beck_rt::session::run(
        app.clone(),
        Duplex {
            out: server_tx,
            inbox: server_rx,
        },
    ));
    bo_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s2","actor":"bo"})
                .to_string()
                .into(),
        ))
        .expect("hello");
    let _ = drain(&mut bo_rx).await;

    // The roster moved — bo is in it — and ana heard nothing, because ana's page is not a function
    // of it.
    assert_eq!(
        app.presence().here(),
        2,
        "both connections are in the roster"
    );
    let after = drain(&mut ana_rx).await;
    assert!(
        after.is_empty(),
        "a program that never reads the roster must not re-render for a connection: {after:?}"
    );

    drop(ana_tx);
    drop(bo_tx);
    let _ = ana.await;
    let _ = bo.await;
}

/// The roster is keyed by a string the client chooses, so what stops it is a capacity.
///
/// §84.4's finding is that a per-actor structure is worth what the actor is worth: under
/// `DevIdentity` the actor is whatever the connection said, so this is the same denial of service
/// the quota exists to prevent, one subsystem over. The bound makes presence *under-report* rather
/// than grow, and this asserts both halves of that — the ceiling, and the count of what it refused.
#[test]
fn the_roster_is_bounded_and_says_what_it_refused() {
    let registry = Registry::new(Config { capacity: 3 });
    let mut held = Vec::new();
    for i in 0..10 {
        held.push(registry.join(&format!("client-{i}")));
    }
    assert_eq!(registry.here(), 3, "the roster stops at its capacity");
    assert_eq!(registry.refused(), 7, "and counts what it turned away");
    let value = registry.value();
    assert_eq!(
        value.as_map().expect("a map").len(),
        3,
        "what a page renders is the bounded roster"
    );
    drop(held);
    assert_eq!(registry.here(), 0, "and every guard still removes itself");
}

/// The `App`'s own roster is the one a page renders against, and it is empty until somebody
/// connects. A process that has just recovered from the log has nobody connected to it, which is
/// the whole difference between this and a fold.
#[tokio::test]
async fn a_recovered_process_starts_with_nobody_in_it() {
    let store = Arc::new(MemoryLog::new());
    let app = App::start(
        runtime("32-here.beck", HERE),
        store.clone(),
        AppConfig::default(),
    )
    .await
    .expect("the app starts");
    let guard = app.presence().join("ana");
    app.propose(
        "c1".to_string(),
        "ana",
        Value::data(
            Arc::from("Command"),
            Some(Arc::from("Post")),
            beck_core::core::Fields::from_iter([(Arc::from("text"), Value::str_("hello"))]),
        ),
    )
    .await
    .expect("the command is accepted");
    assert_eq!(app.presence().here(), 1);
    drop(guard);

    // A second process over the same log: the note is there and the roster is not.
    let again = App::start(runtime("32-here.beck", HERE), store, AppConfig::default())
        .await
        .expect("the app recovers");
    assert_eq!(again.head(), 1, "the log survived");
    assert_eq!(again.presence().here(), 0, "the roster did not");
    assert_eq!(
        again.here().as_map().expect("a roster").len(),
        0,
        "and the value a page would render against is the empty one"
    );
    let page = again.render("ana").await.expect("a page").render();
    assert!(
        page.contains("0 here"),
        "a page rendered with nobody connected says so: {page}"
    );
}

// ---------------------------------------------------------------------------------------------
// The cost: what a connection is worth, at two sizes
// ---------------------------------------------------------------------------------------------

/// A page of the roster costs the **roster** and not the accumulator.
///
/// The shape that would be wrong is a page whose re-render on every connection walked the state:
/// connections are the one input that moves without an event, so a roster change that cost `O(the
/// accumulator)` would make connecting to a large application quadratic in the number of people
/// doing it. `32-here.beck` looks each connected actor's note up by key, so what it costs is the
/// number of people in the room.
///
/// Gated on a **shape** rather than a rate, per `AGENTS.md`: the same constant has to hold at both
/// accumulator sizes, and the budget is the evaluator's own step count, which is deterministic and
/// has no clock in it (`docs/72` made it charge for work). Measured at **112 steps** for a roster
/// of two and **298** for a roster of eight — the same two numbers at 200 notes and at 1,600, to
/// the step, which is what "does not grow with the accumulator" means. The constants below are
/// those with room for a node or two.
#[test]
fn a_page_of_the_roster_costs_the_roster_and_not_the_accumulator() {
    const ROSTER_OF_TWO: u64 = 130;
    const ROSTER_OF_EIGHT: u64 = 330;
    let placed = compile("32-here.beck", HERE);
    let names: Vec<String> = (0..8).map(|i| format!("actor{i}")).collect();

    for notes in [200usize, 1_600] {
        let state = notes_state(&placed, notes);
        for (people, budget) in [(2usize, ROSTER_OF_TWO), (8, ROSTER_OF_EIGHT)] {
            let here =
                beck_core::edge::presence(names.iter().take(people).map(|n| (n.as_str(), 1i64)));
            let backend = beck_eval::backend_with_fuel(&placed, budget);
            let rt = beck_rt::Runtime::new(placed.clone(), backend).expect("it prepares");
            let page = rt.view_with(&state, "actor0", &here);
            assert!(
                page.is_ok(),
                "a roster of {people} took more than {budget} steps to render over {notes} notes, \
                 which is the shape of a page that walks the accumulator when somebody connects: \
                 {page:?}"
            );
        }
    }
}

/// An accumulator holding `n` notes, one per actor.
fn notes_state(placed: &beck_core::Placed, n: usize) -> Value {
    let backend = beck_eval::backend(placed);
    let rt = beck_rt::Runtime::new(placed.clone(), backend).expect("it prepares");
    let mut state = rt.initial_state().expect("an initial state");
    for i in 0..n {
        let event = Value::data(
            Arc::from("Event"),
            Some(Arc::from("Posted")),
            beck_core::core::Fields::from_iter([(
                Arc::from("text"),
                Value::str_(format!("note {i}")),
            )]),
        );
        let envelope = beck_rt::Envelope {
            seq: i as u64,
            at: beck_rt::Instant(i as i64),
            actor: format!("actor{i}"),
            body: event.clone(),
        };
        state = rt.fold(&state, &envelope, event).expect("folding");
    }
    state
}
