//! Where a component renders — the Mode A / Mode B decision, and what it costs to be wrong.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.1 gives two rendering
//! modes over one source:
//!
//! | | **Mode A — thin** | **Mode B — local** |
//! |---|---|---|
//! | `view` runs on | server | client |
//! | Wire carries | DOM patches | data patches (state diffs) |
//! | Optimistic UI | no | yes — the same fold runs locally, reconciled by `seq` |
//!
//! The row that decides everything is the second one. Mode A sends the browser a *rendering* of
//! the state; Mode B sends it the **state**. Everything below follows from taking that literally.
//!
//! # The rule: a Mode B page may not be a function of **who** is asking
//!
//! A view has the shape `(state, session) -> Html`. If it reads *who* the session is, it renders a
//! different page for different actors *from the same state* — which is to say it is filtering,
//! scoping or hiding by identity. Running that view on the client requires giving the client the
//! state it filters, so every actor receives what the filter was removing. The page would still
//! look right. That is the worst kind of wrong.
//!
//! So a component whose view reads the session's identity is refused Mode B, and the refusal names
//! the reason rather than a rule. What is left is exactly the class §5.1 and
//! [`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D5 describe as Mode B's: pages
//! that are the same function of the same state for everybody — editors, typeaheads, drag-and-drop,
//! anything single-user or public.
//!
//! ## Which half of the session, and why the distinction is structural
//!
//! `Session` carries three fields and they are not one kind of thing. `actor` and `claims` say
//! **who** is asking and are what an identity provider verified; `path` says **where** they are and
//! is the client's own statement about itself. The argument above is entirely about the first pair:
//! a page that renders by route is not hiding anything from the browser it is running in, because
//! that browser chose the route and already holds the state.
//!
//! So the refusal is decided by [`SessionUse`], which reads the view's own code and asks which
//! fields of a `Session` it can observe. The coarser fact — whether the page is `per_session` at
//! all — is still what §3.8's fanout analysis and §5.3's shared cut use, and it is still true of a
//! page that reads only the route: two people on two routes see two pages, so the operators below
//! the session are theirs. Eligibility and fanout are different questions, and this is where they
//! stopped being the same answer.
//!
//! # Optimism is a property of what crosses, not of a component
//!
//! "The browser applies the expected event to its local copy speculatively — legitimate because it
//! runs the *same pure fold* the server runs" (D5). That is only available to a client that holds
//! the value the fold is *of*. A client holding a projection — a session's filtered list, say —
//! could not apply an event to it without a second, different fold that no program writes. So
//! optimism is not an extra feature layered on Mode B; it is the same fact stated twice, and this
//! module reports it as one decision with two consequences.
//!
//! # And the rule that points the other way
//!
//! §3.7 asks for one more thing of a guess: that the page can *say* it is one. "`Signal[T]` carries
//! a freshness dimension (`confirmed | pending(n)`) that UI code can render (\"saving…\") —
//! staleness is typed, not pretended away." `freshness()` is that dimension, and it is the only
//! thing here a **server** cannot answer: what a server renders is what it has recorded, so its
//! answer is `Confirmed` at every position of every log. So a page reading it is refused Mode A
//! (`B0518`) exactly as a page reading `presence` is refused Mode B — the two rules are the same
//! rule about two facts that live on opposite sides of the wire.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::core::{Core, CoreKind};
use crate::ty::Ty;

/// The field of a [`Session`](crate::edge::session) that says *where* rather than *who*.
pub const ROUTE_FIELD: &str = "path";

/// What a view can observe about the `Session` it is handed.
///
/// Three verdicts rather than a boolean, because "reads the session" was one word for two facts
/// and Mode B's refusal only ever meant one of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionUse {
    /// The view never touches its `Session`. §5.1's Mode B class, and what `signal_map` produces.
    None,
    /// The view reads the route and nothing else. Eligible for Mode B: the browser chose the route
    /// and already holds the state, so a page that varies by it discloses nothing.
    Route,
    /// The view can observe who is asking — `actor`, `claims`, or the whole record. Refused Mode B.
    Identity {
        /// What was read, in the order a message should name it. `"the session itself"` when a
        /// `Session` reached somewhere this analysis cannot follow it into.
        what: Vec<Arc<str>>,
    },
}

impl SessionUse {
    pub fn reads_identity(&self) -> bool {
        matches!(self, SessionUse::Identity { .. })
    }

