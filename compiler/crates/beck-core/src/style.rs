//! What can reach a `class=`, enumerated.
//!
//! [`docs/104-styling-and-the-component-library.md`](../../../../../docs/104-styling-and-the-component-library.md)
//! §104.4:
//!
//! > The compiler already knows every string that can reach a `class=` attribute, across imported
//! > modules, because it resolved them. […] **Exact extraction.** No false positives (a
//! > `def truncate` is a definition, not a token), no false negatives across a module boundary, and
//! > no configuration.
//!
//! That is the claim; this is the analysis behind it. It answers two questions about a program and
//! neither of them is about Tailwind: **which class names can this page carry**, and **where does
//! the program build one the compiler cannot know**. The first is what a stylesheet emitter needs
//! (§8.5.4's next styling item). The second is what makes the first honest, because a scanner that
//! silently misses a name produces a page missing a rule, which is the failure §104.3 measured in
//! Tailwind's own scanner over this tree.
//!
//! # Why a list is the shape that can be enumerated
//!
//! `class=["btn", "primary" if hot else "plain"]` has four leaves and every one is a literal, so the
//! set is `{btn, primary, plain}` and the analysis is a fold over the tree. `class="btn " + variant`
//! has one leaf that is a value, and no analysis recovers what it can hold. The two are the same
//! page and only one of them can be styled without a safelist, which is the whole reason
//! [`beck_macro`]'s `ui:` lowering learned to take a list.
//!
//! **So this module refuses rather than guesses**, and says which of the two a program wrote. A
//! refusal here is not an error: nothing is rejected, and the caller decides what to do with a site
//! it cannot enumerate. `beck explain style` prints them, and the emitter that follows will need a
//! deliberate escape hatch for the genuine cases (§104.4's `@style(dynamic)`), which is a decision
//! rather than a default.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_diag::Span;

use crate::check::{Def, Program};
use crate::core::{children, Const, Core, CoreKind, Prim};

/// How far a class expression is followed through named definitions.
///
/// `examples/todo.beck` writes `class=done_class(t)` and the body one call away is the `if` whose
/// two arms are the answer, which is the depth this needs and the shape §104.4 predicted programs
/// would already be written in. A bound rather than a budget: what it stops is a cycle of mutually
/// recursive definitions, not a slow analysis.
const DEPTH: usize = 8;

/// Every class a program's pages can carry, and every place one could not be worked out.
#[derive(Clone, Debug, Default)]
pub struct Styles {
    /// The class names, deduplicated and in order. A `class=""` contributes nothing rather than an
    /// empty token, because that is what the browser does with it.
    pub classes: BTreeSet<Arc<str>>,
    /// The `class=` sites whose value is not enumerable, in the order they were found.
    pub dynamic: Vec<Dynamic>,
}

/// One `class=` the analysis could not work out, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dynamic {
    /// The definition the site is in, so a reader can find it without a span map.
    pub in_def: Arc<str>,
    pub span: Span,
    pub because: Because,
}

/// Why a class expression could not be enumerated.
///
/// Three, and they are distinguished because a reader does something different about each: the
/// first is a rewrite the language already invites, the second is a design question about where the
/// name comes from, and the third is a limit of this analysis rather than of the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Because {
    /// A string built with `+`, so the name exists only at run time.
    Concatenated,
    /// A value computed rather than named — a field, a parameter, a lookup, a call to a
    /// primitive.
    FromData,
    /// A shape this analysis does not enter: a call through a value, a `let`-bound name, a
    /// definition deeper than the analysis follows.
    NotFollowed,
}

impl Because {
    /// The sentence a diagnostic or a report prints, in the voice the rest of `beck explain` uses.
    pub fn because(&self) -> &'static str {
        match self {
            Because::Concatenated => {
                "it is built with `+`, so the name exists only while the page is being rendered. A \
                 list of alternatives is the shape that can be styled: `class=[\"btn\", \"primary\" \
                 if hot else \"plain\"]`"
            }
            Because::FromData => {
                "it is computed rather than named — a field, a lookup, a call to a primitive — so \
                 the names it can take are in the data rather than in the program"
            }
            Because::NotFollowed => {
                "it is behind a shape this analysis does not enter — a call through a value, a \
                 `let`-bound name, or a chain of definitions deeper than it follows"
            }
        }
    }
}

/// Enumerate every class a program's `class=` attributes can carry.
pub fn classes(program: &Program) -> Styles {
    let mut out = Styles::default();
    for (name, def) in &program.defs {
        sites(&def.body, name, &program.defs, &mut out);
    }
    out
}

