//! D30's non-durable fold: it runs, and nothing it holds reaches the log.
//!
//! [`docs/10`](../../../../docs/10-decisions.md) D30 and
//! [`docs/104`](../../../../docs/104-styling-and-the-component-library.md) §104.8's Wall 1. D1
//! provided for "high-churn ephemera get non-durable folds — same semantics, no log persistence"
//! and nothing was built for it, because the sentence named the right problem with the
//! wrong mechanism: an accumulator that merely declines to be `durable` is still folded from the
//! log, so it *is* a function of the log and replay would reproduce it whether or not anybody
//! asked. D30's correction is that **ephemerality comes from the stream** — `gestures(step, init)`
//! folds occurrences that were never proposed, never validated and never recorded.
//!
//! `DEFECTS.md::non-durable-fold` named two halves and said which would be forgotten:
//!
//! > a program with a non-durable fold runs and its page reflects it, **and** the fold's state does
//! > not appear in the log after a restart. A fix that only satisfies the first has built a durable
//! > fold with a different spelling.
//!
//! So the second half is the larger part of this file, and it is asserted four ways rather than
//! one: the log is empty, nothing is queued to send, a restart comes back to `init`, and the server
//! cannot decode a gesture at all. Any one of those alone could pass while the construct leaked.

use beck_core::{Bundle, Placed, Value};
use beck_wasm::kernel::{Client, Proposed, Viewer};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

