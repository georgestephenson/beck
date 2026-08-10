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
use crate::patch::{DataFrame, PatchFrame};
use crate::protocol::{ClientMsg, Resumption, ServerMsg};
use crate::telemetry::telemetry;
use beck_core::delta;
use beck_core::diff::{diff, Op};
use beck_core::render::Mode;
use beck_core::Value;

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
pub async fn run<S: Socket>(app: Arc<App>, socket: S) -> Result<()> {
    run_as(app, socket, None).await
}

/// The same, for a connection whose identity was already decided at the HTTP upgrade.
///
/// A browser logged in through [`crate::oidc`] carries its credential in a **cookie**, which the
/// `hello` frame cannot see and must not: putting the token in the frame would mean putting it in
/// the document, where a script can read it. So the upgrade verifies, and hands the result here —
/// and when it does, the frame's `actor` is not consulted at all.
pub async fn run_as<S: Socket>(
    app: Arc<App>,
    mut socket: S,
    verified: Option<crate::identity::Actor>,
) -> Result<()> {
    // A guard, not a pair of calls: a session ends by returning, by erroring, or by the socket
    // dying, and a gauge that only decrements on the happy path drifts upward forever.
    let _connected = SessionGuard::new();
    let Some((sub, from, claimed, path)) = wait_for_hello(&mut socket).await? else {
        return Ok(());
    };

    // The one place a socket's actor is decided, when the upgrade did not already decide it.
    // Before this existed the claim *was* the actor, and every ownership check in every program
    // was enforced against a value the caller chose (`docs/42` §42.6, `docs/43` §43.4).
    let actor = match verified {
        Some(actor) => actor,
        None => match app.identity().verify(&claimed) {
            Ok(a) => a,
            Err(why) => {
                // The operator learns which refusal it was; the client learns that it was refused.
                tracing::warn!(sub = %sub, reason = why.reason(), "identity refused");
                telemetry().unauthenticated.incr();
                send_json(&mut socket, &ServerMsg::error(why.message())).await?;
                return Ok(());
            }
        },
    };

    // In the roster from here until this function returns, whichever way it returns. Joining
    // *before* the first render is what makes a connecting client see itself: the page it is sent
    // is the page of a world it is already in.
    let _here = app.presence().join(crate::program::Viewer::actor(&actor));

    let floor = app.floor().await?;
    let head = app.head();
    let how = match from {
        // The client holds nothing, so there is nothing to be a difference from.
        None => Resumption::Fresh,
        // Position zero is always reachable whatever the log's floor is: the state at zero is the
        // fold's initial accumulator, which is reconstructed rather than read.
        Some(0) => Resumption::Resumed {
            from: 0,
            replayed: head,
        },
        // The gap is unreachable — the log was trimmed, or this `seq` is from another lifetime of
        // the application. Reset, and say so rather than pretending.
        Some(n) if n < floor || n > head => Resumption::Reset { from: n },
        Some(n) => Resumption::Resumed {
            from: n,
            replayed: head - n,
        },
    };

    let who = Subscriber { actor, path };

    // The one branch a rendering mode makes to a subscription.
    match app.runtime().placed().render.mode {
        Mode::Server => mode_a(app, socket, sub, who, how).await,
        Mode::Client => mode_b(app, socket, sub, who, how).await,
    }
}

/// Who this subscription is, and where.
///
/// The identity half is an [`crate::identity::Actor`], which only a provider can mint. The route
/// half is a `String` the client sent, which nothing verifies and nothing should — a route is not
/// evidence of anything, and [`beck_core::render`] is where that difference stops being a comment
/// and becomes a rule about which pages may render in a browser.
///
/// It is one value rather than two arguments because everything downstream takes a
/// [`crate::program::Viewer`], and a route threaded separately would be a second parameter that
/// every render path could forget.
pub(crate) struct Subscriber {
    actor: crate::identity::Actor,
    path: String,
}

