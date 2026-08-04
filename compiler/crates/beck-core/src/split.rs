//! Stage 8 — signal-graph slicing, and the boundaries it synthesises.
//!
//! [`docs/04-compiler-architecture.md`](../../../../../docs/04-compiler-architecture.md) §4.3:
//! "**Slice the signal graph.** Every signal edge that crosses tiers becomes a subscription: the
//! server side gets a diff operator (DOM patches for Mode-A components), the client side a
//! resumable `(subscription, seq)` consumer; `send` becomes the upstream command channel into the
//! ingress. There is no cache-invalidation wiring to synthesise — views are downstream of the log
//! by construction."
//!
//! # What slicing means concretely
//!
//! The program declares a *graph*, not a pipeline:
//!
//! ```text
//! proposals = merge_clients()                        ! ingress   @on(server)
//! events    = decide(proposals, todos, validate)                 @on(server)
//! todos     = durable(fold(apply_event, empty, events))          @on(data)
//! remaining = signal_map(todos, count_remaining)                 (unplaced)
//! page      = per_session(todos, view)                           @on(client)
//! ```
//!
//! [`crate::signal`] builds that graph. This module slices it: for each role the runtime drives it
//! produces a `Core` *function*, because the roadmap says Phase 1's views are "full recompute per
//! event — semantically final, later made incremental".
//!
//! # What changed when the general slicer arrived
//!
//! Phase 1 and Phase 2 recognised **one topology** and refused every other by name — legitimate
//! narrowness, named as debt by `docs/19-phase-1-report.md` §19.9 and again by
//! `docs/20-phase-2-report.md` §20.5. Three things are different now, and each is a property of
//! working from a graph rather than from a pattern:
//!
//! 1. **Any number of durable folds.** They are *fused* into one accumulator — a synthetic record
//!    with a field per fold — because §3.7 fixes one totally-ordered log per application, and two
//!    `durable` folds are two projections of one log rather than two logs. Under the old splitter
//!    a second fold was not refused: it was accepted and sliced with both folds reading the *first*
//!    accumulator, which is the one outcome the narrowness was supposed to prevent.
//! 2. **Any depth and any sharing above the fold.** A signal read by two consumers is computed
//!    once, as a `let` in the sliced function, instead of being inlined per use. That is what §5.3
//!    means by sharing an arrangement, expressed at the only place a Phase-3 view engine could
//!    read it: the plan.
//! 3. **Every tier crossing is enumerated**, with the content-derived id a resumable subscription
//!    is keyed by, instead of one hard-coded sentence about the single crossing the old shape had.
//!
//! The refusals that remain are refusals about *meaning* — a cycle with no fold in it, a stream
//! where a value is required, two pages and no router — and each says which.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::check::Program;
use crate::core::{Core, CoreKind, Prim, VarId};
use crate::signal::{signal_elem, Cut, Graph, Op, SigId, FUSED_STATE};
use crate::ty::{Tier, Ty};

/// A program with its signal graph sliced into the roles the runtime drives.
#[derive(Clone, Debug)]
pub struct Placed {
    pub program: Program,
    pub roles: Roles,
    /// A content-derived id for the command channel, per §4.3: "a stable, content-derived
    /// operation id (`sha256(module, name, signature)[..16]`) — *not* a URL a human maintains, and
    /// stable across refactors that don't change the signature."
    pub wire_id: String,
    /// How stage 7 placed the program, kept so that `beck explain place` prints the derivation
    /// rather than re-deriving it from a second, drifting copy.
    pub placement: crate::place::Solution,
    /// The signal graph itself, kept so that `beck explain flow`, the incremental analysis and any
    /// later view engine read what the slicer read rather than re-deriving it.
    pub graph: Graph,
    /// Whether this is an application or a **library** — a module with no merge point, whose
    /// [`Placed::roles`] are placeholders rather than a slice of anything.
    ///
    /// A library used to have no `Placed` at all, which meant it had no way to run its own tests
    /// (`docs/22` §22.6, `docs/25` §25.6 item 1): every SICP exercise is a library, and so is
    /// every domain module a real project would most want unit tests for. It has one now — but a
    /// placeholder role is a lie a caller must not be able to tell by accident, so the flag is on
    /// the struct and [`Placed::is_application`] is what the paths that drive a *program* ask.
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A merge point, a durable fold and a page: something [`crate::backend`] can drive.
    Application,
    /// Definitions and types, and no application parts. `beck check` has always said "a library";
    /// this is that answer as a value rather than as a sentence.
    Library,
}

impl Placed {
    pub fn is_application(&self) -> bool {
        self.kind == Kind::Application
    }

    /// A module with no merge point, wrapped so that the parts of the toolchain that do not need
    /// one can still reach it.
    ///
    /// The four roles are placeholders and they are chosen to be *inert* rather than plausible: the
    /// fold returns its accumulator unchanged, `validate` refuses everything, the view renders
    /// nothing and the initial state is unit. Nothing should ever call them — [`Kind::Library`] is
    /// the flag that says so, and `beck test` refuses a `given`, a `when` or a page expectation
    /// against a library by name rather than running one of these and reporting a confusing pass.
    pub fn library(program: Program, graph: Graph, wire_id: String) -> Placed {
        let span = beck_diag::Span::NONE;
        let unit = || Core::new(CoreKind::Const(crate::core::Const::Unit), Ty::unit(), span);
        let lam = |n: usize, body: Core| {
            Core::new(
                CoreKind::Lam {
                    params: (0..n as VarId).collect(),
                    body: Box::new(body),
                },
                Ty::fun((0..n).map(|_| Ty::unit()).collect(), Ty::unit()),
                span,
            )
        };
        Placed {
            roles: Roles {
                validate: lam(2, unit()),
                fold: lam(2, Core::new(CoreKind::Var(0), Ty::unit(), span)),
                init: unit(),
                view: lam(2, unit()),
                state_ty: Ty::unit(),
                event_ty: Ty::unit(),
                command_ty: Ty::unit(),
                proposals_name: Arc::from(""),
                events_name: Arc::from(""),
                state_name: Arc::from(""),
                page_name: Arc::from(""),
                inlined: Vec::new(),
                shared: Vec::new(),
                states: Vec::new(),
                view_is_per_session: false,
            },
            program,
            wire_id,
            placement: crate::place::Solution {
                tiers: Default::default(),
                explanations: Vec::new(),
                method: crate::place::Method::Exhaustive,
                total: 0,
                churn: Vec::new(),
                ties: Vec::new(),
            },
            graph,
            kind: Kind::Library,
        }
    }
}

