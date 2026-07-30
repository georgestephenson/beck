//! One subscription: the client side of the signal slice (§4.3 step 3).
//!
//! The server holds, per subscriber, exactly one thing — the last view it rendered for them — and
//! sends the difference whenever the fold moves. That single `Html` value is the per-session cost
//! the Phase 0 exit criteria put a number on.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use beck_p0_core::diff::{diff, Op};
use beck_p0_core::domain::ActorId;
use beck_p0_core::html::Html;
use beck_p0_core::patch::PatchFrame;
use beck_p0_core::protocol::{ClientMsg, Resumption, ServerMsg};
use beck_p0_core::view::Scope;

use crate::app::{scope_of, App};

/// Anything that behaves like a websocket connection: the hyper-upgraded socket in the server, and
/// a plain TCP socket in the load generator.
pub trait Socket:
    futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
    + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
    + Unpin
{
}

impl<T> Socket for T where
    T: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
{
}

pub type ServerSocket = WebSocketStream<TcpStream>;

/// Drive one subscription until the socket closes.
pub async fn run<S: Socket>(app: Arc<App>, mut socket: S) -> Result<()> {
    // Subscribe *before* reading the current view, so an event that lands in between wakes us
    // rather than being missed.
    let mut version = app.watch_version();

    let (sub, from, actor, scope) = match wait_for_hello(&mut socket).await? {
        Some(hello) => hello,
        None => return Ok(()),
    };

    let (how, view_then) = app.resume(from, &scope).await?;
    let (seq, view_now) = app.view_now(&scope).await;

    let ops = match how {
        // The client has nothing we can trust: hand it the whole frame. Same format, same
        // interpreter — a reset is just a patch that happens to replace the root.
        Resumption::Fresh | Resumption::Reset { .. } => vec![Op::Replace {
            path: vec![],
            html: view_now.clone(),
        }],
        // The client has the view as of `from`: send it the difference, which is the gap.
        Resumption::Resumed { .. } => diff(&view_then, &view_now),
    };

    app.metrics().subscription_opened();
    let result = drive(
        &app,
        &mut socket,
        sub,
        actor,
        scope,
        seq,
        view_now,
        how,
        ops,
        &mut version,
    )
    .await;
    app.metrics().subscription_closed();
    result
}

