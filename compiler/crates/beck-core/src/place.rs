//! Stage 7 — placement, verified against effects.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.3:
//! "v0.1 ships **explicit placement, verified against effects** (reject `@on(client)` on anything
//! whose row the client cannot discharge); inference of the unannotated middle ground follows."
//! §3.10 stages this as step 2; the roadmap puts placement *inference* and the cost solver in
//! Phase 2. What is here is the verification half, which §3.10 notes is "already novel, already
//! shippable" on its own.
//!
//! Two rules do the work, and both come straight from the design:
//!
//! * **A tier must be able to discharge every effect placed on it.** `@on(client)` on the durable
//!   fold is the error the design names explicitly, and it is the one that makes "the log and the
//!   business rules provably never ship to a browser" (§3.5) a checked property rather than a
//!   convention.
//! * **A fold's function must be replay-pure** (§3.7). That is enforced where it is expressible —
//!   in `check`, which rejects `uuid()` lexically inside a `fold` — and re-asserted here over the
//!   collected effects, so a fold that reaches nondeterminism through a *named* function is caught
//!   too.

use beck_diag::{Diagnostic, Diagnostics};

use crate::check::Program;
use crate::ty::{Effect, Tier};

/// Verify every annotated placement in a checked program.
pub fn check_placement(program: &Program, diags: &mut Diagnostics) {
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        verify(
            def.tier,
            &def.effects,
            &format!("`{}`", def.name),
            def.span,
            def.tier_span,
            diags,
        );
    }
    for s in &program.signals {
        verify(
            s.tier,
            &s.effects,
            &format!("`{}`", s.name),
            s.span,
            s.tier_span,
            diags,
        );
    }

    // §3.7's determinism rule, over the *named* function a fold is given. `check` catches
    // `uuid()` written lexically inside `fold(...)`; this catches it reached through a function,
    // which is how anyone would actually write it.
    for s in &program.signals {
        for f in fold_functions(&s.expr) {
            let crate::core::CoreKind::Global(name) = &f.kind else {
                continue;
            };
            let Some(def) = program.defs.get(name) else {
                continue;
            };
            if def.effects.is_empty() {
                continue;
            }
            diags.push(
                Diagnostic::error(
                    "B0402",
                    format!("`{name}` is a fold function, so it must be replay-pure"),
                    f.span,
                )
                .with_primary_label(format!(
                    "performs {{{}}}",
                    def.effects
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .with_label(def.span, "defined here")
                .with_note(
                    "replaying the log must reproduce the state bit for bit; time is data on the \
                     envelope (`env.at`) and identity is minted at the edge",
                ),
            );
        }
    }

    // §3.7: "one totally-ordered log per application … there is exactly one of these."
    let ingress: Vec<&crate::check::SignalDecl> = program
        .signals
        .iter()
        .filter(|s| s.effects.contains(&Effect::Ingress))
        .collect();
    if ingress.len() > 1 {
        let mut d = Diagnostic::error(
            "B0403",
            "a program has exactly one merge point",
            ingress[1].span,
        )
        .with_primary_label("a second `merge_clients()`")
        .with_note(
            "the merge point is where time and nondeterminism enter; two of them would mean two \
             total orders, and replay would no longer be a function of the log",
        );
        d = d.with_label(ingress[0].span, "the first one is here");
        diags.push(d);
    }
}

/// Every function argument of a `fold` anywhere in an expression.
fn fold_functions(c: &crate::core::Core) -> Vec<&crate::core::Core> {
    use crate::core::{CoreKind, Prim};
    let mut out = Vec::new();
    if let CoreKind::Prim { op, args } = &c.kind {
        if *op == Prim::Fold {
            if let Some(f) = args.first() {
                out.push(f);
            }
        }
        for a in args {
            out.extend(fold_functions(a));
        }
    }
    out
}

fn verify(
    tier: Tier,
    effects: &[Effect],
    what: &str,
    span: beck_diag::Span,
    tier_span: beck_diag::Span,
    diags: &mut Diagnostics,
) {
    // `Tier::Any` is "unplaced — compiles anywhere" (§3.3), which is only legal when there is
    // nothing to discharge. Anything effectful and unannotated needs a placement, and Phase 1 says
    // so rather than guessing — "ambiguity that survives defaults is a compile error with a
    // suggested annotation, never a silent guess" (§3.4).
    if tier == Tier::Any {
        if let Some(e) = effects.first() {
            let candidates: Vec<&str> = [Tier::Client, Tier::Server, Tier::Data]
                .into_iter()
                .filter(|t| effects.iter().all(|e| t.discharges(e)))
                .map(Tier::name)
                .collect();
            let fix = match candidates.as_slice() {
                [] => "no tier can discharge these effects".to_string(),
                [one] => format!("annotate it with `@on({one})`"),
                many => format!("annotate it with `@on(…)`: {}", many.join(" or ")),
            };
            diags.push(
                Diagnostic::error(
                    "B0400",
                    format!("{what} has effects but no placement"),
                    span,
                )
                .with_primary_label(format!(
                    "performs {{{}}}, which no tier discharges by default",
                    effects
                        .iter()
                        .map(|e| e.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .with_note(format!("`{}` is the effect that forces a choice", e.name()))
                .with_fix(fix),
            );
        }
        return;
    }

    for e in effects {
        if tier.discharges(e) {
            continue;
        }
        let alternatives: Vec<&str> = [Tier::Client, Tier::Server, Tier::Data]
            .into_iter()
            .filter(|t| *t != tier && effects.iter().all(|x| t.discharges(x)))
            .map(Tier::name)
            .collect();
        let mut d = Diagnostic::error(
            "B0401",
            format!(
                "{what} is placed on `{}`, which cannot discharge `{}`",
                tier.name(),
                e.name()
            ),
            tier_span,
        )
        .with_primary_label(format!("`{}` cannot do this", tier.name()))
        .with_label(span, "the definition it is placed on")
        .with_note(match e {
            Effect::Ingress => {
                "`ingress` is the merge point: it admits time and nondeterminism, and only the \
                 server holds it"
            }
            Effect::Durable => {
                "`durable` is the log: placing it on the client would ship the database to the \
                 browser"
            }
            Effect::Dom => "`dom` touches the document, which only the client has",
            Effect::Nondet => {
                "minting ids or reading a clock is not replayable, so the fold engine refuses it"
            }
            other => match other.family() {
                "net.out" | "net.in" => "the browser can only reach its own origin",
                "cap" => {
                    "a capability is held by the tier that mints sessions, which is the server"
                }
                "env" | "fs" => "there is no process environment or filesystem in a browser",
                _ => "this tier cannot discharge that effect",
            },
        });
        if let [only] = alternatives.as_slice() {
            d = d.with_fix(format!("`@on({only})` discharges everything this needs"));
        }
        diags.push(d);
    }
}

#[cfg(test)]
mod tests {
    use crate::compile_str;

    fn errors(src: &str) -> Vec<&'static str> {
        let (_, d, _) = compile_str("t.beck", src);
        d.iter().map(|x| x.code).collect()
    }

    const DOMAIN: &str = "\
union Event:
    Added(id: Str, text: Str)

model State:
    count: Int

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(count=(s.count + 1))
";

    #[test]
    fn a_durable_fold_on_the_client_is_rejected_by_name() {
        let src = format!(
            "{DOMAIN}
@on(client)
todos: Signal[State] = durable(fold(apply_event, State(count=0), events))

@on(server)
events: Stream[Event] = merge_clients()
"
        );
        // The `events` line is deliberately mistyped as a Stream[Event] from merge_clients so the
        // test focuses on placement; both errors are reported, and B0401 is the one under test.
        assert!(errors(&src).contains(&"B0401"), "{:?}", errors(&src));
    }

    #[test]
    fn ingress_on_the_server_is_accepted() {
        let src = format!(
            "{DOMAIN}
@on(server)
proposals: Stream[Proposal] = merge_clients()
"
        );
        let codes = errors(&src);
        assert!(
            !codes.iter().any(|c| c.starts_with("B04")),
            "unexpected placement errors: {codes:?}"
        );
    }

    #[test]
    fn an_effectful_declaration_with_no_placement_suggests_one() {
        let src = format!(
            "{DOMAIN}
proposals: Stream[Proposal] = merge_clients()
"
        );
        let (_, d, _) = compile_str("t.beck", &src);
        let placement = d
            .iter()
            .find(|x| x.code == "B0400")
            .expect("missing placement diagnostic");
        assert!(
            placement
                .fix
                .as_deref()
                .unwrap_or("")
                .contains("@on(server)"),
            "{:?}",
            placement.fix
        );
    }

    #[test]
    fn two_merge_points_are_rejected() {
        let src = format!(
            "{DOMAIN}
@on(server)
a: Stream[Proposal] = merge_clients()

@on(server)
b: Stream[Proposal] = merge_clients()
"
        );
        assert!(errors(&src).contains(&"B0403"), "{:?}", errors(&src));
    }

    #[test]
    fn a_fold_that_reaches_nondeterminism_through_a_function_is_rejected() {
        // The lexical case (`uuid()` written inside `fold(...)`) is caught in `check`; this is the
        // one anyone would actually write — a named fold function that is not replay-pure.
        let src = crate::split::tests::TODO.replace(
            "return s.with(todos=map_remove(s.todos, id))",
            "return s.with(todos=map_remove(s.todos, uuid()))",
        );
        let (_, d, _) = compile_str("t.beck", &src);
        let codes: Vec<&str> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&"B0402"), "got {codes:?}");
    }
}