/// One durable accumulator the program declared.
#[derive(Clone, Debug)]
pub struct StateRole {
    pub name: Arc<str>,
    pub ty: Ty,
    /// The field this fold occupies in the fused accumulator, when there is more than one fold.
    /// `None` when the program has a single fold and its own type *is* the accumulator.
    pub field: Option<Arc<str>>,
    pub node: SigId,
}

/// The five things the runtime needs, each a `Core` value it can call.
///
/// This is deliberately still five: a runtime that drives one log, one accumulator and one page is
/// what Phase 1 built and what Phase 3 has not replaced. What changed is that these are now
/// *derived from the graph* — fusing several folds, inlining or sharing intermediate signals — so
/// the shape of the program and the shape of the runtime are no longer required to be the same.
#[derive(Clone, Debug)]
pub struct Roles {
    /// `(state, proposal) -> Result[list[Event], Rejection]` — the authority chokepoint.
    pub validate: Core,
    /// `(state, Envelope[Event]) -> state` — the replay-pure fold.
    pub fold: Core,
    /// The fold's initial accumulator.
    pub init: Core,
    /// `(state, session) -> Html` — the client-placed view, with intermediate signals inlined or
    /// shared.
    pub view: Core,
    pub state_ty: Ty,
    pub event_ty: Ty,
    pub command_ty: Ty,
    /// Names, for `beck explain` and for the report.
    pub proposals_name: Arc<str>,
    pub events_name: Arc<str>,
    pub state_name: Arc<str>,
    pub page_name: Arc<str>,
    /// Signals that were inlined into the view rather than surviving as their own node.
    pub inlined: Vec<Arc<str>>,
    /// Signals read by more than one consumer, and therefore bound once in the sliced view rather
    /// than recomputed per use. §5.3's shared prefix, at compile time.
    pub shared: Vec<Arc<str>>,
    /// The durable folds, in declaration order. One entry for the ordinary program; several when
    /// the accumulator is fused.
    pub states: Vec<StateRole>,
    pub view_is_per_session: bool,
}

impl Roles {
    /// Whether the accumulator is a synthetic record over several folds.
    pub fn is_fused(&self) -> bool {
        self.states.len() > 1
    }
}

