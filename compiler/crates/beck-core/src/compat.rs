//! `beck check --wire-compat` — boundary versioning, §4.3.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.3, which
//! calls this "a hard requirement, not a nicety":
//!
//! > During a rolling deploy, old clients talk to new servers. Rules: operation ids are
//! > content-derived; a removed operation is retained as a deprecated shim for N releases; the wire
//! > format is field-tagged and tolerates unknown fields; `beck check --wire-compat
//! > <previous-release>` runs in CI and fails on a breaking change without an explicit `@breaking`
//! > marker. Getting this wrong produces the failure that kills adoption — "the deploy worked but
//! > every open browser tab broke."
//!
//! # What "compatible" means, precisely
//!
//! Not "the interface is unchanged" — that would make every release breaking, and a rule nobody can
//! satisfy is a rule everybody turns off. It means: **during the window when both versions are
//! live, neither can produce something the other cannot read.** There are three populations to keep
//! honest and they have different answers, which is why one rule would be wrong:
//!
//! | direction | who writes | who reads | so |
//! |---|---|---|---|
//! | **command** | the old client | the new server | the new server must accept every old command |
//! | **event** | both versions | the log, forever | neither may write what the other cannot fold |
//! | **state** | the fold | its own snapshots | a change needs §3.9's `migrate` |
//!
//! From those three, every rule below follows. A **new command variant** is compatible: no old
//! client sends it. A **removed command variant** is breaking: an old tab still has the button. A
//! **new event variant** is *breaking* even though nothing old sends one — because §3.1's
//! exhaustiveness check means an old fold, still running during the rollout, would have no case for
//! it. That asymmetry between commands and events is the interesting part of this file, and it is
//! not obvious from either type on its own; it comes from which side of the boundary each one
//! crosses.
//!
//! # What this deliberately does not do
//!
//! It does not diff behaviour, and it does not know about `migrate`/`upcast` functions, which are
//! §3.9's and Phase 4's. It compares two published contracts and classifies the differences. That
//! is the check §4.3 asks CI to run, and it is worth being clear that a green `--wire-compat` says
//! "no old client breaks", not "this deploy is safe".

use std::collections::BTreeSet;
use std::fmt;

use crate::iface::{Interface, Item, Kind};
use crate::ty::{Effect, Ty, TyDecl};

/// How bad a change is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Old and new can coexist.
    Compatible,
    /// Something that was live will stop working during the rollout.
    Breaking,
}

/// One difference between two releases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub severity: Severity,
    /// What changed, by name.
    pub what: String,
    /// The change, in one line.
    pub detail: String,
    /// Why it is or is not safe, in the terms of the table above.
    pub because: &'static str,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}: {}",
            match self.severity {
                Severity::Compatible => "compatible",
                Severity::Breaking => "BREAKING  ",
            },
            self.what,
            self.detail
        )
    }
}

/// Compare a previous release's interface with this one.
pub fn compare(previous: &Interface, current: &Interface) -> Vec<Change> {
    let mut out = Vec::new();
    types(previous, current, &mut out);
    items(previous, current, &mut out);
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.what.cmp(&b.what)));
    out
}

/// Is this release shippable against that one?
pub fn is_breaking(changes: &[Change]) -> bool {
    changes.iter().any(|c| c.severity == Severity::Breaking)
}

fn find<'a>(iface: &'a Interface, name: &str) -> Option<&'a TyDecl> {
    iface.types.iter().find(|t| t.name().as_ref() == name)
}

fn types(previous: &Interface, current: &Interface, out: &mut Vec<Change>) {
    for old in &previous.types {
        let name = old.name().to_string();
        let Some(new) = find(current, &name) else {
            out.push(Change {
                severity: Severity::Breaking,
                what: name.clone(),
                detail: "removed".into(),
                because: "a value of this type may already be in the log or in a live client",
            });
            continue;
        };
        compare_decl(&name, old, new, out);
    }
    for new in &current.types {
        if find(previous, new.name()).is_none() {
            out.push(Change {
                severity: Severity::Compatible,
                what: new.name().to_string(),
                detail: "added".into(),
                because: "nothing in the previous release refers to it",
            });
        }
    }
}

