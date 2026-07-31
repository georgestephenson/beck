//! §3.5's security properties, as checks rather than as intentions.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.5:
//! "Placement-as-type makes vulnerability classes *unrepresentable*." This module is where three of
//! that table's rows stop being prose:
//!
//! | property | mechanism, here |
//! |---|---|
//! | secrets cannot reach the browser | [`sendable`] at every tier crossing, and `secret[T]` is not |
//! | the log holds data, never code or views | [`storable`], checked at compile time |
//! | authority is one chokepoint | a `cap.*` effect is discharged only inside `decide`'s validator |
//!
//! The others in that table are checked elsewhere and named in the Phase 2 report: `ingress`/
//! `durable` being undischargeable on the client is [`crate::place`]; escaping in `html""` is
//! [`crate::html`]; effect-derived NetworkPolicy and grants are `beck-infra`; the macro phase's
//! capability restriction is `beck-macro`; and the tamper-evident history is the replay harness.
//!
//! # What "crosses a boundary" means concretely
//!
//! Not every value in a program crosses. In Phase 2's topology exactly three do, and each is a type
//! the splitter already names:
//!
//! * the **command** type — the browser's entire write surface, client → server;
//! * the **event** type — appended to the log, and read back by replay;
//! * the **state** type — the fold's accumulator, which the view consumes and whose rendering the
//!   client subscribes to.
//!
//! So the Sendable check is not a whole-program dataflow analysis; it is three types and the
//! transitive closure of their fields. That is a much stronger position than it sounds: a secret
//! cannot reach the browser without being *reachable from* one of those three, and if it is, the
//! type says so.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

use crate::check::Program;
use crate::core::{CoreKind, Prim};
use crate::ty::{Effect, Tier, Ty, TyDecl};

/// Why a type may not cross a boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotSendable {
    /// The offending type, by name — `secret[Str]`, or the function type.
    pub offender: String,
    /// The field path that reaches it: `State.config.api_key`.
    pub path: Vec<String>,
    pub why: &'static str,
}

impl NotSendable {
    pub fn flow(&self) -> String {
        self.path.join(".")
    }
}

/// May a value of this type cross a tier boundary? §3.5: "Boundary crossings require `Sendable`;
/// `secret[T]` isn't."
pub fn sendable(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Result<(), NotSendable> {
    check(ty, types, Rule::Sendable)
}

/// May a value of this type be written to the log?
///
/// Strictly stronger than [`sendable`]: a rendered view can cross a boundary — that is what a patch
/// stream *is* — but it cannot be stored, because replay must reconstruct it rather than read it
/// back. [`docs/19-phase-1-report.md`](../../../../docs/19-phase-1-report.md) §19.9 predicted this:
/// the runtime refusal in `value_to_repr` was "the right thing to have while the proof is missing",
/// and this is the proof. The refusal stays, now unreachable from a program that compiles.
pub fn storable(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>) -> Result<(), NotSendable> {
    check(ty, types, Rule::Storable)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Sendable,
    Storable,
}

fn check(ty: &Ty, types: &BTreeMap<Arc<str>, TyDecl>, rule: Rule) -> Result<(), NotSendable> {
    fn go(
        ty: &Ty,
        types: &BTreeMap<Arc<str>, TyDecl>,
        rule: Rule,
        path: &mut Vec<String>,
        seen: &mut BTreeSet<Arc<str>>,
    ) -> Result<(), NotSendable> {
        let fail = |offender: String, why: &'static str, path: &Vec<String>| {
            Err(NotSendable {
                offender,
                path: path.clone(),
                why,
            })
        };
        match ty {
            Ty::Var(_) => Ok(()),
            Ty::Fun(..) => fail(
                format!("{ty}"),
                "a function is code, and code does not cross a boundary as data",
                path,
            ),
            Ty::Con(name, args) => {
                match name.as_ref() {
                    Ty::SECRET => {
                        return fail(
                            format!("{ty}"),
                            "`secret[T]` is deliberately not Sendable: that is the whole mechanism",
                            path,
                        )
                    }
                    Ty::HTML | Ty::ATTR if rule == Rule::Storable => {
                        return fail(
                            format!("{ty}"),
                            "a view is derived from state, so storing one would make replay read \
                             it back rather than recompute it",
                            path,
                        )
                    }
                    _ => {}
                }
                for (i, a) in args.iter().enumerate() {
                    path.push(format!("[{i}]"));
                    go(a, types, rule, path, seen)?;
                    path.pop();
                }
                let Some(decl) = types.get(name.as_ref()) else {
                    return Ok(());
                };
                if !seen.insert(name.clone()) {
                    // A recursive type: its fields have already been walked.
                    return Ok(());
                }
                let out = match decl {
                    TyDecl::Model { fields, .. } => {
                        for (f, t) in fields {
                            path.push(f.to_string());
                            go(t, types, rule, path, seen)?;
                            path.pop();
                        }
                        Ok(())
                    }
                    TyDecl::Union { variants, .. } => {
                        for v in variants {
                            for (f, t) in &v.fields {
                                path.push(format!("{}.{f}", v.name));
                                go(t, types, rule, path, seen)?;
                                path.pop();
                            }
                        }
                        Ok(())
                    }
                    TyDecl::Newtype { inner, .. } | TyDecl::Alias { ty: inner, .. } => {
                        go(inner, types, rule, path, seen)
                    }
                };
                seen.remove(name.as_ref());
                out
            }
        }
    }
    let mut path = vec![format!("{ty}")];
    let mut seen = BTreeSet::new();
    go(ty, types, rule, &mut path, &mut seen)
}

