//! The bridge between a compiled program and the runtime that drives it.
//!
//! This is the "Roc platform" of Beck (`docs/05-tier-lowering.md` §5.2): an effectful Rust host
//! owning I/O, scheduling and memory, executing the pure program. The program supplies four
//! closures the splitter sliced out of the signal graph — `validate`, the fold, its initial state,
//! and the view — and the host supplies everything those closures are not allowed to have.
//!
//! Note what is *not* here: no domain types, no todo, no HTML template. That is the whole claim of
//! Phase 1 over Phase 0 — the same runtime, with the application arriving as compiled `Core`
//! rather than as hand-written Rust.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use beck_core::backend::{Backend, Callable};
use beck_core::core::CoreKind;
use beck_core::engine::{Engine, Prepared, Retention, SharedDataflow};
use beck_core::plan::Plan;
use beck_core::{Core, Html, Placed, Value};

use crate::record::Envelope;

/// The compiled program plus the capabilities the host holds on its behalf.
///
/// Note what is absent: any mention of *how* the program executes. The roles are [`Callable`]s a
/// [`Backend`] prepared, so a native backend is a different argument to [`Runtime::new`] rather
/// than a change here — and §4.8's differential test between backends is two `Runtime`s over the
/// same `Placed`.
pub struct Runtime {
    placed: Placed,
    backend: Arc<dyn Backend>,
    /// Prepared once at startup: the roles the splitter sliced out of the signal graph.
    validate: Callable,
    fold_fn: Callable,
    view_fn: Callable,
    /// `awareness(f)`'s `f`, prepared, when the page reads a roster with a payload.
    ///
    /// The runtime is what applies it, because the subscribers are its fact rather than the
    /// graph's: it holds every connection's `Session` and turns each into that client's
    /// contribution. `None` for a page that reads no roster.
    awareness_fn: Option<Callable>,
    /// The same view as a dataflow plan (§5.3), with every operator prepared. Compiled once and
    /// shared by every subscription: an [`Engine`] per subscriber holds the arrangements, and this
    /// holds the code.
    plan: Arc<Prepared>,
    init: Value,
    /// The program's `Command` union, resolved to a decoder. Shared with a Mode B client through
    /// its bundle, so both tiers decode a command the same way.
    command: beck_core::command::Schema,
}

impl Runtime {
    /// Prepare a program for execution by a given backend.
    ///
    /// The backend is an argument rather than a default because a default is how `beck-rt` ends up
    /// naming one implementation again. This crate does not depend on any backend crate, and that
    /// is the property worth keeping.
    pub fn new(placed: Placed, backend: Arc<dyn Backend>) -> Result<Runtime> {
        let role = |code: &Core, what: &str| -> Result<Callable> {
            backend
                .function(code)
                .map_err(|e| anyhow!("preparing {what}: {e}"))
        };
        let validate = role(&placed.roles.validate, "`validate`")?;
        let fold_fn = role(&placed.roles.fold, "the fold")?;
        let view_fn = role(&placed.roles.view, "the view")?;
        let awareness_fn = match &placed.roles.awareness {
            Some(f) => Some(role(f, "the awareness function")?),
            None => None,
        };
        let plan = Arc::new(
            Prepared::compile(&placed, backend.as_ref())
                .map_err(|e| anyhow!("compiling the view plan: {e}"))?,
        );
        let init = backend
            .constant(&placed.roles.init)
            .map_err(|e| anyhow!("evaluating the initial state: {e}"))?;

        let command = beck_core::command::Schema::of(&placed);
        Ok(Runtime {
            placed,
            backend,
            validate,
            fold_fn,
            view_fn,
            awareness_fn,
            plan,
            init,
            command,
        })
    }