    /// What the view reads, for a message and for `beck explain render`.
    pub fn describe(&self) -> String {
        match self {
            SessionUse::None => "nothing".to_string(),
            SessionUse::Route => format!("`session.{ROUTE_FIELD}`"),
            SessionUse::Identity { what } => what
                .iter()
                .map(|w| w.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// Read the view's code, and every definition it reaches, for what it does with a `Session`.
    ///
    /// The rule is one sentence: **a `Session` can only be observed by having a field read off it,
    /// so collect every field read whose base is `Session`-typed anywhere the view can reach.**
    /// That is sound without tracking where the value flows, because flow does not create an
    /// observation — wherever the record ends up, reading it is still a `Field` over a
    /// `Session`-typed base, and every definition it could end up in is in this closure. What flow
    /// *could* hide is an observation that is not a field read: an equality, a digest, a session
    /// stored inside a value that crosses. Those are the `escapes` below, and they are the
    /// conservative answer rather than an ignored case.
    ///
    /// Types make it cheap. A field read needs a concrete record type, so a `Session` passed
    /// through a generic definition cannot have anything read off it there — the parameter is a
    /// rigid variable and `x.actor` does not check. There is nowhere for a read to hide.
    pub fn of(view: &Core, defs: &BTreeMap<Arc<str>, crate::check::Def>) -> SessionUse {
        let mut found = Found::default();
        let mut seen: BTreeSet<Arc<str>> = BTreeSet::new();
        walk(view, defs, &mut seen, &mut found);

        if found.escapes {
            return SessionUse::Identity {
                what: vec![Arc::from("the session itself")],
            };
        }
        let identity: Vec<Arc<str>> = found
            .fields
            .iter()
            .filter(|f| f.as_ref() != ROUTE_FIELD)
            .map(|f| Arc::from(format!("`session.{f}`").as_str()))
            .collect();
        if !identity.is_empty() {
            return SessionUse::Identity { what: identity };
        }
        if found.fields.is_empty() {
            SessionUse::None
        } else {
            SessionUse::Route
        }
    }
}

#[derive(Default)]
struct Found {
    fields: BTreeSet<Arc<str>>,
    /// A `Session` reached somewhere a field read is not what happens to it — a primitive, or the
    /// inside of a constructed value. Neither can be followed, so both are identity.
    escapes: bool,
}

fn is_session(ty: &Ty) -> bool {
    ty.con_name() == Some("Session")
}

fn walk(
    code: &Core,
    defs: &BTreeMap<Arc<str>, crate::check::Def>,
    seen: &mut BTreeSet<Arc<str>>,
    found: &mut Found,
) {
    match &code.kind {
        CoreKind::Field { base, name } if is_session(&base.ty) => {
            found.fields.insert(name.clone());
            walk(base, defs, seen, found);
            return;
        }
        CoreKind::Global(name) => {
            if seen.insert(name.clone()) {
                if let Some(def) = defs.get(name) {
                    walk(&def.body, defs, seen, found);
                }
            }
            return;
        }
        // A primitive is opaque: `==`, a digest, anything that consumes the record whole.
        CoreKind::Prim { args, .. } if args.iter().any(|a| is_session(&a.ty)) => {
            found.escapes = true;
        }
        // A session put *inside* a value goes wherever that value goes, including across the wire.
        CoreKind::Make { fields, .. } | CoreKind::With { fields, .. }
            if fields.iter().any(|(_, v)| is_session(&v.ty)) =>
        {
            found.escapes = true;
        }
        CoreKind::ListLit(items) if items.iter().any(|i| is_session(&i.ty)) => {
            found.escapes = true;
        }
        CoreKind::MapLit(pairs)
            if pairs
                .iter()
                .any(|(k, v)| is_session(&k.ty) || is_session(&v.ty)) =>
        {
            found.escapes = true;
        }
        _ => {}
    }
    for child in crate::core::children(code) {
        walk(child, defs, seen, found);
    }
}

/// Where a component's `view` runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Mode A: the server renders and the wire carries DOM patches. The default.
    Server,
    /// Mode B: the browser renders and the wire carries data patches.
    Client,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Server => "server",
            Mode::Client => "client",
        }
    }

    /// The mode's letter, for a report and for the wire.
    pub fn letter(self) -> &'static str {
        match self {
            Mode::Server => "A",
            Mode::Client => "B",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "server" => Some(Mode::Server),
            "client" => Some(Mode::Client),
            _ => None,
        }
    }
}

/// Why a component renders where it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Nothing said otherwise. §5.1: "**v0.1 ships Mode A only**", and Mode A stays the default
    /// because it is the mode that ships no application code to the browser at all.
    Default,
    /// `@render(client)` or `@render(server)`.
    Declared,
}

