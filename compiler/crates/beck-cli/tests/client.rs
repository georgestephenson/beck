//! Client polish: the router, forms, and what a devtools panel is allowed to know.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) Phase 3's client bullet — "router, forms,
//! lazy routes, focus/scroll preservation, devtools" — and
//! [`docs/94`](../../../../docs/94-the-client-report.md) is what was built of it. This gates the
//! half that does not need a browser; `browser.rs` gates the half that does, because focus and
//! scroll are facts about a DOM and nothing here has one.
//!
//! The load-bearing test is the first one. A route is `session.path`, so the compiler's account of
//! *which* half of a session a page reads is what decides whether it may render in a browser — and
//! that account is the thing which, if it were wrong in the permissive direction, would ship a
//! filtered page's whole state to every actor.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use beck_core::render::{Mode, SessionUse};
use beck_core::Placed;
use beck_rt::{App, AppConfig, MemoryLog};
use tokio::sync::mpsc::unbounded_channel;
use tokio_tungstenite::tungstenite::Message;

mod support;
use support::socket::{drain, Duplex};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

fn source(name: &str) -> String {
    std::fs::read_to_string(root().join("examples").join(name))
        .unwrap_or_else(|_| panic!("examples/{name}"))
}

fn compile(name: &str, src: &str) -> Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

fn refusal(name: &str, src: &str) -> String {
    let (_, diags, map) = beck_core::compile_str(name, src);
    assert!(
        diags.has_errors(),
        "this program was supposed to be refused"
    );
    diags.render(&map)
}

/// The same program with the one line that moves the render to the browser.
fn as_mode_b(src: &str) -> String {
    src.replace("@on(client)\npage:", "@on(client)\n@render(client)\npage:")
}

// --------------------------------------------------- what a Mode B page may read

/// The distinction the router is built on: **where** is not **who**.
///
/// `examples/routed.beck` and `examples/todo.beck` are both `per_session` pages — the coarse fact
/// §3.8's fanout analysis and §5.3's shared cut read is *true of both*. One of them may render in
/// a browser and the other may not, and the difference is which field of the session it reads.
/// Before this, `per_session` was the whole answer and a page that varied by route was refused
/// Mode B for a reason that was about identity.
#[test]
fn a_page_may_be_a_function_of_where_the_browser_is_and_not_of_who_holds_it() {
    let routed = compile("routed.beck", &as_mode_b(&source("routed.beck")));
    assert_eq!(routed.render.mode, Mode::Client);
    assert_eq!(routed.render.uses, SessionUse::Route);
    // …and it is still per-session, which is the point: eligibility and fanout are two questions.
    assert!(
        routed.render.per_session,
        "a page that reads the route renders differently for two subscribers, so its operators \
         below the session are theirs"
    );

    let out = refusal("todo.beck", &as_mode_b(&source("todo.beck")));
    assert!(out.contains("B0514"), "{out}");
    assert!(out.contains("`session.actor`"), "{out}");
    // The refusal has to say what is allowed as well as what is not, because the author's next
    // question is "then how do I have routes".
    assert!(out.contains("Reading `session.path` is allowed"), "{out}");
}

/// The analysis reads the view's code and everything it reaches, so a read through a helper counts.
#[test]
fn a_read_of_the_session_through_a_helper_is_still_a_read() {
    let direct = compile("t.beck", &program("session.actor"));
    assert!(
        direct.render.uses.reads_identity(),
        "{:?}",
        direct.render.uses
    );

    let indirect = compile("t.beck", &program("who(session)"));
    assert!(
        indirect.render.uses.reads_identity(),
        "a read one call away is still a read: {:?}",
        indirect.render.uses
    );

    let route = compile("t.beck", &program("session.path"));
    assert_eq!(route.render.uses, SessionUse::Route);

    let none = compile("t.beck", &program("\"nobody\""));
    assert_eq!(none.render.uses, SessionUse::None);
}

/// The conservative half, and the reason it has to be there.
///
/// A field read is the only way to *observe* a session, so collecting field reads is sound — for
/// everything except an operation that consumes the record whole. A comparison is one. If this
/// went the other way, a page could branch on `session == …` and be called route-only.
#[test]
fn a_session_that_is_compared_rather_than_read_is_identity() {
    let compared = compile("t.beck", &program("marker(session)"));
    assert!(
        compared.render.uses.reads_identity(),
        "a session handed to a primitive is not a route read: {:?}",
        compared.render.uses
    );
}