/// One step of `beck explain flow <T>`: where a type is reachable, and whether that is allowed.
#[derive(Clone, Debug)]
pub struct Reach {
    pub what: Arc<str>,
    pub tier: Tier,
    pub blocked: Option<&'static str>,
}

/// §4.7's `beck explain flow ApiKey`: every definition whose signature mentions a type, the tier it
/// runs on, and whether that is a leak.
pub fn flow(program: &Program, ty_name: &str) -> Vec<Reach> {
    let mut out = Vec::new();
    for name in &program.def_order {
        let Some(d) = program.defs.get(name) else {
            continue;
        };
        let mentions = std::iter::once(&d.ret)
            .chain(d.params.iter().map(|(_, _, t)| t))
            .any(|t| mentions_type(t, ty_name, &program.types));
        if !mentions {
            continue;
        }
        out.push(Reach {
            what: d.name.clone(),
            tier: d.tier,
            blocked: (d.tier == Tier::Client).then_some("a client cannot hold a `secret[T]`"),
        });
    }
    for s in &program.signals {
        if !mentions_type(&s.ty, ty_name, &program.types) {
            continue;
        }
        out.push(Reach {
            what: s.name.clone(),
            tier: s.tier,
            blocked: (s.tier == Tier::Client).then_some("a client cannot hold a `secret[T]`"),
        });
    }
    out
}

fn mentions_type(ty: &Ty, name: &str, types: &BTreeMap<Arc<str>, TyDecl>) -> bool {
    fn go(
        ty: &Ty,
        name: &str,
        types: &BTreeMap<Arc<str>, TyDecl>,
        seen: &mut BTreeSet<Arc<str>>,
    ) -> bool {
        match ty {
            Ty::Var(_) => false,
            Ty::Fun(ps, r, _) => {
                ps.iter().any(|p| go(p, name, types, seen)) || go(r, name, types, seen)
            }
            Ty::Con(n, args) => {
                if n.as_ref() == name {
                    return true;
                }
                if args.iter().any(|a| go(a, name, types, seen)) {
                    return true;
                }
                if !seen.insert(n.clone()) {
                    return false;
                }
                match types.get(n.as_ref()) {
                    Some(TyDecl::Model { fields, .. }) => {
                        fields.iter().any(|(_, t)| go(t, name, types, seen))
                    }
                    Some(TyDecl::Union { variants, .. }) => variants
                        .iter()
                        .any(|v| v.fields.iter().any(|(_, t)| go(t, name, types, seen))),
                    Some(TyDecl::Newtype { inner, .. }) | Some(TyDecl::Alias { ty: inner, .. }) => {
                        go(inner, name, types, seen)
                    }
                    None => false,
                }
            }
        }
    }
    go(ty, name, types, &mut BTreeSet::new())
}

