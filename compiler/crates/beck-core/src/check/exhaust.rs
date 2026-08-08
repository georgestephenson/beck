//! Is this `match` exhaustive, and if not, what is missing?
//!
//! [`docs/03-type-and-effect-system.md`](../../../../../../docs/03-type-and-effect-system.md) §3.1:
//!
//! > a fold over a `union Event` that misses a case is a compile error — this single check carries
//! > the migration story
//!
//! Until nested patterns existed this was a set: each arm named a variant, the check asked which
//! declared variants were missing from the set, and a list was two shapes rather than a union.
//! `case Some(Added(id))` breaks that outright — it names `Some` and does not cover it — so the
//! answer stops being a lookup and becomes Maranget's *usefulness* question (**Warnings for pattern
//! matching**, JFP 2007, §3.1): given the arms already written, is there a value none of them
//! matches?
//!
//! # The algorithm, in the terms this compiler uses
//!
//! `useful(matrix, ty)` asks whether some value of `ty` escapes every row of a matrix of pattern
//! *vectors*. A `match` is exhaustive exactly when a single wildcard is **not** useful against its
//! arms. The recursion is three cases:
//!
//! * the first column has a **complete** set of constructors (every variant of a union, or both
//!   shapes of a list) — recurse once per constructor into the specialised matrix, and the match is
//!   exhaustive only if all of them are;
//! * the first column is **missing** a constructor — that missing one, with wildcards under it, is
//!   the counterexample, and it is what the diagnostic prints;
//! * the first column is all wildcards — recurse on what is left.
//!
//! # Lists are `nil` and `cons`, here and nowhere else
//!
//! A [`Pattern::List`] is flat: `items` fixed elements and an optional tail. That is the right
//! shape for the evaluator, which can check a length and index; it is the wrong shape for this,
//! because "exactly two" and "at least one" do not partition anything. So a list pattern is
//! *viewed* as `nil` / `cons(head, tail)` for the length of this check and nowhere else —
//! `[a, b]` is `cons(a, cons(b, nil))`, `[a, *r]` is `cons(a, _)`. Two constructors that partition
//! a list is what makes the same three cases above work for lists and unions alike.
//!
//! # What it deliberately cannot prove
//!
//! An `Int`, a `Str` and a `Float` have constructors nobody can enumerate, so a column of literal
//! patterns is never complete and the counterexample is "some other value". `case 0` and `case 1`
//! over an `Int` need a wildcard, and saying otherwise would need a range analysis this language
//! has no use for yet.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::core::Pattern;
use crate::ty::{Ty, TyDecl};

/// The program's declarations, which is all this check needs of the checker.
pub type Types = std::collections::BTreeMap<Arc<str>, TyDecl>;

/// One constructor a value of some type can be built with.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Ctor {
    /// A union variant, by name.
    Variant(Arc<str>),
    /// The empty list.
    Nil,
    /// A list with at least one element.
    Cons,
    /// A literal, keyed by how it prints. Never part of a complete set — see the module docs — so
    /// nothing here needs to order two `Const`s, which a `Float` would not let it do anyway.
    Lit(String),
}

impl Ctor {
    /// How many sub-patterns this constructor holds, given the union's declaration.
    fn arity(&self, fields: &[Arc<str>]) -> usize {
        match self {
            Ctor::Variant(_) => fields.len(),
            Ctor::Cons => 2,
            Ctor::Nil | Ctor::Lit(_) => 0,
        }
    }
}

/// A pattern, seen as a constructor applied to sub-patterns — or as a wildcard.
///
/// This is where a flat list pattern becomes a `cons` chain, and where a binder stops being
/// interesting: for exhaustiveness `Bind` and `Wildcard` are the same pattern.
fn view(p: &Pattern, fields: &[Arc<str>]) -> Option<(Ctor, Vec<Pattern>)> {
    match p {
        Pattern::Wildcard | Pattern::Bind(_) => None,
        // An or-pattern is several rows rather than one constructor, and [`heads`] has already
        // made them several by the time anything asks: both functions that inspect column zero
        // expand it first. `alternatives_are_rows_before_anything_views_them` is the test that
        // says so, because a `None` here would quietly read an or-pattern as a wildcard and call
        // a `match` exhaustive that is not.
        Pattern::Or(_) => None,
        Pattern::Const(k) => Some((Ctor::Lit(format!("{k:?}")), Vec::new())),
        Pattern::Ctor { variant, binds } => {
            // Written by name or by position, a pattern may bind a subset of the fields; the ones
            // it does not name match anything.
            let subs = fields
                .iter()
                .map(|f| {
                    binds
                        .iter()
                        .find(|(n, _)| n == f)
                        .map(|(_, p)| p.clone())
                        .unwrap_or(Pattern::Wildcard)
                })
                .collect();
            Some((Ctor::Variant(variant.clone()), subs))
        }
        Pattern::List { items, rest } => match items.split_first() {
            None => match rest {
                // `[*r]` matches every list, so it is a wildcard rather than a constructor.
                Some(_) => None,
                None => Some((Ctor::Nil, Vec::new())),
            },
            Some((head, tail)) => Some((
                Ctor::Cons,
                vec![
                    head.clone(),
                    Pattern::List {
                        items: tail.to_vec(),
                        rest: *rest,
                    },
                ],
            )),
        },
    }
}

