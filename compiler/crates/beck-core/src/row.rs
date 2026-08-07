//! Effect rows — §3.2, made real.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../../docs/03-type-and-effect-system.md) §3.2:
//! "Every function type carries an inferred, row-polymorphic effect row … Effect polymorphism is
//! what keeps one standard library: `map : (list[a], (a -> b ! e)) -> list[b] ! e`."
//!
//! Phase 1 had four atoms, declared with `uses` and *collected* by walking what a body calls. This
//! module is the replacement: a wider atom set, row variables, and a unifier — so a row is a thing
//! the checker solves for rather than a list the programmer maintains.
//!
//! # The shape of a row
//!
//! A row is a **set** of atoms plus a set of row *variables* standing for "whatever else the
//! caller's function argument does":
//!
//! ```text
//! {}                      pure
//! { durable }             closed
//! { dom | e }             open: dom, plus whatever `e` turns out to be
//! { e, f }                the union of two callers' rows
//! ```
//!
//! Sets, not Rémy-style scoped labels: an effect happening twice is an effect happening. That makes
//! *union* — the operation inference actually performs, once per call — trivial and exact, which is
//! the operation that has to be right. Unification is the rarer one.
//!
//! # Why a row can hold several variables
//!
//! `fn twice(f, g) = f(); g()` performs `e_f ∪ e_g`, and there is no single variable that is their
//! union. A row that could hold only one tail would have to force `e_f = e_g`, which is a lie about
//! a program that typechecks. Holding a *set* of tails costs nothing and says the truth.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

/// An effect atom. §3.2's list, with the one correction Phase 2 makes to it (see [`Effect::Nondet`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Effect {
    /// A merge point: arbitrary interleaving. "there is exactly one of these" (§3.7).
    Ingress,
    /// A persistent accumulator — the log.
    Durable,
    /// Touches the document.
    Dom,
    /// Reads a clock, a random source, or mints an id.
    ///
    /// §3.2 files `time` and `rand` under the *ambient* set — "implicitly available outside folds
    /// and elided from signatures". Phase 2 does not, and the reason is §3.3's own table: the fold
    /// engine is a **tier**, and whether a tier can discharge time and randomness is precisely
    /// §3.7's determinism rule. An effect that decides a placement cannot also be elided from the
    /// signature that placement is derived from. `log` and `metrics` stay ambient
    /// ([`Effect::Ambient`]); these do not.
    Nondet,
    /// `net.out(host)` — an outbound call to a named host. The host is what
    /// [`docs/06-kubernetes-and-packaging.md`](../../../../../docs/06-kubernetes-and-packaging.md)
    /// §6.5 turns into a NetworkPolicy peer.
    NetOut(Arc<str>),
    /// `net.in` — accepts inbound connections.
    NetIn,
    /// `fs.read(path)` and `fs.write(path)` — two atoms for one resource.
    ///
    /// §3.2 listed a single `fs(path)` until [`docs/81`](../../../../../docs/81-fs-is-two-atoms-report.md).
    /// One atom naming a resource without saying what is done to it cannot answer the two questions
    /// that are actually asked of it: whether two things may happen at once
    /// ([`crate::check`]'s `parallel:` rule) and whether a mount needs to be writable
    /// ([`docs/06`](../../../../../docs/06-kubernetes-and-packaging.md) §6.5). §3.8's escape
    /// hatches were already two — [`Effect::ExternalRead`] and [`Effect::ExternalWrite`] — and
    /// this is the same split for the same reason.
    FsRead(Arc<str>),
    FsWrite(Arc<str>),
    /// Reads process environment.
    Env,
    /// Starts concurrent work.
    Spawn,
    /// `cap.X` — a capability the caller must hold. §3.5: "forgetting an auth check means the
    /// `cap.*` effect goes undischarged — a compile error, not a pentest finding".
    Cap(Arc<str>),
    /// May diverge or panic.
    Partial,
    /// `raises(E)` — this may fail with a value of the named type.
    ///
    /// An error is a **row label**, not a mechanism: a signature without one provably cannot fail,
    /// and `Result[T, E]` is the *reified* form a handler produces rather than a parallel channel
    /// ([`docs/38`](../../../../../docs/38-literature-survey.md) §38.4, adopting Koka's `exn`). The
    /// atom names the error's type because a handler has to say what it catches — a `try` that
    /// caught everything would turn a caller's unknown failure into this one's `Result`.
    Raises(Arc<str>),
    /// §3.8's escape hatches: an existing store the team already has.
    ExternalRead(Arc<str>),
    ExternalWrite(Arc<str>),
    /// The ambient set that survives §3.2's description: available everywhere, elided from
    /// signatures, and never a reason to place anything.
    Ambient(Ambient),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ambient {
    Log,
    Metrics,
}