/// Why a Mode B client may not guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoOptimism {
    /// The component renders on the server, so there is no local copy to guess about.
    ModeA,
    /// A library, or a program with no chokepoint: nothing to propose, so nothing to guess.
    NotAnApplication,
}

/// How one component renders, and what follows from it.
#[derive(Clone, Debug)]
pub struct Decision {
    pub component: Arc<str>,
    pub mode: Mode,
    pub why: Why,
    /// What crosses to the browser: `Html` in Mode A, the fold's accumulator in Mode B.
    pub carries: Ty,
    /// Whether the client may apply a command speculatively before the server answers.
    pub optimistic: bool,
    pub no_optimism: Option<NoOptimism>,
    /// True when the view reads the session at all — §3.8's fanout fact, and §5.3's shared cut.
    pub per_session: bool,
    /// What the view can observe about the session. **This** is what decides Mode B eligibility:
    /// a page may vary by where the browser is and may not vary by who is holding it.
    pub uses: SessionUse,
    /// True when the view reads `presence()`, which is the second fact that decides it.
    pub reads_presence: bool,
    /// True when the view reads `awareness()`, which decides it for the same reason: a roster with
    /// a payload is still a fact the server holds about its own sockets.
    pub reads_awareness: bool,
    /// True when the view reads `freshness()`. The only condition here that refuses **Mode A**:
    /// a server renders the state it has recorded, so its answer is `Confirmed` and nothing else,
    /// and a page that branches on it is a page with a dead branch.
    pub reads_freshness: bool,
    /// True when the view reads a `gestures(step, init)` — D30's non-durable fold. The second
    /// condition that refuses **Mode A**, and for `reads_freshness`'s reason on the other fact: a
    /// server has received no gestures, so its answer is `init` and nothing else.
    pub reads_gestures: bool,
    /// Where the component is declared, for a diagnostic.
    pub span: Span,
}

impl Decision {
    /// The decision for one component: what it declared, and what follows.
    ///
    /// Takes the roles rather than the [`crate::split::Placed`] they end up in because this is what decides a
    /// field of that struct — and takes the declaration as an argument so that
    /// `beck explain render`, `beck build` and the checker cannot disagree about where it came
    /// from.
    pub fn of(
        roles: &crate::split::Roles,
        defs: &BTreeMap<Arc<str>, crate::check::Def>,
        is_application: bool,
        declared: Option<(Mode, Span)>,
        span: Span,
    ) -> Decision {
        let mode = declared.map_or(Mode::Server, |(m, _)| m);
        let application = is_application;
        let (optimistic, no_optimism) = match (mode, application) {
            (Mode::Server, _) => (false, Some(NoOptimism::ModeA)),
            (Mode::Client, false) => (false, Some(NoOptimism::NotAnApplication)),
            (Mode::Client, true) => (true, None),
        };
        Decision {
            component: roles.page_name.clone(),
            mode,
            why: if declared.is_some() {
                Why::Declared
            } else {
                Why::Default
            },
            carries: match mode {
                Mode::Server => Ty::html(),
                Mode::Client => roles.state_ty.clone(),
            },
            optimistic,
            no_optimism,
            per_session: roles.view_is_per_session,
            uses: SessionUse::of(&roles.view, defs),
            reads_presence: roles.view_reads_presence,
            reads_awareness: roles.awareness.is_some(),
            reads_freshness: roles.view_reads_freshness,
            reads_gestures: roles.gestures.is_some(),
            // A declared mode is refused where it was written; a defaulted one, at the component.
            span: declared.map_or(span, |(_, s)| s),
        }
    }