/// Run §3.5's checks over a placed program.
pub fn check_security(program: &Program, diags: &mut Diagnostics) {
    boundaries(program, diags);
    capabilities(program, diags);
}

/// The three types that cross, and what they are allowed to contain.
fn boundaries(program: &Program, diags: &mut Diagnostics) {
    for s in &program.signals {
        // The durable accumulator, and the events that build it, are what the log holds.
        if s.effects.contains(&Effect::Durable) {
            let state = element(&s.ty);
            if let Err(bad) = storable(&state, &program.types) {
                reject(
                    diags,
                    "B0411",
                    format!("`{}` is durable, so its state must be storable", s.name),
                    s.span,
                    &bad,
                    "the log is the only description of this program's history; a value it cannot \
                     read back is a state replay would not reproduce",
                );
            }
        }
        // Anything the browser subscribes to crosses to the browser.
        if s.tier == Tier::Client {
            let carried = element(&s.ty);
            if let Err(bad) = sendable(&carried, &program.types) {
                reject(
                    diags,
                    "B0410",
                    format!(
                        "`{}` runs on the client, so its value must be Sendable",
                        s.name
                    ),
                    s.span,
                    &bad,
                    "this value crosses to the browser; §3.5's whole claim is that the compiler \
                     proves it cannot carry a secret",
                );
            }
        }
    }

    // The command union is the client's entire write surface (§3.5), so it crosses by definition.
    if let Some(TyDecl::Union { .. }) = program.types.get("Command") {
        if let Err(bad) = sendable(&Ty::con("Command"), &program.types) {
            let span = program
                .signals
                .first()
                .map(|s| s.span)
                .unwrap_or(Span::NONE);
            reject(
                diags,
                "B0410",
                "`Command` is what clients send, so it must be Sendable".to_string(),
                span,
                &bad,
                "a command is minted in the browser: a secret in one would be a secret the browser \
                 already had",
            );
        }
    }

    // A definition placed on the client has its whole signature cross with it.
    for name in &program.def_order {
        let Some(d) = program.defs.get(name) else {
            continue;
        };
        if d.tier != Tier::Client {
            continue;
        }
        for t in std::iter::once(&d.ret).chain(d.params.iter().map(|(_, _, t)| t)) {
            if let Err(bad) = sendable(t, &program.types) {
                if bad.offender.starts_with("secret[") {
                    reject(
                        diags,
                        "B0410",
                        format!("`{}` runs on the client and handles a secret", d.name),
                        d.span,
                        &bad,
                        "`beck explain flow` shows the whole path; the fix is to keep the \
                         definition on a tier that can hold it",
                    );
                }
            }
        }
    }
}

fn reject(
    diags: &mut Diagnostics,
    code: &'static str,
    message: String,
    span: Span,
    bad: &NotSendable,
    note: &str,
) {
    diags.push(
        Diagnostic::error(code, message, span)
            .with_primary_label(format!("`{}` reaches it at `{}`", bad.offender, bad.flow()))
            .with_note(bad.why)
            .with_note(note.to_string()),
    );
}

fn element(t: &Ty) -> Ty {
    match t {
        Ty::Con(n, args)
            if (n.as_ref() == Ty::SIGNAL || n.as_ref() == Ty::STREAM) && args.len() == 1 =>
        {
            args[0].clone()
        }
        other => other.clone(),
    }
}