impl crate::program::Viewer for Subscriber {
    fn actor(&self) -> &str {
        self.actor.name()
    }

    fn claims(&self) -> &std::collections::BTreeMap<std::sync::Arc<str>, std::sync::Arc<str>> {
        self.actor.claims()
    }

    fn path(&self) -> &str {
        &self.path
    }
}

/// Mode A: the server renders, and the frames are DOM patches.
async fn mode_a<S: Socket>(
    app: Arc<App>,
    mut socket: S,
    sub: String,
    who: Subscriber,
    how: Resumption,
) -> Result<()> {
    // Subscribe *before* reading the current view, so an event that lands in between wakes us
    // rather than being missed.
    let mut version = app.subscribe();
    // Declared *before* the engine so it is dropped after it: what the shared dataflow holds
    // changes when this subscription's engine goes, and sampling before that would leave the gauge
    // describing arrangements the process has just released.
    let _shared = SharedGauge(app.clone());
    // One engine per subscription: §5.3's per-subscriber operators, and the arrangements they
    // hold. With sharing on that is *only* the per-session operators — everything above them is one
    // dataflow the application holds. It is created before the first render so that render is the
    // engine's own cold start rather than a recompute the engine then has to catch up with.
    //
    // It is also this subscription's membership of the shared dataflow's reader set, and dropping
    // it is how the dataflow learns the subscription is over (`docs/26` §26.9's lifecycle).
    let mut engine = app.view_engine()?;
    let mut arranged = Arranged::new();
    // `seq` comes back from the render rather than from `app.head()` afterwards: it is the version
    // the page reflects, and this frame will be the one a resuming client asks for the difference
    // from.
    let (view_now, seq) = app.maintain(&mut engine, &who).await?;
    arranged.update(engine.arranged());
    report_shared(&app);

    let ops = match how {
        // The client has nothing we can trust: hand it the whole frame. Same format, same
        // interpreter — a reset is just a patch that happens to replace the root.
        Resumption::Fresh | Resumption::Reset { .. } => vec![Op::Replace {
            path: vec![],
            html: view_now.clone(),
        }],
        // The client has the view as of `from`: send it exactly the difference.
        Resumption::Resumed { from, .. } => {
            let then = app.state_at(from).await?;
            let view_then = app.runtime().view(&then, &who)?;
            diff(&view_then, &view_now)
        }
    };
    let initial = (!ops.is_empty()).then(|| PatchFrame::new(seq, ops).to_json());

    drive(
        &app,
        &mut socket,
        sub,
        who,
        seq,
        how,
        initial,
        &mut version,
        Feed::Dom {
            engine,
            arranged,
            last: view_now,
        },
    )
    .await
}

/// Mode B: the browser renders, and the frames are state diffs.
///
/// Note what this function does not construct: a view engine, and therefore no per-session
/// arrangements. That is D5's "less server work per user", and it is a consequence of the mode
/// rather than an optimisation applied to it.
async fn mode_b<S: Socket>(
    app: Arc<App>,
    mut socket: S,
    sub: String,
    who: Subscriber,
    how: Resumption,
) -> Result<()> {
    // Subscribe *before* reading the state, for the same reason Mode A does: an event that lands
    // in between has to wake us rather than be missed.
    let mut version = app.subscribe();
    // The state and the position it reflects, read together: a client told "this is the state at
    // 41" when it is the state at 42 would apply the next patch to the wrong base, and every patch
    // after that would be wrong too.
    let (state, seq) = app.read_snapshot(|s, q| (s.clone(), q)).await;

    let frame = match how {
        Resumption::Fresh | Resumption::Reset { .. } => DataFrame::whole(seq, &state),
        Resumption::Resumed { from, .. } => {
            let then = app.state_at(from).await?;
            Some(DataFrame::Ops {
                seq,
                ops: delta::diff(&then, &state),
            })
        }
    };
    let initial = frame.filter(|f| !f.is_empty()).map(|f| f.to_json());

    drive(
        &app,
        &mut socket,
        sub,
        who,
        seq,
        how,
        initial,
        &mut version,
        Feed::Data { last: state },
    )
    .await
}