fn compile(name: &str) -> Placed {
    let path = root().join("examples").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("examples/{name}"));
    let (placed, diags, map) = beck_core::compile_str(path.to_str().expect("utf-8"), &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// A program compiled from source, for the refusals — which are about programs that must not
/// compile, so they cannot come from a file in `examples/`.
fn check(src: &str) -> (beck_diag::Diagnostics, beck_diag::SourceMap) {
    let (_, diags, map) = beck_core::compile_str("t.beck", src);
    (diags, map)
}

fn client(placed: &Placed) -> Client {
    let bytes = Bundle::of(placed).to_bytes();
    Client::load(&bytes, Viewer::named("ana")).expect("the bundle loads")
}

fn dom(outcome: Proposed) -> Vec<beck_core::diff::Op> {
    match outcome {
        Proposed::Folded { dom } => dom,
        Proposed::Accepted { .. } => panic!("a gesture was routed as a command"),
        Proposed::Refused { why } => panic!("refused: {why}"),
    }
}

fn gesture(tag: &str) -> serde_json::Value {
    serde_json::json!({ "c": tag })
}

// -------------------------------------------------------------------------------------------
// The half that is easy to get right
// -------------------------------------------------------------------------------------------

/// **It runs, and the page reflects it.**
///
/// The weaker half of the register's gate, and the one a wrong fix would also pass — which is why
/// it is asserted about the *page* rather than about the accumulator. A construct that moved its
/// own state and rendered nothing would satisfy an accumulator check and be useless.
#[test]
fn a_gesture_moves_the_page() {
    let placed = compile("interface.beck");
    let mut c = client(&placed);
    let first = c.repaint().expect("a first paint");
    assert!(!first.is_empty(), "the first paint is the whole frame");

    let before = c.interface().clone();
    // A gesture whose effect the page shows with an *empty* board: sorting no cards renders the
    // same page, and a test that asserted on it would be asserting on the fixture.
    let ops = dom(c.propose("g1", &serde_json::json!({"c": "Inspect", "id": 2}), 0));
    assert!(
        !ops.is_empty(),
        "a gesture that changes the interface has to produce a DOM patch — if this is empty, \
         `paint`'s short-circuit is comparing the state and the freshness and not the interface, \
         and the panel opens in the accumulator while the screen stays shut"
    );
    assert_ne!(&before, c.interface(), "the accumulator moved");
}

/// A gesture that changes nothing renders nothing — `docs/94` §94.12's shortcut, kept.
///
/// The other side of the test above, and the reason the guard compares rather than forcing: a
/// `force` on every gesture would repaint for `ShowEverything` when everything is already shown.
#[test]
fn a_gesture_that_changes_nothing_costs_no_render() {
    let placed = compile("interface.beck");
    let mut c = client(&placed);
    c.repaint().expect("a first paint");
    // `only` starts empty, so this is the gesture that asks for what is already true.
    let ops = dom(c.propose("g1", &gesture("ShowEverything"), 0));
    assert!(
        ops.is_empty(),
        "an interface state that did not move must not repaint"
    );
}

// -------------------------------------------------------------------------------------------
// The half the register said would be forgotten
// -------------------------------------------------------------------------------------------

/// **Nothing a gesture touches reaches the log**, asserted four ways.
///
/// The register's second half. Each assertion below could pass while the construct leaked through
/// one of the others, which is the whole reason there are four:
///
/// 1. the log is empty — the direct claim;
/// 2. nothing is queued to send — a leak that was merely *deferred* would pass (1);
/// 3. a restart comes back to `init` — a leak into the snapshot would pass (1) and (2);
/// 4. the server cannot decode a gesture — a leak by some *other* path would pass all three.
#[test]
fn a_gesture_reaches_neither_the_log_nor_the_wire() {
    let placed = compile("interface.beck");
    let rt = beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed))
        .expect("the program prepares");
    let mut c = client(&placed);
    c.repaint().expect("a first paint");

    for g in ["SortByTitle", "ShowOnly", "Inspect", "ClosePanel"] {
        let payload = match g {
            "ShowOnly" => serde_json::json!({"c": "ShowOnly", "tag": "bug"}),
            "Inspect" => serde_json::json!({"c": "Inspect", "id": 3}),
            other => gesture(other),
        };
        dom(c.propose("g", &payload, 0));
    }
    assert_ne!(
        c.interface(),
        &rt.gestures_init().clone(),
        "four gestures moved the interface"
    );

    // 1. The log. `seq` is what the client has been told about, and it has been told nothing,
    //    because nothing was sent.
    assert_eq!(c.seq(), 0, "no gesture advanced the log");

    // 2. The queue — what a disconnected client owes the server. A gesture that was merely
    //    *deferred* rather than kept would sit here.
    assert!(
        c.queued().is_empty(),
        "a gesture must not be queued for sending: there is no `eventually` for a thing the \
         server has no decoder for"
    );

    // 3. A restart. The snapshot is D7 rung 2 — what a browser stores so a reload is not a fresh
    //    start — and interface state is deliberately not in it.
    let snapshot = c.snapshot().expect("a client with a state snapshots");
    let mut restarted = client(&placed);
    restarted
        .restore(snapshot)
        .expect("a snapshot of the same program restores");
    assert_eq!(
        restarted.interface(),
        rt.gestures_init(),
        "interface state does not survive a restart — that is the construct, not a limitation \
         of it"
    );

    // 4. The server's own decoder. Even handed a gesture directly, the command schema cannot read
    //    it: §3.5's write surface is the `Command` union and a gesture is not in it.
    let refused =
        beck_core::command::Schema::of(&placed).decode(&serde_json::json!({"c": "SortByTitle"}));
    assert!(
        refused.is_err(),
        "the server's command decoder must not accept a gesture"
    );
}

/// The durable accumulator — what the state digest is *of* — does not move under a gesture.
///
/// `DEFECTS.md::non-durable-fold` said the construct "needs an answer to what the state digest
/// covers", and D30's answer is that the question dissolves rather than being settled: the digest
/// covers the durable accumulator exactly as before, and a gesture was never a candidate for it.
/// This asserts the dissolution — the state a replay would reproduce is the same state after four
/// gestures as before them, so `replay.rs`'s `digest(replayed) == digest(live)` cannot be affected
/// by one.
#[test]
fn gestures_do_not_move_the_state_a_replay_reproduces() {
    let placed = compile("interface.beck");
    let mut c = client(&placed);

    let before = c.state().expect("a derived state");
    for g in ["SortByTitle", "ClosePanel", "ShowEverything", "SortByAge"] {
        dom(c.propose("g", &gesture(g), 0));
    }
    let after = c.state().expect("a derived state");
    assert_eq!(
        before, after,
        "D3's invariant is untouched rather than weakened: replay reproduces everything that was \
         ever in the log, and no gesture ever was"
    );
    assert_ne!(
        c.interface(),
        &Value::Unit,
        "the gestures did land somewhere — this test would pass vacuously if they did not"
    );
}