/// §3.5: "Only `validate` — the `ingress` consumer, holding `Session` capabilities — turns commands
/// into events; forgetting an auth check means the `cap.*` effect goes undischarged."
///
/// A capability is *held* at exactly one place in a Beck program: the validator `decide` is given,
/// because that is the only function handed a `Proposal`, and a `Proposal` is the only thing
/// carrying a `Session`. So a `cap.*` effect anywhere the validator does not reach is a capability
/// nobody can discharge — a requirement with no holder, which is what a missing auth check looks
/// like from the type system's side.
fn capabilities(program: &Program, diags: &mut Diagnostics) {
    let authorised = reachable_from_validator(program);
    for name in &program.def_order {
        let Some(d) = program.defs.get(name) else {
            continue;
        };
        let caps: Vec<&Effect> = d
            .effects
            .iter()
            .filter(|e| matches!(e, Effect::Cap(_)))
            .collect();
        if caps.is_empty() || authorised.contains(name) {
            continue;
        }
        let names: Vec<String> = caps.iter().map(|e| e.name()).collect();
        diags.push(
            Diagnostic::error(
                "B0412",
                format!("`{name}` requires a capability nothing can discharge"),
                d.span,
            )
            .with_primary_label(format!("needs {{{}}}", names.join(", ")))
            .with_note(
                "a `Session` reaches exactly one place in a Beck program: the validator `decide` is \
                 given, which is the only function handed a `Proposal`. Authority is one chokepoint \
                 (docs/03 §3.5), so a capability required outside it has no holder",
            )
            .with_fix(
                "call this from `validate` — or, if it genuinely needs no authority, drop the \
                 `cap.*` from its `uses`",
            ),
        );
    }
}