/// Split any or-pattern in column zero into one row per alternative.
///
/// Lazy rather than a full or-normal form: only column zero is ever inspected, and specialising
/// into a constructor puts that constructor's sub-patterns at column zero of the next matrix,
/// where this runs again. Distributing every nested alternative up front would be exponential in
/// the number of them, which is a cost with no reader.
fn heads(rows: &[Vec<Pattern>]) -> Vec<Vec<Pattern>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(first) = row.first() else {
            out.push(row.clone());
            continue;
        };
        let mut stack = vec![first.clone()];
        while let Some(p) = stack.pop() {
            match p {
                Pattern::Or(alts) => stack.extend(alts),
                other => {
                    let mut next = vec![other];
                    next.extend_from_slice(&row[1..]);
                    out.push(next);
                }
            }
        }
    }
    out
}

/// What the columns of a matrix are matching.
#[derive(Clone)]
struct Column {
    ty: Ty,
    /// The declared field order of the union this column's type is, if it is one — so a `Ctor`
    /// pattern's sub-patterns can be put in a fixed order.
    fields: Vec<Arc<str>>,
}

/// The answer: either the match covers everything, or here are the values it does not.
pub enum Coverage {
    Exhaustive,
    /// One shape per uncovered constructor, printed as the source a reader would write.
    Missing(Vec<String>),
}

/// Is `arms` exhaustive for a scrutinee of type `ty`?
///
/// The top level is asked one constructor at a time rather than all at once, so the diagnostic can
/// keep saying "missing: `Added`, `Removed`" the way it has since Phase 1 — a list of what to write
/// is more use than the single counterexample the algorithm naturally produces. Below the top level
/// there is one witness, and it is a whole value: `Some(Removed(_))`, not "some `Some`".
pub fn coverage(arms: &[Pattern], ty: &Ty, types: &Types) -> Coverage {
    let ctx = Ctx { types };
    // Expanded here and not only inside [`Ctx::missing`], because this function specialises the
    // top-level matrix itself: an or-pattern that reached `view` unexpanded would be read as a
    // wildcard, and a `match` covering two of three variants would be called exhaustive.
    let rows = heads(
        &arms
            .iter()
            .map(|p| vec![p.clone()])
            .collect::<Vec<Vec<Pattern>>>(),
    );
    let col = ctx.column(ty);
    let Some(sig) = ctx.signature(&col.ty) else {
        return match ctx.missing(&rows, &[col]) {
            None => Coverage::Exhaustive,
            Some(w) => Coverage::Missing(vec![w
                .first()
                .map(|w| w.describe(true))
                .unwrap_or_else(|| "_".to_string())]),
        };
    };
    let mut missing = Vec::new();
    for ctor in sig {
        let subs = ctx.sub_columns(&ctor, &col);
        let spec = ctx.specialise(&rows, &ctor, &col, subs.len());
        if let Some(w) = ctx.missing(&spec, &subs) {
            let mut whole = ctx.rebuild(&ctor, w, subs.len());
            missing.push(whole.remove(0).describe(true));
        }
    }
    if missing.is_empty() {
        Coverage::Exhaustive
    } else {
        Coverage::Missing(missing)
    }
}