    /// Which backend prepared this program — for a diagnostic, and for the report that says two
    /// backends disagreed.
    pub fn backend(&self) -> &'static str {
        self.backend.name()
    }

    pub fn placed(&self) -> &Placed {
        &self.placed
    }

    pub fn wire_id(&self) -> &str {
        &self.placed.wire_id
    }

    pub fn initial_state(&self) -> Result<Value> {
        Ok(self.init.clone())
    }

    /// Build the `Proposal` record the program's `validate` expects.
    pub fn proposal(&self, actor: &(impl Viewer + ?Sized), command: Value) -> Value {
        beck_core::edge::proposal(actor.actor(), claims_of(actor), actor.path(), command)
    }

    /// The authority chokepoint, as the program wrote it: the whole
    /// `Result[list[Event], Rejection]`.
    ///
    /// [`Runtime::validate`] narrows this to "events, or a message", which is what an ingress
    /// handler needs and what a test cannot use: §21.2's `expect Err(BlankText)` is an assertion
    /// about the *rejection value*, and rendering it to a string first would make the assertion a
    /// string comparison.
    pub fn decide(&self, state: &Value, proposal: &Value) -> Result<Value, String> {
        (self.validate)(vec![state.clone(), proposal.clone()]).map_err(|e| e.to_string())
    }

    /// The authority chokepoint. Returns the events a proposal becomes, or why it was refused.
    pub fn validate(&self, state: &Value, proposal: &Value) -> Result<Vec<Value>, String> {
        let out =
            (self.validate)(vec![state.clone(), proposal.clone()]).map_err(|e| e.to_string())?;
        match out.variant() {
            Some("Ok") => match out.field("value").and_then(|v| v.as_list()) {
                Some(events) => Ok(events.clone()),
                None => Err("validate returned Ok without a list of events".into()),
            },
            Some("Err") => Err(out
                .field("error")
                .map(|e| e.display())
                .unwrap_or_else(|| "rejected".into())),
            _ => Err(format!("validate returned {}", out.display())),
        }
    }

    /// The replay-pure fold. `env` supplies `seq`, `at` and `actor` **as data** (§3.7).
    pub fn fold(&self, state: &Value, env: &Envelope, event: Value) -> Result<Value> {
        (self.fold_fn)(vec![state.clone(), env.to_value(event)])
            .map_err(|e| anyhow!("folding at seq {}: {e}", env.seq))
    }

    /// The per-session view. In Mode A this runs server-side and its output is diffed (§5.1).
    ///
    /// The roster it renders against is the viewer's own — `edge::presence_of` — because a caller
    /// with no connection registry is rendering the page one actor sees while looking at it.
    /// [`Runtime::view_with`] is what an application uses, and it is the same function.
    pub fn view(&self, state: &Value, actor: &(impl Viewer + ?Sized)) -> Result<Html> {
        let mine = self.contribution(actor)?;
        self.view_with_all(
            state,
            actor,
            &beck_core::edge::presence_of(actor.actor()),
            &mine,
        )
    }

    /// This client's own awareness contribution, or an empty roster when the page reads none.
    ///
    /// The one-connection case, and it is [`Runtime::view`]'s reason: a caller with no registry is
    /// rendering the page one actor sees while looking at it, so the roster contains them and
    /// nobody else.
    pub fn contribution(&self, actor: &(impl Viewer + ?Sized)) -> Result<Value> {
        match self.contribution_of(actor)? {
            None => Ok(beck_core::edge::no_awareness()),
            Some(mine) => Ok(beck_core::edge::awareness_of(actor.actor(), mine)),
        }
    }

    /// What this client contributes to everybody else's roster, or `None` when the page reads no
    /// awareness.
    ///
    /// The bare value rather than a roster of one, because the caller that matters is a *registry*
    /// holding one of these per connection (`beck_rt::awareness`), and it keys them itself. `None`
    /// rather than `Unit` so that a program which reads no awareness costs a registry nothing —
    /// there is a difference between contributing nothing and having nothing to contribute.
    pub fn contribution_of(&self, actor: &(impl Viewer + ?Sized)) -> Result<Option<Value>> {
        let Some(f) = &self.awareness_fn else {
            return Ok(None);
        };
        let mine = f(vec![session(actor.actor(), claims_of(actor), actor.path())])
            .map_err(|e| anyhow!("{e}"))
            .context("computing this client's awareness")?;
        Ok(Some(mine))
    }

    /// The same view, against a roster somebody else is keeping (`crate::presence`).
    pub fn view_with(
        &self,
        state: &Value,
        actor: &(impl Viewer + ?Sized),
        here: &Value,
    ) -> Result<Html> {
        let mine = self.contribution(actor)?;
        self.view_with_all(state, actor, here, &mine)
    }

    /// The view, against both rosters a caller may be keeping.
    pub fn view_with_all(
        &self,
        state: &Value,
        actor: &(impl Viewer + ?Sized),
        here: &Value,
        aware: &Value,
    ) -> Result<Html> {
        let out = (self.view_fn)(vec![
            state.clone(),
            session(actor.actor(), claims_of(actor), actor.path()),
            here.clone(),
            aware.clone(),
            // Confirmed, and not a parameter, because this is the server's render: what it holds
            // is the fold over the log, and a guess is something only a Mode B client has. The
            // checker makes the constant unobservable — a page that reads `freshness()` cannot
            // render on the server at all (`B0518`) — so this is the value the SSR of a Mode B
            // page is rendered with and nothing else ever sees it.
            beck_core::edge::confirmed(),
        ])
        .map_err(|e| anyhow!("{e}"))
        .context("rendering the view")?;
        match out {
            Value::Html(h) => Ok((*h).clone()),
            other => Err(anyhow!(
                "the view produced {} rather than Html",
                other.display()
            )),
        }
    }

    /// The view as a dataflow plan — what `beck explain incremental` reports on.
    pub fn plan(&self) -> &Arc<Plan> {
        self.plan.plan()
    }

    /// A maintained view for one subscriber, computing the whole plan itself.
    ///
    /// One per subscription, because §3.8's per-session views are "the norm, not the exception" and
    /// an arrangement below a `per_session` is that subscriber's. Everything *above* it is the same
    /// computation for everybody — [`Plan::shared`] says which nodes — and this engine holds a copy
    /// of it. [`Runtime::shared_dataflow`] is the one that does not.
    pub fn view_engine(&self) -> Result<Engine> {
        Ok(Engine::new(self.plan.clone()))
    }

    /// The shared half of the plan — §5.3's "one shared dataflow" — for a process to hold one of.
    ///
    /// It is created per application rather than per `Runtime` because what it holds is derived
    /// from the accumulator, and the accumulator belongs to the application. A `Runtime` with no
    /// application driving it (`beck test`, the differential harness) never makes one.
    ///
    /// `retention` says how long it keeps what a subscriber might still ask for, and comes from the
    /// application's configuration for the reason `beck_rt::AppConfig::retention` gives.
    pub fn shared_dataflow(&self, retention: Retention) -> Arc<SharedDataflow> {
        Arc::new(SharedDataflow::with_retention(self.plan.clone(), retention))
    }

    /// Render a subscriber's view by maintaining it, rather than by recomputing it.
    ///
    /// Identical output to [`Runtime::view`] — `beck-cli/tests/incremental_engine.rs` is the gate,
    /// over every corpus program and every event of a generated log.
    pub fn render(
        &self,
        engine: &mut Engine,
        state: &Value,
        actor: &(impl Viewer + ?Sized),
        here: &Value,
        aware: &Value,
    ) -> Result<Html> {
        let out = engine
            .render_all(
                state,
                &session(actor.actor(), claims_of(actor), actor.path()),
                here,
                aware,
            )
            .map_err(|e| anyhow!("{e}"))
            .context("maintaining the view")?;
        match out {
            Value::Html(h) => Ok((*h).clone()),
            other => Err(anyhow!(
                "the view produced {} rather than Html",
                other.display()
            )),
        }
    }

    /// The same maintained render, with the operators that do not read the session taken from a
    /// dataflow shared with every other subscriber (§5.3).
    ///
    /// Returns the version the page reflects, which may be newer than `version`: another subscriber
    /// may have advanced the shared side first, and a page of the newer state is right where
    /// unwinding an arrangement back to the older one is not.
    #[allow(clippy::too_many_arguments)]
    pub fn render_shared(
        &self,
        shared: &SharedDataflow,
        engine: &mut Engine,
        state: &Value,
        version: u64,
        actor: &(impl Viewer + ?Sized),
        here: &Value,
        aware: &Value,
    ) -> Result<(Html, u64)> {
        let (out, at) = shared
            .render_all(
                engine,
                state,
                version,
                &session(actor.actor(), claims_of(actor), actor.path()),
                here,
                aware,
            )
            .map_err(|e| anyhow!("{e}"))
            .context("maintaining the view")?;
        match out {
            Value::Html(h) => Ok(((*h).clone(), at)),
            other => Err(anyhow!(
                "the view produced {} rather than Html",
                other.display()
            )),
        }
    }

    /// Prepare an arbitrary `Core` lambda for calling, through the same backend the roles use.
    ///
    /// The one caller is the test runner (§21.2), which has to evaluate an `expect` expression with
    /// `state`, `events` and `result` bound. It goes through [`Backend::function`] rather than
    /// reaching into an evaluator, so a compiling backend serves it unchanged.
    pub fn prepare(&self, code: &Core) -> Result<Callable> {
        self.backend
            .function(code)
            .map_err(|e| anyhow!("preparing an expression: {e}"))
    }

    /// The `Session` value a subscriber's view is rendered against.
    ///
    /// Public because the incremental view engine takes it as an input rather than receiving it
    /// through [`Runtime::view`]: a plan's session is a *node*, and everything not downstream of it
    /// is what §5.3 shares between subscribers.
    pub fn session(&self, actor: &(impl Viewer + ?Sized)) -> Value {
        session(actor.actor(), claims_of(actor), actor.path())
    }

    /// Decode a command from the wire, against the program's own `Command` union.
    ///
    /// The union is resolved to a [`beck_core::command::Schema`] once, at compile time, and both
    /// tiers decode with it: Mode B's client holds a bundle rather than a program, and a second
    /// decoder written against a second reading of the same union is the failure mode that is
    /// worth designing out ([`beck_core::command`]).
    pub fn decode_command(&self, json: &serde_json::Value) -> Result<Value> {
        self.command.decode(json).map_err(|e| anyhow!(e))
    }
}