fn program(what: &str) -> String {
    format!(
        "model State:\n\
         \x20   n: Int\n\
         union Command:\n\
         \x20   Bump\n\
         union Event:\n\
         \x20   Bumped\n\
         union Rejection:\n\
         \x20   No\n\
         def apply_event(s: State, env: Envelope[Event]) -> State:\n\
         \x20   return s.with(n=s.n + 1)\n\
         def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:\n\
         \x20   return Ok(value=[Bumped])\n\
         def who(session: Session) -> Str:\n\
         \x20   return session.actor\n\
         def marker(session: Session) -> Str:\n\
         \x20   return \"same\" if session == session else \"other\"\n\
         def view(s: State, session: Session) -> Html:\n\
         \x20   return page_of({what})\n\
         def page_of(label: Str) -> Html:\n\
         \x20   return ui:\n\
         \x20       main:\n\
         \x20           h1: label\n\
         @on(server)\n\
         proposals: Stream[Proposal] = merge_clients()\n\
         @on(server)\n\
         events: Stream[Event] = decide(proposals, state, validate)\n\
         @on(data)\n\
         state: Signal[State] = durable(fold(apply_event, State(n=0), events))\n\
         @on(client)\n\
         page: Signal[Html] = per_session(state, view)\n"
    )
}

// --------------------------------------------------- the route reaches the page

/// A route is data the edge supplies, exactly as an actor is — so the page is a pure function of
/// it and the runtime holds no route table at all.
#[tokio::test]
async fn the_page_a_route_renders_is_the_route_the_viewer_names() {
    let placed = compile("routed.beck", &source("routed.beck"));
    let app = app_for(placed).await;
    let command = app
        .runtime()
        .decode_command(&serde_json::json!({"c":"Add","id":"t1","text":"milk"}))
        .expect("decodes");
    app.propose("k1".into(), "ana", command).await.expect("ok");
    let toggle = app
        .runtime()
        .decode_command(&serde_json::json!({"c":"Toggle","id":"t1"}))
        .expect("decodes");
    app.propose("k2".into(), "ana", toggle).await.expect("ok");

    let at = |path: &str| beck_rt::program::At {
        who: Arc::<str>::from("ana"),
        path: Arc::from(path),
    };
    let root = app.render(&at("/")).await.expect("renders").render();
    let done = app.render(&at("/done")).await.expect("renders").render();
    let active = app.render(&at("/active")).await.expect("renders").render();

    assert!(root.contains("milk"), "{root}");
    assert!(done.contains("milk"), "{done}");
    assert!(
        !active.contains("milk"),
        "a done task on the active route: {active}"
    );
    assert!(active.contains("0 shown"), "{active}");
}

/// A navigation on a live subscription is a re-render and a patch — the same machinery an event
/// uses, with nothing in the runtime that knows what a route is.
#[tokio::test]
async fn a_navigation_on_an_open_socket_produces_the_new_routes_page() {
    let placed = compile("routed.beck", &source("routed.beck"));
    let app = app_for(placed).await;
    let command = app
        .runtime()
        .decode_command(&serde_json::json!({"c":"Add","id":"t1","text":"milk"}))
        .expect("decodes");
    app.propose("k1".into(), "ana", command).await.expect("ok");

    let (client_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = unbounded_channel::<Message>();
    let socket = Duplex {
        out: server_tx,
        inbox: server_rx,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));

    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","actor":"ana"})
                .to_string()
                .into(),
        ))
        .expect("hello");
    let opening = drain(&mut client_rx).await;
    let first = opening.iter().find(|m| m["t"] == "p").expect("a frame");
    assert!(first.to_string().contains("milk"), "{first}");
    assert!(first.to_string().contains("everything"), "{first}");

    let before = beck_rt::telemetry::telemetry().navigations.get();
    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"g","path":"/done"})
                .to_string()
                .into(),
        ))
        .expect("nav");
    let after = drain(&mut client_rx).await;
    let patch = after
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a patch for the new route");
    assert!(
        patch.to_string().contains("done"),
        "the route's heading is not in the patch: {patch}"
    );
    // What a navigation costs on the wire, in the two directions. `docs/94` §94.3 quotes both, and
    // they are asserted rather than printed so the report's numbers cannot drift past them: a nav
    // frame is a couple of dozen bytes, and Mode A's answer is the *difference between two pages*
    // rather than a page.
    assert_eq!(
        serde_json::json!({"t":"g","path":"/done"})
            .to_string()
            .len(),
        24
    );
    assert!(
        patch.to_string().len() < 200,
        "a navigation's answer is a diff, not a page: {} bytes",
        patch.to_string().len()
    );
    assert!(beck_rt::telemetry::telemetry().navigations.get() > before);

    // Navigating to where the client already is renders nothing, because the session did not move.
    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"g","path":"/done"})
                .to_string()
                .into(),
        ))
        .expect("nav");
    let again = drain(&mut client_rx).await;
    assert!(
        !again.iter().any(|m| m["t"] == "p"),
        "a navigation to the same route sent a frame: {again:?}"
    );

    drop(client_tx);
    let _ = session.await;
}