/// Slice a checked, placement-verified program.
pub fn split(mut program: Program, diags: &mut Diagnostics) -> Option<Placed> {
    let graph = Graph::build(&program, diags)?;

    // ---- the three roles the graph has to contain, found by op rather than by position ----

    let ingress = graph.ingress();
    let Some(&proposals) = ingress.first() else {
        diags.push(
            Diagnostic::error("B0500", "this program has no merge point", Span::NONE)
                .with_note(
                    "a Beck application is a fold over an event stream, and the stream starts at \
                     `merge_clients()` — the one place time enters",
                )
                .with_fix("add `@on(server)` and `proposals: Stream[Proposal] = merge_clients()`"),
        );
        return None;
    };

    let states = graph.states();
    if states.is_empty() {
        diags.push(
            Diagnostic::error("B0501", "this program has no durable state", Span::NONE)
                .with_note("`durable(fold(f, init, s))` is what makes the log a database")
                .with_fix("wrap the fold: `@on(data)` and `durable(fold(apply_event, …, events))`"),
        );
        return None;
    }

    // Every `durable` must wrap a fold: only a fold has an accumulator to persist.
    let mut folds: Vec<(SigId, SigId)> = Vec::new(); // (durable node, fold node)
    for &s in &states {
        let inner = follow_alias(&graph, graph.node(s).inputs[0]);
        if !matches!(graph.node(inner).op, Op::Fold { .. }) {
            diags.push(
                Diagnostic::error("B0502", "`durable` must wrap a `fold`", graph.node(s).span)
                    .with_primary_label("only a fold has an accumulator to persist")
                    .with_label(
                        graph.node(inner).span,
                        format!("this is a `{}`", graph.node(inner).op.name()),
                    ),
            );
            return None;
        }
        folds.push((s, inner));
    }

    // §3.5: "authority is one chokepoint". The graph can hold any number of `decide` nodes; a
    // program may not, and the diagnostic says which sentence in the design that is.
    let decides = graph.decides();
    let Some(&decide) = decides.first() else {
        diags.push(
            Diagnostic::error(
                "B0504",
                "events must come from `decide`",
                graph.node(folds[0].1).span,
            )
            .with_primary_label("this fold has no chokepoint upstream of it")
            .with_note(
                "`decide` is the sole consumer of ingress and the one place a command becomes \
                     an event — §3.5's \"authority is one chokepoint\"",
            ),
        );
        return None;
    };
    if decides.len() > 1 {
        diags.push(
            Diagnostic::error(
                "B0511",
                "a program has one authority chokepoint",
                graph.node(decides[1]).span,
            )
            .with_primary_label("a second `decide`")
            .with_label(graph.node(decide).span, "the first one is here")
            .with_note(
                "§3.5 rests on validation being one place: two of them are two answers to \"may \
                 this actor do this\", and the log would record whichever ran",
            ),
        );
        return None;
    }

    // Each fold's stream, after any `filter_map`, must be the chokepoint's output. Anything else
    // is an event stream the log does not contain.
    let mut fold_filters: Vec<Option<Core>> = Vec::new();
    for &(_, f) in &folds {
        let mut node = follow_alias(&graph, graph.node(f).inputs[0]);
        let mut filter = None;
        if let Op::FilterMap { f: pred } = &graph.node(node).op {
            filter = Some(pred.clone());
            node = follow_alias(&graph, graph.node(node).inputs[0]);
        }
        if node != decide {
            diags.push(
                Diagnostic::error(
                    "B0504",
                    "events must come from `decide`",
                    graph.node(f).span,
                )
                .with_primary_label(format!(
                    "this fold reads `{}`",
                    graph.label(graph.node(f).inputs[0])
                ))
                .with_note(
                    "the log holds what the chokepoint decided, so a fold reads `decide` — \
                         optionally through one `filter_map`, which is how two folds take \
                         different slices of one stream",
                ),
            );
            return None;
        }
        fold_filters.push(filter);
    }

    // ---- the page: a sink, placed on the client, carrying Html ----

    let pages: Vec<SigId> = graph
        .sinks
        .iter()
        .copied()
        .filter(|&s| graph.node(s).tier == Tier::Client && is_html(&graph.node(s).ty))
        .collect();
    let Some(&page) = pages.first() else {
        diags.push(
            Diagnostic::error("B0505", "no signal is placed on the client", Span::NONE)
                .with_note(
                    "`page` is the tier crossing: a `Signal[Html]` the browser subscribes to",
                )
                .with_fix("add `@on(client)` and `page: Signal[Html] = per_session(todos, view)`"),
        );
        return None;
    };
    if pages.len() > 1 {
        diags.push(
            Diagnostic::error(
                "B0510",
                "two signals are the page, and there is no router yet",
                graph.node(pages[1]).span,
            )
            .with_primary_label(format!("`{}`", graph.label(pages[1])))
            .with_label(graph.node(page).span, format!("`{}`", graph.label(page)))
            .with_note(
                "the slicer will slice both; the runtime serves one document per connection, and \
                 choosing between them is routing — a Phase 3 client bullet that is not built",
            )
            .with_fix("combine them in one view, or read one from the other"),
        );
        return None;
    }

    // ---- slicing ----

    let fused = states.len() > 1;
    let mut vars = Vars(max_var(&program));
    let state_var = vars.fresh();
    let session_var = vars.fresh();

    let state_roles: Vec<StateRole> = folds
        .iter()
        .map(|&(d, _)| {
            let n = graph.node(d);
            StateRole {
                name: n.label.clone(),
                ty: signal_elem(&n.ty),
                field: fused.then(|| n.label.clone()),
                node: d,
            }
        })
        .collect();

    let mut slicer = Slicer {
        graph: &graph,
        states: &state_roles,
        state_var,
        session_var,
        bound: BTreeMap::new(),
        lets: Vec::new(),
        inlined: Vec::new(),
        shared: Vec::new(),
        per_session: false,
        vars: &mut vars,
        diags,
    };
    let view_body = slicer.lower_sink(page)?;
    let view_body = slicer.wrap(view_body);
    let inlined = slicer.inlined.clone();
    let shared = slicer.shared.clone();
    let per_session = slicer.per_session;

    let state_ty = if fused {
        Ty::con(FUSED_STATE)
    } else {
        state_roles[0].ty.clone()
    };

    let view = Core {
        kind: CoreKind::Lam {
            params: vec![state_var, session_var],
            body: Box::new(view_body),
        },
        ty: Ty::fun(vec![state_ty.clone(), Ty::con("Session")], Ty::html()),
        tier: Tier::Client,
        span: graph.node(page).span,
        last_use: false,
    };

    // ---- the accumulator, fused when the program declared more than one fold ----

    let (fold, init) = if fused {
        program.types.insert(
            Arc::from(FUSED_STATE),
            crate::signal::fused_state_decl(
                &state_roles
                    .iter()
                    .map(|s| (s.name.clone(), s.ty.clone()))
                    .collect::<Vec<_>>(),
            ),
        );
        fuse(
            &graph,
            &folds,
            &fold_filters,
            &state_roles,
            &state_ty,
            &mut vars,
            graph.node(page).span,
        )
    } else {
        let Op::Fold { step, init } = &graph.node(folds[0].1).op else {
            return None;
        };
        match &fold_filters[0] {
            None => (step.clone(), init.clone()),
            Some(pred) => (
                filtered_step(
                    step,
                    pred,
                    &state_ty,
                    &mut vars,
                    graph.node(folds[0].1).span,
                ),
                init.clone(),
            ),
        }
    };

    // ---- `validate`, which reads whichever accumulator the chokepoint was given ----

    let Op::Decide { validate } = &graph.node(decide).op else {
        return None;
    };
    let validate = if fused {
        let src = follow_alias(&graph, graph.node(decide).inputs[1]);
        let Some(role) = state_roles.iter().find(|s| s.node == src) else {
            diags.push(
                Diagnostic::error(
                    "B0512",
                    "the chokepoint does not read a durable fold",
                    graph.node(decide).span,
                )
                .with_primary_label(format!("it reads `{}`", graph.label(src)))
                .with_note(
                    "`decide` threads the accumulator through validation, so what it reads has to \
                     be one — that is what makes first-writer-wins and ownership decidable (§3.7)",
                ),
            );
            return None;
        };
        let p = vars.fresh();
        let s = vars.fresh();
        let span = graph.node(decide).span;
        Core {
            kind: CoreKind::Lam {
                params: vec![s, p],
                body: Box::new(Core {
                    kind: CoreKind::App {
                        func: Box::new(validate.clone()),
                        args: vec![
                            field(var(s, state_ty.clone(), span), role, span),
                            var(p, Ty::con("Proposal"), span),
                        ],
                    },
                    ty: Ty::unit(),
                    tier: Tier::Server,
                    span,
                    last_use: false,
                }),
            },
            ty: Ty::unit(),
            tier: Tier::Server,
            span,
            last_use: false,
        }
    } else {
        validate.clone()
    };

    let event_ty = signal_elem(&graph.node(decide).ty);
    let command_ty = program
        .types
        .get("Command")
        .map(|_| Ty::con("Command"))
        .unwrap_or_else(Ty::unit);

    // §4.3: "a stable, content-derived operation id … *not* a URL a human maintains, and stable
    // across refactors that don't change the signature".
    //
    // **Content**, not name. Hashing `"Event"` would produce an id that never moves — including
    // when a variant is added, which is precisely the change that breaks every open tab. The three
    // types are hashed *structurally*, through every field of every variant they reach.
    let mut hasher = blake3::Hasher::new();
    hasher.update(program.name.as_bytes());
    for t in [&command_ty, &event_ty, &state_ty] {
        hasher.update(crate::iface::structural(t, &program.types).as_bytes());
        hasher.update(b"\x00");
    }
    let wire_id = hasher.finalize().to_hex()[..16].to_string();

    Some(Placed {
        kind: Kind::Application,
        placement: crate::place::Solution {
            tiers: Default::default(),
            explanations: Vec::new(),
            method: crate::place::Method::Exhaustive,
            total: 0,
            churn: Vec::new(),
            ties: Vec::new(),
        },
        roles: Roles {
            validate,
            fold,
            init,
            view,
            state_ty,
            event_ty,
            command_ty,
            proposals_name: graph.node(proposals).label.clone(),
            events_name: graph.node(decide).label.clone(),
            state_name: state_roles[0].name.clone(),
            page_name: graph.node(page).label.clone(),
            inlined,
            shared,
            states: state_roles,
            view_is_per_session: per_session,
        },
        wire_id,
        program,
        graph,
    })
}