impl Effect {
    pub fn name(&self) -> String {
        match self {
            Effect::Ingress => "ingress".into(),
            Effect::Durable => "durable".into(),
            Effect::Dom => "dom".into(),
            Effect::Nondet => "nondet".into(),
            Effect::NetOut(h) => format!("net.out({h})"),
            Effect::NetIn => "net.in".into(),
            Effect::FsRead(p) => format!("fs.read({p})"),
            Effect::FsWrite(p) => format!("fs.write({p})"),
            Effect::Env => "env".into(),
            Effect::Spawn => "spawn".into(),
            Effect::Cap(c) => format!("cap.{c}"),
            Effect::Partial => "partial".into(),
            Effect::Raises(t) => format!("raises({t})"),
            Effect::ExternalRead(s) => format!("external.read({s})"),
            Effect::ExternalWrite(s) => format!("external.write({s})"),
            Effect::Ambient(Ambient::Log) => "log".into(),
            Effect::Ambient(Ambient::Metrics) => "metrics".into(),
        }
    }

    /// Parse an atom as written in a `uses` clause: `durable`, `net.out(api.example.com)`, `cap.session`.
    pub fn parse(s: &str) -> Option<Effect> {
        let (head, arg) = match s.split_once('(') {
            Some((h, rest)) => (h.trim(), Some(rest.trim_end_matches(')').trim())),
            None => (s.trim(), None),
        };
        Some(match (head, arg) {
            ("ingress", None) => Effect::Ingress,
            ("durable", None) => Effect::Durable,
            ("dom", None) => Effect::Dom,
            ("nondet" | "nondeterministic", None) => Effect::Nondet,
            ("net.out", Some(h)) => Effect::NetOut(Arc::from(h)),
            ("net.in", None) => Effect::NetIn,
            ("fs.read", Some(p)) => Effect::FsRead(Arc::from(p)),
            ("fs.write", Some(p)) => Effect::FsWrite(Arc::from(p)),
            ("env", None) => Effect::Env,
            ("spawn", None) => Effect::Spawn,
            ("partial", None) => Effect::Partial,
            ("raises", Some(t)) => Effect::Raises(Arc::from(t)),
            ("external.read", Some(s)) => Effect::ExternalRead(Arc::from(s)),
            ("external.write", Some(s)) => Effect::ExternalWrite(Arc::from(s)),
            ("log", None) => Effect::Ambient(Ambient::Log),
            ("metrics", None) => Effect::Ambient(Ambient::Metrics),
            (other, None) if other.starts_with("cap.") => Effect::Cap(Arc::from(&other[4..])),
            _ => return None,
        })
    }

    /// Ambient effects are available on every tier and elided from printed signatures (§3.2).
    pub fn is_ambient(&self) -> bool {
        matches!(self, Effect::Ambient(_))
    }

    /// The atom without its argument — what a tier's discharge table and a cost model key on.
    pub fn family(&self) -> &'static str {
        match self {
            Effect::Ingress => "ingress",
            Effect::Durable => "durable",
            Effect::Dom => "dom",
            Effect::Nondet => "nondet",
            Effect::NetOut(_) => "net.out",
            Effect::NetIn => "net.in",
            Effect::FsRead(_) => "fs.read",
            Effect::FsWrite(_) => "fs.write",
            Effect::Env => "env",
            Effect::Spawn => "spawn",
            Effect::Cap(_) => "cap",
            Effect::Partial => "partial",
            Effect::Raises(_) => "raises",
            Effect::ExternalRead(_) => "external.read",
            Effect::ExternalWrite(_) => "external.write",
            Effect::Ambient(_) => "ambient",
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

pub type RowVarId = u32;

/// An effect row: a set of atoms, plus row variables standing for the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub atoms: BTreeSet<Effect>,
    pub tails: BTreeSet<RowVarId>,
}

impl Row {
    pub fn empty() -> Row {
        Row::default()
    }

