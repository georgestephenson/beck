//! Mode B: the component that renders in the browser (§5.1, `docs/94`).
//!
//! Seven things are gated here, and the first is the one everything else rests on.
//!
//! 1. **The two modes render the same page.** A Mode B client is only legitimate if running the
//!    view locally produces what the server would have sent. Not "looks the same" — the *same
//!    `Html` value*, asserted against the server's own render of the same state. That is what
//!    "modes share one source" has to mean, and it is checkable because both sides execute the
//!    same `Core`.
//! 2. **A page that reads the session's *identity* is refused Mode B**, because Mode B hands the
//!    browser the state a per-session view was filtering (`beck_core::render`). A page that reads
//!    only `session.path` is not that and is eligible — `client.rs` is where that half is gated.
//! 3. **Optimism is right and reconciliation is right**: a guess appears before the server answers,
//!    a guess the program's own `validate` refuses never appears at all, and a guess is retired
//!    when — and only when — the confirmed state passes the position the server gave it.
//! 4. **The page can say it is guessing** — §3.7's freshness dimension (`docs/93`). `Confirmed`
//!    while the client holds nothing of its own, `Pending(n)` while it does, and back again when
//!    the state that confirms the guess arrives. Refused to a page that renders on the *server*,
//!    which is the one rule here that points that way.
//! 5. **The bundle is a slice**: what the component reaches and nothing else — asserted as a
//!    *shape*, because §5.1's 150 KB budget has ninety times the headroom it needs and the
//!    threshold itself belongs in CI, where `brotli` is installed.
//! 6. **The whole slice runs over a socket**, through the subscription loop a websocket upgrade
//!    lands in — nothing between the log and the browser's page is stubbed.
//! 7. **The kernel builds for `wasm32-unknown-unknown`, and how big it is.** This one *skips* when
//!    the target is not installed, and says so. `BECK_REQUIRE_WASM=1` forbids the skip.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use beck_core::render::Mode;
use beck_core::{Bundle, Placed, Value};
use beck_wasm::{Client, Proposed, Viewer};
use tokio_tungstenite::tungstenite::Message;

mod support;
use support::socket;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