fn is_html(t: &Ty) -> bool {
    signal_elem(t).con_name() == Some(Ty::HTML)
}

/// Step past `mirror: Signal[T] = todos` declarations, which name a vertex without adding one.
fn follow_alias(graph: &Graph, mut id: SigId) -> SigId {
    let mut guard = 0;
    while matches!(graph.node(id).op, Op::Alias) && guard < graph.nodes.len() {
        id = graph.node(id).inputs[0];
        guard += 1;
    }
    id
}

fn var(v: VarId, ty: Ty, span: Span) -> Core {
    Core {
        kind: CoreKind::Var(v),
        ty,
        tier: Tier::Any,
        span,
        last_use: false,
    }
}

/// Reach one fold's accumulator out of the state parameter.
fn field(base: Core, role: &StateRole, span: Span) -> Core {
    match &role.field {
        None => base,
        Some(f) => Core {
            kind: CoreKind::Field {
                base: Box::new(base),
                name: f.clone(),
            },
            ty: role.ty.clone(),
            tier: Tier::Any,
            span,
            last_use: false,
        },
    }
}

/// `f(args…)`, at a given result type.
fn call(func: Core, args: Vec<Core>, ty: Ty, span: Span) -> Core {
    Core {
        kind: CoreKind::App {
            func: Box::new(func),
            args,
        },
        ty,
        tier: Tier::Any,
        span,
        last_use: false,
    }
}

/// One fold's contribution to a step: `step(state.field, env)`, guarded by the fold's
/// `filter_map` if it has one.
fn fold_field(
    step: &Core,
    filter: &Option<Core>,
    acc: Core,
    env: Core,
    ty: &Ty,
    vars: &mut Vars,
    span: Span,
) -> Core {
    let applied = call(
        step.clone(),
        vec![acc.clone(), env.clone()],
        ty.clone(),
        span,
    );
    let Some(pred) = filter else {
        return applied;
    };
    // `filter_map` between the chokepoint and a fold means this fold sees a *slice* of the log.
    // The runtime appends one stream and folds it once, so the filter moves into the step:
    //
    //     let o = pred(env.body) in
    //     if is_some(o) then step(acc, env.with(body = o.value)) else acc
    //
    // Written with the prims the language already has rather than a synthesised `match`, because
    // an `Arm` carries a pattern and this needs no pattern — only the two answers `Option` has.
    let o = vars.fresh();
    let opt_ty = Ty::option(Ty::unit());
    let body = Core {
        kind: CoreKind::Field {
            base: Box::new(env.clone()),
            name: Arc::from("body"),
        },
        ty: Ty::unit(),
        tier: Tier::Any,
        span,
        last_use: false,
    };
    let inner = Core {
        kind: CoreKind::Field {
            base: Box::new(var(o, opt_ty.clone(), span)),
            name: Arc::from("value"),
        },
        ty: Ty::unit(),
        tier: Tier::Any,
        span,
        last_use: false,
    };
    let narrowed = Core {
        kind: CoreKind::With {
            base: Box::new(env),
            fields: vec![(Arc::from("body"), inner)],
        },
        ty: Ty::unit(),
        tier: Tier::Any,
        span,
        last_use: false,
    };
    Core {
        kind: CoreKind::Let {
            var: o,
            value: Box::new(call(pred.clone(), vec![body], opt_ty.clone(), span)),
            body: Box::new(Core {
                kind: CoreKind::If {
                    cond: Box::new(Core {
                        kind: CoreKind::Prim {
                            op: Prim::OptionIsSome,
                            args: vec![var(o, opt_ty, span)],
                        },
                        ty: Ty::bool_(),
                        tier: Tier::Any,
                        span,
                        last_use: false,
                    }),
                    then: Box::new(call(
                        step.clone(),
                        vec![acc.clone(), narrowed],
                        ty.clone(),
                        span,
                    )),
                    alt: Box::new(acc),
                },
                ty: ty.clone(),
                tier: Tier::Any,
                span,
                last_use: false,
            }),
        },
        ty: ty.clone(),
        tier: Tier::Any,
        span,
        last_use: false,
    }
}

/// The single-fold case of [`fold_field`]: a step wrapped in its own `filter_map`.
fn filtered_step(step: &Core, pred: &Core, state_ty: &Ty, vars: &mut Vars, span: Span) -> Core {
    let s = vars.fresh();
    let e = vars.fresh();
    let body = fold_field(
        step,
        &Some(pred.clone()),
        var(s, state_ty.clone(), span),
        var(e, Ty::unit(), span),
        state_ty,
        vars,
        span,
    );
    Core {
        kind: CoreKind::Lam {
            params: vec![s, e],
            body: Box::new(body),
        },
        ty: Ty::fun(vec![state_ty.clone(), Ty::unit()], state_ty.clone()),
        tier: Tier::Data,
        span,
        last_use: false,
    }
}