/// Find every `html_attr("class", …)` under an expression and enumerate each one's value.
fn sites(c: &Core, in_def: &Arc<str>, defs: &BTreeMap<Arc<str>, Def>, out: &mut Styles) {
    if let CoreKind::Prim {
        op: Prim::HtmlAttr,
        args,
    } = &c.kind
    {
        if args.len() == 2 && is_class(&args[0]) {
            let mut seen = BTreeSet::new();
            if let Err(because) = tokens(&args[1], defs, &mut seen, DEPTH, &mut out.classes) {
                out.dynamic.push(Dynamic {
                    in_def: in_def.clone(),
                    span: args[1].span,
                    because,
                });
            }
        }
    }
    for child in children(c) {
        sites(child, in_def, defs, out);
    }
}

/// Whether this expression is the literal attribute name `class`.
fn is_class(c: &Core) -> bool {
    matches!(&c.kind, CoreKind::Const(Const::Str(s)) if &**s == "class")
}

/// The set of class names an expression can evaluate to, or why that set is not knowable.
fn tokens(
    c: &Core,
    defs: &BTreeMap<Arc<str>, Def>,
    seen: &mut BTreeSet<Arc<str>>,
    depth: usize,
    out: &mut BTreeSet<Arc<str>>,
) -> Result<(), Because> {
    if depth == 0 {
        return Err(Because::NotFollowed);
    }
    match &c.kind {
        // A literal, split the way the browser splits it: `class="a b"` is two tokens, and an
        // empty string is none rather than one empty one.
        CoreKind::Const(Const::Str(s)) => {
            for token in s.split_whitespace() {
                out.insert(Arc::from(token));
            }
            Ok(())
        }
        // What the `ui:` lowering makes of a list, and the list itself.
        CoreKind::Prim {
            op: Prim::StrJoin,
            args,
        } if args.len() == 2 => tokens(&args[0], defs, seen, depth - 1, out),
        CoreKind::ListLit(items) => items
            .iter()
            .try_for_each(|i| tokens(i, defs, seen, depth - 1, out)),
        // Every branch can happen, so every branch contributes. The condition cannot reach the
        // attribute and is not looked at.
        CoreKind::If { then, alt, .. } => {
            tokens(then, defs, seen, depth - 1, out)?;
            tokens(alt, defs, seen, depth - 1, out)
        }
        CoreKind::Match { arms, .. } => arms
            .iter()
            .try_for_each(|a| tokens(&a.body, defs, seen, depth - 1, out)),
        // A definition, whether it is named or called. The arguments are not followed: if the
        // answer depended on one, the body would read a parameter and this would refuse there.
        CoreKind::Global(name) => follow(name, defs, seen, depth, out),
        CoreKind::App { func, .. } => match &func.kind {
            CoreKind::Global(name) => follow(name, defs, seen, depth, out),
            CoreKind::Lam { body, .. } => tokens(body, defs, seen, depth - 1, out),
            _ => Err(Because::NotFollowed),
        },
        CoreKind::Prim {
            op: Prim::Add,
            args,
        } if args.len() == 2 => {
            // Two literals added is still two literals, and constant-folding is not this module's
            // job — but the *reason* a reader gets should name the concatenation rather than the
            // shape underneath it, so this arm exists to say `Concatenated` and not `NotFollowed`.
            let _ = args;
            Err(Because::Concatenated)
        }
        // Everything else that answers with a value rather than with a name. A primitive is here
        // rather than under `NotFollowed` because there is nothing to follow: `str_upper(x)` has an
        // answer and the answer is not in the program.
        CoreKind::Var(_) | CoreKind::Field { .. } | CoreKind::Prim { .. } => Err(Because::FromData),
        _ => Err(Because::NotFollowed),
    }
}

/// Follow a named definition once, refusing a cycle rather than chasing it.
fn follow(
    name: &Arc<str>,
    defs: &BTreeMap<Arc<str>, Def>,
    seen: &mut BTreeSet<Arc<str>>,
    depth: usize,
    out: &mut BTreeSet<Arc<str>>,
) -> Result<(), Because> {
    if !seen.insert(name.clone()) {
        return Err(Because::NotFollowed);
    }
    let Some(def) = defs.get(name) else {
        return Err(Because::NotFollowed);
    };
    // A definition's body is a `Lam` when it takes arguments; what answers is what it returns.
    let body = match &def.body.kind {
        CoreKind::Lam { body, .. } => body,
        _ => &def.body,
    };
    let answer = tokens(body, defs, seen, depth - 1, out);
    seen.remove(name);
    answer
}
