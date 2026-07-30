//! Stage 8 — signal-graph slicing, and the boundaries it synthesises.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.3:
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
//! Slicing walks it and produces, for each role, a `Core` *function* — because the roadmap says
//! Phase 1's views are "full recompute per event — semantically final, later made incremental".
//! The client-placed `page` becomes a `(state, session) -> Html` closure with every intermediate
//! signal inlined, so `remaining` costs a recount today and a differential-dataflow operator in
//! Phase 3 without the program changing.
//!
//! The tier crossing itself needs no code here: `page` is `@on(client)` and its input is `@on(data)`,
//! so the edge between them is exactly one subscription, and the runtime already knows how to diff
//! an `Html` and stream patches.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::check::{Program, SignalDecl};
use crate::core::{Core, CoreKind, Prim, VarId};
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
}

/// The five things the runtime needs, each a `Core` value it can call.
#[derive(Clone, Debug)]
pub struct Roles {
    /// `(state, proposal) -> Result[list[Event], Rejection]` — the authority chokepoint.
    pub validate: Core,
    /// `(state, Envelope[Event]) -> state` — the replay-pure fold.
    pub fold: Core,
    /// The fold's initial accumulator.
    pub init: Core,
    /// `(state, session) -> Html` — the client-placed view, with intermediate signals inlined.
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
    pub view_is_per_session: bool,
}