/// Fuse several durable folds into one accumulator.
///
/// §3.7 fixes one totally-ordered log per application. Two `durable` folds are therefore not two
/// logs; they are two projections of one, and the runtime holds a record with one field per fold.
/// The step applies every fold's own step to its own field, in declaration order, so replay is
/// exactly as deterministic as it was with one.
fn fuse(
    graph: &Graph,
    folds: &[(SigId, SigId)],
    filters: &[Option<Core>],
    roles: &[StateRole],
    state_ty: &Ty,
    vars: &mut Vars,
    span: Span,
) -> (Core, Core) {
    let s = vars.fresh();
    let e = vars.fresh();

    let mut step_fields = Vec::new();
    let mut init_fields = Vec::new();
    for (i, &(_, f)) in folds.iter().enumerate() {
        let Op::Fold { step, init } = &graph.node(f).op else {
            continue;
        };
        let role = &roles[i];
        let acc = field(var(s, state_ty.clone(), span), role, span);
        step_fields.push((
            role.name.clone(),
            fold_field(
                step,
                &filters[i],
                acc,
                var(e, Ty::unit(), span),
                &role.ty,
                vars,
                span,
            ),
        ));
        init_fields.push((role.name.clone(), init.clone()));
    }

    let make = |fields: Vec<(Arc<str>, Core)>| Core {
        kind: CoreKind::Make {
            ty: Arc::from(FUSED_STATE),
            variant: None,
            fields,
        },
        ty: state_ty.clone(),
        tier: Tier::Data,
        span,
        last_use: false,
    };

    (
        Core {
            kind: CoreKind::Lam {
                params: vec![s, e],
                body: Box::new(make(step_fields)),
            },
            ty: Ty::fun(vec![state_ty.clone(), Ty::unit()], state_ty.clone()),
            tier: Tier::Data,
            span,
            last_use: false,
        },
        make(init_fields),
    )
}

/// A source of variables the program does not use.
///
/// The old splitter used variables 0 and 1 for the state and the session, which are also the first
/// two the checker hands out. It got away with it because the sliced body only ever *calls* the
/// program's functions rather than inlining their bodies — but "got away with it" is the whole
/// objection, and a slicer that now emits `let` bindings of its own has no reason to keep it.
struct Vars(VarId);

impl Vars {
    fn fresh(&mut self) -> VarId {
        self.0 += 1;
        self.0
    }
}

/// The largest variable the program uses, so the slicer's own bindings cannot shadow one.
fn max_var(program: &Program) -> VarId {
    fn go(c: &Core, max: &mut VarId) {
        match &c.kind {
            CoreKind::Const(_) | CoreKind::Global(_) => {}
            CoreKind::Var(v) => *max = (*max).max(*v),
            CoreKind::Lam { params, body } => {
                for p in params {
                    *max = (*max).max(*p);
                }
                go(body, max);
            }
            CoreKind::App { func, args } => {
                go(func, max);
                args.iter().for_each(|a| go(a, max));
            }
            CoreKind::Prim { args, .. } => args.iter().for_each(|a| go(a, max)),
            CoreKind::Let { var, value, body } => {
                *max = (*max).max(*var);
                go(value, max);
                go(body, max);
            }
            CoreKind::If { cond, then, alt } => {
                go(cond, max);
                go(then, max);
                go(alt, max);
            }
            CoreKind::Match { scrutinee, arms } => {
                go(scrutinee, max);
                for a in arms {
                    for v in a.pattern.binders() {
                        *max = (*max).max(v);
                    }
                    go(&a.body, max);
                }
            }
            CoreKind::Make { fields, .. } => fields.iter().for_each(|(_, f)| go(f, max)),
            CoreKind::Field { base, .. } => go(base, max),
            CoreKind::With { base, fields } => {
                go(base, max);
                fields.iter().for_each(|(_, f)| go(f, max));
            }
            CoreKind::ListLit(items) => items.iter().for_each(|i| go(i, max)),
            CoreKind::MapLit(pairs) => pairs.iter().for_each(|(k, v)| {
                go(k, max);
                go(v, max);
            }),
        }
    }
    let mut max = 0;
    for d in program.defs.values() {
        go(&d.body, &mut max);
    }
    for s in &program.signals {
        go(&s.expr, &mut max);
    }
    for t in &program.tests {
        max = max
            .max(t.bindings.state)
            .max(t.bindings.events)
            .max(t.bindings.result);
    }
    max
}

/// Rewrites a signal expression into a function of the durable state (and the session).
///
/// This is the slicing itself. `signal_map(s, f)` becomes `f(lower(s))`, `map2(f, a, b)` becomes
/// `f(lower(a), lower(b))`, `per_session(s, f)` becomes `f(lower(s), session)`, and a reference to
/// a durable signal becomes the state parameter — or, when several folds were fused, the field of
/// it that fold occupies.
///
/// What is not a rewrite is the sharing: a vertex read by more than one consumer is bound once, in
/// a `let`, and referred to by name. Under the old splitter it was inlined per use, so a program
/// whose two views both read `summary` recomputed it twice per event and nothing recorded that
/// they were the same computation. §5.3's arrangement sharing needs the opposite, and the plan is
/// the only place a later view engine could learn it.
struct Slicer<'a, 'd> {
    graph: &'a Graph,
    states: &'a [StateRole],
    state_var: VarId,
    session_var: VarId,
    vars: &'d mut Vars,
    /// Vertices already bound in this slice.
    bound: BTreeMap<SigId, VarId>,
    /// The bindings, dependencies first.
    lets: Vec<(VarId, Core)>,
    inlined: Vec<Arc<str>>,
    shared: Vec<Arc<str>>,
    per_session: bool,
    diags: &'d mut Diagnostics,
}

impl Slicer<'_, '_> {
    /// Slice a sink. The sink itself is not "inlined into the view": it *is* the view, and
    /// listing it as one of the signals that disappeared into it would be a report about nothing.
    fn lower_sink(&mut self, id: SigId) -> Option<Core> {
        let body = self.lower(id)?;
        if let Some(name) = &self.graph.node(follow_alias(self.graph, id)).name {
            self.inlined.retain(|n| n != name);
        }
        Some(body)
    }

