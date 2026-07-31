//! The standard library of the walking skeleton.
//!
//! Small on purpose. §3.2's promise is that "effect polymorphism is what keeps one standard
//! library" — `map : (list[a], (a -> b ! e)) -> list[b] ! e`. Phase 2 has effect rows, so that
//! signature is now written as written: `map_list` is polymorphic in what its function argument
//! does, and mapping an effectful function over a list is effectful *in exactly that way*. One
//! library, one definition per operation, usable from any tier the placement solver allows.
//!
//! The rows here are the source of truth for inference. [`Prim::effects`] is the same information
//! for the atoms a primitive performs *itself*, and a test holds the two in agreement.
//!
//! Everything here is a [`Prim`], which means the evaluator implements it and the eventual
//! Cranelift/LLVM backends implement it — never a Beck-source shim that would have to be compiled
//! twice.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::core::Prim;
use crate::ty::{Effect, Row, RowVarId, Scheme, Ty, TyDecl, Variant};

/// A fresh type variable id for a scheme. Scheme variables are numbered from a private range that
/// never collides with the inference variables `Subst` mints, because `instantiate` replaces them.
const A: u32 = 1_000_000;
const B: u32 = 1_000_001;
const C: u32 = 1_000_002;

/// Row-variable ids for the schemes below, in their own range for the same reason `A`/`B`/`C` are:
/// `instantiate` replaces them, so they can never collide with an inference variable.
const E: RowVarId = 2_000_000;

fn v(id: u32) -> Ty {
    Ty::Var(id)
}

fn poly(vars: &[u32], ty: Ty) -> Scheme {
    Scheme {
        vars: vars.to_vec(),
        row_vars: Vec::new(),
        ty,
    }
}

/// A scheme polymorphic in both dimensions — §3.2's `(list[a], (a -> b ! e)) -> list[b] ! e`.
fn poly_eff(vars: &[u32], row_vars: &[RowVarId], ty: Ty) -> Scheme {
    Scheme {
        vars: vars.to_vec(),
        row_vars: row_vars.to_vec(),
        ty,
    }
}

/// A pure function type.
fn fun(params: Vec<Ty>, ret: Ty) -> Ty {
    Ty::fun(params, ret)
}

/// A function type with an effect row.
fn fun_eff(params: Vec<Ty>, ret: Ty, row: Row) -> Ty {
    Ty::fun_eff(params, ret, row)
}