    /// What `beck explain render` prints: the decision, what it puts on the wire, and what would
    /// change it.
    ///
    /// The counterfactual is the useful half. A reader who wants Mode B and has Mode A needs to
    /// know whether one annotation would do it or whether the program's shape refuses — and that
    /// is a question only the compiler can answer, because the answer is `view_is_per_session`.
    pub fn explain(&self, bundle: &crate::bundle::Bundle) -> String {
        let mut out = String::new();
        let line = |out: &mut String, k: &str, v: String| {
            out.push_str(&format!("{k:<18}{v}\n"));
        };
        line(&mut out, "component", self.component.to_string());
        line(
            &mut out,
            "mode",
            format!(
                "{} — the {} renders ({})",
                self.mode.letter(),
                match self.mode {
                    Mode::Server => "server",
                    Mode::Client => "browser",
                },
                match self.why {
                    Why::Declared => format!("declared: `@render({})`", self.mode.name()),
                    Why::Default => "the default".to_string(),
                }
            ),
        );
        line(
            &mut out,
            "the wire carries",
            match self.mode {
                Mode::Server => "Html, as DOM patches".to_string(),
                Mode::Client => format!("{}, as data patches", self.carries),
            },
        );
        line(
            &mut out,
            "optimistic",
            match self.no_optimism {
                None => "yes — the client holds the accumulator the fold is of, so it runs the \
                         same fold locally and reconciles by `seq`"
                    .to_string(),
                Some(NoOptimism::ModeA) => {
                    "no — every interaction is a round trip, because the page is rendered where \
                     the state is"
                        .to_string()
                }
                Some(NoOptimism::NotAnApplication) => {
                    "no — this module has no chokepoint, so there is nothing to propose".to_string()
                }
            },
        );
        if self.mode == Mode::Client {
            line(
                &mut out,
                "bundle",
                format!(
                    "{} Core nodes, {} definitions, {} bytes",
                    bundle.nodes(),
                    bundle.defs.len(),
                    bundle.to_bytes().len()
                ),
            );
        }
        line(&mut out, "reads of session", self.uses.describe());
        // §3.7's freshness dimension, when the program asked for it. Printed beside optimism
        // because it is the same fact seen from the page: optimism is what makes a guess, and this
        // is the page being able to say so.
        if self.reads_gestures {
            line(
                &mut out,
                "interface state",
                "kept — this page folds its own gestures into a client-local accumulator that \
                 never reaches the log (`docs/10` D30)"
                    .to_string(),
            );
        }
        if self.reads_freshness {
            line(
                &mut out,
                "freshness",
                "read — this page renders `Pending(n)` while its own commands are in flight, and \
                 `Confirmed` otherwise"
                    .to_string(),
            );
        }
        out.push('\n');
        // The counterfactual is the useful half, so the *reason* a page cannot move has to be the
        // one that applies. A page reading the roster is refused whatever it does with the session.
        if self.mode == Mode::Server && self.reads_presence {
            out.push_str(
                "This page reads `presence`, so it cannot move to the browser: `@render(client)` \
                 would be refused (B0516). Who is connected is in neither the accumulator nor the \
                 log — it is a fact the server holds about its own sockets.\n",
            );
            return out;
        }
        if self.mode == Mode::Server && self.reads_awareness {
            out.push_str(
                "This page reads `awareness`, so it cannot move to the browser: `@render(client)` \
                 would be refused (B0521). What every other connection is contributing is in \
                 neither the accumulator nor the log.\n",
            );
            return out;
        }
        match (self.mode, &self.uses) {
            (Mode::Server, SessionUse::None) => out.push_str(
                "This page is a function of the state alone, so `@render(client)` would move it \
                 to the browser.\n",
            ),
            (Mode::Server, SessionUse::Route) => out.push_str(
                &format!(
                    "This page is a function of the state and of `session.{ROUTE_FIELD}`, which the \
                     browser chose. `@render(client)` would move it to the browser, where the route \
                     changes without a round trip.\n"
                ),
            ),
            (Mode::Server, SessionUse::Identity { .. }) => out.push_str(
                "This page reads who is asking, so it cannot move to the browser: \
                 `@render(client)` would be refused (B0514). Mode B sends the state rather than \
                 the page, and a page that filters by identity is a page whose state is not the \
                 client's to hold.\n",
            ),
            (Mode::Client, _) => out.push_str(
                "The browser holds the accumulator and renders from it, so an interaction costs \
                 no round trip. What it costs instead is the bundle above, once.\n",
            ),
        }
        out
    }