use beck_core::edge::session;

/// Who a view is rendered for, or a command proposed by.
///
/// A trait rather than a type because the two sources of one are genuinely different and both are
/// legitimate. A **connection** supplies a `beck_rt::identity::Actor`, which only that module's
/// `Identity::verify` can make and which carries the claims the provider verified — the impl for it
/// is there rather than here, because the credential is the host's to check. A **name** supplies
/// itself: `beck test`'s `when session("ana") sends …`, the differential harness, a benchmark, a
/// client of the playground's tab server — none of them is a connection, so none of them has a
/// credential to check, and each of them has no claims because there was nobody to make any.
///
/// One code path either way, which is the point: a `&str` and an `Actor` reach the same `Session`
/// constructor, so a claim cannot appear in one render path and not another.
pub trait Viewer {
    fn actor(&self) -> &str;

    /// Empty unless a provider verified them.
    fn claims(&self) -> &BTreeMap<Arc<str>, Arc<str>> {
        static NONE: std::sync::OnceLock<BTreeMap<Arc<str>, Arc<str>>> = std::sync::OnceLock::new();
        NONE.get_or_init(BTreeMap::new)
    }

    /// Where this viewer is — the route, as the browser last stated it.
    ///
    /// Defaulted rather than required, and the default is the application's root: a viewer that is
    /// not a browser has no route, and `beck test`, the differential harness and every benchmark
    /// are exactly that. The one implementation that overrides it is the subscription's, because a
    /// socket is the only thing that can be told the route changed.
    fn path(&self) -> &str {
        beck_core::edge::ROOT
    }
}