#[allow(clippy::too_many_arguments)]
async fn drive<S: Socket>(
    app: &Arc<App>,
    socket: &mut S,
    sub: String,
    actor: ActorId,
    scope: Scope,
    mut seq: u64,
    mut last_view: Html,
    how: Resumption,
    initial_ops: Vec<Op>,
    version: &mut tokio::sync::watch::Receiver<u64>,
) -> Result<()> {
    send_json(socket, &ServerMsg::welcome(&sub, seq, how)).await?;
    if !initial_ops.is_empty() {
        send_frame(app, socket, &PatchFrame::new(seq, initial_ops)).await?;
    }

    // The highest seq this client was told about that it has not yet seen reflected in its view.
    // Only a client waiting on its own command gets an "up to date" notice (`ServerMsg::up_to_date`).
    let mut awaiting: Option<u64> = None;

    loop {
        tokio::select! {
            // The fold moved. Recompute this subscription's view and send what changed.
            changed = version.changed() => {
                if changed.is_err() {
                    break; // the application is gone
                }
                let (at, view) = app.view_now(&scope).await;
                let ops = diff(&last_view, &view);
                last_view = view;
                seq = at;
                if !ops.is_empty() {
                    send_frame(app, socket, &PatchFrame::new(seq, ops)).await?;
                    awaiting = awaiting.filter(|waiting| *waiting > seq);
                } else if awaiting.is_some_and(|waiting| waiting <= seq) {
                    awaiting = None;
                    app.metrics().up_to_date_sent();
                    send_json(socket, &ServerMsg::up_to_date(seq)).await?;
                }
            }
            incoming = socket.next() => {
                let Some(message) = incoming else { break };
                match message? {
                    Message::Text(text) => {
                        match handle_client_message(app, socket, &actor, &text).await? {
                            Handled::Continue { committed: Some(at) } => {
                                awaiting = Some(awaiting.map_or(at, |waiting| waiting.max(at)));
                            }
                            Handled::Continue { committed: None } => {}
                            Handled::Close => break,
                        }
                    }
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

enum Handled {
    /// `committed` is the seq of a command this client is now waiting to see.
    Continue {
        committed: Option<u64>,
    },
    Close,
}

async fn handle_client_message<S: Socket>(
    app: &Arc<App>,
    socket: &mut S,
    actor: &ActorId,
    text: &str,
) -> Result<Handled> {
    match ClientMsg::parse(text) {
        Ok(ClientMsg::Cmd { id, command }) => {
            // `send!` — the reverse arrow, feeding merge_clients with the session attached.
            match app.propose(id, actor.clone(), command).await {
                Ok(seq) => {
                    send_json(socket, &ServerMsg::ack(id, seq)).await?;
                    return Ok(Handled::Continue {
                        committed: Some(seq),
                    });
                }
                Err(rejection) => {
                    send_json(socket, &ServerMsg::nack(id, &rejection.to_string())).await?
                }
            }
            Ok(Handled::Continue { committed: None })
        }
        Ok(ClientMsg::Ping) => {
            send_json(socket, &serde_json::json!({"t": "pong"})).await?;
            Ok(Handled::Continue { committed: None })
        }
        // A second hello on a live socket is a protocol error, not a resubscribe: the client
        // reconnects to resume.
        Ok(ClientMsg::Hello { .. }) => Ok(Handled::Close),
        Err(e) => {
            tracing::debug!(error = %e, "unparseable client message");
            Ok(Handled::Continue { committed: None })
        }
    }
}

type Hello = (String, u64, ActorId, Scope);

async fn wait_for_hello<S: Socket>(socket: &mut S) -> Result<Option<Hello>> {
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(text) => match ClientMsg::parse(&text) {
                Ok(ClientMsg::Hello {
                    sub,
                    seq,
                    actor,
                    scope,
                }) => {
                    let actor = ActorId::new(actor);
                    let scope = scope_of(scope, &actor);
                    return Ok(Some((sub, seq, actor, scope)));
                }
                Ok(_) => continue, // commands before hello are ignored, not honoured
                Err(e) => {
                    tracing::debug!(error = %e, "unparseable hello");
                    continue;
                }
            },
            Message::Close(_) => return Ok(None),
            _ => continue,
        }
    }
    Ok(None)
}

async fn send_json<S: Socket>(socket: &mut S, value: &serde_json::Value) -> Result<()> {
    socket.send(Message::Text(value.to_string())).await?;
    Ok(())
}

async fn send_frame<S: Socket>(app: &Arc<App>, socket: &mut S, frame: &PatchFrame) -> Result<()> {
    let text = frame.to_json().to_string();
    app.metrics().patch_sent(frame.ops.len(), text.len());
    socket.send(Message::Text(text)).await?;
    Ok(())
}

/// Per-subscription memory, as the runtime itself accounts for it: the rendered view plus the
/// fixed cost of the task and its socket buffers. Reported next to the measured RSS delta in the
/// Phase 0 report so the two can be compared honestly.
pub fn view_bytes(view: &Html) -> usize {
    fn walk(node: &Html) -> usize {
        match node {
            Html::Text { text, .. } => std::mem::size_of::<Html>() + text.len(),
            Html::Element {
                tag,
                attrs,
                key,
                children,
                ..
            } => {
                std::mem::size_of::<Html>()
                    + tag.len()
                    + key.as_ref().map_or(0, |k| k.len())
                    + attrs
                        .iter()
                        .map(|(k, v)| k.len() + v.len() + std::mem::size_of::<(String, String)>())
                        .sum::<usize>()
                    + children.iter().map(walk).sum::<usize>()
            }
        }
    }
    walk(view)
}

/// Total patch bytes sent since start — used by the benchmark harness.
pub fn patch_bytes(app: &App) -> u64 {
    app.metrics().patch_bytes.load(Ordering::Relaxed)
}