    fn lower(&mut self, id: SigId) -> Option<Core> {
        let id = follow_alias(self.graph, id);
        let node = self.graph.node(id);

        // A durable accumulator is where slicing stops: it is a parameter, not a computation. This
        // is also why a cycle through a fold terminates and one without a fold cannot.
        if let Some(role) = self.states.iter().find(|s| s.node == id) {
            return Some(field(
                var(self.state_var, Ty::unit(), node.span),
                role,
                node.span,
            ));
        }
        if let Some(&v) = self.bound.get(&id) {
            return Some(var(v, signal_elem(&node.ty), node.span));
        }

        let span = node.span;
        let ty = signal_elem(&node.ty);
        let body = match &node.op {
            Op::Map { f } => {
                let input = self.lower(node.inputs[0])?;
                call(f.clone(), vec![input], ty.clone(), span)
            }
            Op::Map2 { f } => {
                let a = self.lower(node.inputs[0])?;
                let b = self.lower(node.inputs[1])?;
                call(f.clone(), vec![a, b], ty.clone(), span)
            }
            Op::PerSession { f } => {
                self.per_session = true;
                let input = self.lower(node.inputs[0])?;
                let session = var(self.session_var, Ty::con("Session"), span);
                call(f.clone(), vec![input, session], ty.clone(), span)
            }
            Op::Fold { .. } => {
                // A fold the program did not mark `durable`. The value exists in the semantics —
                // a transient accumulator — and there is nowhere to keep it: the runtime persists
                // the log and snapshots what `durable` names, and nothing else.
                self.diags.push(
                    Diagnostic::error(
                        "B0513",
                        format!("`{}` is a fold that is not durable", self.graph.label(id)),
                        span,
                    )
                    .with_primary_label("its accumulator has nowhere to live across a restart")
                    .with_note(
                        "the log is what survives, and `durable` is what says an accumulator is \
                         folded from it — a fold outside one would be rebuilt from nothing on \
                         every deploy",
                    )
                    .with_fix("wrap it: `durable(fold(…))`"),
                );
                return None;
            }
            Op::Ingress | Op::Decide { .. } | Op::FilterMap { .. } => {
                self.diags.push(
                    Diagnostic::error(
                        "B0507",
                        format!(
                            "a view cannot read `{}`, which is a stream",
                            self.graph.label(id)
                        ),
                        span,
                    )
                    .with_primary_label(format!("`{}` produces occurrences", node.op.name()))
                    .with_note(
                        "§3.7: a `Stream` is discrete occurrences and a `Signal` is a value \
                         defined at all times. A view renders a value, so it reads what a stream \
                         was folded into",
                    ),
                );
                return None;
            }
            Op::Durable | Op::Alias => unreachable!("handled above"),
        };

        if let Some(name) = &node.name {
            if !self.inlined.contains(name) {
                self.inlined.push(name.clone());
            }
        }

        // Shared: read by more than one consumer, so computing it once is the whole difference
        // between a plan and an expansion.
        if self.graph.consumers(id).len() > 1 {
            let v = self.vars.fresh();
            self.bound.insert(id, v);
            self.lets.push((v, body));
            if let Some(name) = &node.name {
                self.shared.push(name.clone());
            }
            return Some(var(v, ty, span));
        }
        Some(body)
    }

    /// Wrap the sliced expression in the bindings it accumulated, dependencies outermost.
    fn wrap(&self, body: Core) -> Core {
        self.lets.iter().rev().fold(body, |acc, (v, value)| Core {
            kind: CoreKind::Let {
                var: *v,
                value: Box::new(value.clone()),
                body: Box::new(acc.clone()),
            },
            ty: acc.ty.clone(),
            tier: Tier::Client,
            span: acc.span,
            last_use: false,
        })
    }
}

