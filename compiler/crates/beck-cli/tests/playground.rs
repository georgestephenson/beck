//! The playground, held to the two things it claims (`docs/17`, `docs/98`).
//!
//! Rung A claims to show **what the compiler derives**, which is only worth anything if it is the
//! same compiler: `the_playground_shows_what_the_command_line_shows` runs `beck explain` and
//! compares the bytes.
//!
//! Rung B claims that "by the differential harness's own guarantee, rung-B behaviour *is* the
//! deployed behaviour" (§17.2). That sentence names this file. `the_tab_and_the_server_agree_…`
//! drives the same commands through `beck_rt::App` — the real sequencer, the real subscription,
//! over the socket harness — and through a `beck_play::Tab`, and asserts the pages and the frames
//! are the same. It is the gate the whole rung rests on, and it goes red if the tab ever starts
//! being a second implementation of anything that matters.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use beck_core::Placed;
use beck_play::{dispatch, Playground, Tab};
use serde_json::json;

mod support;
use support::socket::{drain, Duplex};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

/// The programs the playground offers, by the path `beck` would be given for each.
fn examples() -> Vec<(&'static str, PathBuf)> {
    vec![
        (
            "counter",
            root().join("crates/beck-play/examples/counter.beck"),
        ),
        ("todo", root().join("examples/todo.beck")),
        ("board", root().join("examples/board.beck")),
    ]
}

fn compiled(path: &Path) -> Placed {
    let src = std::fs::read_to_string(path).expect("the example");
    let (placed, diags, map) = beck_core::compile_str(path.to_str().expect("utf-8"), &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// Run the `beck` this workspace builds, in a directory, and take its standard output.
fn beck_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("beck runs");
    assert!(
        out.status.success(),
        "beck {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

fn analysed(source: &str) -> beck_play::Analysis {
    beck_play::analyse(source)
}

fn section<'a>(analysis: &'a beck_play::Analysis, id: &str) -> &'a str {
    &analysis
        .sections
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("the playground has no `{id}` section"))
        .text
}

// ------------------------------------------------------------------ rung A

/// The claim: "source on the left, *what the compiler derives* on the right" (§17.1).
///
/// What makes that worth saying is that it is the **same** compiler, and the way to be sure is to
/// compare the characters. This runs the command line and the playground over the same file and
/// asserts they produce identical text for every section that has a `beck explain` behind it.
///
/// It goes red the day somebody renders a placement table twice — which is exactly what the code
/// looked like before this crate existed, with `explain_place` printing in `main.rs`.
#[test]
fn the_playground_shows_what_the_command_line_shows() {
    for (name, path) in examples() {
        let source = std::fs::read_to_string(&path).expect("the example");
        let analysis = analysed(&source);
        assert_eq!(analysis.errors, 0, "{name} does not compile");

        // The same source under the same module name. A crossing's id is content-derived from the
        // module (§4.3), so `beck explain flow todo.beck` and a playground holding the same text
        // legitimately print different digests — and comparing them anyway would be asserting that
        // two different modules are one.
        let dir = root().join("target/playground-gate").join(name);
        std::fs::create_dir_all(&dir).expect("a directory to compare in");
        std::fs::write(dir.join("playground.beck"), &source).expect("the copy");

        for (id, command) in [
            ("place", vec!["explain", "place", "playground.beck"]),
            ("flow", vec!["explain", "flow", "playground.beck"]),
            ("wire", vec!["explain", "wire", "playground.beck"]),
            ("render", vec!["explain", "render", "playground.beck"]),
            (
                "incremental",
                vec!["explain", "incremental", "playground.beck"],
            ),
            ("cost", vec!["explain", "cost", "playground.beck"]),
            ("sql", vec!["explain", "sql", "playground.beck"]),
            ("query", vec!["explain", "query", "playground.beck"]),
            ("deploy", vec!["explain", "deploy", "playground.beck"]),
        ] {
            assert_eq!(
                section(&analysis, id),
                beck_in(&dir, &command),
                "`beck {}` and the playground's `{id}` section disagree about {name}",
                command.join(" ")
            );
        }
    }
}

/// A program that does not compile gets diagnostics and **nothing else**.
///
/// The failure this forbids is a playground that shows a stale placement beside a red error — a
/// visitor learning that the compiler answers questions about programs it has rejected.
#[test]
fn a_program_that_does_not_compile_derives_nothing() {
    let analysis = analysed("def broken(x: Int) -> Int:\n    return x + \"not a number\"\n");
    assert!(analysis.errors > 0, "that program should not compile");
    assert!(!analysis.diagnostics.is_empty());
    assert!(
        analysis.sections.iter().all(|s| s.id != "place"),
        "a program with no types was given a placement"
    );
    assert!(!analysis.runnable);
}

/// A module with no merge point is a **library**, and the page says so rather than showing errors.
///
/// `beck check` answers "ok: … a library: no merge point, so there is nothing to run". A visitor
/// pasting three definitions into an empty editor has written one, and a playground that answered
/// with three red errors would be teaching that a Beck program has to be an application.
#[test]
fn a_library_is_a_library_rather_than_three_errors() {
    let analysis = analysed(
        "def double(x: Int) -> Int:\n    return x * 2\n\ntest \"doubling\":\n    expect double(2) == 4\n",
    );
    assert_eq!(analysis.errors, 0, "a library was reported as broken");
    assert!(!analysis.runnable, "a library has nothing to run");
    assert!(analysis.diagnostics.contains("A library"));
    // What it can still be asked, and what it cannot.
    assert!(section(&analysis, "place").contains("double"));
    assert!(section(&analysis, "iface").contains("double"));
    assert!(
        analysis.sections.iter().all(|s| s.id != "k8s"),
        "a library was given a deployment"
    );
}