// -------------------------------------------------------------------------------------------
// The refusals, which are the more interesting half of the design
// -------------------------------------------------------------------------------------------

/// A base program with a gesture fold, for the refusals to edit.
const PROGRAM: &str = r#"
model State:
    n: Int

model Ui:
    open: Bool

union Command:
    Bump

union Event:
    Bumped

union Rejection:
    Never

union Nudge:
    Open
    Close

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Bumped:
            return s.with(n=(s.n + 1))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Bump:
            return Ok(value=[Bumped])

def apply_nudge(u: Ui, g: Nudge) -> Ui:
    match g:
        case Open:
            return u.with(open=True)
        case Close:
            return u.with(open=False)

def view(s: State, u: Ui) -> Html:
    return ui:
        main:
            p: str(s.n)
            button(on_click=Open): "open"
            if u.open:
                div: "open"

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, count, validate)
count: Signal[State] = durable(fold(apply_event, State(n=0), events))
panel: Signal[Ui] = gestures(apply_nudge, Ui(open=False))

@render(client)
page: Signal[Html] = map2(view, count, panel)
"#;

fn refuses(code: &str, edit: &[(&str, &str)]) {
    let mut src = PROGRAM.to_string();
    for (from, to) in edit {
        assert!(src.contains(from), "the base program has no `{from}`");
        src = src.replace(from, to);
    }
    let (diags, map) = check(&src);
    let rendered = format!("{diags:?}");
    assert!(
        rendered.contains(code),
        "expected {code}, got:\n{}",
        diags.render(&map)
    );
}

/// The base program compiles — without which every refusal below could pass for the wrong reason.
#[test]
fn the_base_program_compiles() {
    let (diags, map) = check(PROGRAM);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
}

/// `B0522` — a page that reads interface state cannot render on the server.
#[test]
fn a_page_with_interface_state_cannot_render_on_the_server() {
    refuses("B0522", &[("@render(client)\n", "")]);
}

/// `B0523` — the chokepoint cannot decide from a gesture.
///
/// The strongest of the three "not in the log" rules: `presence` and `awareness` are facts the
/// server holds and does not record, and a gesture is a fact the server never had.
#[test]
fn the_chokepoint_cannot_decide_from_interface_state() {
    refuses(
        "B0523",
        &[(
            "events: Stream[Event] = decide(proposals, count, validate)",
            "events: Stream[Event] = decide(proposals, map2(pair, count, panel), validate)",
        ),
        (
            "def view(s: State, u: Ui) -> Html:",
            "def pair(s: State, u: Ui) -> State:\n    if u.open:\n        return s\n    return s\n\ndef view(s: State, u: Ui) -> Html:",
        )],
    );
}

/// `B0524` — a variant may not be both a command and a gesture.
#[test]
fn a_variant_cannot_be_both_a_command_and_a_gesture() {
    // Rename the *command*, not the gesture: the page's handler names `Open`, so renaming that
    // one would fail at `B0340` before the graph is built and this test would pass for the wrong
    // reason. `the_base_program_compiles` is what makes that distinction checkable at all.
    refuses(
        "B0524",
        &[
            ("union Command:\n    Bump", "union Command:\n    Open"),
            ("        case Bump:", "        case Open:"),
        ],
    );
}

/// `B0519` — a fold over the *log's* stream still has to be `durable`.
///
/// The rule D30 keeps, and the reason the construct is a new primitive rather than a permission:
/// this accumulator is a function of the log whatever it is called, so declining to persist it
/// makes a state the log can reconstruct and the process cannot.
#[test]
fn a_fold_over_the_log_still_has_to_be_durable() {
    refuses(
        "B0519",
        &[(
            "durable(fold(apply_event, State(n=0), events))",
            "fold(apply_event, State(n=0), events)",
        )],
    );
}
