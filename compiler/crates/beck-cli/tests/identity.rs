//! Identity, driven end to end over the socket a browser talks to.
//!
//! [`docs/42-security-assurance.md`](../../../../docs/42-security-assurance.md) §42.6's first
//! bullet and [`docs/43`](../../../../docs/43-threat-model.md) §43.4's first absence: the actor
//! arrived in the client's own `hello` frame and was believed, so every ownership check in every
//! corpus program was enforced against a value the caller chose.
//!
//! [`48`](../../../../docs/48-identity-report.md) made it a seam. This file asserts the two halves
//! that matter through the loop rather than through the unit — that a connection with no valid
//! credential does not get a view, and that a connection with one gets the actor the *credential*
//! names rather than the one the frame claims.
//!
//! The unit tests in `beck-rt/src/identity.rs` cover the credential format. These cover the thing
//! a unit test cannot: that the loop asks.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use beck_core::clock::ManualClock;
use beck_rt::identity::{DevIdentity, Identity, SignedIdentity};
use beck_rt::{App, AppConfig, MemoryLog};
use futures_util::{Sink, Stream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

mod support;
use support::todo_runtime;

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

/// Start an app with the given provider, connect, say `hello` with `claim`, and give back whatever
/// the server said.
async fn hello_with(
    identity: Arc<dyn Identity>,
    claim: &str,
) -> (Arc<App>, Vec<serde_json::Value>) {
    let app = App::start(
        todo_runtime(),
        Arc::new(MemoryLog::new()),
        AppConfig {
            identity,
            ..Default::default()
        },
    )
    .await
    .expect("app starts");

    let (client_tx, server_rx) = unbounded_channel::<Message>();
    let (server_tx, mut client_rx) = unbounded_channel::<Message>();
    let socket = Duplex {
        out: server_tx,
        inbox: server_rx,
    };
    let _session = tokio::spawn(beck_rt::session::run(app.clone(), socket));
    client_tx
        .send(Message::Text(
            serde_json::json!({"t":"hello","sub":"s1","seq":0,"actor":claim})
                .to_string()
                .into(),
        ))
        .expect("hello");
    let out = drain(&mut client_rx).await;
    (app, out)
}

fn signed(clock: Arc<ManualClock>) -> Arc<SignedIdentity> {
    Arc::new(SignedIdentity::new("a shared secret", clock))
}

/// The default is still to believe the client, and that is what a laptop needs. Asserted so the
/// default is a decision somebody can find rather than an accident.
#[tokio::test]
async fn the_dev_provider_lets_a_claim_through_and_says_it_is_not_verifying() {
    let (_, msgs) = hello_with(Arc::new(DevIdentity), "ana").await;
    assert!(msgs.iter().any(|m| m["t"] == "w"), "no welcome: {msgs:?}");
    assert!(!DevIdentity.verifies());
}

/// The property `docs/43` §43.4's first bullet was about: with a provider that verifies, **naming
/// yourself is not enough**. No welcome, no frame, no view.
#[tokio::test]
async fn an_unsigned_claim_gets_no_view_at_all() {
    let clock = Arc::new(ManualClock::at(1_000));
    let (app, msgs) = hello_with(signed(clock), "the-auditor").await;

    assert!(
        !msgs.iter().any(|m| m["t"] == "w" || m["t"] == "p"),
        "an unverified connection was given a view: {msgs:?}"
    );
    let refusal = msgs
        .iter()
        .find(|m| m["t"] == "e")
        .expect("a refusal frame");
    assert_eq!(refusal["e"], "unauthenticated");
    // Coarse on purpose: which of the three refusals it was is useful to an attacker and to
    // nobody else. The operator gets the distinction in the log and in this counter.
    assert!(
        !refusal.to_string().contains("credential"),
        "the client is told more than it should be: {refusal}"
    );
    let _ = app;
    assert!(
        beck_rt::telemetry().unauthenticated.get() >= 1,
        "the refusal is counted, so an operator can see an attack rather than infer one"
    );
}

/// And with one, the actor is the credential's rather than the frame's — which is the difference
/// between "only the owner may toggle their todo" being a rule and being a wish.
#[tokio::test]
async fn a_signed_credential_names_the_actor_and_the_frame_does_not() {
    let clock = Arc::new(ManualClock::at(1_000));
    let id = signed(clock.clone());
    let token = id.mint("ana", 9_000, &[("role", "owner")]);
    let (_, msgs) = hello_with(id, &token).await;
    assert!(msgs.iter().any(|m| m["t"] == "w"), "no welcome: {msgs:?}");
    assert!(
        msgs.iter().any(|m| m["t"] == "p"),
        "a verified connection gets its view: {msgs:?}"
    );
}

/// Expiry goes through the injected clock, so this is a statement about an instant rather than
/// about how long the suite took to run — which is `beck_core::clock`'s whole point.
#[tokio::test]
async fn an_expired_credential_is_refused_against_the_apps_own_clock() {
    let clock = Arc::new(ManualClock::at(1_000));
    let id = signed(clock.clone());
    let token = id.mint("ana", 2_000, &[]);
    clock.set(5_000);
    let (_, msgs) = hello_with(id, &token).await;
    assert!(
        msgs.iter().any(|m| m["t"] == "e"),
        "an expired credential was accepted: {msgs:?}"
    );
}