/// What this subscription sends when the state moves — §5.1's table, as two variants.
///
/// The variants are deliberately different sizes: a Mode A subscription carries a view engine and
/// its arrangements, a Mode B one carries a `Value`. Boxing the larger to even them out would hide
/// the asymmetry that is the mode's whole point (D5's "less server work per user"), for one
/// allocation per connection.
#[allow(clippy::large_enum_variant)]
enum Feed {
    /// The server renders per subscriber and streams the difference between two pages.
    Dom {
        engine: beck_core::engine::Engine,
        arranged: Arranged,
        last: Html,
    },
    /// The browser renders, so what moves is the accumulator.
    Data { last: Value },
}

impl Feed {
    /// The frame this subscriber is owed now, and the position it brings them to.
    ///
    /// `None` means nothing changed *for this subscriber* — the common case on a busy application,
    /// and the reason an idle connection costs no bytes in either mode.
    async fn advance(
        &mut self,
        app: &Arc<App>,
        who: &Subscriber,
    ) -> Result<(Option<serde_json::Value>, u64)> {
        match self {
            Feed::Dom {
                engine,
                arranged,
                last,
            } => {
                let (view, at) = app.maintain(engine, who).await?;
                arranged.update(engine.arranged());
                report_shared(app);
                let started = std::time::Instant::now();
                let ops = diff(last, &view);
                telemetry().diff.record(started.elapsed());
                *last = view;
                Ok((
                    (!ops.is_empty()).then(|| PatchFrame::new(at, ops).to_json()),
                    at,
                ))
            }
            Feed::Data { last } => {
                let (state, at) = app.read_snapshot(|s, q| (s.clone(), q)).await;
                let started = std::time::Instant::now();
                let ops = delta::diff(last, &state);
                telemetry().diff.record(started.elapsed());
                *last = state;
                Ok((
                    (!ops.is_empty()).then(|| DataFrame::Ops { seq: at, ops }.to_json()),
                    at,
                ))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive<S: Socket>(
    app: &Arc<App>,
    socket: &mut S,
    sub: String,
    mut who: Subscriber,
    mut seq: u64,
    how: Resumption,
    initial: Option<serde_json::Value>,
    version: &mut tokio::sync::watch::Receiver<u64>,
    mut feed: Feed,
) -> Result<()> {
    // A second thing this subscription may have to wake on, and only when the program asked: a
    // page that never mentions `presence` must not re-render because somebody else connected.
    // Which is a compile-time fact, so this is a property of the program rather than a heuristic.
    let mut here = app
        .runtime()
        .placed()
        .roles
        .view_reads_presence
        .then(|| app.presence().watch());
    // How a subscriber was brought up to date is exactly the distinction Phase 0 got wrong twice
    // (§18.5 item 1): an ack means committed, a frame means your view has caught up.
    tracing::info!(seq, sub = %sub, how = how.label(), "subscribed");
    send_json(socket, &ServerMsg::welcome(&sub, seq, how)).await?;
    if let Some(frame) = initial {
        send_json(socket, &frame).await?;
    }

    // The highest seq this client was told about but has not yet seen reflected in its view. Only
    // a client waiting on its own command is sent an "up to date" notice.
    let mut awaiting: Option<u64> = None;

    let mut draining = app.draining();
    loop {
        tokio::select! {
            // A drained server hands its subscriptions back rather than holding them open: the
            // client reconnects, to this process or to the one that replaced it (§5.2).
            _ = draining.changed() => {
                if *draining.borrow() {
                    tracing::info!(sub = %sub, "draining: ending the subscription");
                    break;
                }
            }
            // The roster moved: somebody arrived or left. Nothing in the log moved, so `seq` does
            // not, and what this sends is a patch labelled with the position it already had.
            changed = wait(&mut here), if here.is_some() => {
                if changed.is_err() {
                    break; // the application is gone
                }
                let (frame, _) = feed.advance(app, &who).await?;
                if let Some(frame) = frame {
                    send_json(socket, &frame).await?;
                }
            }
            changed = version.changed() => {
                if changed.is_err() {
                    break; // the application is gone
                }
                let (frame, at) = feed.advance(app, &who).await?;
                seq = at;
                if let Some(frame) = frame {
                    send_json(socket, &frame).await?;
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
                        match app.propose(id.clone(), who.actor.clone(), decoded).await {
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
                    Ok(ClientMsg::Nav { path }) => {
                        // The route is a field of the `Session` the view is rendered against, so a
                        // navigation is a re-render and nothing else — no route table, no second
                        // rendering path, and no code in the runtime that knows what a route is.
                        //
                        // In Mode B this produces no frame at all, which is right rather than a
                        // gap: the browser holds the state and renders its own page, and what the
                        // server needs the route for is the `Session` it hands `validate`.
                        if who.path != path {
                            who.path = path;
                            telemetry().navigations.incr();
                            let (frame, at) = feed.advance(app, &who).await?;
                            seq = at;
                            if let Some(frame) = frame {
                                send_json(socket, &frame).await?;
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

/// Wait on an optional watch, so `select!` can have an arm that is only sometimes armed.
///
/// The `if here.is_some()` guard is what disables the arm, and a disabled arm's expression is still
/// *evaluated* — only its future is never polled — so the call has to be legal with no receiver.
async fn wait(
    here: &mut Option<tokio::sync::watch::Receiver<beck_core::Value>>,
) -> Result<(), tokio::sync::watch::error::RecvError> {
    match here {
        Some(rx) => rx.changed().await,
        None => std::future::pending().await,
    }
}

async fn wait_for_hello<S: Socket>(
    socket: &mut S,
) -> Result<Option<(String, Option<u64>, String, String)>> {
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(t) => match ClientMsg::parse(&t) {
                Ok(ClientMsg::Hello {
                    sub,
                    seq,
                    actor,
                    path,
                }) => return Ok(Some((sub, seq, actor, path))),
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
/// §5.3 names per-session memory as a metric to export, and `docs/24-incremental-views-report.md`
/// §24.10 recorded that `Engine::footprint` computed one and nothing exported it. This exports the
/// unit that scales — arrangement *entries*, `O(operators)` to read — rather than bytes, which
/// would need a walk of the accumulator on every render.
///
/// A guard rather than a pair of calls, for the same reason [`SessionGuard`] is one: a subscription
/// ends by returning, by erroring or by its socket dying, and a gauge that only releases its share
/// on the happy path drifts upward until it is describing connections that closed hours ago.
/// Sample what the **one** shared dataflow holds, after a render has just moved it.
///
/// Three numbers, and they answer different questions: `arranged` is what a fanout costs once,
/// `retained` is how far behind the laggiest subscriber is, and `releases` is how often the process
/// has thrown the arrangements away because nobody was connected. A render is the right moment for
/// all three — it is when they change, and it is `O(operators)` to read (`docs/26`).
fn report_shared(app: &Arc<App>) {
    let shared = app.shared_dataflow();
    telemetry().shared_arranged.set(shared.arranged());
    telemetry().shared_retained.set(shared.retained() as u64);
    telemetry().shared_releases.sync(shared.releases());
}

/// Re-samples the shared dataflow's numbers when a subscription ends.
///
/// A guard rather than a call at the end of `run`, for the reason [`Arranged`] is one: a
/// subscription ends by returning, by erroring or by its socket dying. And it matters more here
/// than it looks — the *last* subscription to end is the one that releases the arrangements, so
/// without this the gauge sits at whatever the fanout was holding for as long as the process is
/// idle, which is the one moment an operator most wants it to say zero.
struct SharedGauge(Arc<App>);

impl Drop for SharedGauge {
    fn drop(&mut self) {
        report_shared(&self.0);
    }
}

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