/// The route the *first paint* is of, which is what makes a deep link and a reload work.
///
/// A client whose route were established by a message after the document would render the root's
/// page first and then correct it. This is the assertion that it does not have to.
#[tokio::test]
async fn a_hello_carries_the_route_so_a_deep_link_never_renders_the_wrong_page_first() {
    let placed = compile("routed.beck", &source("routed.beck"));
    let app = app_for(placed).await;
    let command = app
        .runtime()
        .decode_command(&serde_json::json!({"c":"Add","id":"t1","text":"milk"}))
        .expect("decodes");
    app.propose("k1".into(), "ana", command).await.expect("ok");

    let (client_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = unbounded_channel::<Message>();
    let socket = Duplex {
        out: server_tx,
        inbox: server_rx,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));
    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","actor":"ana","path":"/done"})
                .to_string()
                .into(),
        ))
        .expect("hello");
    let opening = drain(&mut client_rx).await;
    let first = opening.iter().find(|m| m["t"] == "p").expect("a frame");
    let text = first.to_string();
    assert!(text.contains("done"), "{text}");
    assert!(
        !text.contains("milk"),
        "the first frame is the root's page rather than the route's: {text}"
    );
    drop(client_tx);
    let _ = session.await;
}

async fn app_for(placed: Placed) -> Arc<App> {
    let backend = beck_eval::backend(&placed);
    App::start(
        beck_rt::Runtime::new(placed, backend).expect("prepares"),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("the app starts")
}

// --------------------------------------------------- the residue, held to the page

/// Every `data-b-*` binding the examples' pages emit is one the shipped JavaScript listens for.
///
/// Every binding a real page emits is one the residue captures.
///
/// This reads the *pages* rather than a list beside them, and it is now the second of two gates
/// rather than the only one: `the_event_vocabulary_is_what_the_client_listens_for` below holds the
/// compiler's table against the client's listeners, and this holds the programs against the
/// client. A table that agreed with the client and with no program would pass the first and fail
/// here.
#[test]
fn every_binding_a_page_emits_is_one_the_residue_captures() {
    let mut checked = 0usize;
    for name in ["todo.beck", "board.beck", "routed.beck"] {
        let src = source(name);
        for (i, _) in src.match_indices("on_") {
            // `on_click=` in the source is `data-b-click` in the page. Only inside a `ui:` block,
            // and every occurrence in these files is.
            let event: String = src[i + 3..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase())
                .collect();
            if event.is_empty() {
                continue;
            }
            assert!(
                beck_rt::PATCH_CLIENT.contains(&format!("data-b-{event}")),
                "examples/{name} binds `on_{event}` and no client listens for `data-b-{event}`"
            );
            checked += 1;
        }
    }
    assert!(checked >= 4, "only {checked} bindings were found");
}

/// The compiler's event vocabulary **is** the client's listener table, in both directions.
///
/// `ui:` refuses `on_<x>` for an `x` it does not know (`B0217`), which is only worth anything if
/// what it knows is what the client handles. The two are written in different languages in
/// different crates, so the agreement is asserted rather than arranged: the names come out of
/// `beck-patch.js`'s own `on(<dom event>, "data-b-<name>", …)` registrations, which is the table
/// itself and not a comment about it.
///
/// Both directions matter and they fail differently. An event in the compiler's table that the
/// client dropped is a handler that compiles and does nothing — the defect this vocabulary exists
/// to close, arriving from the other side. An event the client gained that the compiler does not
/// know is a feature nobody can reach.
#[test]
fn the_event_vocabulary_is_what_the_client_listens_for() {
    // `on("keydown", "data-b-enter", …)` — the DOM event, then the attribute the macro writes.
    let mut listened: Vec<String> = Vec::new();
    for (i, _) in beck_rt::PATCH_CLIENT.match_indices("\"data-b-") {
        let rest = &beck_rt::PATCH_CLIENT[i + 8..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase())
            .collect();
        // Only where it is being *registered*: `on("click", "data-b-click", …)`.
        let before = &beck_rt::PATCH_CLIENT[..i];
        if before.trim_end().ends_with(',') && before.contains("on(") && !name.is_empty() {
            let opened = before.rfind("on(").unwrap_or(0);
            if before[opened..].matches('\n').count() == 0 {
                listened.push(name);
            }
        }
    }
    listened.sort_unstable();
    listened.dedup();

    let mut known: Vec<String> = beck_macro::vocabulary::EVENTS
        .iter()
        .map(|(e, _)| (*e).to_string())
        .collect();
    known.sort_unstable();

    assert_eq!(
        listened, known,
        "the client listens for {listened:?} and the compiler will write {known:?}"
    );
    assert!(!known.is_empty(), "the vocabulary cannot be empty");
}

/// An attribute that is genuinely yours has a spelling, and it is HTML's own.
///
/// The vocabulary is closed, which is only reasonable if there is a way out of it. There is, and it
/// is not a Beck invention: `data-` is HTML's extension point and `aria-` is a namespace, so both
/// are admitted by prefix. This is the half of `B0218` that has to keep compiling.
#[test]
fn an_attribute_of_your_own_is_spelled_data() {
    let src = source("todo.beck").replace(
        "li(key=t.id, class=done_class(t)):",
        "li(key=t.id, class=done_class(t), data_row=t.text, aria_label=t.text):",
    );
    assert_ne!(src, source("todo.beck"), "the edit did not apply");
    // Compiles, which is the assertion: `compile` refuses to return if anything was diagnosed.
    compile("escape.beck", &src);

    // And the closed half is still closed, or the escape hatch would be the whole vocabulary.
    let refused = refusal(
        "closed.beck",
        &source("todo.beck").replace(
            "li(key=t.id, class=done_class(t)):",
            "li(key=t.id, klass=done_class(t)):",
        ),
    );
    assert!(refused.contains("B0218"), "{refused}");
}

/// The three holes a handler's template can carry, and the fact that they are filled at any depth.
///
/// `Id(value="$id")` puts a hole one level down, so a filler that only looked at the top of the
/// command would append the literal `"$id"` to the log. That was true until forms needed
/// `$field:`, and it was invisible because every command in the tree happened to flatten.
#[test]
fn a_handlers_holes_are_named_in_one_place_and_filled_at_any_depth() {
    for hole in ["$id", "$value", "$field:"] {
        assert!(
            beck_rt::PATCH_CLIENT.contains(hole),
            "the residue does not know the hole `{hole}`"
        );
    }
    // The filler recurses: an object, an array and a nested object all go through `walk`.
    assert!(
        beck_rt::PATCH_CLIENT.contains("Array.isArray(node)"),
        "the filler does not descend into a command's own structure"
    );
    // And a form's page uses one, so the hole is exercised rather than merely implemented.
    assert!(
        source("routed.beck").contains("$field:text"),
        "no example writes a form field"
    );
}

/// A path this process answers is not a path a program can route.
///
/// The list is a public function rather than a comment because a program's author needs it, and it
/// is held here to the router itself: a reserved path that the router does not actually answer
/// would be a route taken away from programs for nothing.
#[test]
fn the_reserved_routes_are_the_ones_this_process_answers() {
    let routes = beck_rt::http::reserved_routes();
    assert!(routes.contains(&"/socket"));
    assert!(routes.contains(&"/beck-patch.js"));
    assert!(routes.contains(&"/healthz"));
    // Every one of them is a literal in the router's own match, which is what stops the list
    // becoming a description of something that has moved.
    let router = include_str!("../src/../../beck-rt/src/http.rs");
    for route in routes {
        assert!(
            router.contains(&format!("\"{route}\"")) || router.contains("LOGIN_PATH"),
            "`{route}` is reserved and the router does not mention it"
        );
    }
    // And a program's route is not one of them.
    assert!(!routes.contains(&"/done"));
}

// --------------------------------------------------- what the panel is shown

/// The panel's data is the program's own graph, and it carries nothing about the state.
///
/// A devtools panel is the one place a developer will believe without checking, so what it is
/// *not* given matters as much as what it is: no accumulator, no source, no types. A Mode A page
/// is precisely the part of the state its viewer may see, and an endpoint that handed a browser
/// the rest would be a disclosure with a friendly name.
#[test]
fn the_signal_graph_a_panel_draws_is_the_programs_own_and_carries_no_state() {
    let placed = compile("routed.beck", &source("routed.beck"));
    let plan = beck_core::plan::Plan::compile(&placed);
    let doc = beck_rt::signals::of(&placed, &plan);

    assert_eq!(doc["page"], "page");
    assert_eq!(doc["mode"], "A");
    assert_eq!(doc["reads"], "`session.path`");
    let nodes = doc["nodes"].as_array().expect("nodes");
    assert!(nodes.len() >= 4, "{doc}");
    assert!(
        nodes.iter().any(|n| n["label"] == "board"),
        "the program's own names are missing: {doc}"
    );
    assert!(doc["plan"]["operators"].as_u64().unwrap_or(0) > 0);

    // Nothing about the accumulator. The one thing a panel must not be able to leak.
    let text = doc.to_string();
    for forbidden in ["milk", "state\":{", "\"init\""] {
        assert!(
            !text.contains(forbidden),
            "the graph carries {forbidden}: {text}"
        );
    }
}

/// The panel names the three things Phase 3 asks it for, and loads only when it is asked for.
#[test]
fn the_panel_shows_the_three_things_and_is_not_shipped_by_default() {
    for what in ["signal graph", "patch traffic", "pending"] {
        assert!(
            beck_rt::DEVTOOLS_CLIENT.contains(what),
            "the panel does not show `{what}`"
        );
    }
    // It reads what the residue counts rather than counting again.
    assert!(beck_rt::DEVTOOLS_CLIENT.contains("beck.stats"));
    assert!(beck_rt::DEVTOOLS_CLIENT.contains("beck.inspect.describe"));
    assert!(beck_rt::DEVTOOLS_CLIENT.contains("/beck-signals.json"));
    // And no page loads it unless it was asked for: the shell carries the two mode clients, and
    // the panel arrives through `beck.devtools()`.
    assert!(
        beck_rt::PATCH_CLIENT.contains("localStorage.getItem(\"beck:devtools\")"),
        "the panel is not behind a switch"
    );
    for client in [beck_rt::THIN_CLIENT, beck_rt::MODE_B_CLIENT] {
        assert!(
            client.contains("beck.devtools()"),
            "a mode that cannot open the panel"
        );
    }
}

/// A document with no URL of its own reports the application's root, and follows no links.
///
/// The playground's clients are `srcdoc` iframes ([`docs/98`](../../../../docs/98-playground-report.md)),
/// and `about:srcdoc`'s `location.pathname` is the string `srcdoc`. A residue that read a route off
/// it would put that on the `hello` frame, no program would ever match it, and every test in this
/// workspace would still pass — which is the shape of defect this suite exists for.
#[test]
fn a_document_with_no_url_has_no_route_and_no_links_to_follow() {
    // What the `hello` frame carries is `here()`, not `location.pathname` — one predicate rather
    // than a read at each site, so there is one place for this to be right.
    assert!(
        beck_rt::PATCH_CLIENT.contains("path: here(),"),
        "the hello frame does not carry the route through `here()`"
    );
    assert!(
        beck_rt::PATCH_CLIENT.contains("const addressed = ()"),
        "nothing decides whether this document has a URL"
    );
    // And the router installs nothing when there is no address bar to move.
    assert!(beck_rt::PATCH_CLIENT.contains("if (!addressed()) return;"));
    // Neither mode reads the location itself; both go through the router.
    for client in [beck_rt::THIN_CLIENT, beck_rt::MODE_B_CLIENT] {
        assert!(
            !client.contains("location.pathname"),
            "a mode that reads the location instead of asking the router"
        );
    }
}

/// Both modes publish the same three facts about themselves, under the same names.
///
/// A panel that had to ask "which mode is this" before it could read anything would be two panels.
#[test]
fn both_modes_describe_themselves_in_one_vocabulary() {
    for client in [beck_rt::THIN_CLIENT, beck_rt::MODE_B_CLIENT] {
        assert!(client.contains("beck.inspect.describe = "));
        for field in ["mode:", "seq:", "path:", "pending:"] {
            assert!(
                client.contains(field),
                "a mode that does not report `{field}`"
            );
        }
        assert!(
            client.contains("beck.route("),
            "a mode that does not follow a link"
        );
    }
}