    /// Refuse a component that may not render where it says it does.
    ///
    /// Three conditions, and they do not all point the same way. Two are things a page can read
    /// that a browser handed the accumulator would not have: **who** is asking ([`SessionUse`])
    /// and **who is connected** (`presence`). Where the browser *is* is not one of them — it chose
    /// the route. The third is the mirror: **whether a guess is outstanding** (`freshness`) is
    /// something only a browser can have, so a page reading it may not render on the *server*.
    ///
    /// What is deliberately not a third condition is worth writing down. Mode B puts the
    /// **accumulator** on the wire, so the obvious check is §3.5's `Sendable` — and it is
    /// already discharged: a durable fold's state must be *storable* (`B0411`), storable is
    /// strictly stronger than sendable ([`crate::secure`]), and the accumulator is what crosses.
    /// A `secret[T]` therefore cannot reach a Mode B client because it cannot reach the log, and
    /// a check here would be a second gate on a door that is shut. `mode_b.rs` asserts that
    /// composition rather than trusting it.
    pub fn refuse(&self, diags: &mut Diagnostics) {
        if self.mode == Mode::Server {
            // The one refusal that points the other way. Every rule above asks whether the browser
            // may be given something; this asks whether the *server* can answer something, and the
            // answer is no: a server renders what it has recorded, so `freshness()` there is
            // `Confirmed` at every seq of every log. A page that renders "saving…" from it would
            // have written a branch nothing can take.
            if self.reads_freshness {
                diags.push(
                    Diagnostic::error(
                        "B0518",
                        format!(
                            "`{}` reads `freshness`, so it cannot render on the server",
                            self.component
                        ),
                        self.span,
                    )
                    .with_primary_label("a server has nothing in flight")
                    .with_note(
                        "freshness is a client's account of the commands it has proposed and not \
                         yet had confirmed. The server holds the log: what it renders is confirmed \
                         by definition, so this page would render `Confirmed` at every position of \
                         every log and its other branch would be unreachable.",
                    )
                    .with_fix(
                        "render this component in the browser — `@render(client)`, which is what \
                         makes a guess possible in the first place — or take `freshness` out of \
                         its page",
                    ),
                );
            }
            // The same refusal about the other thing only a client holds. D30's five homes put
            // this one fourth on purpose: a page that reaches for interface state and cannot
            // render in the browser is usually a page whose state belongs in the platform or the
            // URL, and those are free.
            if self.reads_gestures {
                diags.push(
                    Diagnostic::error(
                        "B0522",
                        format!(
                            "`{}` reads a `gestures` fold, so it cannot render on the server",
                            self.component
                        ),
                        self.span,
                    )
                    .with_primary_label("a server has received no gestures")
                    .with_note(
                        "a gesture is one client's movement of its own interface: it is not \
                         proposed, not validated and not recorded, so it never reaches a server. A \
                         page rendered there would render the interface state's initial value at \
                         every position of every log, and every branch that depended on a gesture \
                         would be unreachable.",
                    )
                    .with_fix(
                        "render this component in the browser — `@render(client)` — or give the \
                         state one of the homes that survives a server render: markup the platform \
                         already knows (`<dialog>`, `popover`, `<details name>`), or the route on \
                         the `Session` (`docs/10` D30)",
                    ),
                );
            }
            return;
        }
        if self.uses.reads_identity() {
            diags.push(
                Diagnostic::error(
                    "B0514",
                    format!(
                        "`{}` renders differently for each *actor*, so it cannot render on the client",
                        self.component
                    ),
                    self.span,
                )
                .with_primary_label("`@render(client)` sends the browser the state, not the page")
                .with_note(format!(
                    "This page reads {}: it filters, scopes or hides by identity. A client that \
                     rendered it locally would first have to be given the state it filters — \
                     including everything the filter removes. Reading `session.{ROUTE_FIELD}` is \
                     allowed and is not this: the browser chose the route and already holds the \
                     state.",
                    self.uses.describe()
                ))
                .with_fix(
                    "render this component on the server (the default), or make the page a \
                     function of the state and the route alone",
                ),
            );
        }
        if self.reads_presence {
            diags.push(
                Diagnostic::error(
                    "B0516",
                    format!(
                        "`{}` reads `presence`, so it cannot render on the client",
                        self.component
                    ),
                    self.span,
                )
                .with_primary_label("`@render(client)` sends the browser the accumulator")
                .with_note(
                    "Who is connected is not in the accumulator and is not in the log: it is a \
                     fact the server holds about its own sockets. A browser handed the state \
                     would have nothing to render this part of the page from, and shipping the \
                     roster alongside would be a second wire nothing reconciles by `seq`.",
                )
                .with_fix(
                    "render this component on the server (the default), or take `presence` out of \
                     its page",
                ),
            );
        }
        if self.reads_awareness {
            diags.push(
                Diagnostic::error(
                    "B0521",
                    format!(
                        "`{}` reads `awareness`, so it cannot render on the client",
                        self.component
                    ),
                    self.span,
                )
                .with_primary_label("`@render(client)` sends the browser the accumulator")
                .with_note(
                    "What every other connection is contributing is not in the accumulator and \
                     is not in the log: like `presence`, it is a fact the server holds about its \
                     own sockets, and unlike `presence` it carries a value each of those sockets \
                     chose. A browser handed the state would have nothing to render this part of \
                     the page from.",
                )
                .with_fix(
                    "render this component on the server (the default), or take `awareness` out \
                     of its page",
                ),
            );
        }
    }
}
