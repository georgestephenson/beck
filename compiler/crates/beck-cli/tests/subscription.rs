//! One subscription, driven end to end over an in-memory socket.
//!
//! Everything else in this directory tests the runtime by calling into it. This drives
//! [`beck_rt::session::run`] — the loop a real browser talks to: hello, welcome, the first frame,
//! a command, an ack, the patch frame the command produced. `beck-rt`'s `Socket` trait was written
//! for exactly this ("the upgraded socket in the server, and an in-memory duplex in the tests") and
//! nothing had used the second half of that sentence.
//!
//! The reason it exists now is [`docs/24-incremental-views-report.md`]: the subscription loop is
//! where the incremental view engine is switched on, one engine per connection. The differential
//! harness proves the engine renders what a recompute would; this proves the loop is holding one.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use beck_rt::{App, AppConfig, MemoryLog};
use futures_util::{Sink, Stream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

mod support;
use support::{command, todo_runtime};

/// A socket that is two channels. Implements exactly the bounds `beck_rt::session::Socket` needs.
struct Duplex {
    out: UnboundedSender<Message>,
    inbox: UnboundedReceiver<Message>,
}

impl Sink<Message> for Duplex {
    type Error = WsError;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), WsError> {
        let _ = self.out.send(item);
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
}

impl Stream for Duplex {
    type Item = Result<Message, WsError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inbox.poll_recv(cx).map(|m| m.map(Ok))
    }
}

/// Everything the server has said so far, as JSON.
async fn drain(rx: &mut UnboundedReceiver<Message>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    // The loop is cooperative, so a short yield is enough for the session task to make progress
    // without a sleep long enough to be a flake.
    for _ in 0..64 {
        while let Ok(Message::Text(t)) = rx.try_recv() {
            if let Ok(v) = serde_json::from_str(&t) {
                out.push(v);
            }
        }
        tokio::task::yield_now().await;
    }
    out
}

#[tokio::test]
async fn a_subscription_maintains_its_view_and_streams_the_patches() {
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("app starts");
    assert!(
        app.maintains_views(),
        "the default has to be the path the report measures"
    );

    let (client_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = unbounded_channel::<Message>();
    let socket = Duplex {
        out: server_tx,
        inbox: server_rx,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));

    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","seq":0,"actor":"ana"}).to_string(),
        ))
        .expect("hello");

    let opening = drain(&mut client_rx).await;
    assert!(
        opening.iter().any(|m| m["t"] == "w"),
        "no welcome: {opening:?}"
    );
    let first = opening
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a first frame");
    assert!(
        first.to_string().contains("0 remaining"),
        "the first frame is not the page: {first}"
    );

    // A command through the same socket: the ack means committed, the frame means the view caught
    // up, and Phase 0 learned the hard way that those are different facts (§18.5 item 1).
    client_tx
        .send(Message::Text(
            serde_json::json!({
                "t":"c","id":"c1",
                "command":{"c":"Add","id":"t1","text":"milk"}
            })
            .to_string(),
        ))
        .expect("cmd");

    let after = drain(&mut client_rx).await;
    assert!(after.iter().any(|m| m["t"] == "a"), "no ack: {after:?}");
    let patch = after
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a patch frame for the new todo");
    let text = patch.to_string();
    assert!(
        text.contains("milk"),
        "the patch does not carry the todo: {text}"
    );
    assert!(
        text.contains("1 remaining"),
        "the maintained count did not reach the client: {text}"
    );

    // The state the engine maintained and the state a recompute produces are the same page.
    let recomputed = app.render("ana").await.expect("recompute").render();
    assert!(recomputed.contains("1 remaining"));

    drop(client_tx);
    let _ = session.await;
}

#[tokio::test]
async fn a_subscription_serves_the_same_page_with_maintenance_switched_off() {
    // `AppConfig::maintain_views` is a switch because the engine is a memory-for-time trade
    // (docs/24 §24.7). A switch nothing exercises is a switch that stops working, and the failure
    // would be silent: the page would still render, from the other path.
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig {
            maintain_views: false,
            ..AppConfig::default()
        },
    )
    .await
    .expect("app starts");
    assert!(!app.maintains_views());

    app.propose(
        "c1".into(),
        "ana".into(),
        command("Add", &[("id", "t1"), ("text", "milk")]),
    )
    .await
    .expect("accepted");

    let (client_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = unbounded_channel::<Message>();
    let socket = Duplex {
        out: server_tx,
        inbox: server_rx,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));
    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","seq":0,"actor":"ana"}).to_string(),
        ))
        .expect("hello");

    let opening = drain(&mut client_rx).await;
    let first = opening
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a first frame");
    assert!(first.to_string().contains("milk"), "{first}");
    assert!(first.to_string().contains("1 remaining"), "{first}");

    drop(client_tx);
    let _ = session.await;
}