    pub fn of(atoms: impl IntoIterator<Item = Effect>) -> Row {
        Row {
            atoms: atoms.into_iter().collect(),
            tails: BTreeSet::new(),
        }
    }

    pub fn var(v: RowVarId) -> Row {
        Row {
            atoms: BTreeSet::new(),
            tails: BTreeSet::from([v]),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.tails.is_empty()
    }

    pub fn is_pure(&self) -> bool {
        self.atoms.is_empty() && self.tails.is_empty()
    }

    /// The union — the operation inference performs once per call.
    pub fn union(mut self, other: &Row) -> Row {
        self.atoms.extend(other.atoms.iter().cloned());
        self.tails.extend(other.tails.iter().copied());
        self
    }

    pub fn add(&mut self, e: Effect) {
        self.atoms.insert(e);
    }

    /// The atoms, in a stable order, with ambient ones dropped — how a signature prints (§3.2).
    pub fn visible(&self) -> Vec<Effect> {
        self.atoms
            .iter()
            .filter(|e| !e.is_ambient())
            .cloned()
            .collect()
    }
}

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let atoms: Vec<String> = self.atoms.iter().map(|e| e.name()).collect();
        let tails: Vec<String> = self.tails.iter().map(|v| format!("e{v}")).collect();
        match (atoms.is_empty(), tails.is_empty()) {
            (true, true) => f.write_str("{}"),
            (false, true) => write!(f, "{{{}}}", atoms.join(", ")),
            (true, false) => write!(f, "{{{}}}", tails.join(" | ")),
            (false, false) => write!(f, "{{{} | {}}}", atoms.join(", "), tails.join(" | ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atoms_round_trip_through_their_written_form() {
        for text in [
            "ingress",
            "durable",
            "dom",
            "nondet",
            "net.out(api.example.com)",
            "net.in",
            "fs.read(/var/lib/beck)",
            "fs.write(/var/lib/beck)",
            "env",
            "spawn",
            "cap.session",
            "partial",
            "external.read(legacy)",
            "external.write(legacy)",
            "log",
            "metrics",
        ] {
            let e = Effect::parse(text).unwrap_or_else(|| panic!("`{text}` should parse"));
            assert_eq!(e.name(), text, "`{text}` must print as it parsed");
        }
        assert!(Effect::parse("teleport").is_none());
    }

    #[test]
    fn only_log_and_metrics_are_ambient() {
        // §3.2 also lists `time` and `rand`. Phase 2 does not, and `Effect::Nondet` says why:
        // whether a tier discharges them *is* the determinism rule, so they cannot be elided.
        assert!(Effect::Ambient(Ambient::Log).is_ambient());
        assert!(Effect::Ambient(Ambient::Metrics).is_ambient());
        assert!(!Effect::Nondet.is_ambient());
        let row = Row::of([Effect::Durable, Effect::Ambient(Ambient::Log)]);
        assert_eq!(row.visible(), vec![Effect::Durable]);
    }

    #[test]
    fn a_row_prints_its_variables_after_a_bar() {
        assert_eq!(Row::empty().to_string(), "{}");
        assert_eq!(Row::of([Effect::Dom]).to_string(), "{dom}");
        assert_eq!(Row::var(3).to_string(), "{e3}");
        assert_eq!(
            Row::of([Effect::Dom]).union(&Row::var(3)).to_string(),
            "{dom | e3}"
        );
    }

    #[test]
    fn union_is_idempotent_and_commutative() {
        let a = Row::of([Effect::Dom]).union(&Row::var(1));
        let b = Row::of([Effect::Durable]).union(&Row::var(1));
        assert_eq!(a.clone().union(&b), b.union(&a));
        assert_eq!(a.clone().union(&a), a);
    }
}