/// Every example the page offers is one it can actually answer for.
#[test]
fn every_example_the_playground_offers_compiles() {
    for (name, source) in beck_play::tab::examples() {
        let analysis = analysed(source);
        assert_eq!(
            analysis.errors, 0,
            "the `{name}` example does not compile:\n{}",
            analysis.diagnostics
        );
    }
}

// ------------------------------------------------------------------ rung B

/// The commands this differential drives, chosen so that every path through the merge point is
/// taken: an accepted command, a refused one, a retry of an accepted one, and a command from a
/// second actor.
fn script() -> Vec<(&'static str, &'static str, serde_json::Value)> {
    vec![
        ("ana", "k1", json!({"c": "Bump", "by": 1})),
        ("bo", "k2", json!({"c": "Bump", "by": 41})),
        ("ana", "k1", json!({"c": "Bump", "by": 1})), // a retry: the same id
        ("ana", "k3", json!({"c": "Bump", "by": 100})), // refused: over the ceiling
        ("bo", "k4", json!({"c": "Reset"})),
        ("ana", "k5", json!({"c": "Bump", "by": 7})),
    ]
}

/// §17.2's guarantee, as a test: the tab is the deployed runtime, on a different host.
///
/// Two applications of the same program — one `beck_rt::App` with its sequencer, its log store and
/// two subscriptions over the socket harness, one `beck_play::Tab` — take the same commands in the
/// same order. After each one, the page every subscriber holds must be the same page, and the
/// answer each proposer got must be the same answer.
#[tokio::test]
async fn the_tab_and_the_server_agree_on_every_state_a_log_can_reach() {
    let placed = compiled(&root().join("crates/beck-play/examples/counter.beck"));
    let app = beck_rt::App::start(
        beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed)).expect("prepares"),
        Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig::default(),
    )
    .await
    .expect("the app starts");

    let mut tab = Tab::load(placed).expect("the tab loads the same program");
    for (sub, actor) in [("s-ana", "ana"), ("s-bo", "bo")] {
        tab.hello(sub, actor, "/", None);
    }

    for (actor, id, command) in script() {
        let decoded = app
            .runtime()
            .decode_command(&command)
            .expect("the command decodes");
        let there = app.propose(id.to_string(), actor, decoded).await;
        let here = tab.command(if actor == "ana" { "s-ana" } else { "s-bo" }, id, &command);

        // The proposer's answer: an ack with a position, or a refusal. Both hosts must give the
        // same one — a tab that accepted what the server refuses would be teaching the wrong
        // authority model, which is the worst thing a playground can do.
        let reply = here
            .iter()
            .find(|o| o.msg["t"] == "a" || o.msg["t"] == "n")
            .map(|o| o.msg.clone())
            .expect("the proposer was answered");
        match there {
            Ok(at) => {
                assert_eq!(
                    reply["t"], "a",
                    "the server accepted `{id}` and the tab did not"
                );
                assert_eq!(
                    reply["q"], at,
                    "the two hosts disagree about where `{id}` landed"
                );
            }
            Err(why) => {
                assert_eq!(
                    reply["t"], "n",
                    "the server refused `{id}` and the tab did not"
                );
                assert_eq!(reply["e"], why, "the two hosts refused `{id}` differently");
            }
        }

        // And the page. Rendered from each host's own accumulator, through each host's own view.
        for actor in ["ana", "bo"] {
            assert_eq!(
                tab.rendered(actor).expect("the tab renders"),
                app.render(actor)
                    .await
                    .expect("the server renders")
                    .render(),
                "after `{id}`, the tab and the server render different pages for {actor}"
            );
        }
    }
}

/// The frames, not just the pages: what the tab puts on its wire is what the server puts on its
/// socket, for the same subscription at the same position.
///
/// Separate from the page comparison above because they can fail apart. A tab that re-sent the
/// whole page on every event would agree about every page and be a different protocol — and would
/// throw away the property Mode A exists for.
#[tokio::test]
async fn the_tab_and_the_server_send_the_same_frames() {
    let placed = compiled(&root().join("crates/beck-play/examples/counter.beck"));
    let app = beck_rt::App::start(
        beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed)).expect("prepares"),
        Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig::default(),
    )
    .await
    .expect("the app starts");

    // A real subscription over the socket harness, so what is compared is what `session::run`
    // wrote — not a second reading of the differ.
    let (to_server, from_client) = tokio::sync::mpsc::unbounded_channel();
    let (to_client, mut from_server) = tokio::sync::mpsc::unbounded_channel();
    let socket = Duplex {
        out: to_client,
        inbox: from_client,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));
    to_server
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"t": "hello", "sub": "s1", "actor": "ana"})
                .to_string()
                .into(),
        ))
        .expect("the hello goes up");
    let opening = drain(&mut from_server).await;

    let mut tab = Tab::load(placed).expect("the tab loads");
    let here = tab.hello("s1", "ana", "/", None);

    assert_eq!(
        frames(&opening, "p"),
        here.iter()
            .filter(|o| o.msg["t"] == "p")
            .map(|o| o.msg["o"].clone())
            .collect::<Vec<_>>(),
        "the first frame differs"
    );

    for (id, command) in [
        ("k1", json!({"c": "Bump", "by": 3})),
        ("k2", json!({"c": "Bump", "by": -1})),
    ] {
        to_server
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"t": "c", "id": id, "command": command})
                    .to_string()
                    .into(),
            ))
            .expect("the command goes up");
        let there = drain(&mut from_server).await;
        let here = tab.command("s1", id, &command);
        assert_eq!(
            frames(&there, "p"),
            here.iter()
                .filter(|o| o.msg["t"] == "p")
                .map(|o| o.msg["o"].clone())
                .collect::<Vec<_>>(),
            "after `{id}` the two hosts sent different patch ops"
        );
    }

    drop(to_server);
    let _ = session.await;
}