/// Every primitive's name and type.
pub fn prims() -> Vec<(&'static str, Prim, Scheme)> {
    let int = Ty::int();
    let bool_ = Ty::bool_();
    let str_ = Ty::str_();
    let html = Ty::html();
    let attr = Ty::con(Ty::ATTR);

    vec![
        // `+` is resolved bidirectionally in `check` so that it can also concatenate strings
        // without introducing a numeric type class; the scheme here is its Int form.
        (
            "+",
            Prim::Add,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "-",
            Prim::Sub,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "*",
            Prim::Mul,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "/",
            Prim::Div,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "%",
            Prim::Rem,
            Scheme::mono(fun(vec![int.clone(), int.clone()], int.clone())),
        ),
        (
            "negate",
            Prim::Neg,
            Scheme::mono(fun(vec![int.clone()], int.clone())),
        ),
        (
            "==",
            Prim::Eq,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "!=",
            Prim::Ne,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "<",
            Prim::Lt,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "<=",
            Prim::Le,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            ">",
            Prim::Gt,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            ">=",
            Prim::Ge,
            poly(&[A], fun(vec![v(A), v(A)], bool_.clone())),
        ),
        (
            "and",
            Prim::And,
            Scheme::mono(fun(vec![bool_.clone(), bool_.clone()], bool_.clone())),
        ),
        (
            "or",
            Prim::Or,
            Scheme::mono(fun(vec![bool_.clone(), bool_.clone()], bool_.clone())),
        ),
        (
            "not",
            Prim::Not,
            Scheme::mono(fun(vec![bool_.clone()], bool_.clone())),
        ),
        (
            "str",
            Prim::ToStr,
            poly(&[A], fun(vec![v(A)], str_.clone())),
        ),
        (
            "str_trim",
            Prim::StrTrim,
            Scheme::mono(fun(vec![str_.clone()], str_.clone())),
        ),
        (
            "str_is_empty",
            Prim::StrIsEmpty,
            Scheme::mono(fun(vec![str_.clone()], bool_.clone())),
        ),
        (
            "list_len",
            Prim::ListLen,
            poly(&[A], fun(vec![Ty::list(v(A))], int.clone())),
        ),
        (
            "list_is_empty",
            Prim::ListIsEmpty,
            poly(&[A], fun(vec![Ty::list(v(A))], bool_.clone())),
        ),
        // §3.2, verbatim: `map : (list[a], (a -> b ! e)) -> list[b] ! e`. Mapping a function that
        // touches the dom over a list touches the dom; mapping a pure one does not.
        (
            "map_list",
            Prim::MapList,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![Ty::list(v(A)), fun_eff(vec![v(A)], v(B), Row::var(E))],
                    Ty::list(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "filter_list",
            Prim::FilterList,
            poly_eff(
                &[A],
                &[E],
                fun_eff(
                    vec![
                        Ty::list(v(A)),
                        fun_eff(vec![v(A)], bool_.clone(), Row::var(E)),
                    ],
                    Ty::list(v(A)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "concat_lists",
            Prim::ConcatLists,
            poly(&[A], fun(vec![Ty::list(Ty::list(v(A)))], Ty::list(v(A)))),
        ),
        (
            "sort_by",
            Prim::SortBy,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![Ty::list(v(A)), fun_eff(vec![v(A)], v(B), Row::var(E))],
                    Ty::list(v(A)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "map_get",
            Prim::MapGet,
            poly(
                &[A, B],
                fun(vec![Ty::map(v(A), v(B)), v(A)], Ty::option(v(B))),
            ),
        ),
        (
            "map_insert",
            Prim::MapInsert,
            poly(
                &[A, B],
                fun(vec![Ty::map(v(A), v(B)), v(A), v(B)], Ty::map(v(A), v(B))),
            ),
        ),
        (
            "map_remove",
            Prim::MapRemove,
            poly(
                &[A, B],
                fun(vec![Ty::map(v(A), v(B)), v(A)], Ty::map(v(A), v(B))),
            ),
        ),
        (
            "map_values",
            Prim::MapValues,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B))], Ty::list(v(B)))),
        ),
        (
            "map_contains",
            Prim::MapContains,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B)), v(A)], bool_.clone())),
        ),
        (
            "map_len",
            Prim::MapLen,
            poly(&[A, B], fun(vec![Ty::map(v(A), v(B))], int.clone())),
        ),
        (
            "is_some",
            Prim::OptionIsSome,
            poly(&[A], fun(vec![Ty::option(v(A))], bool_.clone())),
        ),
        (
            "unwrap_or",
            Prim::OptionUnwrapOr,
            poly(&[A], fun(vec![Ty::option(v(A)), v(A)], v(A))),
        ),
        (
            "html_el",
            Prim::HtmlEl,
            Scheme::mono(fun(
                vec![str_.clone(), Ty::list(attr.clone()), Ty::list(html.clone())],
                html.clone(),
            )),
        ),
        (
            "html_text",
            Prim::HtmlText,
            poly(&[A], fun(vec![v(A)], html.clone())),
        ),
        (
            "html_attr",
            Prim::HtmlAttr,
            poly(&[A], fun(vec![str_.clone(), v(A)], attr.clone())),
        ),
        (
            "html_on",
            Prim::HtmlOn,
            poly(&[A], fun(vec![str_.clone(), v(A)], attr.clone())),
        ),
        (
            "html_key",
            Prim::HtmlKey,
            poly(&[A], fun(vec![v(A)], attr.clone())),
        ),
        (
            "uuid",
            Prim::NewUuid,
            Scheme::mono(fun_eff(vec![], str_.clone(), Row::of([Effect::Nondet]))),
        ),
        // The other half of §3.7's forbidden pair. `now()` is legal anywhere a clock exists and
        // illegal inside a fold — which is a statement about its row, not about its name.
        (
            "now",
            Prim::Now,
            Scheme::mono(fun_eff(vec![], int.clone(), Row::of([Effect::Nondet]))),
        ),
        // §3.5's `type ApiKey = secret[str]`, given a source. Reading the process environment is
        // `env`, which no client discharges — so a secret cannot even be *obtained* on the tier it
        // must not reach, before Sendable is consulted at the boundary.
        (
            "secret_env",
            Prim::SecretEnv,
            Scheme::mono(fun_eff(
                vec![str_.clone()],
                Ty::secret(str_.clone()),
                Row::of([Effect::Env]),
            )),
        ),
        // §3.5's missing quadrant: storable, never Sendable.
        //
        // Wrapping is pure and free — recording a fact is not an effect. *Reading* one performs
        // `cap.internal`, which no tier but the server discharges and which
        // [`crate::secure`] discharges only inside the authority chokepoint. So a view cannot
        // unwrap one to render it: not because rendering is forbidden, but because the view is not
        // somewhere a capability is held.
        (
            "internal_of",
            Prim::InternalOf,
            poly(&[A], fun(vec![v(A)], Ty::internal(v(A)))),
        ),
        (
            "reveal",
            Prim::Reveal,
            poly(
                &[A],
                fun_eff(
                    vec![Ty::internal(v(A))],
                    v(A),
                    Row::of([Effect::Cap(Arc::from("internal"))]),
                ),
            ),
        ),
        // ---- the signal vocabulary (§3.7) ----
        //
        // `merge_clients : () -> Stream[(Session × Command)] ! { ingress }`. Phase 1 has no tuple
        // type, so the pair is the `Proposal` model the prelude declares below — the same shape,
        // named.
        (
            "merge_clients",
            Prim::MergeClients,
            Scheme::mono(fun_eff(
                vec![],
                Ty::stream(Ty::con("Proposal")),
                Row::of([Effect::Ingress]),
            )),
        ),
        (
            "filter_map",
            Prim::StreamFilterMap,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![
                        Ty::stream(v(A)),
                        fun_eff(vec![v(A)], Ty::option(v(B)), Row::var(E)),
                    ],
                    Ty::stream(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        // §3.7: "`fold`'s function must be *replay-pure*: effect row ⊆ {}". That could be written
        // as a closed empty row here, and unification would reject an impure fold — with a message
        // about rows failing to unify. The row is a *variable* instead, so the row is inferred and
        // then judged by `place`, which can say which effect, where it came from, and why the rule
        // exists. A checked property is worth no more than the diagnostic that delivers it.
        (
            "fold",
            Prim::Fold,
            poly_eff(
                &[A, B],
                &[E],
                fun(
                    vec![
                        fun_eff(
                            vec![v(A), Ty::app(Ty::ENVELOPE, vec![v(B)])],
                            v(A),
                            Row::var(E),
                        ),
                        v(A),
                        Ty::stream(v(B)),
                    ],
                    Ty::signal(v(A)),
                ),
            ),
        ),
        (
            "durable",
            Prim::Durable,
            poly(
                &[A],
                fun_eff(
                    vec![Ty::signal(v(A))],
                    Ty::signal(v(A)),
                    Row::of([Effect::Durable]),
                ),
            ),
        ),
        // A signal edge carries its function's row to the signal, which is what makes a view that
        // reaches the log a *placement* error on the client rather than a runtime surprise.
        (
            "signal_map",
            Prim::SignalMap,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![Ty::signal(v(A)), fun_eff(vec![v(A)], v(B), Row::var(E))],
                    Ty::signal(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "map2",
            Prim::SignalMap2,
            poly_eff(
                &[A, B, C],
                &[E],
                fun_eff(
                    vec![
                        fun_eff(vec![v(A), v(B)], v(C), Row::var(E)),
                        Ty::signal(v(A)),
                        Ty::signal(v(B)),
                    ],
                    Ty::signal(v(C)),
                    Row::var(E),
                ),
            ),
        ),
        (
            "per_session",
            Prim::PerSession,
            poly_eff(
                &[A, B],
                &[E],
                fun_eff(
                    vec![
                        Ty::signal(v(A)),
                        fun_eff(vec![v(A), Ty::con("Session")], v(B), Row::var(E)),
                    ],
                    Ty::signal(v(B)),
                    Row::var(E),
                ),
            ),
        ),
        // `validate : (Session, Command) -> list[Event]` (§3.7), with the accumulator threaded so
        // that client-minted ids can be checked for freshness and ownership against the actor —
        // the two obligations F2 puts on validation and the todo sketch deliberately skips.
        (
            "decide",
            Prim::Decide,
            poly_eff(
                &[A, B, C],
                &[E],
                fun_eff(
                    vec![
                        Ty::stream(Ty::con("Proposal")),
                        Ty::signal(v(A)),
                        fun_eff(
                            vec![v(A), Ty::con("Proposal")],
                            Ty::app(Ty::RESULT, vec![Ty::list(v(B)), v(C)]),
                            Row::var(E),
                        ),
                    ],
                    Ty::stream(v(B)),
                    Row::var(E),
                ),
            ),
        ),
    ]
}

/// Types every program has: `Option`, `Result`, `Envelope`, `Session`, `Proposal`.
///
/// `Envelope` is §3.7's, field for field — "`seq`: position in the total order — assigned here,
/// nowhere else; `at`: wall-clock, captured as data; `actor`: stable authenticated identity —
/// **never** the live `Session` capability or token".
pub fn types() -> BTreeMap<Arc<str>, TyDecl> {
    let mut out = BTreeMap::new();
    let mut add = |d: TyDecl| {
        out.insert(d.name().clone(), d);
    };

    add(TyDecl::Union {
        name: Arc::from(Ty::OPTION),
        variants: vec![
            Variant {
                name: Arc::from("Some"),
                fields: vec![(Arc::from("value"), Ty::Var(A))],
            },
            Variant {
                name: Arc::from("None"),
                fields: vec![],
            },
        ],
    });
    add(TyDecl::Union {
        name: Arc::from(Ty::RESULT),
        variants: vec![
            Variant {
                name: Arc::from("Ok"),
                fields: vec![(Arc::from("value"), Ty::Var(A))],
            },
            Variant {
                name: Arc::from("Err"),
                fields: vec![(Arc::from("error"), Ty::Var(B))],
            },
        ],
    });
    add(TyDecl::Model {
        name: Arc::from(Ty::ENVELOPE),
        fields: vec![
            (Arc::from("seq"), Ty::int()),
            (Arc::from("at"), Ty::int()),
            (Arc::from("actor"), Ty::str_()),
            (Arc::from("body"), Ty::Var(A)),
        ],
    });
    // "`Session` is minted by the identity subsystem … with verified claims mapped to typed
    // capabilities" (§3.7). Phase 1 carries the actor only — dev-mode identity, as Phase 0 had.
    add(TyDecl::Model {
        name: Arc::from("Session"),
        fields: vec![(Arc::from("actor"), Ty::str_())],
    });
    add(TyDecl::Model {
        name: Arc::from("Proposal"),
        fields: vec![
            (Arc::from("session"), Ty::con("Session")),
            (Arc::from("command"), Ty::con("Command")),
        ],
    });
    out
}

/// The type-constructor arities the checker knows without a declaration.
pub fn builtin_arity(name: &str) -> Option<usize> {
    Some(match name {
        Ty::INT | Ty::STR | Ty::BOOL | Ty::FLOAT | Ty::UNIT | Ty::HTML | Ty::ATTR => 0,
        Ty::LIST
        | Ty::OPTION
        | Ty::STREAM
        | Ty::SIGNAL
        | Ty::ENVELOPE
        | Ty::SECRET
        | Ty::INTERNAL => 1,
        Ty::MAP | Ty::RESULT => 2,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prim_has_a_signature_and_the_names_are_unique() {
        let all = prims();
        let mut names: Vec<&str> = all.iter().map(|(n, _, _)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate prelude name");
        for (name, prim, _) in &all {
            assert_eq!(*name, prim.name(), "prelude name must match Prim::name");
        }
    }

    #[test]
    fn folds_type_the_way_section_3_7_says() {
        // `fold(f, init, s) : Signal[S]` where `f : (S, Envelope[E]) -> S`.
        let all = prims();
        let (_, _, scheme) = all
            .iter()
            .find(|(n, _, _)| *n == "fold")
            .expect("fold exists");
        match &scheme.ty {
            Ty::Fun(params, ret, _) => {
                assert_eq!(params.len(), 3);
                assert_eq!(ret.con_name(), Some(Ty::SIGNAL));
                assert!(matches!(&params[0], Ty::Fun(ps, _, _) if ps.len() == 2));
                assert_eq!(params[2].con_name(), Some(Ty::STREAM));
            }
            other => panic!("fold should be a function, got {other}"),
        }
    }

    #[test]
    fn the_standard_library_is_effect_polymorphic_where_section_3_2_says_it_is() {
        // "Effect polymorphism is what keeps one standard library." If `map_list` were monomorphic
        // in its function's row there would have to be a pure `map` and an effectful `map`, and the
        // choice would be the caller's problem rather than the compiler's.
        let all = prims();
        for name in [
            "map_list",
            "filter_list",
            "sort_by",
            "signal_map",
            "per_session",
            "decide",
        ] {
            let (_, _, scheme) = all
                .iter()
                .find(|(n, _, _)| *n == name)
                .unwrap_or_else(|| panic!("{name} exists"));
            assert!(
                !scheme.row_vars.is_empty(),
                "`{name}` takes a function, so it must be polymorphic in that function's row"
            );
        }
        // …and the effectful primitives carry their atom, closed.
        for (name, atom) in [
            ("merge_clients", Effect::Ingress),
            ("durable", Effect::Durable),
            ("uuid", Effect::Nondet),
            ("now", Effect::Nondet),
            ("secret_env", Effect::Env),
        ] {
            let (_, _, scheme) = all.iter().find(|(n, _, _)| *n == name).unwrap();
            let Ty::Fun(_, _, row) = &scheme.ty else {
                panic!("{name} is a function")
            };
            assert!(
                row.atoms.contains(&atom),
                "`{name}` should perform `{atom}`"
            );
        }
    }

    #[test]
    fn every_primitives_own_atoms_agree_with_its_scheme() {
        // Two statements of the same fact — the table `Prim::effects` returns and the row in the
        // scheme — so a primitive cannot acquire an effect in one and not the other.
        for (name, prim, scheme) in prims() {
            let Ty::Fun(_, _, row) = &scheme.ty else {
                continue;
            };
            for e in &prim.effects() {
                assert!(
                    row.atoms.contains(e),
                    "`{name}` performs `{e}` but its scheme does not say so"
                );
            }
            for e in &row.atoms {
                assert!(
                    prim.effects().contains(e),
                    "`{name}`'s scheme carries `{e}` but `Prim::effects` does not"
                );
            }
        }
    }

    #[test]
    fn the_envelope_carries_an_actor_and_never_a_session() {
        let ts = types();
        match ts.get(Ty::ENVELOPE).expect("Envelope exists") {
            TyDecl::Model { fields, .. } => {
                let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_ref()).collect();
                assert_eq!(names, ["seq", "at", "actor", "body"]);
                assert!(!names.contains(&"session"), "F5: no capability in the log");
            }
            other => panic!("Envelope should be a model, got {other:?}"),
        }
    }
}