impl Viewer for str {
    fn actor(&self) -> &str {
        self
    }
}

/// So a caller holding a `&&str` — which is what iterating a `[&str]` gives — needs no ceremony.
impl<T: Viewer + ?Sized> Viewer for &T {
    fn actor(&self) -> &str {
        (**self).actor()
    }

    fn claims(&self) -> &BTreeMap<Arc<str>, Arc<str>> {
        (**self).claims()
    }

    fn path(&self) -> &str {
        (**self).path()
    }
}

impl Viewer for String {
    fn actor(&self) -> &str {
        self
    }
}

impl Viewer for Arc<str> {
    fn actor(&self) -> &str {
        self
    }
}

/// A viewer, somewhere.
///
/// Who is asking and where they are are separate facts, and this is the pair. It is generic over
/// the identity half because both sources of one need a route: the document handler wraps the
/// actor a provider verified with the path of the request that rendered it, and `beck test` wraps
/// the name a test wrote with the route it wrote beside it. The socket's equivalent is
/// `session::Subscriber`, which is this pair with a route that can move — an HTTP request is one
/// route by construction and a subscription is not.
pub struct At<W> {
    pub who: W,
    pub path: Arc<str>,
}

impl<W: Viewer> Viewer for At<W> {
    fn actor(&self) -> &str {
        self.who.actor()
    }

    fn claims(&self) -> &BTreeMap<Arc<str>, Arc<str>> {
        self.who.claims()
    }

    fn path(&self) -> &str {
        &self.path
    }
}

/// A viewer's claims, in the shape [`beck_core::edge::session`] takes them.
fn claims_of(viewer: &(impl Viewer + ?Sized)) -> impl Iterator<Item = (&str, &str)> {
    viewer
        .claims()
        .iter()
        .map(|(k, v)| (k.as_ref(), v.as_ref()))
}

/// A `Core` value's shape, for `beck explain`.
pub fn describe(c: &Core) -> String {
    match &c.kind {
        CoreKind::Lam { params, .. } => format!("fn/{}", params.len()),
        CoreKind::Global(n) => n.to_string(),
        CoreKind::Prim { op, .. } => op.name().to_string(),
        _ => format!("{}", c.ty),
    }
}