/// Slice a checked, placement-verified program.
pub fn split(program: Program, diags: &mut Diagnostics) -> Option<Placed> {
    let by_name: BTreeMap<Arc<str>, &SignalDecl> = program
        .signals
        .iter()
        .map(|s| (s.name.clone(), s))
        .collect();

    let find = |op: Prim| -> Option<&SignalDecl> {
        program
            .signals
            .iter()
            .find(|s| matches!(&s.expr.kind, CoreKind::Prim { op: o, .. } if *o == op))
    };

    let Some(proposals) = find(Prim::MergeClients) else {
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

    // The durable fold: `durable(fold(f, init, events))`.
    let Some(state_decl) = find(Prim::Durable) else {
        diags.push(
            Diagnostic::error("B0501", "this program has no durable state", Span::NONE)
                .with_note("`durable(fold(f, init, events))` is what makes the log a database")
                .with_fix("wrap the fold: `@on(data)` and `durable(fold(apply_event, …, events))`"),
        );
        return None;
    };
    let CoreKind::Prim { args, .. } = &state_decl.expr.kind else {
        return None;
    };
    let fold_expr = args.first()?;
    let CoreKind::Prim {
        op: Prim::Fold,
        args: fold_args,
    } = &fold_expr.kind
    else {
        diags.push(
            Diagnostic::error(
                "B0502",
                "`durable` must wrap a `fold`",
                state_decl.expr.span,
            )
            .with_primary_label("only a fold has an accumulator to persist"),
        );
        return None;
    };
    if fold_args.len() != 3 {
        return None;
    }
    let fold = fold_args[0].clone();
    let init = fold_args[1].clone();
    let events_ref = &fold_args[2];

    // The stream feeding the fold names the `decide` node.
    let events_name = signal_name(events_ref).unwrap_or_else(|| Arc::from("events"));
    let Some(events_decl) = by_name.get(&events_name) else {
        diags.push(Diagnostic::error(
            "B0503",
            "the fold's stream is not a declared signal",
            events_ref.span,
        ));
        return None;
    };
    let CoreKind::Prim {
        op: Prim::Decide,
        args: decide_args,
    } = &events_decl.expr.kind
    else {
        diags.push(
            Diagnostic::error(
                "B0504",
                "events must come from `decide`",
                events_decl.expr.span,
            )
            .with_primary_label("Phase 1 slices `decide(proposals, state, validate)`")
            .with_note(
                "`decide` is the sole consumer of ingress and the one place a command becomes an \
                 event — §3.5's \"authority is one chokepoint\"",
            ),
        );
        return None;
    };
    let validate = decide_args.get(2).cloned()?;

    // The client-placed signal is the page.
    let Some(page) = program.signals.iter().find(|s| s.tier == Tier::Client) else {
        diags.push(
            Diagnostic::error("B0505", "no signal is placed on the client", Span::NONE)
                .with_note(
                    "`page` is the tier crossing: a `Signal[Html]` the browser subscribes to",
                )
                .with_fix("add `@on(client)` and `page: Signal[Html] = per_session(todos, view)`"),
        );
        return None;
    };

    let mut inliner = Inliner {
        by_name: &by_name,
        state_name: state_decl.name.clone(),
        state_var: 0,
        session_var: 1,
        inlined: Vec::new(),
        per_session: false,
        diags,
    };
    let view_body = inliner.lower(&page.expr)?;
    let inlined = inliner.inlined.clone();
    let per_session = inliner.per_session;

    let view = Core {
        kind: CoreKind::Lam {
            params: vec![0, 1],
            body: Box::new(view_body),
        },
        ty: Ty::Fun(
            vec![state_ty(&state_decl.ty), Ty::con("Session")],
            Box::new(Ty::html()),
        ),
        tier: Tier::Client,
        span: page.span,
    };

    let event_ty = stream_elem(&events_decl.ty);
    let command_ty = program
        .types
        .get("Command")
        .map(|_| Ty::con("Command"))
        .unwrap_or_else(Ty::unit);

    // §4.3: content-derived, stable across refactors that do not change the signature.
    let mut hasher = blake3::Hasher::new();
    hasher.update(program.name.as_bytes());
    hasher.update(format!("{}", command_ty).as_bytes());
    hasher.update(format!("{}", event_ty).as_bytes());
    hasher.update(format!("{}", state_decl.ty).as_bytes());
    let wire_id = hasher.finalize().to_hex()[..16].to_string();

    Some(Placed {
        roles: Roles {
            validate,
            fold,
            init,
            view,
            state_ty: state_ty(&state_decl.ty),
            event_ty,
            command_ty,
            proposals_name: proposals.name.clone(),
            events_name: events_decl.name.clone(),
            state_name: state_decl.name.clone(),
            page_name: page.name.clone(),
            inlined,
            view_is_per_session: per_session,
        },
        wire_id,
        program,
    })
}

fn state_ty(t: &Ty) -> Ty {
    match t {
        Ty::Con(n, args) if n.as_ref() == Ty::SIGNAL && args.len() == 1 => args[0].clone(),
        other => other.clone(),
    }
}

fn stream_elem(t: &Ty) -> Ty {
    match t {
        Ty::Con(n, args)
            if (n.as_ref() == Ty::STREAM || n.as_ref() == Ty::SIGNAL) && args.len() == 1 =>
        {
            args[0].clone()
        }
        other => other.clone(),
    }
}

fn signal_name(c: &Core) -> Option<Arc<str>> {
    match &c.kind {
        CoreKind::Global(n) => Some(n.clone()),
        _ => None,
    }
}

/// Rewrites a signal expression into a function of the durable state (and the session).
///
/// This is the slicing itself. `signal_map(s, f)` becomes `f(lower(s))`, `map2(f, a, b)` becomes
/// `f(lower(a), lower(b))`, `per_session(s, f)` becomes `f(lower(s), session)`, and a reference to
/// the durable signal becomes the state parameter. Every intermediate signal disappears into the
/// expression — which is exactly what "views are full recompute per event" means, stated as a
/// program transformation instead of a runtime convention.
struct Inliner<'a, 'd> {
    by_name: &'a BTreeMap<Arc<str>, &'a SignalDecl>,
    state_name: Arc<str>,
    state_var: VarId,
    session_var: VarId,
    inlined: Vec<Arc<str>>,
    per_session: bool,
    diags: &'d mut Diagnostics,
}

impl<'a, 'd> Inliner<'a, 'd> {
    fn lower(&mut self, expr: &Core) -> Option<Core> {
        match &expr.kind {
            CoreKind::Global(name) if *name == self.state_name => Some(Core {
                kind: CoreKind::Var(self.state_var),
                ty: state_ty(&expr.ty),
                tier: Tier::Client,
                span: expr.span,
            }),
            CoreKind::Global(name) => {
                let Some(decl) = self.by_name.get(name) else {
                    self.diags.push(Diagnostic::error(
                        "B0506",
                        format!("`{name}` is not a signal this view can reach"),
                        expr.span,
                    ));
                    return None;
                };
                if !self.inlined.contains(name) {
                    self.inlined.push(name.clone());
                }
                let decl_expr = decl.expr.clone();
                self.lower(&decl_expr)
            }
            CoreKind::Prim { op, args } => match op {
                Prim::SignalMap if args.len() == 2 => {
                    let input = self.lower(&args[0])?;
                    Some(call(args[1].clone(), vec![input], expr.span, &expr.ty))
                }
                Prim::PerSession if args.len() == 2 => {
                    self.per_session = true;
                    let input = self.lower(&args[0])?;
                    let session = Core {
                        kind: CoreKind::Var(self.session_var),
                        ty: Ty::con("Session"),
                        tier: Tier::Client,
                        span: expr.span,
                    };
                    Some(call(
                        args[1].clone(),
                        vec![input, session],
                        expr.span,
                        &expr.ty,
                    ))
                }
                Prim::SignalMap2 if args.len() == 3 => {
                    let a = self.lower(&args[1])?;
                    let b = self.lower(&args[2])?;
                    Some(call(args[0].clone(), vec![a, b], expr.span, &expr.ty))
                }
                Prim::Durable if args.len() == 1 => Some(Core {
                    kind: CoreKind::Var(self.state_var),
                    ty: state_ty(&expr.ty),
                    tier: Tier::Client,
                    span: expr.span,
                }),
                other => {
                    self.diags.push(
                        Diagnostic::error(
                            "B0507",
                            format!("`{}` cannot appear in a view's signal graph", other.name()),
                            expr.span,
                        )
                        .with_note(
                            "a view is a pure function of signals: `signal_map`, `map2` and \
                             `per_session` are the edges Phase 1 slices",
                        ),
                    );
                    None
                }
            },
            _ => {
                self.diags.push(Diagnostic::error(
                    "B0508",
                    "unsupported signal expression",
                    expr.span,
                ));
                None
            }
        }
    }
}

fn call(func: Core, args: Vec<Core>, span: Span, sig_ty: &Ty) -> Core {
    Core {
        kind: CoreKind::App {
            func: Box::new(func),
            args,
        },
        ty: stream_elem(sig_ty),
        tier: Tier::Client,
        span,
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
    fn a_program_with_no_merge_point_is_told_what_is_missing() {
        let (_, d, _) = compile_str("t.beck", "def f() -> Int:\n    return 1\n");
        assert!(d.iter().any(|x| x.code == "B0500" && x.fix.is_some()));
    }
}
