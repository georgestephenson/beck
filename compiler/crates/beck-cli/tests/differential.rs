//! The differential harness: the whole program single-process versus split across tiers.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.8: "Differential
//! execution: run the whole program single-process vs. split across tiers, assert identical
//! observable behaviour. **This is the highest-value test in the project.**" And §8.3: "The
//! differential harness is the project's conscience. It is the mechanised statement of the central
//! promise; keep it green."
//!
//! What is being compared:
//!
//! * **Single process.** No wire, no sequencer, no diff. Start at the initial accumulator; for each
//!   command, call `validate`, fold the events it yields, and evaluate `view`. This is the program
//!   as written, read literally.
//! * **Split.** The real runtime: commands go through the ingress channel into the sequencer, are
//!   validated under the write lock, appended to a log, folded, and the resulting view is *diffed*
//!   against the previous one. A client applies the patches with `diff::apply` and holds a DOM it
//!   built from nothing but patches.
//!
//! The assertion is that the client's reconstructed DOM equals the single-process view, at every
//! step. If that holds, the tier split is not observable — which is the whole claim of the language.

use std::sync::Arc;

use beck_core::{Html, Placed, Value};
use beck_rt::{diff, App, AppConfig, MemoryLog, Runtime};

mod support;
use support::{command, todo_program, todo_runtime, ACTORS};

/// The program run as one process: no wire, no sequencer, no diff.
struct SingleProcess {
    runtime: Runtime,
    state: Value,
    seq: u64,
}

impl SingleProcess {
    fn new(placed: Placed) -> SingleProcess {
        let backend = beck_eval::backend(&placed);
        let runtime = Runtime::new(placed, backend).expect("runtime");
        let state = runtime.initial_state().expect("initial state");
        SingleProcess {
            runtime,
            state,
            seq: 0,
        }
    }

    /// Returns whether the command was accepted.
    fn propose(&mut self, actor: &str, cmd: Value) -> bool {
        let proposal = self.runtime.proposal(actor, cmd);
        let Ok(events) = self.runtime.validate(&self.state, &proposal) else {
            return false;
        };
        for e in events {
            self.seq += 1;
            let env = beck_rt::Envelope {
                seq: self.seq,
                at: beck_rt::Instant(self.seq as i64),
                actor: actor.to_string(),
                body: beck_core::core::value_to_repr(&e).expect("an event is data"),
            };
            self.state = self.runtime.fold(&self.state, &env, e).expect("fold");
        }
        true
    }

    fn view(&self, actor: &str) -> Html {
        self.runtime.view(&self.state, actor).expect("view")
    }
}

/// A client that knows nothing but patches — the browser's half of Mode A.
struct PatchClient {
    dom: Option<Html>,
}

impl PatchClient {
    fn new() -> PatchClient {
        PatchClient { dom: None }
    }

    fn apply(&mut self, ops: &[diff::Op]) {
        // A `Replace` at the root is how a fresh subscription and a reset both arrive: same
        // format, same interpreter.
        if let Some(diff::Op::Replace { path, html }) = ops.first() {
            if path.is_empty() {
                self.dom = Some(html.clone());
                if ops.len() == 1 {
                    return;
                }
            }
        }
        let current = self
            .dom
            .clone()
            .expect("a client must be seeded before it is patched");
        self.dom = Some(diff::apply(&current, ops));
    }

    fn rendered(&self) -> String {
        self.dom.as_ref().map(Html::render).unwrap_or_default()
    }
}

/// A deterministic script of commands, covering acceptance, rejection and cross-actor ownership.
fn script() -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    for (i, actor) in ACTORS.iter().enumerate() {
        out.push((
            *actor,
            command(
                "Add",
                &[("id", &format!("id-{i}")), ("text", "write the fold")],
            ),
        ));
        out.push((
            *actor,
            command(
                "Add",
                &[("id", &format!("id-{i}")), ("text", "a duplicate id")],
            ),
        ));
        out.push((
            *actor,
            command("Add", &[("id", &format!("blank-{i}")), ("text", "   ")]),
        ));
        out.push((*actor, command("Toggle", &[("id", &format!("id-{i}"))])));
    }
    // Bob tries to toggle Alice's todo: accepted by the type system, refused by `validate`.
    out.push(("bob", command("Toggle", &[("id", "id-0")])));
    out.push((
        "alice",
        command("Add", &[("id", "id-later"), ("text", "aaa sorts first")]),
    ));
    out.push(("alice", command("Delete", &[("id", "id-0")])));
    out
}

#[tokio::test]
async fn split_execution_is_indistinguishable_from_single_process() {
    let placed = todo_program();

    // ---- the split side: the real runtime, with a client that only ever sees patches ----
    let store = Arc::new(MemoryLog::new());
    let app = App::start(todo_runtime(), store, AppConfig::default())
        .await
        .expect("app starts");

    let mut clients: Vec<(String, PatchClient, Html)> = Vec::new();
    for actor in ACTORS {
        let view = app.render(actor).await.expect("render");
        let mut client = PatchClient::new();
        client.apply(&[diff::Op::Replace {
            path: vec![],
            html: view.clone(),
        }]);
        clients.push((actor.to_string(), client, view));
    }

    // ---- the single-process side: the program read literally ----
    let mut single = SingleProcess::new(placed);

    for (step, (actor, cmd)) in script().into_iter().enumerate() {
        let accepted_split = app
            .propose(format!("cmd-{step}"), actor.to_string(), cmd.clone())
            .await
            .is_ok();
        let accepted_single = single.propose(actor, cmd);
        assert_eq!(
            accepted_split, accepted_single,
            "step {step}: the two sides disagreed about whether the command was accepted"
        );

        // Every subscriber wakes, re-renders, and is sent the difference.
        for (subscriber, client, last) in clients.iter_mut() {
            let now = app.render(subscriber).await.expect("render");
            let ops = diff(last, &now);
            if !ops.is_empty() {
                client.apply(&ops);
            }
            *last = now;

            assert_eq!(
                client.rendered(),
                single.view(subscriber).render(),
                "step {step}: `{subscriber}`'s patch-built DOM diverged from the single-process view"
            );
        }
    }

    // A subscriber who never received a byte of application logic still holds the right page.
    let alice = clients
        .iter()
        .find(|(a, _, _)| a == "alice")
        .expect("alice");
    assert!(alice.1.rendered().contains("aaa sorts first"));
    assert!(!alice.1.rendered().contains("write the fold"), "deleted");
}

#[tokio::test]
async fn a_per_session_view_never_shows_another_actors_state() {
    // §3.5: "the log and the business rules never ship to clients". The filter provably runs
    // server-side, so an idle subscriber of a per-session view is not merely *not shown* someone
    // else's todo — it is never sent one.
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("app starts");

    let mut alice_last = app.render("alice").await.expect("render");
    app.propose(
        "c1".into(),
        "bob".into(),
        command("Add", &[("id", "b1"), ("text", "bob's secret")]),
    )
    .await
    .expect("accepted");

    let alice_now = app.render("alice").await.expect("render");
    let ops = diff(&alice_last, &alice_now);
    assert!(
        ops.is_empty(),
        "alice was sent {} operations for a change she cannot see",
        ops.len()
    );
    assert!(!alice_now.render().contains("bob's secret"));

    // …and bob does see it.
    let bob = app.render("bob").await.expect("render");
    assert!(bob.render().contains("bob's secret"), "{}", bob.render());
    alice_last = alice_now;
    let _ = alice_last;
}
