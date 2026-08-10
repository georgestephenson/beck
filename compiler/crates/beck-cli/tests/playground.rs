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
        tab.hello(sub, actor, None);
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
    let here = tab.hello("s1", "ana", None);

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
    tab.hello("s-ana", "ana", None);
    tab.hello("s-bo", "bo", None);

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
/// (`docs/96`), so it is also the one place a tab could quietly answer a
/// different question than a server: `beck_host::Runtime::view` renders against the viewer's own
/// roster, which is right for `beck test` and wrong for an application. The tab keeps a roster —
/// its own subscriptions — exactly as `beck_rt::App` keeps a registry.
#[test]
fn presence_in_the_tab_is_who_is_connected_to_the_tab() {
    let mut tab = Tab::load(compiled(&root().join("corpus/32-here.beck"))).expect("the tab loads");

    tab.hello("s-ana", "ana", None);
    assert!(
        tab.rendered("ana").expect("renders").contains("1 here"),
        "one client, one in the roster"
    );

    // The second client arrives. Its own page counts both — and so does the first one's, which is
    // the frame this returns and the reason presence is a *signal* rather than a page's guess.
    let out = tab.hello("s-bo", "bo", None);
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
/// behaviour (`docs/94` §94.13 is what it cost to get wrong).
#[test]
fn a_retried_command_is_acknowledged_and_appended_once() {
    let mut tab = Tab::load(compiled(
        &root().join("crates/beck-play/examples/counter.beck"),
    ))
    .expect("loads");
    tab.hello("s1", "ana", None);

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
    tab.hello("s1", "ana", None);
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

/// A program whose page renders on the client is refused by the tab, rather than served wrongly.
///
/// The tab holds a Mode A subscription: the server renders and sends DOM patches. A Mode B
/// component would need the *kernel* in the client iframe and its bundle over the port, which is
/// §98.7's item rather than a thing to half-do.
#[test]
fn a_mode_b_program_is_named_rather_than_served_as_mode_a() {
    let placed = compiled(&root().join("examples/board.beck"));
    let tab = Tab::load(placed).expect("loads");
    assert_eq!(tab.mode(), beck_core::render::Mode::Client);
}

// ------------------------------------------------------------------ the artefact

/// Every file the page asks a browser for is a file the playground ships.
///
/// `docs/94` §94.7 is the reason this exists: the served document referenced an element that was
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
    for name in referenced {
        assert!(
            shipped.contains(&name.as_str()) || name == "beck-play.wasm",
            "the page asks for `{name}` and the bundle does not carry it: {shipped:?}"
        );
    }
}

/// Every `src="…"`, `href="…"` and `new Worker("…")` in a shipped file, as a bare name.
fn references(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (marker, open) in [("src=\"", '"'), ("href=\"", '"'), ("Worker(\"", '"')] {
        let mut rest = body;
        while let Some(at) = rest.find(marker) {
            rest = &rest[at + marker.len()..];
            let Some(end) = rest.find(open) else { break };
            let name = &rest[..end];
            if !name.starts_with("http") && !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
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
// things to keep in step (`docs/98` §98.5).

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
