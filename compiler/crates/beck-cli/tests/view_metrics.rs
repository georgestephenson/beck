//! What a fanout of maintained views costs, as a running process reports it.
//!
//! Its own test binary, and that is the point rather than an accident: [`beck_rt::telemetry`] is one
//! value per process ("a metric registry that has to be threaded through every call site gets
//! threaded through some of them"), so a test that asserts a *gauge returned to zero* cannot share a
//! binary with another test that has a subscription open. Cargo gives each file its own process;
//! this file has one test.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use beck_rt::{App, AppConfig, MemoryLog};
use futures_util::{Sink, Stream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

mod support;
use support::{command, todo_runtime};

/// A socket that is two channels — the same duplex `subscription.rs` drives.
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

async fn drain(rx: &mut UnboundedReceiver<Message>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
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
async fn what_the_views_cost_is_exported_while_the_process_is_running() {
    // §5.3 names per-session memory as one of three metrics to export. `Engine::footprint` computed
    // one and nothing exported it (docs/24 §24.10), which meant the number existed in a report and
    // not on a dashboard. What is exported is arrangement *entries* — the unit that scales, and
    // cheap enough to read on every render — split into the part paid once and the part paid per
    // connection, because that split is the operational question.
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig::default(),
    )
    .await
    .expect("app starts");
    for i in 0..5 {
        app.propose(
            format!("c{i}"),
            "ana".into(),
            command("Add", &[("id", &format!("t{i}")), ("text", "milk")]),
        )
        .await
        .expect("accepted");
    }

    let mut sockets = Vec::new();
    for (sub, actor) in [("s1", "ana"), ("s2", "ana")] {
        let (client_tx, server_rx) = unbounded_channel::<Message>();
        let (server_tx, mut client_rx) = unbounded_channel::<Message>();
        let socket = Duplex {
            out: server_tx,
            inbox: server_rx,
        };
        let task = tokio::spawn(beck_rt::session::run(app.clone(), socket));
        client_tx
            .send(Message::Text(
                serde_json::json!({"t":"hello","sub":sub,"actor":actor})
                    .to_string()
                    .into(),
            ))
            .expect("hello");
        drain(&mut client_rx).await;
        sockets.push((client_tx, client_rx, task));
    }

    let t = beck_rt::telemetry::telemetry();
    let shared = t.shared_arranged.get();
    let per_session = t.session_arranged.get();
    assert!(
        shared > 0,
        "the shared dataflow reports no entries with two subscriptions on five todos"
    );
    assert!(
        per_session > 0,
        "the subscriptions report no entries between them"
    );

    // Nobody is behind, so nothing is being kept for anybody. Zero rather than one because the
    // events all landed before either subscription opened: the single advance happened with no
    // warm reader to use its changes, so the step was retained for nobody the moment it was made.
    // A subscription that was already rendering when an event arrived would pin exactly one.
    assert_eq!(
        t.shared_retained.get(),
        0,
        "two subscriptions that are up to date are pinning {} versions of change history",
        t.shared_retained.get()
    );
    assert_eq!(t.shared_releases.get(), 0, "released while subscribed");

    // The gauge is a sum over connections, so it has to come back down when they end — a gauge
    // that only goes up describes connections that closed.
    let (first, rest) = sockets.split_at_mut(1);
    for (tx, _, task) in first.iter_mut() {
        drop(std::mem::replace(tx, unbounded_channel().0));
        task.await.expect("the first subscription ends").ok();
    }
    // One subscription left, so the shared dataflow is still being read and still holds what it
    // was holding. This is the half that says the release is about the *last* reader and not about
    // any of them.
    assert_eq!(
        t.shared_arranged.get(),
        shared,
        "the shared dataflow's entries moved when one of two subscriptions ended; they are not \
         per session"
    );
    assert_eq!(
        t.shared_releases.get(),
        0,
        "released with a reader attached"
    );

    for (tx, _, task) in rest.iter_mut() {
        drop(std::mem::replace(tx, unbounded_channel().0));
        task.await.expect("the last subscription ends").ok();
    }
    assert_eq!(
        t.session_arranged.get(),
        0,
        "two subscriptions ended and the per-session gauge is still {}",
        t.session_arranged.get()
    );
    // And with nobody reading it, the one shared dataflow gives its arrangements back — the
    // lifecycle rule, as the process reports it rather than as the engine's own tests assert it.
    assert_eq!(
        t.shared_arranged.get(),
        0,
        "the last subscription ended and the process still reports {} shared entries",
        t.shared_arranged.get()
    );
    assert_eq!(
        t.shared_releases.get(),
        1,
        "the arrangements went without the process counting a release"
    );
}
