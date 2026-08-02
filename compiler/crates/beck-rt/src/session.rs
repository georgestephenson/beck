//! One subscription: the client half of the tier crossing the splitter found.
//!
//! The `page` signal is `@on(client)` and its input is `@on(data)`, so exactly one edge crosses,
//! and this is what the compiler synthesises for it (§4.3 stage 3): "the server side gets a diff
//! operator (DOM patches for Mode-A components), the client side a resumable `(subscription, seq)`
//! consumer; `send` becomes the upstream command channel into the ingress."

use std::sync::Arc;

use anyhow::Result;
use beck_core::Html;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use crate::app::App;
use crate::diff::{diff, Op};
use crate::patch::PatchFrame;
use crate::protocol::{ClientMsg, Resumption, ServerMsg};
use crate::telemetry::telemetry;

/// Anything that behaves like a websocket connection: the upgraded socket in the server, and an
/// in-memory duplex in the tests.
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

/// Drive one subscription until the socket closes.
pub async fn run<S: Socket>(app: Arc<App>, mut socket: S) -> Result<()> {
    // A guard, not a pair of calls: a session ends by returning, by erroring, or by the socket
    // dying, and a gauge that only decrements on the happy path drifts upward forever.
    let _connected = SessionGuard::new();
    // Subscribe *before* reading the current view, so an event that lands in between wakes us
    // rather than being missed.
    let mut version = app.subscribe();

    let Some((sub, from, actor)) = wait_for_hello(&mut socket).await? else {
        return Ok(());
    };

    let floor = app.floor().await?;
    let head = app.head();
    let how = if from == 0 {
        Resumption::Fresh
    } else if from < floor || from > head {
        // The gap is unreachable — the log was trimmed, or this `seq` is from another lifetime of
        // the application. Reset, and say so rather than pretending.
        Resumption::Reset { from }
    } else {
        Resumption::Resumed {
            from,
            replayed: head - from,
        }
    };

    // One engine per subscription: §5.3's per-subscriber operators, and the arrangements they
    // hold. With sharing on that is *only* the per-session operators — everything above them is one
    // dataflow the application holds. It is created before the first render so that render is the
    // engine's own cold start rather than a recompute the engine then has to catch up with.
    let mut engine = app.view_engine()?;
    let mut arranged = Arranged::new();
    // `seq` comes back from the render rather than from `app.head()` afterwards: it is the version
    // the page reflects, and this frame will be the one a resuming client asks for the difference
    // from.
    let (view_now, seq) = app.maintain(&mut engine, &actor).await?;
    arranged.update(engine.arranged());
    telemetry()
        .shared_arranged
        .set(app.shared_dataflow().arranged());

    let ops = match how {
        // The client has nothing we can trust: hand it the whole frame. Same format, same
        // interpreter — a reset is just a patch that happens to replace the root.
        Resumption::Fresh | Resumption::Reset { .. } => vec![Op::Replace {
            path: vec![],
            html: view_now.clone(),
        }],
        // The client has the view as of `from`: send it exactly the difference.
        Resumption::Resumed { .. } => {
            let then = app.state_at(from).await?;
            let view_then = app.runtime().view(&then, &actor)?;
            diff(&view_then, &view_now)
        }
    };

    drive(
        &app,
        &mut socket,
        sub,
        actor,
        seq,
        view_now,
        how,
        ops,
        &mut version,
        &mut engine,
        &mut arranged,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn drive<S: Socket>(
    app: &Arc<App>,
    socket: &mut S,
    sub: String,
    actor: String,
    mut seq: u64,
    mut last_view: Html,
    how: Resumption,
    initial_ops: Vec<Op>,
    version: &mut tokio::sync::watch::Receiver<u64>,
    engine: &mut beck_core::engine::Engine,
    arranged: &mut Arranged,
) -> Result<()> {
    // How a subscriber was brought up to date is exactly the distinction Phase 0 got wrong twice
    // (§18.5 item 1): an ack means committed, a frame means your view has caught up.
    tracing::info!(seq, sub = %sub, how = how.label(), "subscribed");
    send_json(socket, &ServerMsg::welcome(&sub, seq, how)).await?;
    if !initial_ops.is_empty() {
        send_json(socket, &PatchFrame::new(seq, initial_ops).to_json()).await?;
    }

    // The highest seq this client was told about but has not yet seen reflected in its view. Only
    // a client waiting on its own command is sent an "up to date" notice.
    let mut awaiting: Option<u64> = None;

    loop {
        tokio::select! {
            changed = version.changed() => {
                if changed.is_err() {
                    break; // the application is gone
                }
                let (view, at) = app.maintain(engine, &actor).await?;
                arranged.update(engine.arranged());
                telemetry()
                    .shared_arranged
                    .set(app.shared_dataflow().arranged());
                let started = std::time::Instant::now();
                let ops = diff(&last_view, &view);
                telemetry().diff.record(started.elapsed());
                last_view = view;
                seq = at;
                if !ops.is_empty() {
                    send_json(socket, &PatchFrame::new(seq, ops).to_json()).await?;
                    awaiting = awaiting.filter(|w| *w > seq);
                } else if let Some(w) = awaiting {
                    // No frame is owed, but this client is waiting on its own command. Tell it
                    // where its view stands, or it waits forever (§18.5 item 1).
                    if seq >= w {
                        send_json(socket, &ServerMsg::up_to_date(seq)).await?;
                        awaiting = None;
                    }
                }
            }
            incoming = socket.next() => {
                let Some(message) = incoming else { break };
                let text = match message? {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    Message::Ping(p) => {
                        socket.send(Message::Pong(p)).await?;
                        continue;
                    }
                    _ => continue,
                };
                match ClientMsg::parse(&text) {
                    Ok(ClientMsg::Cmd { id, command }) => {
                        let decoded = match app.runtime().decode_command(&command) {
                            Ok(v) => v,
                            Err(e) => {
                                send_json(socket, &ServerMsg::nack(&id, &e.to_string())).await?;
                                continue;
                            }
                        };
                        match app.propose(id.clone(), actor.clone(), decoded).await {
                            Ok(at) => {
                                send_json(socket, &ServerMsg::ack(&id, at)).await?;
                                if at > seq {
                                    awaiting = Some(at);
                                }
                            }
                            Err(why) => {
                                send_json(socket, &ServerMsg::nack(&id, &why)).await?;
                            }
                        }
                    }
                    Ok(ClientMsg::Ping) => send_json(socket, &serde_json::json!({"t":"pong"})).await?,
                    Ok(ClientMsg::Hello { .. }) => {}
                    Err(e) => {
                        telemetry().bad_messages.incr();
                        tracing::debug!(error = %e, "unparseable client message");
                    }
                }
            }
        }
    }
    Ok(())
}

async fn wait_for_hello<S: Socket>(socket: &mut S) -> Result<Option<(String, u64, String)>> {
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(t) => match ClientMsg::parse(&t) {
                Ok(ClientMsg::Hello { sub, seq, actor }) => return Ok(Some((sub, seq, actor))),
                Ok(_) => continue,
                Err(e) => {
                    telemetry().bad_messages.incr();
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

/// Holds this subscription's share of the arranged-entries gauge.
///
/// §5.3 names per-session memory as a metric to export, and [`docs/24-incremental-views-report.md`]
/// §24.10 recorded that `Engine::footprint` computed one and nothing exported it. This exports the
/// unit that scales — arrangement *entries*, `O(operators)` to read — rather than bytes, which
/// would need a walk of the accumulator on every render.
///
/// A guard rather than a pair of calls, for the same reason [`SessionGuard`] is one: a subscription
/// ends by returning, by erroring or by its socket dying, and a gauge that only releases its share
/// on the happy path drifts upward until it is describing connections that closed hours ago.
struct Arranged(u64);

impl Arranged {
    fn new() -> Arranged {
        Arranged(0)
    }

    fn update(&mut self, now: u64) {
        telemetry().session_arranged.adjust(self.0, now);
        self.0 = now;
    }
}

impl Drop for Arranged {
    fn drop(&mut self) {
        telemetry().session_arranged.adjust(self.0, 0);
    }
}

/// Holds the active-session count for as long as a session is running.
struct SessionGuard;

impl SessionGuard {
    fn new() -> SessionGuard {
        telemetry().sessions.incr();
        SessionGuard
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        telemetry().sessions.decr();
    }
}

async fn send_json<S: Socket>(socket: &mut S, value: &serde_json::Value) -> Result<()> {
    let text = value.to_string();
    // Counted here rather than at each call site: every frame the server sends goes through this
    // function, so the count cannot drift from what was actually written to a socket.
    telemetry().patch_frames.incr();
    telemetry().patch_bytes.add(text.len() as u64);
    socket.send(Message::Text(text.into())).await?;
    Ok(())
}