fn frames(msgs: &[serde_json::Value], kind: &str) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["t"] == kind)
        .map(|m| m["o"].clone())
        .collect()
}

/// One client's command reaches the other client's page, and an idle subscriber whose page did not
/// move is sent nothing.
///
/// This is the fanout property the whole design exists for, and it is what makes two iframes a
/// demonstration rather than a mirror. `todo.beck`'s view filters by the session, so ana's todo is
/// invisible to bo — and bo, who is not waiting on anything, is owed no frame at all.
#[test]
fn a_command_moves_every_page_it_changes_and_no_others() {
    let mut tab = Tab::load(compiled(&root().join("examples/todo.beck"))).expect("the tab loads");
    tab.hello("s-ana", "ana", "/", None);
    tab.hello("s-bo", "bo", "/", None);

    let out = tab.command(
        "s-ana",
        "k1",
        &json!({"c": "Add", "id": "1", "text": "milk"}),
    );
    let for_bo: Vec<_> = out.iter().filter(|o| o.sub == "s-bo").collect();
    assert!(
        for_bo.is_empty(),
        "an idle subscriber whose page did not change was sent {}",
        for_bo.len()
    );
    assert!(
        out.iter().any(|o| o.sub == "s-ana" && o.msg["t"] == "p"),
        "the client that proposed was not sent its own page"
    );
    assert!(tab.rendered("ana").expect("renders").contains("milk"));
    assert!(!tab.rendered("bo").expect("renders").contains("milk"));
}

/// Who is connected is who is connected *to the tab*, and somebody arriving moves the other pages.
///
/// D6's presence signal is the one input to a view that moves without an event
/// (`docs/48`), so it is also the one place a tab could quietly answer a
/// different question than a server: `beck_host::Runtime::view` renders against the viewer's own
/// roster, which is right for `beck test` and wrong for an application. The tab keeps a roster —
/// its own subscriptions — exactly as `beck_rt::App` keeps a registry.
#[test]
fn presence_in_the_tab_is_who_is_connected_to_the_tab() {
    let mut tab = Tab::load(compiled(&root().join("corpus/32-here.beck"))).expect("the tab loads");

    tab.hello("s-ana", "ana", "/", None);
    assert!(
        tab.rendered("ana").expect("renders").contains("1 here"),
        "one client, one in the roster"
    );

    // The second client arrives. Its own page counts both — and so does the first one's, which is
    // the frame this returns and the reason presence is a *signal* rather than a page's guess.
    let out = tab.hello("s-bo", "bo", "/", None);
    assert!(
        tab.rendered("ana").expect("renders").contains("2 here"),
        "a second connection did not reach the first client's page"
    );
    assert!(
        out.iter().any(|o| o.sub == "s-ana" && o.msg["t"] == "p"),
        "the client already connected was not sent the change"
    );
}

/// A retry is acknowledged with the position the first attempt got, and appends nothing.
///
/// The rule is `beck_host::sequence`'s rather than the tab's, which is the point: the tab inherits
/// it by running that function, and this asserts the inheritance rather than a second copy of the
/// behaviour (`docs/94` §94.10 is what it cost to get wrong).
#[test]
fn a_retried_command_is_acknowledged_and_appended_once() {
    let mut tab = Tab::load(compiled(
        &root().join("crates/beck-play/examples/counter.beck"),
    ))
    .expect("loads");
    tab.hello("s1", "ana", "/", None);

    let first = tab.command("s1", "k1", &json!({"c": "Bump", "by": 1}));
    let at = first
        .iter()
        .find(|o| o.msg["t"] == "a")
        .expect("an ack")
        .msg["q"]
        .clone();
    assert_eq!(tab.head(), 1);

    let again = tab.command("s1", "k1", &json!({"c": "Bump", "by": 1}));
    let reply = &again
        .iter()
        .find(|o| o.msg["t"] != "p")
        .expect("an answer")
        .msg;
    assert_eq!(
        reply["t"], "a",
        "a retry was refused rather than acknowledged"
    );
    assert_eq!(reply["q"], at, "a retry was given a different position");
    assert_eq!(tab.head(), 1, "a retry was appended a second time");
}

/// The scrubber folds the log; it does not replay a recording.
///
/// Every position in the history has to render the page that position's *state* produces, computed
/// by the same fold — which is what makes dragging it a demonstration of determinism rather than
/// an undo stack. Asserted against a second, independent fold of the same log.
#[test]
fn the_scrubber_renders_the_state_the_log_produces_at_every_position() {
    let placed = compiled(&root().join("crates/beck-play/examples/counter.beck"));
    let mut tab = Tab::load(placed.clone()).expect("loads");
    tab.hello("s1", "ana", "/", None);
    for (i, by) in [1, 2, 3, 4].into_iter().enumerate() {
        tab.command("s1", &format!("k{i}"), &json!({"c": "Bump", "by": by}));
    }
    assert_eq!(tab.head(), 4);

    // The oracle is the *other* host: the same commands through a `beck_rt::App`, with its page
    // captured after each one. Independent of the tab in the way that matters — a different log,
    // a different sequencer, the same program.
    let expected = support::browser::runtime().block_on(async {
        let app = beck_rt::App::start(
            beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed)).expect("prepares"),
            Arc::new(beck_rt::MemoryLog::new()),
            beck_rt::AppConfig::default(),
        )
        .await
        .expect("the app starts");
        let mut pages = vec![app.render("ana").await.expect("renders").render()];
        for (i, by) in [1, 2, 3, 4].into_iter().enumerate() {
            let command = app
                .runtime()
                .decode_command(&json!({"c": "Bump", "by": by}))
                .expect("decodes");
            app.propose(format!("k{i}"), "ana", command)
                .await
                .expect("accepted");
            pages.push(app.render("ana").await.expect("renders").render());
        }
        pages
    });

    for (seq, want) in expected.iter().enumerate() {
        assert_eq!(
            &tab.page_at(seq as u64, "ana")
                .expect("the scrubber")
                .render(),
            want,
            "the page at seq {seq} is not the page that log produces"
        );
    }
}