/// What `beck explain flow` prints: the graph as a graph, rather than the four names the one
/// recognised topology had.
///
/// §4.7 asks `beck explain` to answer "why is this here" from the compiler's own data. The old
/// version printed a fixed four-line summary and one hard-coded sentence claiming there was
/// exactly one tier crossing — true of the todo sketch and of nothing the general slicer now
/// accepts. This prints what the slicer read.
pub fn flow_report(placed: &Placed) -> String {
    use std::fmt::Write;
    let g = &placed.graph;
    let r = &placed.roles;
    let mut out = String::new();
    let cycles = g.dep.cycles().count();
    let _ = writeln!(
        out,
        "signal graph — {} vertices, {} {}, {} tier {}\n",
        g.nodes.len(),
        cycles,
        if cycles == 1 { "cycle" } else { "cycles" },
        g.cuts.len(),
        if g.cuts.len() == 1 {
            "crossing"
        } else {
            "crossings"
        },
    );

    let in_cycle: BTreeSet<SigId> = g
        .dep
        .cycles()
        .flat_map(|c| c.iter().map(|n| n.0 as usize))
        .collect();
    let page = g.by_name.get(&r.page_name).copied();
    let rows: Vec<(SigId, String, String)> = g
        .order()
        .into_iter()
        .map(|id| {
            let n = g.node(id);
            let inputs: Vec<&str> = n.inputs.iter().map(|&i| g.label(i)).collect();
            (
                id,
                n.label.to_string(),
                format!("{}({})", n.op.name(), inputs.join(", ")),
            )
        })
        .collect();
    let lw = rows.iter().map(|r| r.1.chars().count()).max().unwrap_or(0);
    let ew = rows.iter().map(|r| r.2.chars().count()).max().unwrap_or(0);
    for (id, label, expr) in &rows {
        let mut note = String::new();
        if in_cycle.contains(id) {
            note.push_str("  ↺");
        }
        if Some(*id) == page {
            note.push_str(if r.view_is_per_session {
                "  ← the page, per session"
            } else {
                "  ← the page, broadcast"
            });
        } else if g.sinks.contains(id) {
            note.push_str("  ← a sink nothing reads");
        }
        let _ = writeln!(
            out,
            "  {label:<lw$}  {expr:<ew$}  {:<7}{note}",
            g.node(*id).tier.name(),
        );
    }

    let _ = writeln!(out, "\naccumulator");
    if r.is_fused() {
        let _ = writeln!(
            out,
            "  {} durable folds, fused into one record — §3.7 fixes one totally-ordered log per\n  \
             application, so two folds are two projections of it rather than two logs.",
            r.states.len()
        );
        for s in &r.states {
            let _ = writeln!(out, "    {FUSED_STATE}.{} : {}", s.name, s.ty);
        }
    } else {
        let _ = writeln!(
            out,
            "  one durable fold — `{}` : {}",
            r.states[0].name, r.states[0].ty
        );
    }

    let plan = slice_of(g, page.unwrap_or(0));
    let computed: Vec<&str> = plan
        .iter()
        .copied()
        .filter(|&i| {
            !matches!(g.node(i).op, Op::Durable | Op::Fold { .. })
                && !g.node(i).op.is_stream()
                && Some(i) != page
        })
        .map(|i| g.label(i))
        .collect();
    let _ = writeln!(out, "\nthe view recomputes, per event");
    let _ = writeln!(
        out,
        "  {}",
        if computed.is_empty() {
            "nothing between the accumulator and the page".to_string()
        } else {
            computed.join(", ")
        }
    );
    let _ = writeln!(
        out,
        "  shared: {}",
        if r.shared.is_empty() {
            "—  (no signal is read by two consumers, so nothing is bound twice)".to_string()
        } else {
            format!(
                "{}  (read by more than one consumer, so computed once)",
                r.shared
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
    let _ = writeln!(
        out,
        "  (§5.3 makes these incremental; today every one is a full recompute)"
    );

    if !g.cuts.is_empty() {
        let _ = writeln!(
            out,
            "\ntier crossings — each is one subscription, resumable by (id, seq) (§4.3)"
        );
        let edges: Vec<(String, String, String)> = g
            .cuts
            .iter()
            .map(|c| {
                (
                    format!("{} → {}", g.label(c.from), g.label(c.to)),
                    format!(
                        "{} → {}",
                        g.node(c.from).tier.name(),
                        g.node(c.to).tier.name()
                    ),
                    format!("{}", c.carries),
                )
            })
            .collect();
        let nw = edges.iter().map(|e| e.0.chars().count()).max().unwrap_or(0);
        let tw = edges.iter().map(|e| e.1.chars().count()).max().unwrap_or(0);
        let cw = edges.iter().map(|e| e.2.chars().count()).max().unwrap_or(0);
        for (c, (names, tiers, carries)) in g.cuts.iter().zip(&edges) {
            let _ = writeln!(
                out,
                "  {names:<nw$}  {tiers:<tw$}  carries {carries:<cw$}  {}",
                c.id
            );
        }
    }
    out
}

/// Every tier crossing, for `beck explain flow` and for the report.
pub fn crossings(placed: &Placed) -> &[Cut] {
    &placed.graph.cuts
}

/// Every vertex reachable from a sink, in dependency order — the sub-plan one role executes.
pub fn slice_of(graph: &Graph, sink: SigId) -> Vec<SigId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![sink];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for &i in &graph.node(id).inputs {
            stack.push(i);
        }
    }
    graph
        .order()
        .into_iter()
        .filter(|i| seen.contains(i))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::compile_str;

    /// The sketch's program shape, in the Python surface.
    pub const TODO: &str = r#"
type Id = newtype[Str]

model Todo:
    id: Id
    text: Str
    done: Bool
    owner: Str

model State:
    todos: Map[Id, Todo]

union Command:
    Add(id: Id, text: Str)
    Toggle(id: Id)
    Delete(id: Id)

union Event:
    Added(id: Id, text: Str)
    Toggled(id: Id)
    Deleted(id: Id)

union Rejection:
    BlankText
    IdTaken
    NoSuchTodo
    NotOwner

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Added(id, text):
            return s.with(todos=map_insert(s.todos, id, Todo(id=id, text=text, done=False, owner=env.actor)))
        case Toggled(id):
            return toggle(s, id)
        case Deleted(id):
            return s.with(todos=map_remove(s.todos, id))

def toggle(s: State, id: Id) -> State:
    match map_get(s.todos, id):
        case Some(value):
            return s.with(todos=map_insert(s.todos, id, value.with(done=not value.done)))
        case None:
            return s

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Add(id, text):
            if str_is_empty(str_trim(text)):
                return Err(error=BlankText)
            if map_contains(s.todos, id):
                return Err(error=IdTaken)
            return Ok(value=[Added(id=id, text=text)])
        case Toggle(id):
            return owned(s, p, id, [Toggled(id=id)])
        case Delete(id):
            return owned(s, p, id, [Deleted(id=id)])

def owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection]:
    match map_get(s.todos, id):
        case Some(value):
            if value.owner != p.session.actor:
                return Err(error=NotOwner)
            return Ok(value=evs)
        case None:
            return Err(error=NoSuchTodo)

def mine(s: State, session: Session) -> list[Todo]:
    return sort_by(filter_list(map_values(s.todos), lambda t: t.owner == session.actor), lambda t: t.text)

def remaining_of(todos: list[Todo]) -> Int:
    return list_len(filter_list(todos, lambda t: not t.done))

def view(s: State, session: Session) -> Html:
    todos = mine(s, session)
    return render(todos, remaining_of(todos))

def render(todos: list[Todo], remaining: Int) -> Html:
    return ui:
        main:
            h1: "todos"
            ul:
                for t in todos:
                    li(key=t.id, class=done_class(t)):
                        span(on_click=Toggle(id=t.id)): t.text
            footer: (str(remaining) + " remaining")

def done_class(t: Todo) -> Str:
    return "done" if t.done else ""

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, todos, validate)

@on(data)
todos: Signal[State] = durable(fold(apply_event, State(todos={}), events))