#[tokio::test]
async fn two_live_subscriptions_over_one_shared_dataflow_get_their_own_pages() {
    // The end-to-end shape of §5.3: two browsers, two sessions, one dataflow between them. Every
    // property of the sharing is asserted in `shared_arrangements.rs` against the engine directly;
    // this is the one that says the *subscription loop* is the thing holding it, and that two
    // sockets driven concurrently do not see each other's rows.
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("app starts");
    assert!(
        app.shares_arrangements(),
        "the default has to be the path the report measures"
    );

    let mut sockets = Vec::new();
    for (sub, actor) in [("s1", "ana"), ("s2", "bo")] {
        let (client_tx, server_rx) = unbounded_channel::<Message>();
        let (server_tx, client_rx) = unbounded_channel::<Message>();
        let socket = Duplex {
            out: server_tx,
            inbox: server_rx,
        };
        let task = tokio::spawn(beck_rt::session::run(app.clone(), socket));
        client_tx
            .send(Message::Text(
                serde_json::json!({"t":"hello","sub":sub,"seq":0,"actor":actor}).to_string(),
            ))
            .expect("hello");
        sockets.push((actor, client_tx, client_rx, task));
    }
    for (_, _, rx, _) in sockets.iter_mut() {
        drain(rx).await;
    }

    // `ana` adds a todo. The sketch's view is `mine`, so it must reach `ana` and not `bo`.
    sockets[0]
        .1
        .send(Message::Text(
            serde_json::json!({
                "t":"c","id":"c1",
                "command":{"c":"Add","id":"t1","text":"milk"}
            })
            .to_string(),
        ))
        .expect("cmd");

    let ana = drain(&mut sockets[0].2).await;
    let bo = drain(&mut sockets[1].2).await;
    let ana_text = format!("{ana:?}");
    let bo_text = format!("{bo:?}");
    assert!(
        ana_text.contains("milk") && ana_text.contains("1 remaining"),
        "the owner did not get their own todo: {ana_text}"
    );
    assert!(
        !bo_text.contains("milk"),
        "a shared arrangement leaked one subscriber's todo into another's page: {bo_text}"
    );

    // One dataflow, advanced per event and not per connection. Two subscriptions rendered at every
    // version and the advances counted the versions.
    let shared = app.shared_dataflow();
    assert_eq!(
        shared.version(),
        app.head(),
        "the shared dataflow is not at the head the subscribers were served"
    );
    assert!(
        shared.advances() <= app.head() + 1,
        "two subscriptions advanced the shared dataflow {} times over {} versions",
        shared.advances(),
        app.head() + 1
    );

    for (_, tx, _, task) in sockets {
        drop(tx);
        let _ = task.await;
    }
}

#[tokio::test]
async fn a_subscription_serves_the_same_page_with_sharing_switched_off() {
    // The other half of the switch tested above. With `share_arrangements` off every subscription
    // computes the whole plan itself, which is what every subscription did before docs/25 — and the
    // failure of a switch nobody exercises is silent, because the page still renders.
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig {
            share_arrangements: false,
            ..AppConfig::default()
        },
    )
    .await
    .expect("app starts");
    assert!(app.maintains_views() && !app.shares_arrangements());

    app.propose(
        "c1".into(),
        "ana".into(),
        command("Add", &[("id", "t1"), ("text", "milk")]),
    )
    .await
    .expect("accepted");

    let (client_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = unbounded_channel::<Message>();
    let socket = Duplex {
        out: server_tx,
        inbox: server_rx,
    };
    let session = tokio::spawn(beck_rt::session::run(app.clone(), socket));
    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","seq":0,"actor":"ana"}).to_string(),
        ))
        .expect("hello");

    let opening = drain(&mut client_rx).await;
    let first = opening
        .iter()
        .find(|m| m["t"] == "p")
        .expect("a first frame");
    assert!(first.to_string().contains("milk"), "{first}");
    assert!(first.to_string().contains("1 remaining"), "{first}");
    assert_eq!(
        app.shared_dataflow().advances(),
        0,
        "the shared dataflow was advanced even though sharing is off"
    );

    drop(client_tx);
    let _ = session.await;
}

#[tokio::test]
async fn a_maintained_page_comes_back_with_the_version_it_reflects() {
    // The `seq` a patch frame carries is what a resuming client asks for the difference from
    // (§4.3), so a frame labelled with a version its page does not show is a wrong DOM after the
    // next reconnect. `session::drive` used to take it from `app.head()` sampled *after* the
    // render, which is a larger number whenever an event landed in between — the state is read
    // under a lock and the head was not.
    //
    // Asserted where the pairing is made rather than on the wire, because that is where it can be
    // wrong: the page and the version come out of one call, and the check is that recomputing the
    // state at that version reproduces exactly that page.
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("app starts");
    let mut engine = app.view_engine().expect("an engine");

    for i in 0..12 {
        app.propose(
            format!("c{i}"),
            "ana".into(),
            command("Add", &[("id", &format!("t{i}")), ("text", "milk")]),
        )
        .await
        .expect("accepted");

        let (page, at) = app.maintain(&mut engine, "ana").await.expect("a render");
        let state = app.state_at(at).await.expect("the state at that version");
        assert_eq!(
            page.render(),
            app.runtime()
                .view(&state, "ana")
                .expect("the recomputed page")
                .render(),
            "the page returned with version {at} is not the page of the state at version {at}"
        );
        assert!(
            at <= app.head(),
            "a render claims version {at} against a head of {}",
            app.head()
        );
    }
}