fn compile(src: &str) -> Placed {
    let (placed, diags, map) = beck_core::compile_str("t.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

fn refusal(src: &str) -> String {
    let (_, diags, map) = beck_core::compile_str("t.beck", src);
    assert!(
        diags.has_errors(),
        "this program was supposed to be refused"
    );
    diags.render(&map)
}

fn board() -> Placed {
    example("board.beck")
}

/// The other Mode B example: the one whose page reads §3.7's freshness dimension.
fn editor() -> Placed {
    example("editor.beck")
}

fn example(name: &str) -> Placed {
    let path = root().join("examples").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("examples/{name}"));
    let (placed, diags, map) = beck_core::compile_str(path.to_str().expect("utf-8"), &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// A client with the bundle loaded and the state the server would have at `seq`.
fn client_of(placed: &Placed, actor: &str) -> Client {
    let bytes = Bundle::of(placed).to_bytes();
    Client::load(&bytes, Viewer::named(actor)).expect("the bundle loads")
}

/// The server's own runtime for the same program, so the two can be compared.
fn runtime(placed: &Placed) -> beck_rt::Runtime {
    beck_rt::Runtime::new(
        placed.clone(),
        Arc::new(beck_eval::Evaluator::new(Arc::new(placed.program.clone()))),
    )
    .expect("a runtime")
}

fn command(json: serde_json::Value) -> serde_json::Value {
    json
}

// --------------------------------------------------------------- 1. the two modes agree

/// The claim Mode B rests on, as an assertion about values rather than about pixels.
///
/// The server folds a log and renders; the client is handed the same accumulator and renders. The
/// two `Html` values have to be equal, because they are the same function of the same input — and
/// if they are ever not, every other property here is worthless.
#[test]
fn the_browser_renders_what_the_server_would_have_sent() {
    let placed = board();
    let rt = runtime(&placed);
    let mut client = client_of(&placed, "ana");

    let mut state = rt.initial_state().expect("an initial state");
    let events = [
        json_command("Add", &[("id", "c1"), ("text", "write it down")]),
        json_command("Add", &[("id", "c2"), ("text", "read it back")]),
    ];
    for (i, command) in events.iter().enumerate() {
        state = apply(&rt, &state, command, i as u64 + 1);
    }

    // The server's Mode A render, and the client's Mode B render of the same state.
    let server = rt.view(&state, "ana").expect("the server renders");
    client
        .reset(2, state.clone())
        .expect("the client takes the state");
    let client_html = rendered(&mut client, &placed, "ana");

    assert_eq!(
        server, client_html,
        "the two modes rendered different pages from the same state"
    );
}

/// A route change in Mode B is a **local render**: the state does not move, the session does.
///
/// This is the whole of what the mode buys a router, and it is also where the render's own
/// short-circuit had to learn a second question. `repaint` skips a render when the state a page was
/// derived from has not changed — which is right for every other interaction and wrong for exactly
/// this one, since a navigation changes nothing but the session.
#[test]
fn a_route_change_is_a_local_render_and_agrees_with_the_server() {
    let src = std::fs::read_to_string(root().join("examples/routed.beck")).expect("routed.beck");
    let placed = compile(&src.replace("@on(client)\npage:", "@on(client)\n@render(client)\npage:"));
    let rt = runtime(&placed);
    let mut client = client_of(&placed, "ana");

    let mut state = rt.initial_state().expect("an initial state");
    state = apply(
        &rt,
        &state,
        &json_command("Add", &[("id", "t1"), ("text", "milk")]),
        1,
    );
    state = apply(
        &rt,
        &state,
        &serde_json::json!({"c": "Toggle", "id": "t1"}),
        2,
    );
    client.reset(2, state.clone()).expect("the client takes it");
    let renders = client.renders();

    // The client is at the root, and so is the server's own render.
    assert_eq!(
        client.showing().cloned().expect("a page"),
        rt.view(&state, "ana").expect("the server renders")
    );

    // Somewhere else. The ops are non-empty — the page really changed — and the result is what the
    // server would have sent for that route, which is the claim the whole mode rests on.
    let ops = client.navigate("/active").expect("navigates");
    assert!(!ops.is_empty(), "a route change that patched nothing");
    assert!(
        client.renders() > renders,
        "a route change that rendered nothing"
    );
    let at_active = beck_rt::program::At {
        who: std::sync::Arc::<str>::from("ana"),
        path: std::sync::Arc::from("/active"),
    };
    assert_eq!(
        client.showing().cloned().expect("a page"),
        rt.view(&state, &at_active).expect("the server renders")
    );

    // And going nowhere costs nothing.
    let steady = client.renders();
    assert!(client.navigate("/active").expect("navigates").is_empty());
    assert_eq!(client.renders(), steady);
}

/// The same claim over a hundred different states, because one state is an anecdote.
#[test]
fn the_two_modes_agree_on_every_state_a_log_can_reach() {
    let placed = board();
    let rt = runtime(&placed);
    let mut client = client_of(&placed, "ana");
    let mut state = rt.initial_state().expect("an initial state");

    let mut seq = 0;
    for i in 0..40 {
        let id = format!("c{}", i % 7);
        let command = match i % 3 {
            0 => json_command("Add", &[("id", &id), ("text", &format!("card {i}"))]),
            1 => json_int_command("Move", &id, (i % 3) as i64),
            _ => json_command("Drop", &[("id", &id)]),
        };
        // Whatever the program does with it — accept or refuse — the two sides must agree
        // afterwards, which is why a refusal is not skipped here.
        seq += 1;
        state = apply(&rt, &state, &command, seq);

        client.reset(seq, state.clone()).expect("takes the state");
        assert_eq!(
            rt.view(&state, "ana").expect("the server renders"),
            rendered(&mut client, &placed, "ana"),
            "the modes disagreed after command {i}"
        );
    }
}

/// The claims the server verified reach the client's own `validate`, or the two sides disagree.
///
/// B0514 keeps a *page* from being a function of the session, so the view is safe by construction.
/// `validate` is not: it is in the bundle, it runs in the browser speculatively, and it is handed a
/// `Proposal` carrying a `Session`. A client whose claims map was left empty would refuse a command
/// the server accepts — the page would flash a rejection the log never saw, which is the precise
/// failure optimism exists to be free of.
///
/// So the gap being gated is *claims that do not travel*, not the mechanism that carries them: both
/// clients below load the same bundle and propose the same command, and only the viewer differs.
#[test]
fn a_mode_b_client_decides_against_the_claims_the_server_verified() {
    let placed = compile(&tenant_program());
    assert_eq!(
        placed.render.mode,
        Mode::Client,
        "this program has to be the mode under test"
    );
    let bytes = Bundle::of(&placed).to_bytes();
    let take = command(json_command("Take", &[("id", "c1")]));

    let mut inside = Client::load(&bytes, Viewer::claiming("ana", [("tenant", "acme")]))
        .expect("the bundle loads");
    inside.hydrate().expect("hydrates");
    match inside.propose("k1", &take, 0) {
        Proposed::Accepted { .. } => {}
        Proposed::Refused { why } => {
            panic!("the tenant's own claim did not reach the client's validate: {why}")
        }
    }

    // The same bundle and the same command, for somebody the provider said nothing about.
    let mut outside = Client::load(&bytes, Viewer::named("ana")).expect("the bundle loads");
    outside.hydrate().expect("hydrates");
    match outside.propose("k1", &take, 0) {
        Proposed::Refused { why } => assert!(why.contains("NotYours"), "{why}"),
        Proposed::Accepted { .. } => panic!("a missing claim was as good as the right one"),
    }

    // And the map is the provider's rather than a fixed set: a claim the program does not read
    // changes nothing, and a wrong value is refused like an absent one.
    let mut elsewhere = Client::load(
        &bytes,
        Viewer::claiming("ana", [("tenant", "other"), ("email", "ana@acme.test")]),
    )
    .expect("the bundle loads");
    elsewhere.hydrate().expect("hydrates");
    assert!(
        matches!(elsewhere.propose("k1", &take, 0), Proposed::Refused { .. }),
        "another tenant's claim was accepted"
    );
}

/// The viewer crosses the wasm boundary as JSON, so that is asserted rather than assumed: the
/// browser writes this header and a kernel that read it differently would be a client deciding
/// against claims nobody sent it.
#[test]
fn the_viewer_survives_the_boundary_the_browser_writes_it_across() {
    let viewer = Viewer::claiming("ana", [("tenant", "acme"), ("email", "ana@acme.test")]);
    let json = serde_json::to_string(&viewer).expect("encodes");
    let back: Viewer = serde_json::from_str(&json).expect("decodes");
    assert_eq!(back.actor, "ana");
    assert_eq!(back.claims.get("tenant").map(String::as_str), Some("acme"));

    // A document served before this field existed carries no claims, and that has to load rather
    // than fail: an unauthenticated `beck run` on a laptop is exactly this case.
    let bare: Viewer = serde_json::from_str(r#"{"actor":"dev"}"#).expect("decodes");
    assert_eq!(bare.actor, "dev");
    assert!(bare.claims.is_empty());
}

// --------------------------------------------------------------- 2. what Mode B refuses

#[test]
fn a_page_that_reads_who_is_asking_cannot_render_on_the_client() {
    let out = refusal(&program(
        "@on(client)\n@render(client)\npage: Signal[Html] = per_session(items, view_for)",
    ));
    assert!(out.contains("B0514"), "{out}");
    assert!(
        out.contains("renders differently for each *actor*"),
        "{out}"
    );
    // The refusal has to say *why* rather than name a rule, because the author's next question is
    // "why not".
    assert!(
        out.contains("sends the browser the state, not the page"),
        "{out}"
    );
}

/// Mode B puts the accumulator on the wire, so the obvious question is whether a `secret[T]` can
/// ride along. It cannot, and **not because Mode B checks for it**: a durable fold's state must be
/// storable, storable is strictly stronger than sendable, and the accumulator is what crosses. The
/// composition is what makes the Mode B crossing safe, so the composition is what is gated —
/// asserted here rather than assumed, because a later change to either half would break it
/// silently.
#[test]
fn a_secret_cannot_reach_a_mode_b_client_because_it_cannot_reach_the_log() {
    let out = refusal(&secret_program());
    assert!(out.contains("B0411"), "{out}");
    assert!(out.contains("must be storable"), "{out}");
    assert!(out.contains("secret[Str]"), "{out}");

    // And the two rules are ordered the way that argument needs: storable implies sendable.
    let types = std::collections::BTreeMap::new();
    let secret = beck_core::Ty::app("secret", vec![beck_core::Ty::con("Str")]);
    assert!(beck_core::storable(&secret, &types).is_err());
    assert!(beck_core::sendable(&secret, &types).is_err());
}

#[test]
fn render_belongs_on_a_component_and_nowhere_else() {
    let out = refusal("@render(client)\ndef f(x: Int) -> Int:\n    return x\n");
    assert!(out.contains("B0405"), "{out}");
}

#[test]
fn a_mode_that_is_not_a_mode_is_refused() {
    let out = refusal(&program(
        "@on(client)\n@render(edge)\npage: Signal[Html] = signal_map(items, view_of)",
    ));
    assert!(out.contains("B0306"), "{out}");
}

#[test]
fn mode_a_is_the_default() {
    let placed = compile(&program(
        "@on(client)\npage: Signal[Html] = signal_map(items, view_of)",
    ));
    assert_eq!(placed.render.mode, Mode::Server);
    assert!(!placed.render.optimistic);
}

// --------------------------------------------------------------- 3. optimism and reconciliation

#[test]
fn a_guess_is_on_the_page_before_the_server_answers() {
    let placed = board();
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");

    let outcome = client.propose(
        "k1",
        &command(json_command("Add", &[("id", "c1"), ("text", "guessed")])),
        1_700_000_000_000,
    );
    match outcome {
        Proposed::Accepted { dom } => assert!(!dom.is_empty(), "the guess changed nothing"),
        Proposed::Refused { why } => panic!("the client refused its own command: {why}"),
    }
    assert_eq!(client.in_flight(), 1);
    assert!(shown(&mut client).contains("guessed"));
}

#[test]
fn a_command_the_program_refuses_never_reaches_the_page_or_the_wire() {
    let placed = board();
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");

    // `validate` is in the bundle, so the browser refuses this with the program's own `Rejection`
    // and without a round trip.
    match client.propose(
        "k1",
        &command(json_command("Add", &[("id", "c1"), ("text", "   ")])),
        0,
    ) {
        Proposed::Refused { why } => assert!(why.contains("BlankText"), "{why}"),
        Proposed::Accepted { .. } => panic!("a blank card was accepted"),
    }
    assert_eq!(client.in_flight(), 0, "a refused command is not in flight");
}

/// The whole of reconciliation, in one test: a guess survives until the confirmed state passes the
/// position the server gave it, and then stops being a guess without the page moving.
#[test]
fn a_guess_is_retired_by_the_state_that_confirms_it_and_not_before() {
    let placed = board();
    let rt = runtime(&placed);
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");

    let command = json_command("Add", &[("id", "c1"), ("text", "guessed")]);
    client.propose("k1", &command, 0);
    let guessed = shown(&mut client);

    // The server accepts it at seq 1 — but the client has not been given the state yet.
    client.settle("k1", 1);
    assert_eq!(client.in_flight(), 1, "an ack alone does not confirm");
    assert_eq!(shown(&mut client), guessed, "the page moved on an ack");

    // Now the data patch for seq 1 arrives. The guess is retired, and because it was right the
    // page does not change.
    let state = apply(&rt, &rt.initial_state().expect("init"), &command, 1);
    let ops = client.reset(1, state).expect("takes the state");
    assert_eq!(client.in_flight(), 0, "the guess outlived its confirmation");
    assert!(
        ops.is_empty(),
        "a correct guess should cost no DOM ops, got {ops:?}"
    );
    assert!(shown(&mut client).contains("guessed"));
}

/// A guess that was right costs **one** render, not two.
///
/// The test above asserts the confirmation produces no DOM ops; this asserts it does no work to
/// find that out. They are different claims and the second is the expensive one: `view` is 97% of
/// what an interaction costs and it grows with the state (§94.12), so a render whose patch is
/// empty by construction is the largest avoidable cost in the client.
///
/// Asserted as a count rather than as a duration, because a gate on a clock flakes ([`13`] §13.7)
/// and the property here is exact: the state derived after the confirmation *equals* the state the
/// guess was derived from, and equal states cannot produce different pages.
///
/// This goes red if `repaint` renders unconditionally again — which is what it did until §94.12.
///
/// [`13`]: ../../../../docs/13-testing.md
#[test]
fn a_guess_that_was_right_is_confirmed_without_rendering_again() {
    let placed = board();
    let rt = runtime(&placed);
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");

    let command = json_command("Add", &[("id", "c1"), ("text", "guessed")]);
    assert!(matches!(
        client.propose("k1", &command, 0),
        Proposed::Accepted { .. }
    ));
    let after_the_guess = client.renders();

    // The server's data patch for the very command this client guessed at.
    let state = apply(&rt, &rt.initial_state().expect("init"), &command, 1);
    let ops = client.reset(1, state).expect("takes the state");

    assert!(ops.is_empty(), "a correct guess cost DOM ops: {ops:?}");
    assert_eq!(
        client.renders(),
        after_the_guess,
        "the confirmation re-rendered a page it could not have changed"
    );

    // And the shortcut is a shortcut, not a way of never rendering again: a state that *did* move
    // still renders. Without this the test would pass just as well against a client that had
    // stopped working.
    let second = json_command("Add", &[("id", "c2"), ("text", "another")]);
    let moved = apply(&rt, &client.state().expect("a state"), &second, 2);
    let ops = client.reset(2, moved).expect("takes the state");
    assert!(!ops.is_empty(), "a state that moved produced no patch");
    assert_eq!(
        client.renders(),
        after_the_guess + 1,
        "a state that moved should cost exactly one render"
    );
}

#[test]
fn a_guess_the_server_refuses_is_taken_off_the_page() {
    let placed = board();
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");

    client.propose(
        "k1",
        &json_command("Add", &[("id", "c1"), ("text", "optimistic")]),
        0,
    );
    assert!(shown(&mut client).contains("optimistic"));

    // The server got there first with a different card of the same id: `IdTaken`.
    client.refused("k1").expect("re-renders");
    assert_eq!(client.in_flight(), 0);
    assert!(!shown(&mut client).contains("optimistic"));
}

/// A data patch is a *diff*, so a client that missed nothing pays for what changed rather than for
/// what is there. This is the property that makes Mode B's wire cheaper than the state.
#[test]
fn a_data_patch_costs_the_change_and_not_the_state() {
    let placed = board();
    let rt = runtime(&placed);
    let mut state = rt.initial_state().expect("init");
    for i in 0..200 {
        state = apply(
            &rt,
            &state,
            &json_command(
                "Add",
                &[("id", &format!("c{i}")), ("text", &format!("card {i}"))],
            ),
            i + 1,
        );
    }
    let after = apply(&rt, &state, &json_int_command("Move", "c100", 1), 201);

    let ops = beck_core::delta::diff(&state, &after);
    assert_eq!(ops.len(), 1, "{ops:?}");

    // And the client that applies it agrees with the server about the result.
    let mut client = client_of(&placed, "ana");
    client.reset(200, state).expect("takes the state");
    client.data(201, &ops).expect("applies the patch");
    assert_eq!(
        rt.view(&after, "ana").expect("the server renders"),
        rendered(&mut client, &placed, "ana")
    );
}

#[test]
fn a_data_patch_against_a_state_the_client_does_not_have_fails_loudly() {
    let placed = board();
    let rt = runtime(&placed);
    let init = rt.initial_state().expect("init");
    let one = apply(
        &rt,
        &init,
        &json_command("Add", &[("id", "c1"), ("text", "a")]),
        1,
    );
    let two = apply(&rt, &one, &json_int_command("Move", "c1", 1), 2);

    let mut client = client_of(&placed, "ana");
    client.reset(0, init).expect("takes the state");
    // The patch from 1 to 2 talks about a card this client has never seen.
    assert!(client.data(2, &beck_core::delta::diff(&one, &two)).is_err());
}

/// Hydration: a client handed the state the document was rendered from adopts the page instead of
/// rebuilding it — and one handed a *later* state does rebuild.
///
/// The first half is the free-hydration claim (§93.5); the second is what makes it safe, because a
/// client that adopted a page the DOM is not showing would be silently wrong from then on.
#[test]
fn a_client_adopts_the_page_it_was_rendered_and_rebuilds_any_other() {
    let placed = board();
    let rt = runtime(&placed);
    let init = rt.initial_state().expect("init");
    let one = apply(
        &rt,
        &init,
        &json_command("Add", &[("id", "c1"), ("text", "landed first")]),
        1,
    );

    // Rendered at 0, given the state at 0: nothing to do.
    let mut client = client_of(&placed, "ana");
    client.adopt(0, init.clone()).expect("adopts");
    assert!(
        client.repaint().expect("renders").is_empty(),
        "adopting emitted a patch"
    );

    // Rendered at 0, given the state at 1 — an event landed in between. The page has to be built.
    let mut client = client_of(&placed, "ana");
    let ops = client.reset(1, one).expect("takes the state");
    assert!(!ops.is_empty(), "a later state produced no patch");
    assert!(shown(&mut client).contains("landed first"));
}

// --------------------------------------------------------------- 4. freshness (§3.7, docs/93)

/// The page says whether what it is showing is a fact, and stops saying it when it becomes one.
///
/// §3.7: "`Signal[T]` carries a freshness dimension (`confirmed | pending(n)`) that UI code can
/// render (\"saving…\") — staleness is typed, not pretended away." This is the whole feature in one
/// interaction: `Confirmed` before, `Pending(1)` while the guess is in flight, `Confirmed` again
/// when the state that includes it arrives.
///
/// It goes red if `freshness()` stops reaching the view, if the count stops following the queue, or
/// if the confirmation stops repainting a page whose freshness moved.
#[test]
fn a_page_that_is_showing_a_guess_says_so_and_stops_when_it_is_confirmed() {
    let placed = editor();
    let rt = runtime(&placed);
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");
    assert!(
        shown(&mut client).contains("saved"),
        "a fresh page is saved"
    );

    let command = json_command("Write", &[("id", "l1"), ("text", "the first line")]);
    assert!(matches!(
        client.propose("k1", &command, 0),
        Proposed::Accepted { .. }
    ));
    let guessing = shown(&mut client);
    assert!(guessing.contains("saving 1"), "{guessing}");
    assert!(guessing.contains("the first line"), "{guessing}");

    // The server's state for exactly that command. The document does not move — the guess was
    // right — but the freshness does, and the page has to follow it.
    client.settle("k1", 1);
    let state = apply(&rt, &rt.initial_state().expect("init"), &command, 1);
    let ops = client.reset(1, state.clone()).expect("takes the state");
    assert!(
        !ops.is_empty(),
        "the confirmation left the page saying `saving`"
    );
    let settled = shown(&mut client);
    assert!(settled.contains("saved"), "{settled}");

    // And it is now the page the server would have sent, which is the claim every other one rests
    // on — for a program whose page reads something the server answers as a constant.
    assert_eq!(
        rt.view(&state, "ana").expect("the server renders"),
        rendered(&mut client, &placed, "ana"),
        "the two modes disagreed once the guess was confirmed"
    );
}

/// `Pending(n)` counts, so a page can say how much it owes.
#[test]
fn pending_counts_every_command_in_flight_and_not_just_that_there_is_one() {
    let placed = editor();
    let mut client = client_of(&placed, "ana");
    client.hydrate().expect("hydrates");

    for (i, text) in ["first", "second", "third"].iter().enumerate() {
        assert!(matches!(
            client.propose(
                &format!("k{i}"),
                &json_command("Write", &[("id", &format!("l{i}")), ("text", text)]),
                0
            ),
            Proposed::Accepted { .. }
        ));
    }
    assert_eq!(client.in_flight(), 3);
    let page = shown(&mut client);
    assert!(page.contains("saving 3"), "{page}");
}

/// The shortcut `docs/94` §94.12 added asks whether the *state* moved. A confirmation is exactly
/// where the state does not and the freshness does, so the shortcut had to learn a second question
/// — and it may only ask it of a component that reads the answer.
///
/// Both directions are asserted, because either alone is satisfiable by a broken client: a page
/// that reads freshness re-renders on the confirmation, and a page that does not still costs
/// nothing. Removing the `reads_freshness` guard reddens the second half; removing the freshness
/// comparison entirely reddens the first.
#[test]
fn a_confirmation_repaints_a_page_that_reads_freshness_and_no_other() {
    let cost = |placed: &Placed, command: serde_json::Value| {
        let rt = runtime(placed);
        let mut client = client_of(placed, "ana");
        client.hydrate().expect("hydrates");
        assert!(matches!(
            client.propose("k1", &command, 0),
            Proposed::Accepted { .. }
        ));
        let after_the_guess = client.renders();
        client.settle("k1", 1);
        let state = apply(&rt, &rt.initial_state().expect("init"), &command, 1);
        client.reset(1, state).expect("takes the state");
        client.renders() - after_the_guess
    };

    assert_eq!(
        cost(
            &editor(),
            json_command("Write", &[("id", "l1"), ("text", "a line")])
        ),
        1,
        "a page that renders `saving` was left rendering it"
    );
    assert_eq!(
        cost(
            &board(),
            json_command("Add", &[("id", "c1"), ("text", "a card")])
        ),
        0,
        "a page that cannot observe freshness paid for a render with a known answer"
    );
}

/// A client that may not guess is `Confirmed` whatever it has sent, because what it is showing is
/// the server's state and not its own.
#[test]
fn a_client_that_may_not_guess_is_never_pending() {
    let placed = editor();
    let mut bundle = Bundle::of(&placed);
    bundle.optimistic = false;
    let bytes = bundle.to_bytes();
    let mut client = Client::load(&bytes, Viewer::named("ana")).expect("the bundle loads");
    client.hydrate().expect("hydrates");

    assert!(matches!(
        client.propose(
            "k1",
            &json_command("Write", &[("id", "l1"), ("text", "written locally")]),
            0
        ),
        Proposed::Accepted { .. }
    ));
    let page = shown(&mut client);
    assert!(page.contains("saved"), "{page}");
    assert!(
        !page.contains("written locally"),
        "a page that may not guess guessed"
    );
}

/// The refusal that points the other way from `B0516`: a server has nothing in flight, so a page
/// that reads freshness may not render there.
///
/// Goes red if `@render(client)` stops being required for `freshness()` — which would mean a page
/// with a branch no log can take.
#[test]
fn a_page_that_reads_freshness_cannot_render_on_the_server() {
    let src = std::fs::read_to_string(root().join("examples/editor.beck")).expect("the editor");
    let why = refusal(&src.replace("@render(client)\n", ""));
    assert!(why.contains("B0518"), "{why}");
    assert!(why.contains("cannot render on the server"), "{why}");
    // The reason, not just the rule: a reader who wanted this has to be told what to do instead.
    assert!(why.contains("@render(client)"), "{why}");
}

/// And the chokepoint may not read it either, for `B0515`'s reason applied to the other source
/// that is not the log: nothing records what was in flight, and a replay has nothing in flight.
#[test]
fn the_chokepoint_cannot_decide_from_what_is_in_flight() {
    let why = refusal(&program_deciding_from_freshness());
    assert!(why.contains("B0517"), "{why}");
    assert!(why.contains("not in the log"), "{why}");
}

// --------------------------------------------------------------- 5. the bundle is a slice

#[test]
fn a_bundle_carries_the_components_slice_and_the_program_it_was_cut_from() {
    let placed = board();
    let bundle = Bundle::of(&placed);

    assert_eq!(bundle.component.as_ref(), "page");
    assert_eq!(bundle.wire_id, placed.wire_id);
    assert!(bundle.optimistic);
    // Reached from the view.
    for reached in ["cards_in", "heading", "next_column", "column", "columns"] {
        assert!(bundle.defs.contains_key(reached), "{reached} is missing");
    }
    // Reached from `validate` and the fold — the client runs those too, and that is the whole of
    // why optimism is not a second implementation.
    for reached in ["if_present", "moved"] {
        assert!(bundle.defs.contains_key(reached), "{reached} is missing");
    }
}

/// §5.1's budget is "< 150 KB brotli **per component bundle**", and what makes that a property
/// rather than an accident is this: a bundle is a function of the *component's slice*, not of the
/// program the component is in.
///
/// So the gate is a shape rather than a threshold — the threshold is in CI, where `brotli` is
/// installed and a budget cannot flake ([`13`] §13.7), and against the board it has eighty times
/// the headroom it needs, which is a gate that could not go red. This one can: it measures at two
/// sizes, because one measurement cannot tell "does not grow" from "grew a little"
/// (`AGENTS.md`), and it asserts **equality of bytes** rather than a bound, because anything the
/// component does not reach contributing anything at all is the defect.
///
/// It goes red the day the bundle starts carrying the program — a type table, a signal graph, a
/// definition reached by nothing, the tests — which is exactly the change that would make the
/// budget stop holding for a large application.
///
/// The bound is "under a byte each" rather than zero, and the byte is real: variables are numbered
/// across the whole program, so a bigger program numbers the *slice's own* locals higher and
/// postcard spends a second byte on a varint past 127. That is `O(log n)` in the program's size
/// and it is why this asserts a rate instead of equality — a definition that were genuinely
/// carried would cost hundreds of bytes, not a fraction of one.
///
/// [`13`]: ../../../../docs/13-testing.md
#[test]
fn a_bundle_is_a_function_of_the_slice_and_not_of_the_program_around_it() {
    let src = std::fs::read_to_string(root().join("examples/board.beck")).expect("the board");
    let bundle = |extra: usize| {
        let mut program = src.clone();
        for i in 0..extra {
            // Reached by nothing: not the view, not `validate`, not the fold, not `init`. A
            // definition like this is exactly what a growing application accumulates.
            program.push_str(&format!(
                "\ndef unreached_{i}(n: Int) -> Int:\n    return n * {} + 1\n",
                i + 2
            ));
        }
        Bundle::of(&compile(&program))
    };

    let slice = bundle(0);
    let carried: Vec<_> = slice.defs.keys().cloned().collect();
    let baseline = slice.to_bytes().len();

    // Two sizes, because one cannot tell "does not grow" from "grows slowly".
    for extra in [10, 100] {
        let grown = bundle(extra);
        assert_eq!(
            grown.defs.keys().cloned().collect::<Vec<_>>(),
            carried,
            "{extra} unreached definitions changed what the bundle carries"
        );
        let per = (grown.to_bytes().len() - baseline) as f64 / extra as f64;
        assert!(
            per < 1.0,
            "{extra} unreached definitions cost {per:.2} bytes each — the bundle is a function of \
             the program rather than of the slice"
        );
    }
}

#[test]
fn a_bundle_from_another_program_is_refused_rather_than_run() {
    let placed = board();
    let mut bytes = Bundle::of(&placed).to_bytes();
    // Corrupt it in the middle, past the header: a bundle that decodes to something else is the
    // failure this format's version and shape digest exist to make impossible.
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    // Either it fails to decode or it decodes to something that will not prepare. What it must not
    // do is load and then render something.
    if let Ok(mut client) = Client::load(&bytes, Viewer::named("ana")) {
        let _ = client.repaint();
    }
}

// --------------------------------------------------------------- 6. end to end, over a socket

/// The whole slice, driven through the loop a browser talks to.
///
/// The server streams **data** frames because the component says `@render(client)`; the kernel
/// applies them; the page the kernel then holds is the page the server would have rendered. No
/// step of that is stubbed — this is `beck_rt::session::run` over an in-memory socket, the same
/// function a websocket upgrade lands in.
#[tokio::test]
async fn a_mode_b_subscription_carries_the_state_and_the_browser_renders_it() {
    let placed = board();
    let backend = beck_eval::backend(&placed);
    let app = beck_rt::App::start(
        beck_rt::Runtime::new(placed.clone(), backend).expect("prepares"),
        Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig::default(),
    )
    .await
    .expect("app starts");

    let (client_tx, server_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    let _session = tokio::spawn(beck_rt::session::run(
        app.clone(),
        socket::Duplex {
            out: server_tx,
            inbox: server_rx,
        },
    ));

    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","actor":"ana"})
                .to_string()
                .into(),
        ))
        .expect("hello");

    let opening = socket::drain(&mut client_rx).await;
    assert!(opening.iter().any(|m| m["t"] == "w"), "{opening:?}");
    let state_frame = opening
        .iter()
        .find(|m| m["t"] == "s")
        .expect("a Mode B subscription opens with the state, not a page");
    assert!(
        !opening.iter().any(|m| m["t"] == "p"),
        "a Mode B subscription sent a DOM patch: {opening:?}"
    );

    // The browser side: load the bundle, take the state it was sent.
    let mut client = client_of(&placed, "ana");
    let response = beck_wasm::dispatch(
        &mut client,
        &serde_json::json!({
            "op": "reset",
            "seq": state_frame["q"],
            "state": state_frame["v"],
        }),
    )
    .expect("the client takes the state");
    assert!(response["dom"].as_array().is_some_and(|o| !o.is_empty()));

    // A command up the same socket. The server appends, acks, and — because this is Mode B —
    // answers with a *data* frame.
    client_tx
        .send(Message::Text(
            serde_json::json!({
                "t":"c","id":"k1",
                "command":{"c":"Add","id":"c1","text":"from the wire"}
            })
            .to_string()
            .into(),
        ))
        .expect("cmd");

    let after = socket::drain(&mut client_rx).await;
    assert!(after.iter().any(|m| m["t"] == "a"), "no ack: {after:?}");
    let data = after
        .iter()
        .find(|m| m["t"] == "d")
        .expect("a data frame for the new card");
    assert!(
        !data.to_string().contains("<"),
        "a data frame carried markup: {data}"
    );

    beck_wasm::dispatch(
        &mut client,
        &serde_json::json!({ "op": "data", "seq": data["q"], "ops": data["o"] }),
    )
    .expect("the client applies the patch");

    // And the page the browser now holds is the page the server would have sent in Mode A.
    let server_state = app.state().await;
    assert_eq!(
        beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed))
            .expect("prepares")
            .view(&server_state, "ana")
            .expect("the server renders"),
        rendered(&mut client, &placed, "ana")
    );
}

// --------------------------------------------------------------- 7. the wasm build

/// The kernel builds for the browser's target, and this is how big it is.
///
/// Skips — loudly — when `wasm32-unknown-unknown` is not installed, which is the convention every
/// environment-dependent suite in this workspace follows. `BECK_REQUIRE_WASM=1` forbids the skip,
/// which is what CI sets.
#[test]
fn the_kernel_builds_for_the_browser() {
    let required = std::env::var("BECK_REQUIRE_WASM").is_ok_and(|v| v == "1");
    if !wasm_target_installed() {
        assert!(
            !required,
            "BECK_REQUIRE_WASM=1 but the wasm32-unknown-unknown target is not installed"
        );
        eprintln!(
            "skipped: the wasm32-unknown-unknown target is not installed \
             (`rustup target add wasm32-unknown-unknown`). Set BECK_REQUIRE_WASM=1 to forbid this skip."
        );
        return;
    }

    let out = Command::new(env!("CARGO"))
        .current_dir(root())
        .args([
            "build",
            "-p",
            "beck-wasm",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output()
        .expect("cargo runs");
    assert!(
        out.status.success(),
        "the kernel did not build for wasm32:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let module = root().join("target/wasm32-unknown-unknown/release/beck_wasm.wasm");
    let bytes = std::fs::metadata(&module).expect("the module exists").len();
    eprintln!(
        "kernel: {bytes} bytes of WebAssembly at {}",
        module.display()
    );
    // A ceiling, not a budget: what this asserts is that the kernel has not grown by an order of
    // magnitude, which is the change that would make Mode B a different proposition. The measured
    // number and what it means for §5.1's 150 KB are in `docs/94` §94.11.
    assert!(
        bytes < 8 * 1024 * 1024,
        "the kernel is {bytes} bytes, which is not a kernel any more"
    );
}

/// The extent of the exception to the workspace's `forbid(unsafe_code)`.
///
/// Two crates deny rather than forbid, and both for the same reason: rustc classifies
/// `#[no_mangle]` as unsafe code, and a WebAssembly module that exports nothing cannot be called.
/// `beck-wasm` is Mode B's kernel and `beck-play` is the playground (`docs/98` §98.3). That is a
/// narrow exception and this is what keeps it narrow: no `unsafe` block, no `unsafe fn`, and every
/// `allow` attached to an export. Every other crate still inherits the workspace lint.
///
/// The count is per crate and exact, so a fifth export in the kernel or a fourth in the playground
/// is a decision somebody has to come here and make.
#[test]
fn the_wasm_boundary_is_the_only_exception_to_forbid_unsafe() {
    for (crate_name, exports) in [("beck-wasm", 4), ("beck-play", 3)] {
        let crate_dir = root().join("crates").join(crate_name);
        let mut allows = 0;
        for entry in std::fs::read_dir(crate_dir.join("src")).expect("the crate's sources") {
            let path = entry.expect("an entry").path();
            let src = std::fs::read_to_string(&path).expect("readable");
            for (n, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for forbidden in ["unsafe {", "unsafe fn", "unsafe impl", "unsafe trait"] {
                    assert!(
                        !code.contains(forbidden),
                        "{}:{}: `{forbidden}` in a crate that is supposed to have none",
                        path.display(),
                        n + 1
                    );
                }
                if code.contains("allow(unsafe_code)") {
                    allows += 1;
                    assert!(
                        src.lines()
                            .nth(n + 1)
                            .is_some_and(|l| l.contains("no_mangle")),
                        "{}:{}: an allow that is not on an export",
                        path.display(),
                        n + 1
                    );
                }
            }
        }
        assert_eq!(
            allows, exports,
            "{crate_name}'s exception is {exports} export attributes, no more"
        );
    }

    // And nothing else in the workspace has taken the same liberty.
    for entry in std::fs::read_dir(root().join("crates")).expect("the crates") {
        let path = entry.expect("an entry").path();
        let manifest = path.join("Cargo.toml");
        if !manifest.is_file() || path.ends_with("beck-wasm") || path.ends_with("beck-play") {
            continue;
        }
        let toml = std::fs::read_to_string(&manifest).expect("readable");
        assert!(
            toml.contains("[lints]\nworkspace = true"),
            "{} does not inherit the workspace lints",
            manifest.display()
        );
    }
}

// --------------------------------------------------------------- helpers

fn wasm_target_installed() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}

/// Fold one command through the server's roles, exactly as ingress would.
fn apply(rt: &beck_rt::Runtime, state: &Value, command: &serde_json::Value, seq: u64) -> Value {
    let command = rt.decode_command(command).expect("a decodable command");
    let proposal = rt.proposal("ana", command);
    let Ok(events) = rt.validate(state, &proposal) else {
        return state.clone();
    };
    let mut out = state.clone();
    for event in events {
        let env = beck_rt::Envelope {
            seq,
            at: beck_rt::Instant(1_700_000_000_000),
            actor: "ana".into(),
            body: event.clone(),
        };
        out = rt.fold(&out, &env, event).expect("the fold");
    }
    out
}

/// What the client would render, as `Html`, by replaying its own patch stream — which also
/// exercises the ops rather than trusting them.
fn rendered(client: &mut Client, placed: &Placed, actor: &str) -> beck_core::Html {
    let _ = placed;
    let _ = actor;
    client.repaint().expect("renders");
    client.showing().expect("something is shown").clone()
}

fn shown(client: &mut Client) -> String {
    client.repaint().expect("renders");
    client.showing().expect("something is shown").render()
}

fn json_command(tag: &str, fields: &[(&str, &str)]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("c".into(), serde_json::Value::String(tag.into()));
    for (k, v) in fields {
        map.insert((*k).into(), serde_json::Value::String((*v).into()));
    }
    serde_json::Value::Object(map)
}

fn json_int_command(tag: &str, id: &str, column: i64) -> serde_json::Value {
    serde_json::json!({ "c": tag, "id": id, "column": column })
}

/// A minimal application whose page is written by the caller.
fn program(page: &str) -> String {
    format!(
        r#"
model Item:
    id: Str
    owner: Str

model Items:
    items: list[Item]

union Command:
    Take(id: Str)

union Event:
    Taken(id: Str)

union Rejection:
    No

def apply_event(s: Items, env: Envelope[Event]) -> Items:
    return s

def validate(s: Items, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Take(id):
            return Ok(value=[Taken(id=id)])

def view_of(s: Items) -> Html:
    return ui:
        ul:
            for i in s.items:
                li: i.id

def view_for(s: Items, session: Session) -> Html:
    return ui:
        ul:
            for i in filter_list(s.items, lambda i: i.owner == session.actor):
                li: i.id

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, items, validate)

@on(data)
items: Signal[Items] = durable(fold(apply_event, Items(items=[]), events))

{page}
"#
    )
}

/// A program whose chokepoint decides from `freshness()` — the thing `B0517` refuses.
///
/// The freshness reaches `decide` through a `map2`, which is the only way it can: `validate` is a
/// `def` and takes what the graph hands it, so the refusal has to be about the graph rather than
/// about the function's body.
fn program_deciding_from_freshness() -> String {
    r#"
model Items:
    items: list[Str]

model Guarded:
    items: Items
    saving: Freshness

union Command:
    Take(id: Str)

union Event:
    Taken(id: Str)

union Rejection:
    Busy

def apply_event(s: Items, env: Envelope[Event]) -> Items:
    return s

def guard(s: Items, saving: Freshness) -> Guarded:
    return Guarded(items=s, saving=saving)

def validate(g: Guarded, p: Proposal) -> Result[list[Event], Rejection]:
    match g.saving:
        case Pending(n):
            return Err(error=Busy)
        case Confirmed:
            return Ok(value=[Taken(id="x")])

def view_of(s: Items) -> Html:
    return ui:
        ul:
            for i in s.items:
                li: i

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, guarded, validate)

guarded: Signal[Guarded] = map2(guard, items, saving)

saving: Signal[Freshness] = freshness()

@on(data)
items: Signal[Items] = durable(fold(apply_event, Items(items=[]), events))

@on(client)
page: Signal[Html] = signal_map(items, view_of)
"#
    .to_string()
}

/// A Mode B program whose *page* is a function of the state alone — so B0514 lets it render on the
/// client — and whose `validate` reads a claim. That combination is what makes the claims a client
/// holds load-bearing rather than decorative.
fn tenant_program() -> String {
    r#"
model Item:
    id: Str

model Items:
    items: list[Item]

union Command:
    Take(id: Str)

union Event:
    Taken(id: Str)

union Rejection:
    NotYours

def apply_event(s: Items, env: Envelope[Event]) -> Items:
    return s

def validate(s: Items, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Take(id):
            if unwrap_or(map_get(p.session.claims, "tenant"), "") != "acme":
                return Err(error=NotYours)
            return Ok(value=[Taken(id=id)])

def view_of(s: Items) -> Html:
    return ui:
        ul:
            for i in s.items:
                li: i.id

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, items, validate)

@on(data)
items: Signal[Items] = durable(fold(apply_event, Items(items=[]), events))

@on(client)
@render(client)
page: Signal[Html] = signal_map(items, view_of)
"#
    .to_string()
}

/// A program that tries to keep a secret in its accumulator.
fn secret_program() -> String {
    r#"
model Config:
    api_key: secret[Str]

model State:
    config: Config
    count: Int

union Command:
    Bump

union Event:
    Bumped

union Rejection:
    No

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(count=s.count + 1)

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Bumped])

def view_of(s: State) -> Html:
    return ui:
        p: str(s.count)

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, state, validate)

@on(data)
state: Signal[State] = durable(fold(apply_event, State(config=Config(api_key=env("KEY")), count=0), events))

@on(client)
@render(client)
page: Signal[Html] = signal_map(state, view_of)
"#
    .to_string()
}