// ------------------------------------------------------------------ Mode B, in the tab

/// A `@render(client)` program's subscription carries the **state**, not the page.
///
/// The mode is one branch in a subscription and this is it, on the host that is a browser tab:
/// `board.beck` renders in the client, so what a `hello` gets back is `{"t":"s"}` with the whole
/// accumulator and what a command gets back is `{"t":"d"}` with the difference. A tab that sent DOM
/// patches to a kernel would be sending frames nothing on the other end applies.
#[test]
fn a_mode_b_subscription_carries_the_state_rather_than_the_page() {
    let placed = compiled(&root().join("examples/board.beck"));
    let mut tab = Tab::load(placed).expect("loads");
    assert_eq!(tab.mode(), beck_core::render::Mode::Client);

    let opening = tab.hello("s1", "ana", "/", None);
    assert_eq!(opening[0].msg["t"], "w", "a welcome first, in either mode");
    assert_eq!(
        opening[1].msg["t"], "s",
        "a fresh Mode B subscription is sent the whole accumulator"
    );
    assert!(opening[1].msg["v"].is_object() || opening[1].msg["v"].is_array());

    let after = tab.command("s1", "k1", &json!({"c": "Add", "id": "1", "text": "milk"}));
    assert_eq!(after[0].msg["t"], "a", "the proposer is acknowledged");
    let data: Vec<&beck_play::tab::Outgoing> = after.iter().filter(|o| o.msg["t"] == "d").collect();
    assert_eq!(data.len(), 1, "one data frame, to the one subscriber");
    assert!(
        !data[0].msg["o"].as_array().expect("ops").is_empty(),
        "the state moved and the frame says how"
    );
    assert!(
        after.iter().all(|o| o.msg["t"] != "p"),
        "no DOM patch reaches a client that is rendering for itself"
    );
}

/// And they are the *server's* frames: the same subscription, the same ops.
///
/// The Mode A half of this claim is `the_tab_and_the_server_send_the_same_frames`; this is the half
/// §98.9 could not make, because the tab did not serve the mode. Compared against a real
/// subscription over the socket harness, so what is on the left is what `session::mode_b` wrote.
#[tokio::test]
async fn the_tab_and_the_server_send_the_same_data_frames() {
    let placed = compiled(&root().join("examples/board.beck"));
    let app = beck_rt::App::start(
        beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed)).expect("prepares"),
        Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig::default(),
    )
    .await
    .expect("the app starts");

    let (to_server, from_client) = tokio::sync::mpsc::unbounded_channel();
    let (to_client, mut from_server) = tokio::sync::mpsc::unbounded_channel();
    let socket = Duplex {
        out: to_client,
        inbox: from_client,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));
    to_server
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"t": "hello", "sub": "s1", "actor": "ana"})
                .to_string()
                .into(),
        ))
        .expect("the hello goes up");
    let opening = drain(&mut from_server).await;

    let mut tab = Tab::load(placed).expect("the tab loads");
    let here = tab.hello("s1", "ana", "/", None);

    // The whole accumulator, as each host encoded it. This is the frame a kernel calls `reset` on,
    // so a difference here is a client that renders a different first page than the server does.
    let state_of = |msgs: &[serde_json::Value]| {
        msgs.iter()
            .find(|m| m["t"] == "s")
            .map(|m| m["v"].clone())
            .expect("a state frame")
    };
    assert_eq!(
        state_of(&opening),
        state_of(&here.iter().map(|o| o.msg.clone()).collect::<Vec<_>>()),
        "the two hosts encoded the same accumulator differently"
    );

    for (id, command) in [
        ("k1", json!({"c": "Add", "id": "1", "text": "milk"})),
        ("k2", json!({"c": "Move", "id": "1", "column": 1})),
    ] {
        to_server
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"t": "c", "id": id, "command": command})
                    .to_string()
                    .into(),
            ))
            .expect("the command goes up");
        let there = drain(&mut from_server).await;
        let here = tab.command("s1", id, &command);
        assert_eq!(
            frames(&there, "d"),
            here.iter()
                .filter(|o| o.msg["t"] == "d")
                .map(|o| o.msg["o"].clone())
                .collect::<Vec<_>>(),
            "after `{id}` the two hosts sent different data ops"
        );
    }

    drop(to_server);
    let _ = session.await;
}

/// The bundle a Mode B client renders from is the running program's own slice.
///
/// Not a file and not a build artefact: it is derived from the `Placed` the tab is executing, which
/// is what makes "the tab cannot hand a client a bundle it is not itself executing" true here for
/// the same reason it is true of `beck run`.
#[test]
fn the_bundle_the_tab_hands_over_is_the_program_it_is_running() {
    let placed = compiled(&root().join("examples/board.beck"));
    let tab = Tab::load(placed.clone()).expect("loads");
    assert_eq!(tab.bundle(), beck_core::Bundle::of(&placed).to_bytes());
    assert!(!tab.bundle().is_empty());
}