/// Which arms can never match, because the arms above them already cover everything they do.
///
/// The same question as [`coverage`] and Maranget's paper answers both: an arm is reachable
/// exactly when it is *useful* against the arms before it — when some value it matches escapes all
/// of them. Nested patterns are what makes this worth having: `case Some(Circle(r))` followed by
/// `case Some(_)` followed by `case Some(Square(s))` is a program whose third arm is dead, and
/// nothing above the pattern language can see that.
///
/// Reported as a warning rather than an error, because `case _:` written after every variant of a
/// union is a habit rather than a mistake, and turning a habit into a build failure is a change to
/// what compiles rather than a diagnostic.
pub fn unreachable(arms: &[Pattern], ty: &Ty, types: &Types) -> Vec<usize> {
    let ctx = Ctx { types };
    let col = ctx.column(ty);
    let mut out = Vec::new();
    for i in 0..arms.len() {
        let above: Vec<Vec<Pattern>> = arms[..i].iter().map(|p| vec![p.clone()]).collect();
        // Useful means: some value *this* arm matches escapes every arm above it. Asking whether
        // the arm's own pattern escapes is the same question with the arm as the value.
        if !ctx.escapes(
            &above,
            std::slice::from_ref(&col),
            std::slice::from_ref(&arms[i]),
        ) {
            out.push(i);
        }
    }
    out
}

struct Ctx<'a> {
    types: &'a Types,
}

/// A counterexample, as a tree that can be printed back as source.
#[derive(Clone)]
enum Witness {
    Any,
    Ctor(Arc<str>, Vec<Witness>),
    Nil,
    /// A head and a tail, so a counterexample can be a list of a *length* — `[_]` when the only
    /// arm is `case [a, b]` — rather than the vaguer "a list with elements".
    Cons(Box<Witness>, Box<Witness>),
    Other,
}

impl Witness {
    /// A `cons` chain written back as the list literal it stands for. A tail that is not itself a
    /// `cons` or the empty list is `*rest`: the counterexample is every list at least this long.
    fn list_form(&self) -> String {
        let mut items = Vec::new();
        let mut cur = self;
        loop {
            match cur {
                Witness::Cons(head, tail) => {
                    items.push(head.describe(false));
                    cur = tail;
                }
                Witness::Nil => return format!("[{}]", items.join(", ")),
                _ => {
                    items.push("*rest".to_string());
                    return format!("[{}]", items.join(", "));
                }
            }
        }
    }

    /// How the diagnostic says it.
    ///
    /// `top` is whether this is the whole missing value rather than a part of one, and it buys the
    /// two list shapes their prose: at the top of a `match` on a list, "the empty list — `case []`"
    /// is what a reader needs, and inside `Some(…)` the same sentence would be about the wrong
    /// thing. A constructor whose parts are all wildcards prints as its bare name, so a `match` on
    /// a union that misses a variant says what it always said.
    fn describe(&self, top: bool) -> String {
        match self {
            Witness::Any => "_".to_string(),
            Witness::Other => "a value no arm names".to_string(),
            Witness::Nil if top => "the empty list — `case []`".to_string(),
            Witness::Nil => "[]".to_string(),
            Witness::Cons(head, tail)
                if top && matches!(**head, Witness::Any) && matches!(**tail, Witness::Any) =>
            {
                "a list with elements — `case [first, *rest]`".to_string()
            }
            Witness::Cons(..) => self.list_form(),
            Witness::Ctor(name, subs)
                if subs.is_empty() || subs.iter().all(|s| matches!(s, Witness::Any)) =>
            {
                name.to_string()
            }
            Witness::Ctor(name, subs) => {
                let inner: Vec<String> = subs.iter().map(|s| s.describe(false)).collect();
                format!("{name}({})", inner.join(", "))
            }
        }
    }
}

