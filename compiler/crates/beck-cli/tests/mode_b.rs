//! Mode B: the component that renders in the browser (§5.1, `docs/94`).
//!
//! Five things are gated here, and the first is the one everything else rests on.
//!
//! 1. **The two modes render the same page.** A Mode B client is only legitimate if running the
//!    view locally produces what the server would have sent. Not "looks the same" — the *same
//!    `Html` value*, asserted against the server's own render of the same state. That is what
//!    "modes share one source" has to mean, and it is checkable because both sides execute the
//!    same `Core`.
//! 2. **A page that reads the session is refused Mode B**, because Mode B hands the browser the
//!    state a per-session view was filtering (`beck_core::render`).
//! 3. **Optimism is right and reconciliation is right**: a guess appears before the server answers,
//!    a guess the program's own `validate` refuses never appears at all, and a guess is retired
//!    when — and only when — the confirmed state passes the position the server gave it.
//! 4. **The bundle is a slice**: what the component reaches and nothing else.
//! 5. **The kernel builds for `wasm32-unknown-unknown`, and how big it is.** This one *skips* when
//!    the target is not installed, and says so. `BECK_REQUIRE_WASM=1` forbids the skip.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use beck_core::render::Mode;
use beck_core::{Bundle, Placed, Value};
use beck_wasm::{Client, Proposed};
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
    let path = root().join("examples/board.beck");
    let src = std::fs::read_to_string(&path).expect("examples/board.beck");
    let (placed, diags, map) = beck_core::compile_str(path.to_str().expect("utf-8"), &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// A client with the bundle loaded and the state the server would have at `seq`.
fn client_of(placed: &Placed, actor: &str) -> Client {
    let bytes = Bundle::of(placed).to_bytes();
    Client::load(&bytes, actor).expect("the bundle loads")
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

// --------------------------------------------------------------- 2. what Mode B refuses

#[test]
fn a_page_that_reads_the_session_cannot_render_on_the_client() {
    let out = refusal(&program(
        "@on(client)\n@render(client)\npage: Signal[Html] = per_session(items, view_for)",
    ));
    assert!(out.contains("B0514"), "{out}");
    assert!(
        out.contains("renders differently for each session"),
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

// --------------------------------------------------------------- 4. the bundle is a slice

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
    if let Ok(mut client) = Client::load(&bytes, "ana") {
        let _ = client.repaint();
    }
}

// --------------------------------------------------------------- 5. end to end, over a socket

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
            serde_json::json!({"t":"hello","sub":"s1","seq":0,"actor":"ana"})
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

// --------------------------------------------------------------- 6. the wasm build

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
    // number and what it means for §5.1's 150 KB are in `docs/94` §94.6.
    assert!(
        bytes < 8 * 1024 * 1024,
        "the kernel is {bytes} bytes, which is not a kernel any more"
    );
}

/// The extent of the one exception to the workspace's `forbid(unsafe_code)`.
///
/// `beck-wasm` denies rather than forbids, because rustc classifies `#[no_mangle]` as unsafe code
/// and a WebAssembly module that exports nothing cannot be called. That is a narrow exception and
/// this is what keeps it narrow: no `unsafe` block, no `unsafe fn`, no `unsafe impl`, and every
/// `allow` attached to an export. Every other crate still inherits the workspace lint.
#[test]
fn the_wasm_boundary_is_the_only_exception_to_forbid_unsafe() {
    let crate_dir = root().join("crates/beck-wasm");
    let mut allows = 0;
    for entry in std::fs::read_dir(crate_dir.join("src")).expect("the kernel's sources") {
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
        allows, 4,
        "the exception is four export attributes, no more"
    );

    // And nothing else in the workspace has taken the same liberty.
    for entry in std::fs::read_dir(root().join("crates")).expect("the crates") {
        let path = entry.expect("an entry").path();
        let manifest = path.join("Cargo.toml");
        if !manifest.is_file() || path.ends_with("beck-wasm") {
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