fn compare_decl(name: &str, old: &TyDecl, new: &TyDecl, out: &mut Vec<Change>) {
    match (old, new) {
        (TyDecl::Union { variants: a, .. }, TyDecl::Union { variants: b, .. }) => {
            let is_event = name == "Event";
            let old_names: BTreeSet<&str> = a.iter().map(|v| v.name.as_ref()).collect();
            let new_names: BTreeSet<&str> = b.iter().map(|v| v.name.as_ref()).collect();
            for gone in old_names.difference(&new_names) {
                out.push(Change {
                    severity: Severity::Breaking,
                    what: format!("{name}.{gone}"),
                    detail: "variant removed".into(),
                    because: if is_event {
                        "the log still holds these, and replay must reproduce state from the \
                         first event"
                    } else {
                        "an old client still has the button that sends it"
                    },
                });
            }
            for added in new_names.difference(&old_names) {
                // The asymmetry this whole file exists to get right.
                out.push(if is_event {
                    Change {
                        severity: Severity::Breaking,
                        what: format!("{name}.{added}"),
                        detail: "variant added".into(),
                        because: "an old fold is still running during the rollout, and §3.1's \
                                  exhaustiveness means it has no case for this",
                    }
                } else {
                    Change {
                        severity: Severity::Compatible,
                        what: format!("{name}.{added}"),
                        detail: "variant added".into(),
                        because: "no old client sends it, and the new server understands it",
                    }
                });
            }
            for v in b {
                let Some(o) = a.iter().find(|x| x.name == v.name) else {
                    continue;
                };
                compare_fields(
                    &format!("{name}.{}", v.name),
                    &o.fields,
                    &v.fields,
                    is_event,
                    out,
                );
            }
        }
        (TyDecl::Model { fields: a, .. }, TyDecl::Model { fields: b, .. }) => {
            compare_fields(name, a, b, false, out);
        }
        (TyDecl::Newtype { inner: a, .. }, TyDecl::Newtype { inner: b, .. })
        | (TyDecl::Alias { ty: a, .. }, TyDecl::Alias { ty: b, .. }) => {
            if a != b {
                out.push(Change {
                    severity: Severity::Breaking,
                    what: name.to_string(),
                    detail: format!("changed from `{a}` to `{b}`"),
                    because: "the encoding of every value of this type changes with it",
                });
            }
        }
        _ => out.push(Change {
            severity: Severity::Breaking,
            what: name.to_string(),
            detail: "changed kind".into(),
            because: "a model and a union do not encode the same way",
        }),
    }
}

fn compare_fields(
    what: &str,
    old: &[(std::sync::Arc<str>, Ty)],
    new: &[(std::sync::Arc<str>, Ty)],
    is_event: bool,
    out: &mut Vec<Change>,
) {
    for (name, ty) in old {
        match new.iter().find(|(n, _)| n == name) {
            None => out.push(Change {
                severity: Severity::Breaking,
                what: format!("{what}.{name}"),
                detail: "field removed".into(),
                because: "a reader of the old shape expects it to be there",
            }),
            Some((_, t)) if t != ty => out.push(Change {
                severity: Severity::Breaking,
                what: format!("{what}.{name}"),
                detail: format!("type changed from `{ty}` to `{t}`"),
                because: "the old and new encodings of this field disagree",
            }),
            Some(_) => {}
        }
    }
    for (name, _) in new {
        if old.iter().any(|(n, _)| n == name) {
            continue;
        }
        // §4.4: "the wire format is field-tagged and tolerates unknown fields". The reverse — a
        // *missing* field — is what a required addition is, from an old writer's side.
        out.push(Change {
            severity: Severity::Breaking,
            what: format!("{what}.{name}"),
            detail: "field added".into(),
            because: if is_event {
                "an old event in the log has no value for it, so replay would have to invent one"
            } else {
                "an old client sends this without the field, so the new server has none to read"
            },
        });
    }
}

fn items(previous: &Interface, current: &Interface, out: &mut Vec<Change>) {
    for old in &previous.items {
        let Some(new) = current.item(&old.name) else {
            out.push(Change {
                severity: Severity::Breaking,
                what: old.name.to_string(),
                detail: "removed".into(),
                because: "§4.3 asks for a deprecated shim rather than a removal",
            });
            continue;
        };
        compare_item(old, new, out);
    }
    for new in &current.items {
        if previous.item(&new.name).is_none() {
            out.push(Change {
                severity: Severity::Compatible,
                what: new.name.to_string(),
                detail: "added".into(),
                because: "nothing in the previous release calls it",
            });
        }
    }
}