/// Every definition reachable from the validator `decide` was given.
fn reachable_from_validator(program: &Program) -> BTreeSet<Arc<str>> {
    let mut roots: Vec<Arc<str>> = Vec::new();
    for s in &program.signals {
        if let CoreKind::Prim {
            op: Prim::Decide,
            args,
        } = &s.expr.kind
        {
            if let Some(v) = args.get(2) {
                let mut names = BTreeSet::new();
                crate::place::mentions(v, &mut names);
                roots.extend(names);
            }
        }
    }
    let mut out: BTreeSet<Arc<str>> = BTreeSet::new();
    while let Some(n) = roots.pop() {
        if !out.insert(n.clone()) {
            continue;
        }
        if let Some(d) = program.defs.get(&n) {
            let mut names = BTreeSet::new();
            crate::place::mentions(&d.body, &mut names);
            roots.extend(names);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{check_str, compile_str};

    fn types() -> BTreeMap<Arc<str>, TyDecl> {
        BTreeMap::from([
            (
                Arc::from("Config"),
                TyDecl::Model {
                    name: Arc::from("Config"),
                    fields: vec![
                        (Arc::from("host"), Ty::str_()),
                        (Arc::from("key"), Ty::secret(Ty::str_())),
                    ],
                },
            ),
            (
                Arc::from("State"),
                TyDecl::Model {
                    name: Arc::from("State"),
                    fields: vec![(Arc::from("config"), Ty::con("Config"))],
                },
            ),
        ])
    }

    #[test]
    fn a_secret_is_not_sendable_however_deeply_it_is_buried() {
        let t = types();
        assert!(sendable(&Ty::str_(), &t).is_ok());
        let bad = sendable(&Ty::con("State"), &t).expect_err("State reaches a secret");
        // The path is the diagnostic: §4.7's `beck explain flow` is this string.
        assert_eq!(bad.flow(), "State.config.key");
        assert_eq!(bad.offender, "secret[Str]");
        // …and through a collection, too.
        assert!(sendable(&Ty::list(Ty::con("Config")), &t).is_err());
        assert!(sendable(&Ty::map(Ty::str_(), Ty::con("Config")), &t).is_err());
    }

    #[test]
    fn a_view_may_cross_a_boundary_but_may_not_be_stored() {
        // The distinction docs/19 §19.9 could not express: a patch stream is a view crossing a
        // boundary, and it is fine; a view *in the log* is a state replay would read rather than
        // recompute.
        let t = types();
        assert!(sendable(&Ty::html(), &t).is_ok());
        assert!(storable(&Ty::html(), &t).is_err());
    }

    #[test]
    fn a_recursive_type_terminates() {
        let t = BTreeMap::from([(
            Arc::from("Tree"),
            TyDecl::Model {
                name: Arc::from("Tree"),
                fields: vec![(Arc::from("kids"), Ty::list(Ty::con("Tree")))],
            },
        )]);
        assert!(sendable(&Ty::con("Tree"), &t).is_ok());
    }

    #[test]
    fn a_state_that_caches_a_view_is_refused_at_compile_time() {
        // The exact program docs/19 §19.9 named as compiling today and writing `unit` into the log:
        // "`model State: cached: Html` compiles today, and the encoder would have written `unit`
        // into a snapshot — silently". It does not compile now.
        let src = crate::split::tests::TODO.replace(
            "model State:\n    todos: Map[Id, Todo]",
            "model State:\n    todos: Map[Id, Todo]\n    cached: Html",
        );
        let (_, d, _) = compile_str("t.beck", &src);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0411"), "got {codes:?}");
    }

    #[test]
    fn a_secret_in_the_command_union_is_refused() {
        // "Clients can only *propose*": the command union is the browser's entire write surface, so
        // a secret in one would be a secret the browser already held.
        let src = crate::split::tests::TODO.replace(
            "union Command:\n    Add(id: Id, text: Str)",
            "union Command:\n    Add(id: Id, text: Str, token: secret[Str])",
        );
        let (_, d, _) = compile_str("t.beck", &src);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0410"), "got {codes:?}");
    }

    #[test]
    fn a_capability_required_outside_the_chokepoint_has_no_holder() {
        let src = crate::split::tests::TODO.replace(
            "def done_class(t: Todo) -> Str:",
            "def audit(t: Todo) -> Str uses cap.admin:\n    return t.text\n\n\
             def done_class(t: Todo) -> Str:",
        );
        let (_, d, _) = compile_str("t.beck", &src);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0412"), "got {codes:?}");
    }

    #[test]
    fn a_capability_required_inside_the_chokepoint_is_exactly_what_it_is_for() {
        // The same effect, reached from `validate`, is not an error — it is the design. And it
        // moves the whole authority path to the server, because no other tier discharges `cap.*`.
        let src = crate::split::tests::TODO
            .replace(
                "def owned(s: State, p: Proposal, id: Id, evs: list[Event])",
                "def admin(p: Proposal) -> Bool uses cap.admin:\n\
                 \x20   return p.session.actor != \"\"\n\n\
                 def owned(s: State, p: Proposal, id: Id, evs: list[Event])",
            )
            .replace(
                "    match map_get(s.todos, id):\n        case Some(value):\n            if value.owner != p.session.actor:",
                "    match map_get(s.todos, id):\n        case Some(value):\n            if not admin(p):",
            );
        let (program, d, map) = check_str("t.beck", &src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let mut diags = Diagnostics::new();
        let solution = crate::place::solve(&program, None);
        let mut program = program;
        crate::place::apply(&mut program, &solution);
        check_security(&program, &mut diags);
        assert!(
            !diags.iter().any(|x| x.code == "B0412"),
            "{}",
            diags.render(&map)
        );
        assert_eq!(
            program.defs["admin"].tier,
            Tier::Server,
            "only the server holds a capability"
        );
    }

    #[test]
    fn explain_flow_names_the_definitions_a_type_reaches() {
        let src = "\
model Config:
    key: secret[Str]

def load() -> Config uses env:
    return Config(key=secret_env(\"API_KEY\"))

def host(c: Config) -> Str:
    return \"api.example.com\"
";
        let (program, d, map) = check_str("t.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let reached: Vec<String> = flow(&program, "Config")
            .into_iter()
            .map(|r| r.what.to_string())
            .collect();
        assert_eq!(reached, ["load", "host"]);
    }
}
