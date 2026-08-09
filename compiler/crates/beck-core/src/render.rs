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
//! # The rule: a Mode B page may not be a function of who is asking
//!
//! A view has the shape `(state, session) -> Html`. If it reads the session, it renders a
//! different page for different actors *from the same state* — which is to say it is filtering,
//! scoping or hiding by identity. Running that view on the client requires giving the client the
//! state it filters, so every actor receives what the filter was removing. The page would still
//! look right. That is the worst kind of wrong.
//!
//! So a component whose view is per-session is refused Mode B, and the refusal names the reason
//! rather than a rule. What is left is exactly the class §5.1 and
//! [`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D5 describe as Mode B's: pages
//! that are the same function of the same state for everybody — editors, typeaheads, drag-and-drop,
//! anything single-user or public.
//!
//! This is a *placement* rule and not a lint: it is decided from the slicer's own account of the
//! view ([`crate::split::Roles::view_is_per_session`]), which is the same fact §3.8's fanout
//! analysis reads, so a program cannot be per-session for one of them and not the other.
//!
//! # Optimism is a property of what crosses, not of a component
//!
//! "The browser applies the expected event to its local copy speculatively — legitimate because it
//! runs the *same pure fold* the server runs" (D5). That is only available to a client that holds
//! the value the fold is *of*. A client holding a projection — a session's filtered list, say —
//! could not apply an event to it without a second, different fold that no program writes. So
//! optimism is not an extra feature layered on Mode B; it is the same fact stated twice, and this
//! module reports it as one decision with two consequences.

use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::ty::Ty;

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
    /// True when the view reads the session — the fact that decides eligibility.
    pub per_session: bool,
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
        out.push('\n');
        match (self.mode, self.per_session) {
            (Mode::Server, false) => out.push_str(
                "This page is a function of the state alone, so `@render(client)` would move it \
                 to the browser.\n",
            ),
            (Mode::Server, true) => out.push_str(
                "This page reads the session, so it cannot move to the browser: `@render(client)` \
                 would be refused (B0514). Mode B sends the state rather than the page, and a page \
                 that filters by identity is a page whose state is not the client's to hold.\n",
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
    /// One condition, and the reason there is only one is worth writing down. Mode B puts the
    /// **accumulator** on the wire, so the obvious second check is §3.5's `Sendable` — and it is
    /// already discharged: a durable fold's state must be *storable* (`B0411`), storable is
    /// strictly stronger than sendable ([`crate::secure`]), and the accumulator is what crosses.
    /// A `secret[T]` therefore cannot reach a Mode B client because it cannot reach the log, and
    /// a check here would be a second gate on a door that is shut. `mode_b.rs` asserts that
    /// composition rather than trusting it.
    pub fn refuse(&self, diags: &mut Diagnostics) {
        if self.mode != Mode::Client {
            return;
        }
        if self.per_session {
            diags.push(
                Diagnostic::error(
                    "B0514",
                    format!(
                        "`{}` renders differently for each session, so it cannot render on the client",
                        self.component
                    ),
                    self.span,
                )
                .with_primary_label("`@render(client)` sends the browser the state, not the page")
                .with_note(
                    "This page is a function of the session as well as of the state: it filters, \
                     scopes or hides by identity. A client that rendered it locally would first \
                     have to be given the state it filters — including everything the filter \
                     removes.",
                )
                .with_fix(
                    "render this component on the server (the default), or make the page a \
                     function of the state alone",
                ),
            );
        }
    }
}