fn compare_item(old: &Item, new: &Item, out: &mut Vec<Change>) {
    let name = old.name.to_string();
    match (&old.kind, &new.kind) {
        (
            Kind::Function {
                params: a, ret: ra, ..
            },
            Kind::Function {
                params: b, ret: rb, ..
            },
        ) => {
            if a.len() != b.len() || a.iter().zip(b).any(|((_, x), (_, y))| x != y) || ra != rb {
                out.push(Change {
                    severity: Severity::Breaking,
                    what: name.clone(),
                    detail: "signature changed".into(),
                    because: "a caller compiled against the old shape is still in the cluster",
                });
            }
        }
        (Kind::Signal { ty: a }, Kind::Signal { ty: b }) => {
            if a != b {
                out.push(Change {
                    severity: Severity::Breaking,
                    what: name.clone(),
                    detail: format!("type changed from `{a}` to `{b}`"),
                    because: "a subscriber resuming at a `seq` expects the shape it left",
                });
            }
        }
        _ => out.push(Change {
            severity: Severity::Breaking,
            what: name.clone(),
            detail: "changed between a function and a signal".into(),
            because: "one is called and the other subscribed to",
        }),
    }

    // §3.6: "effect widening is a breaking API change flagged by `beck check --api` — a library
    // that starts phoning home cannot do so silently — a novel supply-chain property."
    let before: BTreeSet<&Effect> = old.effects.iter().collect();
    let after: BTreeSet<&Effect> = new.effects.iter().collect();
    let widened: Vec<String> = after.difference(&before).map(|e| e.name()).collect();
    let narrowed: Vec<String> = before.difference(&after).map(|e| e.name()).collect();
    if !widened.is_empty() {
        out.push(Change {
            severity: Severity::Breaking,
            what: name.clone(),
            detail: format!("effects widened: +{{{}}}", widened.join(", ")),
            because: "a library that starts phoning home cannot do so silently — this is the \
                      supply-chain property, and it is only worth having if it fails the build",
        });
    }
    if !narrowed.is_empty() {
        out.push(Change {
            severity: Severity::Compatible,
            what: name.clone(),
            detail: format!("effects narrowed: -{{{}}}", narrowed.join(", ")),
            because: "doing less than promised breaks nobody",
        });
    }

    if old.tier != new.tier {
        // Placement is part of the signature (§3.6), but moving code between tiers does not change
        // what crosses the wire — the splitter re-synthesises the boundary either way.
        out.push(Change {
            severity: Severity::Compatible,
            what: name,
            detail: format!("moved from {} to {}", old.tier.name(), new.tier.name()),
            because: "where code runs is a deployment change, not a wire change",
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_str;

    fn iface(src: &str) -> Interface {
        let (placed, d, map) = compile_str("todo.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        Interface::of(&placed.expect("it compiles").program)
    }

    fn changes(edit: impl Fn(&str) -> String) -> Vec<Change> {
        let before = iface(crate::split::tests::TODO);
        let after = iface(&edit(crate::split::tests::TODO));
        compare(&before, &after)
    }

    fn breaking_about(changes: &[Change], what: &str) -> bool {
        changes
            .iter()
            .any(|c| c.severity == Severity::Breaking && c.what.contains(what))
    }

    #[test]
    fn a_release_is_compatible_with_itself() {
        let i = iface(crate::split::tests::TODO);
        assert!(compare(&i, &i).is_empty());
        assert!(!is_breaking(&compare(&i, &i)));
    }

    #[test]
    fn a_body_edit_is_not_a_wire_change_at_all() {
        let c = changes(|s| {
            s.replace(
                r#""done" if t.done else """#,
                r#""done" if t.done else " ""#,
            )
        });
        assert!(c.is_empty(), "{c:?}");
    }

    /// The sketch with a new command variant, handled — because §3.1's exhaustiveness check makes
    /// the *within-module* half of this a compile error, and only the cross-release half is left
    /// for `--wire-compat` to catch.
    fn with_new_command(s: &str) -> String {
        s.replace(
            "    Toggle(id: Id)\n    Delete(id: Id)\n\nunion Event:",
            "    Toggle(id: Id)\n    Delete(id: Id)\n    Star(id: Id)\n\nunion Event:",
        )
        .replace(
            "        case Delete(id):\n            return owned(s, p, id, [Deleted(id=id)])",
            "        case Delete(id):\n            return owned(s, p, id, [Deleted(id=id)])\n        case Star(id):\n            return owned(s, p, id, [Toggled(id=id)])",
        )
    }

    /// The same, for an event — with the fold case an old deployment would not have.
    fn with_new_event(s: &str) -> String {
        s.replace(
            "    Toggled(id: Id)\n    Deleted(id: Id)",
            "    Toggled(id: Id)\n    Deleted(id: Id)\n    Starred(id: Id)",
        )
        .replace(
            "        case Deleted(id):\n            return s.with(todos=map_remove(s.todos, id))",
            "        case Deleted(id):\n            return s.with(todos=map_remove(s.todos, id))\n        case Starred(id):\n            return toggle(s, id)",
        )
    }

    /// The sketch with `Delete` gone from the command union, and from the validator with it.
    fn without_delete_command(s: &str) -> String {
        s.replace(
            "    Toggle(id: Id)\n    Delete(id: Id)\n\nunion Event:",
            "    Toggle(id: Id)\n\nunion Event:",
        )
        .replace(
            "        case Delete(id):\n            return owned(s, p, id, [Deleted(id=id)])\n",
            "",
        )
    }

    #[test]
    fn a_new_command_is_compatible_and_a_new_event_is_not() {
        // The asymmetry, which is the whole reason this is a check and not a diff. Nothing old
        // *sends* a new event either — but an old fold is still running during the rollout, and
        // §3.1's exhaustiveness means it has no case for one.
        let added_command = changes(with_new_command);
        assert!(!is_breaking(&added_command), "{added_command:?}");
        assert!(
            added_command
                .iter()
                .any(|c| c.what == "Command.Star" && c.severity == Severity::Compatible),
            "{added_command:?}"
        );

        let added_event = changes(with_new_event);
        assert!(
            breaking_about(&added_event, "Event.Starred"),
            "{added_event:?}"
        );
    }

    #[test]
    fn removing_a_command_variant_breaks_the_tab_that_still_has_the_button() {
        let c = changes(without_delete_command);
        assert!(breaking_about(&c, "Command.Delete"), "{c:?}");
    }

    #[test]
    fn adding_a_field_to_an_event_is_breaking_because_the_log_has_no_value_for_it() {
        let c = changes(|s| {
            s.replace(
                "union Event:\n    Added(id: Id, text: Str)",
                "union Event:\n    Added(id: Id, text: Str, priority: Int)",
            )
            .replace(
                "return Ok(value=[Added(id=id, text=text)])",
                "return Ok(value=[Added(id=id, text=text, priority=0)])",
            )
        });
        assert!(breaking_about(&c, "Event.Added.priority"), "{c:?}");
    }

    #[test]
    fn widening_an_effect_is_breaking_and_narrowing_one_is_not() {
        // §3.6's supply-chain property. It is only worth having if it fails the build.
        let widened = changes(|s| {
            s.replace(
                "def owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection]:",
                "def owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection] uses net.out(audit.example.com):",
            )
        });
        assert!(
            widened.iter().any(|c| c.severity == Severity::Breaking
                && c.detail.contains("effects widened")
                && c.detail.contains("net.out(audit.example.com)")),
            "{widened:?}"
        );

        // The reverse: the *previous* release performed it and the current one does not.
        let before = iface(&crate::split::tests::TODO.replace(
            "def owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection]:",
            "def owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection] uses net.out(audit.example.com):",
        ));
        let after = iface(crate::split::tests::TODO);
        let narrowed = compare(&before, &after);
        assert!(!is_breaking(&narrowed), "{narrowed:?}");
        assert!(narrowed
            .iter()
            .any(|c| c.detail.contains("effects narrowed")));
    }

    #[test]
    fn moving_code_between_tiers_is_not_a_wire_change() {
        let c = changes(|s| s.replace("@on(data)\ntodos", "@on(server)\ntodos"));
        assert!(!is_breaking(&c), "{c:?}");
        assert!(
            c.iter()
                .any(|x| x.detail.contains("moved from data to server")),
            "{c:?}"
        );
    }

    #[test]
    fn changing_the_state_type_is_breaking_because_snapshots_are_of_the_old_one() {
        let c = changes(|s| {
            s.replace(
                "model State:\n    todos: Map[Id, Todo]",
                "model State:\n    todos: Map[Id, Todo]\n    revision: Int",
            )
            .replace("State(todos={})", "State(todos={}, revision=0)")
        });
        assert!(breaking_about(&c, "State.revision"), "{c:?}");
    }

    #[test]
    fn every_change_carries_a_reason_someone_can_argue_with() {
        // A CI gate that says "breaking" and nothing else gets turned off. Each classification here
        // has to say which of the three populations it is protecting.
        let c = changes(with_new_event);
        assert!(!c.is_empty());
        for change in &c {
            assert!(!change.because.is_empty(), "{change:?}");
            assert!(!change.detail.is_empty(), "{change:?}");
        }
    }
}