/// A route is part of the session in the tab, in both modes.
///
/// `docs/94` made the route a field of the session, and a host that ignored the `g` frame would be
/// running the program against a session no deployment builds — silently, because a page that does
/// not read `session.path` cannot tell.
#[test]
fn a_client_that_navigates_is_a_client_somewhere_else() {
    let source = std::fs::read_to_string(root().join("examples/routed.beck")).expect("the example");
    let (placed, diags, map) = beck_core::compile_str("routed.beck", &source);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let mut tab = Tab::load(placed.expect("an application")).expect("loads");

    tab.hello("s1", "ana", "/", None);
    let at_root = tab
        .rendered(&beck_rt::program::At {
            who: "ana".to_string(),
            path: "/".into(),
        })
        .expect("renders");

    let out = tab.nav("s1", "/done");
    let moved = tab
        .rendered(&beck_rt::program::At {
            who: "ana".to_string(),
            path: "/done".into(),
        })
        .expect("renders");
    assert_ne!(
        at_root, moved,
        "this example's page is a function of the route"
    );
    assert!(
        out.iter().any(|o| o.msg["t"] == "p"),
        "a Mode A client that navigated is sent the page it navigated to"
    );
}

// ------------------------------------------------------------------ the log a reload survives

/// A log handed out as records and read back is the same log.
///
/// The oracle is the tab that produced them: every page at every position must be the page the
/// restored tab produces, which is the same comparison the scrubber test makes and for the same
/// reason — a store that lost an event, reordered two, or dropped an actor would still *have* a
/// log, and only a fold would say so.
#[test]
fn a_log_kept_by_the_page_is_the_log_the_tab_had() {
    let placed = compiled(&root().join("examples/todo.beck"));
    let mut first = Tab::load(placed.clone()).expect("loads");
    first.hello("s-ana", "ana", "/", None);
    for (id, command) in [
        ("k1", json!({"c": "Add", "id": "1", "text": "milk"})),
        ("k2", json!({"c": "Add", "id": "2", "text": "bread"})),
    ] {
        first.command("s-ana", id, &command);
    }
    assert_eq!(first.head(), 2);

    let records = first.records(0).expect("the records");
    assert_eq!(records.len(), 2);

    let mut back = Tab::load(placed).expect("loads");
    assert_eq!(back.restore(&records).expect("it restores"), 2);
    for seq in 0..=2 {
        assert_eq!(
            back.page_at(seq, "ana")
                .expect("the restored page")
                .render(),
            first.page_at(seq, "ana").expect("the page").render(),
            "the restored log renders a different page at {seq}"
        );
    }
    // And the incremental half: a page that has stored up to 1 asks for the rest, and gets it.
    assert_eq!(first.records(1).expect("the rest").len(), 1);
}

/// The two ways a restore is wrong, refused rather than folded.
#[test]
fn a_restore_into_a_running_tab_or_of_a_gapped_log_is_refused() {
    let placed = compiled(&root().join("examples/todo.beck"));
    let mut tab = Tab::load(placed.clone()).expect("loads");
    tab.hello("s-ana", "ana", "/", None);
    tab.command(
        "s-ana",
        "k1",
        &json!({"c": "Add", "id": "1", "text": "milk"}),
    );
    let records = tab.records(0).expect("the records");

    // Into a tab that has already rendered: a restore afterwards is history rewritten under a
    // client that has seen it.
    let why = tab.restore(&records).expect_err("it is refused");
    assert!(why.contains("has not run yet"), "{why}");

    // And a log with a hole in it. Dense `seq`s from 1 is the contract every fold in this
    // repository depends on, so a store that dropped a record has to be caught here rather than
    // produce a state no history could have reached.
    tab.command(
        "s-ana",
        "k2",
        &json!({"c": "Add", "id": "2", "text": "bread"}),
    );
    let mut gapped = tab.records(0).expect("the records");
    gapped.remove(0);
    let mut fresh = Tab::load(placed).expect("loads");
    let why = fresh.restore(&gapped).expect_err("it is refused");
    assert!(why.contains("contiguous"), "{why}");
}

// ------------------------------------------------------------------ sharing (§17.4)

/// A share link is the program, and it is named by its digest.
///
/// The digest is `beck_core::digest::of` — the same BLAKE3 a Beck program's own `digest()` computes
/// — so "content-addressed" is the compiler's addressing rather than the playground's. What is
/// gated here is the round trip through the module's boundary, because that is what the page uses:
/// a link that opened as a *different program* than the digest names would be the failure worth
/// preventing, and `share.rs` has the unit tests for the refusals.
#[test]
fn a_share_link_carries_the_program_and_is_named_by_its_digest() {
    let mut state = Playground::new();
    let source = std::fs::read_to_string(root().join("crates/beck-play/examples/counter.beck"))
        .expect("the example");

    let link = dispatch(&mut state, &json!({"op": "share", "source": source})).expect("a link");
    assert_eq!(link["digest"], beck_core::digest::of(&source));
    let fragment = link["fragment"].as_str().expect("a fragment");
    assert!(
        fragment.starts_with(&beck_core::digest::of(&source)[..16]),
        "a link names the program it carries"
    );

    let opened = dispatch(&mut state, &json!({"op": "open", "fragment": fragment})).expect("opens");
    assert_eq!(opened["source"], source);
    assert_eq!(opened["digest"], link["digest"]);

    // A link nobody can open is an error with a reason, not a blank editor.
    let why = dispatch(&mut state, &json!({"op": "open", "fragment": "not-a-link"}))
        .expect_err("it is refused");
    assert!(why.contains("share link"), "{why}");
}