@on(client)
page: Signal[Html] = per_session(todos, view)
"#;

    #[test]
    fn the_sketch_compiles_and_slices_into_roles() {
        let (placed, d, map) = compile_str("todo.beck", TODO);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let placed = placed.expect("splitting succeeds");
        assert_eq!(placed.roles.state_name.as_ref(), "todos");
        assert_eq!(placed.roles.events_name.as_ref(), "events");
        assert_eq!(placed.roles.page_name.as_ref(), "page");
        assert!(placed.roles.view_is_per_session);
        assert_eq!(placed.roles.event_ty.con_name(), Some("Event"));
        assert_eq!(placed.roles.command_ty.con_name(), Some("Command"));
        assert_eq!(placed.wire_id.len(), 16);
        // One fold, so the accumulator is the program's own type and nothing is fused: every
        // claim any earlier phase made about this program is unchanged by the general slicer.
        assert!(!placed.roles.is_fused());
        assert_eq!(placed.roles.states.len(), 1);
        assert_eq!(placed.roles.state_ty.con_name(), Some("State"));
    }

    #[test]
    fn the_graph_holds_the_fold_as_its_own_vertex() {
        // `durable(fold(…))` is two operations and therefore two vertices, even though the program
        // named only one of them. That is the difference between a graph and a pattern: nothing
        // downstream has to know that `durable` "means" `durable-of-a-fold`.
        let (placed, _, _) = compile_str("todo.beck", TODO);
        let g = &placed.expect("placed").graph;
        assert_eq!(g.states().len(), 1);
        let durable = g.states()[0];
        let inner = g.node(durable).inputs[0];
        assert!(matches!(g.node(inner).op, Op::Fold { .. }));
        assert_eq!(g.label(inner), "todos·fold");
        assert_eq!(
            g.node(inner).name,
            None,
            "an inner vertex has no written name"
        );
    }

    #[test]
    fn the_only_cycle_is_the_one_the_design_says_is_sound() {
        // §3.7: "`events` is decided from the state, and the state is folded from `events`. The
        // cycle is real and it is sound."
        let (placed, _, _) = compile_str("todo.beck", TODO);
        let g = &placed.expect("placed").graph;
        let cycles: Vec<Vec<String>> = g
            .dep
            .cycles()
            .map(|c| {
                c.iter()
                    .map(|n| g.label(n.0 as usize).to_string())
                    .collect()
            })
            .collect();
        assert_eq!(cycles.len(), 1, "{cycles:?}");
        assert!(cycles[0].iter().any(|n| n == "events"));
        assert!(cycles[0].iter().any(|n| n == "todos"));
        assert!(cycles[0].iter().any(|n| n == "todos·fold"));
    }

    #[test]
    fn the_wire_id_is_content_derived_and_stable_under_a_body_edit() {
        let (a, _, _) = compile_str("todo.beck", TODO);
        // Change a body, not a signature: the operation id must not move (§4.3).
        let edited = TODO.replace(
            r#""done" if t.done else """#,
            r#""done" if t.done else " ""#,
        );
        let (b, d, map) = compile_str("todo.beck", &edited);
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert_eq!(
            a.expect("a").wire_id,
            b.expect("b").wire_id,
            "a body edit must not change the wire id"
        );
    }

    #[test]
    fn the_wire_id_moves_when_the_wire_actually_changes() {
        // The other half of the same requirement, and the one a name-hash silently fails: adding a
        // variant to `Event` changes what a subscriber can be sent, so the operation id has to move
        // or a rolling deploy has no way to notice.
        let (a, _, _) = compile_str("todo.beck", TODO);
        let changed = TODO
            .replace(
                "    Toggled(id: Id)\n    Deleted(id: Id)",
                "    Toggled(id: Id)\n    Deleted(id: Id)\n    Starred(id: Id)",
            )
            .replace(
                "        case Deleted(id):\n            return s.with(todos=map_remove(s.todos, id))",
                "        case Deleted(id):\n            return s.with(todos=map_remove(s.todos, id))\n        case Starred(id):\n            return toggle(s, id)",
            );
        let (b, d, map) = compile_str("todo.beck", &changed);
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert_ne!(a.expect("a").wire_id, b.expect("b").wire_id);

        // …and a field added to a command moves it too, which a hash of the type's *name* would
        // not have caught either.
        let widened = TODO
            .replace(
                "    Toggle(id: Id)\n    Delete(id: Id)",
                "    Toggle(id: Id, at: Int)\n    Delete(id: Id)",
            )
            .replace("case Toggle(id):", "case Toggle(id, at):")
            .replace(
                "span(on_click=Toggle(id=t.id)): t.text",
                "span(on_click=Toggle(id=t.id, at=0)): t.text",
            );
        let (c, d, map) = compile_str("todo.beck", &widened);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let (a, _, _) = compile_str("todo.beck", TODO);
        assert_ne!(a.expect("a").wire_id, c.expect("c").wire_id);
    }

    #[test]
    fn a_program_with_no_merge_point_is_told_what_is_missing() {
        let (_, d, _) = compile_str("t.beck", "def f() -> Int:\n    return 1\n");
        assert!(d.iter().any(|x| x.code == "B0500" && x.fix.is_some()));
    }

    #[test]
    fn a_view_that_reads_a_stream_is_refused_by_name() {
        // The narrowness that remains is about *meaning*: a `Stream` is occurrences and a view
        // renders a value. B0507 says which, rather than "unsupported".
        let src = TODO
            .replace(
                "@on(client)\npage: Signal[Html] = per_session(todos, view)",
                "@on(client)\npage: Signal[Html] = signal_map(events, render_ev)",
            )
            .replace(
                "@on(server)\nproposals",
                "def render_ev(e: Event) -> Html:\n    return ui:\n        main: \"x\"\n\n@on(server)\nproposals",
            );
        let (placed, d, _) = compile_str("t.beck", &src);
        assert!(placed.is_none());
        assert!(d.has_errors(), "a refusal must say why");
    }

    #[test]
    fn a_cycle_with_no_fold_in_it_is_refused_rather_than_looped_on() {
        // The rule that makes slicing terminate, stated as a program the compiler must reject.
        let src = TODO.replace(
            "@on(data)\ntodos: Signal[State] = durable(fold(apply_event, State(todos={}), events))",
            "@on(data)\ntodos: Signal[State] = durable(fold(apply_event, State(todos={}), events))\n\
             \nloop_a: Signal[State] = signal_map(loop_b, identity_state)\n\
             \nloop_b: Signal[State] = signal_map(loop_a, identity_state)",
        );
        let src = src.replace(
            "def done_class",
            "def identity_state(s: State) -> State:\n    return s\n\ndef done_class",
        );
        let (placed, d, _) = compile_str("t.beck", &src);
        assert!(placed.is_none(), "a self-defined signal has no first value");
        assert!(
            d.iter().any(|x| x.code == "B0509"),
            "{:?}",
            d.iter().map(|x| x.code).collect::<Vec<_>>()
        );
    }
}