impl Ctx<'_> {
    fn column(&self, ty: &Ty) -> Column {
        let fields = match ty.con_name().and_then(|c| self.types.get(c)) {
            Some(TyDecl::Union { variants, .. }) => {
                // Every field name any variant declares, in declaration order: one column layout
                // for the whole union, so specialising by variant can index it.
                let mut seen: Vec<Arc<str>> = Vec::new();
                for v in variants {
                    for (n, _) in &v.fields {
                        if !seen.contains(n) {
                            seen.push(n.clone());
                        }
                    }
                }
                seen
            }
            _ => Vec::new(),
        };
        Column {
            ty: ty.clone(),
            fields,
        }
    }

    /// The declared constructors of a type, when they can be enumerated.
    fn signature(&self, ty: &Ty) -> Option<Vec<Ctor>> {
        match ty.con_name() {
            Some(Ty::LIST) => Some(vec![Ctor::Nil, Ctor::Cons]),
            Some(c) => match self.types.get(c) {
                Some(TyDecl::Union { variants, .. }) => Some(
                    variants
                        .iter()
                        .map(|v| Ctor::Variant(v.name.clone()))
                        .collect(),
                ),
                _ => None,
            },
            None => None,
        }
    }

    /// The types the sub-patterns of one constructor match.
    fn sub_columns(&self, ctor: &Ctor, col: &Column) -> Vec<Column> {
        match ctor {
            Ctor::Nil | Ctor::Lit(_) => Vec::new(),
            Ctor::Cons => {
                let elem = elem_of(&col.ty);
                vec![self.column(&elem), self.column(&col.ty)]
            }
            Ctor::Variant(name) => {
                let tys = self.variant_fields(&col.ty, name);
                col.fields
                    .iter()
                    .map(|f| {
                        self.column(
                            &tys.iter()
                                .find(|(n, _)| n == f)
                                .map(|(_, t)| t.clone())
                                .unwrap_or_else(Ty::unit),
                        )
                    })
                    .collect()
            }
        }
    }

    /// A variant's declared fields, with the scrutinee's own type arguments substituted in — so
    /// matching `Leaf(v)` against a `Tree[Str]` recurses into a `Str` and not into `T`.
    fn variant_fields(&self, ty: &Ty, variant: &str) -> Vec<(Arc<str>, Ty)> {
        let Some(TyDecl::Union { variants, .. }) = ty.con_name().and_then(|c| self.types.get(c))
        else {
            return Vec::new();
        };
        let args: Vec<Ty> = match ty {
            Ty::Con(_, args) => args.clone(),
            _ => Vec::new(),
        };
        let Some(v) = variants.iter().find(|v| v.name.as_ref() == variant) else {
            return Vec::new();
        };
        v.fields
            .iter()
            .map(|(n, t)| (n.clone(), crate::ty::instantiate_decl(t, &args)))
            .collect()
    }

    /// Does some value matched by `q` escape every row of `rows`?
    ///
    /// [`Ctx::missing`] is this with `q` all wildcards; the general form is what an unreachable-arm
    /// check needs, since the question there is about the arm's own pattern rather than about any
    /// value at all.
    fn escapes(&self, rows: &[Vec<Pattern>], cols: &[Column], q: &[Pattern]) -> bool {
        let rows = &heads(rows);
        if cols.is_empty() {
            return rows.is_empty();
        }
        // The *query* may be an or-pattern too — `case A | B` asks whether either alternative
        // escapes — so it is expanded the same way and the arm is reachable if any of them is.
        let split = heads(std::slice::from_ref(&q.to_vec()));
        if split.len() > 1 {
            return split.iter().any(|q| self.escapes(rows, cols, q));
        }
        let q = split.first().map(|v| v.as_slice()).unwrap_or(q);
        let head = &cols[0];
        match view(&q[0], &head.fields) {
            // `q` names a constructor: only the rows that could match it matter.
            Some((ctor, mut subs)) => {
                let sub_cols = self.sub_columns(&ctor, head);
                subs.resize(sub_cols.len(), Pattern::Wildcard);
                let mut next_cols = sub_cols.clone();
                next_cols.extend_from_slice(&cols[1..]);
                let mut next_q = subs;
                next_q.extend_from_slice(&q[1..]);
                self.escapes(
                    &self.specialise(rows, &ctor, head, sub_cols.len()),
                    &next_cols,
                    &next_q,
                )
            }
            // `q` is a wildcard here: it escapes if any constructor of this column does, and a
            // column whose constructors are not all written escapes through the ones that are not.
            None => {
                let present: BTreeSet<Ctor> = rows
                    .iter()
                    .filter_map(|r| view(&r[0], &head.fields).map(|(c, _)| c))
                    .collect();
                let complete = match self.signature(&head.ty) {
                    Some(sig) => sig.iter().all(|c| present.contains(c)).then_some(sig),
                    None => None,
                };
                match complete {
                    Some(sig) => sig.into_iter().any(|ctor| {
                        let sub_cols = self.sub_columns(&ctor, head);
                        let mut next_cols = sub_cols.clone();
                        next_cols.extend_from_slice(&cols[1..]);
                        let mut next_q = vec![Pattern::Wildcard; sub_cols.len()];
                        next_q.extend_from_slice(&q[1..]);
                        self.escapes(
                            &self.specialise(rows, &ctor, head, sub_cols.len()),
                            &next_cols,
                            &next_q,
                        )
                    }),
                    None => self.escapes(&self.default(rows), &cols[1..], &q[1..]),
                }
            }
        }
    }

    /// The heart of it: a value of these columns that no row matches, if there is one.
    fn missing(&self, rows: &[Vec<Pattern>], cols: &[Column]) -> Option<Vec<Witness>> {
        let rows = &heads(rows);
        if cols.is_empty() {
            // No columns left: the value is fully described. It escapes only if no row remains.
            return rows.is_empty().then(Vec::new);
        }
        // A row that is *shorter* than the columns cannot happen — every row is built from one
        // pattern per column — so an empty matrix with columns left means everything escapes.
        if rows.is_empty() {
            return Some(cols.iter().map(|_| Witness::Any).collect());
        }

        let head = &cols[0];
        let present: BTreeSet<Ctor> = rows
            .iter()
            .filter_map(|r| view(&r[0], &head.fields).map(|(c, _)| c))
            .collect();

        let complete = match self.signature(&head.ty) {
            Some(sig) => sig.iter().all(|c| present.contains(c)).then_some(sig),
            None => None,
        };

        match complete {
            // Every constructor is written, so a value escapes only under one of them.
            Some(sig) => {
                for ctor in sig {
                    let subs = self.sub_columns(&ctor, head);
                    let mut next = subs.clone();
                    next.extend_from_slice(&cols[1..]);
                    if let Some(w) =
                        self.missing(&self.specialise(rows, &ctor, head, subs.len()), &next)
                    {
                        return Some(self.rebuild(&ctor, w, subs.len()));
                    }
                }
                None
            }
            // Some constructor is unwritten — or the type has no enumerable ones, and then it is a
            // literal column and "some other value" is the honest witness.
            None => {
                let rest = self.missing(&self.default(rows), &cols[1..])?;
                let mut out = vec![self.unwritten(&present, head)];
                out.extend(rest);
                Some(out)
            }
        }
    }

    /// The rows that can match `ctor`, with its sub-patterns spliced in at the front.
    fn specialise(
        &self,
        rows: &[Vec<Pattern>],
        ctor: &Ctor,
        col: &Column,
        arity: usize,
    ) -> Vec<Vec<Pattern>> {
        let mut out = Vec::new();
        for row in rows {
            let subs = match view(&row[0], &col.fields) {
                Some((c, subs)) if &c == ctor => subs,
                // A wildcard matches this constructor with wildcards underneath.
                None => vec![Pattern::Wildcard; arity],
                Some(_) => continue,
            };
            let mut next = subs;
            next.resize(arity, Pattern::Wildcard);
            next.extend_from_slice(&row[1..]);
            out.push(next);
        }
        out
    }

    /// The rows whose first pattern is a wildcard, with that column dropped.
    fn default(&self, rows: &[Vec<Pattern>]) -> Vec<Vec<Pattern>> {
        rows.iter()
            .filter(|r| r[0].irrefutable() || matches!(r[0], Pattern::Bind(_)))
            .map(|r| r[1..].to_vec())
            .collect()
    }

    /// A constructor of this type that no row wrote, as the witness to print.
    ///
    /// A column nothing has written a constructor in is a column of wildcards, and the witness for
    /// it is `_` rather than "a value no arm names" — the escape is somewhere else, and naming this
    /// column would point the reader at the one part of the value that is not the problem.
    fn unwritten(&self, present: &BTreeSet<Ctor>, col: &Column) -> Witness {
        if present.is_empty() {
            return Witness::Any;
        }
        let Some(sig) = self.signature(&col.ty) else {
            return Witness::Other;
        };
        match sig.into_iter().find(|c| !present.contains(c)) {
            Some(Ctor::Variant(name)) => {
                let arity = Ctor::Variant(name.clone()).arity(&col.fields);
                Witness::Ctor(name, vec![Witness::Any; arity.min(col.fields.len())])
            }
            Some(Ctor::Nil) => Witness::Nil,
            Some(Ctor::Cons) => Witness::Cons(Box::new(Witness::Any), Box::new(Witness::Any)),
            _ => Witness::Other,
        }
    }

    /// Put a witness for `ctor`'s sub-values back together into a witness for the value itself.
    fn rebuild(&self, ctor: &Ctor, mut sub: Vec<Witness>, arity: usize) -> Vec<Witness> {
        let rest = sub.split_off(arity.min(sub.len()));
        let here = match ctor {
            Ctor::Variant(name) => Witness::Ctor(name.clone(), sub),
            Ctor::Nil => Witness::Nil,
            Ctor::Cons => {
                let mut it = sub.into_iter();
                let head = it.next().unwrap_or(Witness::Any);
                let tail = it.next().unwrap_or(Witness::Any);
                Witness::Cons(Box::new(head), Box::new(tail))
            }
            Ctor::Lit(_) => Witness::Other,
        };
        let mut out = vec![here];
        out.extend(rest);
        out
    }
}

fn elem_of(ty: &Ty) -> Ty {
    match ty {
        Ty::Con(_, args) if !args.is_empty() => args[0].clone(),
        _ => Ty::unit(),
    }
}