// ------------------------------------------------------------------ the editor (docs/65, §98.9)

/// The playground's editor is the language server's, on a different host.
///
/// §98.9's last-but-one item was "a `<textarea>`, with no highlighting, no completion and no inline
/// diagnostics — which is odd given `docs/65` built an LSP over this same front end". The answer is
/// not a second implementation in JavaScript: both ask `beck_core::editor`, and this drives the
/// **`beck lsp` binary** over stdio and the playground module over its own boundary, on the same
/// source, and compares the answers.
///
/// It goes red the day somebody highlights a keyword in the page's own JavaScript.
#[test]
fn the_playground_and_the_language_server_answer_the_same_questions() {
    let source = std::fs::read_to_string(root().join("crates/beck-play/examples/counter.beck"))
        .expect("the example");
    let mut state = Playground::new();

    // Highlighting. The server's encoding is deltas of lines and UTF-16 columns; the playground's
    // is flat UTF-16 offsets. Both are converted to (line, column, length, type) here, which is
    // the only shape in which two encodings of one answer can be compared at all.
    let legend = lsp_legend();
    let theirs = lsp_semantic_tokens(&source, &legend);
    let ours = dispatch(&mut state, &json!({"op": "tokens", "source": source})).expect("tokens")
        ["tokens"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|t| {
            let (line, column) = utf16_line_col(&source, t["s"].as_u64().expect("an offset"));
            let length = t["e"].as_u64().expect("an offset") - t["s"].as_u64().expect("an offset");
            (
                line,
                column,
                length,
                lsp_type_of(t["k"].as_str().expect("a kind")),
            )
        })
        .collect::<Vec<_>>();
    assert!(!ours.is_empty(), "the playground highlighted nothing");
    assert_eq!(
        ours, theirs,
        "the page and the editor colour different things"
    );

    // Completion, at the end of a name the program declares — in UTF-16 units, which is what both
    // hosts count in and what this file has to convert to in order to ask the same question twice.
    let caret = utf16_of(
        &source,
        source.find("def view").expect("counter has a view") + 8,
    );
    let theirs = lsp_completion(&source, caret);
    let ours = dispatch(
        &mut state,
        &json!({"op": "complete", "source": source, "offset": caret}),
    )
    .expect("completions")["items"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|c| {
            (
                c["label"].as_str().unwrap_or_default().to_string(),
                c["detail"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert!(
        ours.iter().any(|(label, _)| label == "view"),
        "the name under the caret is not offered: {ours:?}"
    );
    assert_eq!(
        ours, theirs,
        "the page and the editor offer different names"
    );
}

/// Highlighting, completion and squiggles on a file that does not compile — which is the state a
/// file being typed into is in, and the state in which an editor is worth having.
#[test]
fn the_editor_answers_while_the_file_is_half_written() {
    let mut state = Playground::new();
    let good = std::fs::read_to_string(root().join("crates/beck-play/examples/counter.beck"))
        .expect("the example");
    // One good analysis first, exactly as a page has after the debounce.
    dispatch(
        &mut state,
        &json!({"op": "complete", "source": good, "offset": 0}),
    )
    .expect("names");

    let broken = format!("{good}\ndef half() -> Int:\n    return pag\n");
    let caret = utf16_of(&broken, broken.rfind("pag").expect("it is there") + 3);

    // Colours: still a lex, so `def` is still a keyword.
    let tokens = dispatch(&mut state, &json!({"op": "tokens", "source": broken})).expect("tokens");
    assert!(
        tokens["tokens"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|t| t["k"] == "keyword"),
        "highlighting stopped when the program did"
    );

    // Names: the last analysis's, and the module says they are.
    let offered = dispatch(
        &mut state,
        &json!({"op": "complete", "source": broken, "offset": caret}),
    )
    .expect("completions");
    assert_eq!(offered["prefix"], "pag");
    assert_eq!(offered["stale"], true, "a borrowed name table says so");
    assert!(
        offered["items"]
            .as_array()
            .expect("a list")
            .iter()
            .any(|c| c["label"] == "page"),
        "a half-typed name completes from the last text that checked"
    );

    // And the squiggle is under the word that is wrong, in the units the editor counts in.
    let analysis =
        dispatch(&mut state, &json!({"op": "analyse", "source": broken})).expect("an analysis");
    let marks = analysis["marks"].as_array().expect("a list");
    assert!(
        !marks.is_empty(),
        "an error with no span is an error nobody can see"
    );
    let first = &marks[0];
    let (start, end) = (
        first["s"].as_u64().expect("a start") as usize,
        first["e"].as_u64().expect("an end") as usize,
    );
    let utf16: Vec<u16> = broken.encode_utf16().collect();
    let under = String::from_utf16(&utf16[start..end]).expect("a span");
    assert!(under.contains("pag"), "the squiggle is under `{under}`");
    assert_eq!(first["error"], true);
    assert!(first["message"]
        .as_str()
        .expect("a message")
        .contains("pag"));
}

/// The one URI these two tests open, and the one name both hosts compile under.
const EDITED: &str = "file:///playground.beck";

fn lsp_legend() -> Vec<String> {
    let mut server = support::lsp::Server::start();
    let reply = server.request(
        "initialize",
        json!({ "processId": null, "rootUri": null, "capabilities": {} }),
    );
    let legend = reply
        .pointer("/result/capabilities/semanticTokensProvider/legend/tokenTypes")
        .and_then(|t| t.as_array())
        .expect("the server publishes a semantic-token legend")
        .iter()
        .map(|t| t.as_str().unwrap_or_default().to_string())
        .collect();
    server.notify("initialized", json!({}));
    server.shutdown();
    legend
}

/// The server's highlighting, decoded out of the protocol's delta encoding.
fn lsp_semantic_tokens(source: &str, legend: &[String]) -> Vec<(u64, u64, u64, String)> {
    let mut server = support::lsp::handshake();
    server.open(EDITED, source);
    let reply = server.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": EDITED } }),
    );
    let data: Vec<u64> = reply
        .pointer("/result/data")
        .and_then(|d| d.as_array())
        .expect("five integers per token")
        .iter()
        .map(|v| v.as_u64().expect("an integer"))
        .collect();
    server.shutdown();

    let (mut line, mut column) = (0u64, 0u64);
    data.chunks(5)
        .map(|t| {
            line += t[0];
            column = if t[0] == 0 { column + t[1] } else { t[1] };
            (line, column, t[2], legend[t[3] as usize].clone())
        })
        .collect()
}

/// The server's completions, as label and detail.
fn lsp_completion(source: &str, caret: u64) -> Vec<(String, String)> {
    let (line, character) = utf16_line_col(source, caret);
    let mut server = support::lsp::handshake();
    server.open(EDITED, source);
    let reply = server.request(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": EDITED },
            "position": { "line": line, "character": character },
        }),
    );
    let items = reply
        .pointer("/result/items")
        .and_then(|i| i.as_array())
        .expect("a completion list")
        .iter()
        .map(|c| {
            (
                c["label"].as_str().unwrap_or_default().to_string(),
                c["detail"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    server.shutdown();
    items
}

/// A byte offset as a UTF-16 offset — the units every editor answer crosses a boundary in, because
/// a `<textarea>` and the protocol both count in them and only the compiler counts in bytes.
fn utf16_of(source: &str, byte: usize) -> u64 {
    source[..byte].encode_utf16().count() as u64
}

/// A UTF-16 offset as the protocol's line and character. The playground counts in flat offsets and
/// the protocol counts in lines, so one of them has to be converted to compare them at all.
fn utf16_line_col(source: &str, offset: u64) -> (u64, u64) {
    let (mut line, mut column, mut at) = (0u64, 0u64, 0u64);
    for c in source.chars() {
        if at >= offset {
            break;
        }
        at += c.len_utf16() as u64;
        if c == '\n' {
            line += 1;
            column = 0;
        } else {
            column += c.len_utf16() as u64;
        }
    }
    (line, column)
}

/// The playground's category name, as the protocol's token type — the mapping the compiler
/// publishes, so this test cannot silently agree with itself.
fn lsp_type_of(kind: &str) -> String {
    use beck_core::editor::TokenKind;
    for k in [
        TokenKind::Keyword,
        TokenKind::Name,
        TokenKind::Atom,
        TokenKind::Number,
        TokenKind::Str,
        TokenKind::Comment,
        TokenKind::Doc,
        TokenKind::Punct,
    ] {
        if k.name() == kind {
            return k.lsp_type().to_string();
        }
    }
    panic!("the playground used a token kind the compiler does not define: `{kind}`");
}

// ------------------------------------------------------------------ the artefact

/// Every file the page asks a browser for is a file the playground ships.
///
/// `docs/94` §94.13 is the reason this exists: the served document referenced an element that was
/// never in it, in every browser, since Phase 1, with every test in the workspace green. A missing
/// asset is the same class of defect and this is the cheapest place to catch it.
#[test]
fn the_bundle_carries_everything_the_page_asks_for() {
    let shipped: Vec<&str> = beck_play::serve::bundle().iter().map(|a| a.path).collect();
    let referenced = beck_play::serve::bundle()
        .iter()
        .flat_map(|a| references(a.body))
        .collect::<Vec<_>>();
    assert!(!referenced.is_empty(), "the page references nothing at all");
    // The three names that are asked for and are deliberately not in the list: two WebAssembly
    // modules, which are build artefacts copied in by `serve::write` because building them needs a
    // target the compiler's own build does not — and the bundle, which is not a file at all. It is
    // derived from the running program and handed to the iframe over the port, so a client can
    // never load a slice of a program the tab is not executing (docs/103).
    let elsewhere = ["beck-play.wasm", "beck-kernel.wasm", "beck-bundle.bpk"];
    for name in referenced {
        assert!(
            shipped.contains(&name.as_str()) || elsewhere.contains(&name.as_str()),
            "the page asks for `{name}` and the bundle does not carry it: {shipped:?}"
        );
    }
    // And the two modules are what the writer writes, so "copied in elsewhere" is a fact rather
    // than an excuse.
    for name in ["beck-play.wasm", "beck-kernel.wasm"] {
        assert!(
            beck_play::serve::written_modules().contains(&name),
            "`{name}` is asked for and `serve::write` does not write it"
        );
    }
}

/// Every name a shipped file asks a browser for.
///
/// **Every quoted string that looks like a file**, rather than the arguments of the three or four
/// syntactic forms that happened to be in the page when this was written. The page asks in markup
/// (`src=`), in JavaScript (`new Worker`), through `beck.asset` — which is how Mode B's residue
/// asks for the kernel and its bundle — and through a ternary that names two files and is the
/// argument of nothing at all. A scanner keyed on forms would have missed the last two; a scanner
/// keyed on what a filename looks like cannot.
fn references(body: &str) -> Vec<String> {
    let extensions = [".js", ".css", ".wasm", ".bpk", ".html"];
    let mut out = Vec::new();
    for piece in body.split(['"', '\'', '`']) {
        let name = piece.trim();
        if name.starts_with("http") || name.contains(['$', ' ', '/', '<']) {
            continue;
        }
        if extensions.iter().any(|e| name.ends_with(e)) {
            out.push(name.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The playground's client is the deployed client, not a copy of it.
///
/// §17.2 says the tab holds "the thin patch client … speaking the identical patch/command protocol
/// over a `MessageChannel` instead of a websocket". A playground that shipped its own patch
/// interpreter would be demonstrating a second client, and every claim about parity would be about
/// the wrong program.
#[test]
fn the_playground_serves_the_runtimes_own_residue() {
    let bundle = beck_play::serve::bundle();
    let of = |name: &str| {
        bundle
            .iter()
            .find(|a| a.path == name)
            .unwrap_or_else(|| panic!("the bundle carries {name}"))
            .body
    };
    assert_eq!(of("beck-patch.js"), beck_rt::PATCH_CLIENT);
    assert_eq!(of("beck-thin.js"), beck_rt::THIN_CLIENT);
    // And the one file that is the playground's own is the *only* one: a transport, and nothing
    // that interprets a patch or decides anything.
    let port = of("beck-play-port.js");
    assert!(
        port.contains("beck.dial"),
        "the port file is not a transport"
    );
    assert!(
        !port.contains("apply(") && !port.contains("createElement"),
        "the playground's transport has started interpreting patches"
    );
}

// The exception to `forbid(unsafe_code)` — three export attributes and no `unsafe` code — is
// gated in `mode_b.rs::the_wasm_boundary_is_the_only_exception_to_forbid_unsafe`, which counts it
// per crate and asserts that every *other* crate still inherits the workspace lint. One gate rather
// than one per module: the property is about the workspace, and two copies of it would be two
// things to keep in step (`docs/98` §98.3).

/// The module builds for the browser.
///
/// Skips loudly without the target, as every environment-dependent suite here does;
/// `BECK_REQUIRE_WASM=1` forbids the skip, which is what CI sets. A playground that only ever
/// builds for this machine is not a playground.
///
/// The absent target and a failed build are two different findings, and only the first is a skip:
/// a dependency that cannot compile for wasm32 is a defect in the dependency graph, and reporting
/// it as "no target installed" is how it would reach CI unnoticed.
#[test]
fn the_playground_builds_for_the_browser() {
    let built = std::process::Command::new(env!("CARGO"))
        .current_dir(root())
        .args([
            "build",
            "-p",
            "beck-play",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output();
    let module = root().join("target/wasm32-unknown-unknown/release/beck_play.wasm");
    let installed = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"));
    if !installed {
        eprintln!(
            "skipped: no `wasm32-unknown-unknown` target. \
             `rustup target add wasm32-unknown-unknown` installs it."
        );
        assert!(
            std::env::var("BECK_REQUIRE_WASM").as_deref() != Ok("1"),
            "BECK_REQUIRE_WASM=1 but the wasm32-unknown-unknown target is not installed"
        );
        return;
    }
    let out = built.expect("cargo runs");
    assert!(
        out.status.success() && module.is_file(),
        "the target is installed and the playground still did not build for the browser:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A ceiling rather than a budget: enough to catch the module becoming a different kind of
    // object, and deliberately not a size gate, which would flake on a machine without `brotli`
    // (`docs/13` §13.7).
    let bytes = module.metadata().expect("its size").len();
    assert!(bytes < 16 * 1024 * 1024, "the module is {bytes} bytes");
}

/// One request in, one answer out — the boundary the browser actually crosses.
///
/// `dispatch` is what the three exports call, so driving it here is driving the module; what the
/// exports add is pointers, and `mode_b.rs` already establishes that shape.
#[test]
fn the_module_answers_the_requests_the_page_makes() {
    let mut state = Playground::new();
    let source = std::fs::read_to_string(root().join("crates/beck-play/examples/counter.beck"))
        .expect("the example");

    let analysis =
        dispatch(&mut state, &json!({"op": "analyse", "source": source})).expect("an analysis");
    assert_eq!(analysis["errors"], 0);
    assert_eq!(analysis["runnable"], true);

    // Nothing is running yet, and the module says so rather than answering anyway.
    assert!(dispatch(&mut state, &json!({"op": "history"})).is_err());

    let loaded = dispatch(
        &mut state,
        &json!({"op": "load", "source": source, "now": 7}),
    )
    .expect("it loads");
    assert_eq!(loaded["mode"], "a");
    assert_eq!(loaded["head"], 0);

    let welcome = dispatch(
        &mut state,
        &json!({"op": "hello", "sub": "s1", "actor": "ana"}),
    )
    .expect("a welcome");
    assert_eq!(welcome["out"][0]["msg"]["t"], "w");

    let after = dispatch(
        &mut state,
        &json!({"op": "command", "sub": "s1", "id": "k1",
                "command": {"c": "Bump", "by": 2}, "now": 8}),
    )
    .expect("a command");
    assert_eq!(after["out"][0]["msg"]["t"], "a");

    let history = dispatch(&mut state, &json!({"op": "history"})).expect("a history");
    assert_eq!(history["head"], 1);
    assert_eq!(history["events"][0]["actor"], "ana");
    // The envelope carries the page's clock reading, as data — §3.7's "the one place time enters",
    // supplied rather than read.
    assert_eq!(history["events"][0]["at"], 8);

    let at = dispatch(&mut state, &json!({"op": "at", "seq": 0, "actor": "ana"})).expect("a page");
    assert!(at["html"].as_str().expect("markup").contains(">0<"));
    let now = dispatch(&mut state, &json!({"op": "rendered", "actor": "ana"})).expect("a page");
    assert!(now["html"].as_str().expect("markup").contains(">2<"));

    assert!(dispatch(&mut state, &json!({"op": "fly"})).is_err());
}
