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
use std::fmt::Write;
use std::sync::Arc;

use beck_diag::{Diagnostic, Diagnostics, Span};

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
    /// Every literal that reaches a `class=`, with where it was written.
    ///
    /// [`Styles::classes`] is the set a sheet is emitted from and has no positions, because a name
    /// written in three places is one rule. This is the other question — *where did this name come
    /// from* — and it is what a diagnostic needs, so it keeps one entry per site rather than one
    /// per name.
    pub named: Vec<Named>,
}

/// One literal class token, where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub class: Arc<str>,
    /// The definition the literal is in — which is not always the one the `class=` is in, because
    /// the analysis follows a call.
    pub in_def: Arc<str>,
    pub span: Span,
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
            if let Err(because) = tokens(&args[1], defs, &mut seen, DEPTH, in_def, out) {
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
    in_def: &Arc<str>,
    out: &mut Styles,
) -> Result<(), Because> {
    if depth == 0 {
        return Err(Because::NotFollowed);
    }
    match &c.kind {
        // A literal, split the way the browser splits it: `class="a b"` is two tokens, and an
        // empty string is none rather than one empty one.
        CoreKind::Const(Const::Str(s)) => {
            for token in s.split_whitespace() {
                let class: Arc<str> = Arc::from(token);
                out.classes.insert(class.clone());
                out.named.push(Named {
                    class,
                    in_def: in_def.clone(),
                    span: c.span,
                });
            }
            Ok(())
        }
        // What the `ui:` lowering makes of a list, and the list itself.
        CoreKind::Prim {
            op: Prim::StrJoin,
            args,
        } if args.len() == 2 => tokens(&args[0], defs, seen, depth - 1, in_def, out),
        CoreKind::ListLit(items) => items
            .iter()
            .try_for_each(|i| tokens(i, defs, seen, depth - 1, in_def, out)),
        // Every branch can happen, so every branch contributes. The condition cannot reach the
        // attribute and is not looked at.
        CoreKind::If { then, alt, .. } => {
            tokens(then, defs, seen, depth - 1, in_def, out)?;
            tokens(alt, defs, seen, depth - 1, in_def, out)
        }
        CoreKind::Match { arms, .. } => arms
            .iter()
            .try_for_each(|a| tokens(&a.body, defs, seen, depth - 1, in_def, out)),
        // A definition, whether it is named or called. The arguments are not followed: if the
        // answer depended on one, the body would read a parameter and this would refuse there.
        CoreKind::Global(name) => follow(name, defs, seen, depth, out),
        CoreKind::App { func, .. } => match &func.kind {
            CoreKind::Global(name) => follow(name, defs, seen, depth, out),
            CoreKind::Lam { body, .. } => tokens(body, defs, seen, depth - 1, in_def, out),
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
    out: &mut Styles,
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
    // The literals are in *this* definition, whatever called it: a diagnostic that pointed at
    // the caller would send a reader to a line with no class name on it.
    let answer = tokens(body, defs, seen, depth - 1, name, out);
    seen.remove(name);
    answer
}

// -------------------------------------------------------------------------------------------
// A misspelled utility
// -------------------------------------------------------------------------------------------

/// Warn about a class that is one slip away from a utility.
///
/// [`docs/104`](../../../../../docs/104-styling-and-the-component-library.md) §104.4: "a
/// misspelling is a diagnostic. `rounded-ful` gets a `B0…` with a did-you-mean, because
/// Levenshtein over a known table is what the compiler already does for field names. **This is the
/// whole difference between Tailwind and a language that absorbed it.**"
///
/// # Why it is a warning and not an error
///
/// The class vocabulary is **open**, which is what makes this different from `B0217` and `B0218`
/// one file over: every attribute must be an HTML attribute, and every event must be one the
/// client listens for, so an unknown one is wrong. An unknown *class* is not wrong — it is the
/// program's own name, and this tree has eight of them. So the compiler cannot say "this is a
/// mistake"; it can only say "this is one edit from something that would have had a rule", which
/// is a warning with a suggestion in it.
///
/// # The threshold, and the margin it has
///
/// Distance 1 always, and distance 2 from eight characters up. `rounded-ful`, `bg-emerald-550`,
/// `text-4xxl`, `flexx`, `font-mediumm` and `justify-arround` are all one edit from a real utility
/// and `items-centre` is two, so the misspellings are inside it. The other population is further
/// away than the rule needs: the nearest utility to any class this tree's own programs write is
/// **three** edits — `card` to `grid`, `here` to `h-px`, `mine` to `inline` — so the rule has a
/// margin of one rather than sitting on the boundary.
///
/// `style.rs::a_misspelled_utility_is_a_diagnostic` asserts the **margin** rather than the
/// outcome, which is the difference between a gate that can fail and one that cannot: a family
/// added to the table that lands two edits from somebody's own class name turns it red before
/// anybody's build starts warning about a name they chose.
pub fn check_classes(program: &Program, diags: &mut Diagnostics) {
    for site in &classes(program).named {
        if is_utility(&site.class) {
            continue;
        }
        let allowed = usize::from(site.class.len() >= 8) + 1;
        let Some((_, near)) = nearest_utility(&site.class).filter(|(d, _)| *d <= allowed) else {
            continue;
        };
        diags.push(
            Diagnostic::warning(
                "B0222",
                format!(
                    "`{}` is not a utility, and is one slip from one",
                    site.class
                ),
                site.span,
            )
            .with_primary_label("no rule is emitted for this class")
            .with_note(
                "a class this compiler does not know is a class of your own and is left alone — \
                 `beck explain style` lists which of a page's are which. This one is close enough \
                 to a utility to be worth asking about",
            )
            .with_fix(format!("did you mean `{near}`?")),
        );
    }
}

/// The utility nearest a name, and how far away it is.
///
/// Over [`enumerate`]'s closed names, which is the whole table but for the open scales: `p-2.75` is
/// a utility and no misspelling of it is close to a *name*, because the thing that went wrong there
/// is a number.
///
/// It answers with the distance rather than applying the threshold, so a caller can ask **how far**
/// — which is what turns [`check_classes`]'s gate from "did it warn" into "how much room does the
/// rule have", and the second is the one that can fail before a user notices.
pub fn nearest_utility(name: &str) -> Option<(usize, &'static str)> {
    let mut best: Option<((usize, usize), &'static str)> = None;
    for candidate in CLOSED.iter() {
        let d = distance(name, candidate);
        // Ties broken towards the candidate of the same length, because a substitution is a
        // likelier slip than a deletion: `bg-emerald-550` is one edit from `bg-emerald-50` and
        // from `bg-emerald-500`, and only the second is a shade somebody meant.
        let rank = (d, name.len().abs_diff(candidate.len()));
        if best.is_none_or(|(b, _)| rank < b) {
            best = Some((rank, candidate));
        }
    }
    best.map(|((d, _), name)| (d, name))
}

/// Levenshtein distance, iterative over one row.
///
/// The same measure `beck_macro::vocabulary` uses for attribute names and the checker for field
/// names, written again here rather than shared because the two crates do not depend on each other
/// in that direction and a distance function is six lines.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut corner = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (row[j] + 1)
                .min(row[j + 1] + 1)
                .min(corner + usize::from(x != *y));
            corner = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// Every closed name the table knows, built once.
static CLOSED: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| enumerate().0.into_iter().collect());

// -------------------------------------------------------------------------------------------
// Which names are utilities, and what they mean
// -------------------------------------------------------------------------------------------

/// One utility's CSS: where it sits, what it selects, and what it declares.
///
/// [`docs/104`](../../../../../docs/104-styling-and-the-component-library.md) §104.4 takes
/// Tailwind's **design system** and refuses its delivery mechanism. This is that design system, as
/// a total function from a name to a rule — which is the same shape Tailwind's own compiler has,
/// and the reason its output can be the oracle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The at-rules this sits inside, outermost first: `@media (width >= 48rem)`.
    pub at: Vec<&'static str>,
    /// The selector, with the class name escaped as CSS requires.
    pub selector: String,
    /// The declarations, in the order they are written.
    pub decls: Vec<(&'static str, String)>,
}

impl Rule {
    /// Every theme token this rule reads, so a sheet can define the ones it needs and no others.
    fn tokens(&self, out: &mut BTreeSet<&'static str>) {
        for (_, value) in &self.decls {
            let mut rest = value.as_str();
            while let Some(at) = rest.find("var(--") {
                rest = &rest[at + 4..];
                let end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .unwrap_or(rest.len());
                if let Some((name, _)) = THEME.iter().find(|(n, _)| *n == &rest[..end]) {
                    out.insert(name);
                }
                rest = &rest[end..];
            }
        }
    }
}

/// Whether this class is a Tailwind utility Beck knows.
///
/// **Defined as "there is a rule for it"**, which is not a tidiness: a predicate and a generator
/// that could disagree would put a class in a stylesheet with no rule under it, or refuse one the
/// emitter can render — and the first is a page missing a style with every gate green, which is the
/// failure the whole arrangement exists to prevent. There is one table and this reads it.
pub fn is_utility(name: &str) -> bool {
    rule(name).is_some()
}

/// The rule for one class, or `None` if it is not a utility this knows.
///
/// # It is a subset, and the gate measures which one
///
/// Tailwind's surface is enormous and this covers the families a page is actually built from —
/// layout, spacing, colour, type, borders, flex and grid — with the variants in front of them.
/// **What matters is the direction of the error.** A name this accepts must be one Tailwind emits
/// the same rule for; a name Tailwind refuses must be refused here. A name Tailwind accepts and
/// this does not is a *gap*, counted rather than tolerated silently, and
/// `style.rs::the_utility_table_agrees_with_tailwind` is where all three are asserted against
/// Tailwind's own output rather than against a table somebody typed in.
///
/// # What made a fixed table wrong, found by asking
///
/// Tailwind 4's spacing is multiplicative — `calc(var(--spacing) * n)` — so `p-2.75` and `gap-13.5`
/// are rules, and any list of steps would have refused them. So the spacing families here take a
/// number rather than a member of a set, which is a thing the oracle said and a person would not
/// have. Asking it about the *rule* rather than only about the name said three more: `1` is
/// `var(--spacing)` and not `calc(var(--spacing) * 1)`, `0` is `0px` rather than `0`, and `auto` is
/// a padding value in no family at all — which this table used to accept in seventeen names the
/// candidate list had never asked about.
pub fn rule(name: &str) -> Option<Rule> {
    let mut parts: Vec<&str> = name.split(':').collect();
    let base = parts.pop()?;
    let mut at = Vec::new();
    let mut pseudo = String::new();
    for v in &parts {
        let (media, suffix) = variant(v)?;
        at.extend(media);
        pseudo.push_str(suffix);
    }
    let (decls, shape) = base_rule(base)?;
    let class = format!(".{}{pseudo}", escape(name));
    Some(Rule {
        at,
        selector: match shape {
            // `space-x-4` spaces an element's *children*, so the rule selects them rather than it.
            Shape::Between => format!(":where({class} > :not(:last-child))"),
            Shape::Element => class,
        },
        decls,
    })
}

/// What a utility's rule selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// The element carrying the class.
    Element,
    /// Every child of it but the last — `space-x` and `space-y`, which are a gap written as a
    /// margin so that it collapses at the edge.
    Between,
}

/// A variant, as the at-rule it opens and the pseudo-class it appends.
///
/// Stacking is Tailwind's own and so is the order: the leftmost variant is the outermost at-rule,
/// and every pseudo-class is appended to the one selector.
fn variant(v: &str) -> Option<(Option<&'static str>, &'static str)> {
    Some(match v {
        "hover" => (Some("@media (hover: hover)"), ":hover"),
        "focus" => (None, ":focus"),
        "focus-visible" => (None, ":focus-visible"),
        "focus-within" => (None, ":focus-within"),
        "active" => (None, ":active"),
        "visited" => (None, ":visited"),
        "disabled" => (None, ":disabled"),
        "checked" => (None, ":checked"),
        "first" => (None, ":first-child"),
        "last" => (None, ":last-child"),
        "odd" => (None, ":nth-child(odd)"),
        "even" => (None, ":nth-child(even)"),
        "empty" => (None, ":empty"),
        "dark" => (Some("@media (prefers-color-scheme: dark)"), ""),
        "motion-safe" => (Some("@media (prefers-reduced-motion: no-preference)"), ""),
        "motion-reduce" => (Some("@media (prefers-reduced-motion: reduce)"), ""),
        "print" => (Some("@media print"), ""),
        "sm" => (Some("@media (width >= 40rem)"), ""),
        "md" => (Some("@media (width >= 48rem)"), ""),
        "lg" => (Some("@media (width >= 64rem)"), ""),
        "xl" => (Some("@media (width >= 80rem)"), ""),
        "2xl" => (Some("@media (width >= 96rem)"), ""),
        _ => return None,
    })
}

/// A class name as a CSS identifier.
///
/// `:` and `.` are punctuation in a selector and are escaped one by one; a **leading digit** is not
/// escapable that way and takes CSS's hex form, `\32 ` — a code point and a terminating space. That
/// space is part of the escape rather than whitespace in the selector, which is what made an
/// earlier reader of the oracle stop at it and lose every `2xl:` rule.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        match c {
            '0'..='9' if i == 0 => out.push_str(&format!("\\3{c} ")),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => out.push(c),
            _ if !c.is_ascii() => out.push(c),
            _ => {
                out.push('\\');
                out.push(c);
            }
        }
    }
    out
}

/// The declarations of one utility, without its variants.
fn base_rule(base: &str) -> Option<(Vec<(&'static str, String)>, Shape)> {
    let one = |property: &'static str, value: &str| {
        Some((vec![(property, value.to_string())], Shape::Element))
    };
    if let Some(decls) = WORDS.iter().find(|(n, _)| *n == base) {
        return Some((
            decls.1.iter().map(|(p, v)| (*p, v.to_string())).collect(),
            Shape::Element,
        ));
    }
    if let Some(rest) = base.strip_prefix("text-") {
        if TEXT_SIZES.contains(&rest) {
            return Some((
                vec![
                    ("font-size", format!("var(--text-{rest})")),
                    (
                        "line-height",
                        format!("var(--tw-leading, var(--text-{rest}--line-height))"),
                    ),
                ],
                Shape::Element,
            ));
        }
        return colour(rest).and_then(|v| one("color", &v));
    }
    if let Some(rest) = base.strip_prefix("font-") {
        if WEIGHTS.contains(&rest) {
            return Some((
                vec![
                    ("--tw-font-weight", format!("var(--font-weight-{rest})")),
                    ("font-weight", format!("var(--font-weight-{rest})")),
                ],
                Shape::Element,
            ));
        }
        if ["sans", "serif", "mono"].contains(&rest) {
            return one("font-family", &format!("var(--font-{rest})"));
        }
        return None;
    }
    if let Some(rest) = base.strip_prefix("rounded-") {
        return match rest {
            "none" => one("border-radius", "0"),
            "full" => one("border-radius", "calc(infinity * 1px)"),
            "xs" | "sm" | "md" | "lg" | "xl" | "2xl" | "3xl" | "4xl" => {
                one("border-radius", &format!("var(--radius-{rest})"))
            }
            _ => None,
        };
    }
    if let Some(rest) = base.strip_prefix("border-") {
        if ["0", "2", "4", "8"].contains(&rest) {
            return Some((
                vec![
                    ("border-style", "var(--tw-border-style)".to_string()),
                    ("border-width", format!("{rest}px")),
                ],
                Shape::Element,
            ));
        }
        return colour(rest).and_then(|v| one("border-color", &v));
    }
    if let Some(rest) = base.strip_prefix("items-") {
        return match rest {
            "start" => one("align-items", "flex-start"),
            "center" => one("align-items", "center"),
            "end" => one("align-items", "flex-end"),
            "baseline" => one("align-items", "baseline"),
            "stretch" => one("align-items", "stretch"),
            _ => None,
        };
    }
    if let Some(rest) = base.strip_prefix("justify-") {
        return match rest {
            "start" => one("justify-content", "flex-start"),
            "center" => one("justify-content", "center"),
            "end" => one("justify-content", "flex-end"),
            "between" => one("justify-content", "space-between"),
            "around" => one("justify-content", "space-around"),
            "evenly" => one("justify-content", "space-evenly"),
            "stretch" => one("justify-content", "stretch"),
            _ => None,
        };
    }
    if let Some(rest) = base.strip_prefix("flex-") {
        return match rest {
            "row" => one("flex-direction", "row"),
            "row-reverse" => one("flex-direction", "row-reverse"),
            "col" => one("flex-direction", "column"),
            "col-reverse" => one("flex-direction", "column-reverse"),
            "wrap" => one("flex-wrap", "wrap"),
            "nowrap" => one("flex-wrap", "nowrap"),
            "wrap-reverse" => one("flex-wrap", "wrap-reverse"),
            "1" => one("flex", "1"),
            "auto" => one("flex", "auto"),
            "initial" => one("flex", "0 auto"),
            "none" => one("flex", "none"),
            _ => None,
        };
    }
    if let Some(rest) = base.strip_prefix("overflow-") {
        return match rest {
            "auto" | "hidden" | "clip" | "visible" | "scroll" => one("overflow", rest),
            _ => None,
        };
    }
    for (family, property) in COLOURED {
        if let Some(rest) = base.strip_prefix(family) {
            return colour(rest).and_then(|v| one(property, &v));
        }
    }
    for (family, properties, screen) in SIZED {
        if let Some(rest) = base.strip_prefix(family) {
            let value = match rest {
                "full" => "100%".to_string(),
                "min" => "min-content".to_string(),
                "max" => "max-content".to_string(),
                "fit" => "fit-content".to_string(),
                "auto" if !family.starts_with("max-") => "auto".to_string(),
                "screen" if *screen => match family.contains('h') {
                    true => "100vh".to_string(),
                    false => "100vw".to_string(),
                },
                _ => spacing(rest)?,
            };
            return Some((
                properties.iter().map(|p| (*p, value.clone())).collect(),
                Shape::Element,
            ));
        }
    }
    for (family, kind) in SPACED {
        let Some(rest) = base.strip_prefix(family) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('-') else {
            continue;
        };
        let value = match rest {
            "auto" => match kind {
                Spaced::Padding { .. } | Spaced::Between { .. } => return None,
                Spaced::Margin { property } => return one(property, "auto"),
            },
            _ => spacing(rest)?,
        };
        return Some(match kind {
            Spaced::Padding { property } | Spaced::Margin { property } => {
                (vec![(*property, value)], Shape::Element)
            }
            // `--tw-space-x-reverse` is `0` unless something flips it, so the margin lands on the
            // start of every child but the first — written as a pair so that `flex-row-reverse`
            // can invert it without a second class.
            Spaced::Between {
                reverse,
                start,
                end,
            } => (
                match value == "0px" {
                    // Nothing to reverse about a gap of nothing, and Tailwind says so by writing
                    // the margin out rather than the `calc` around it.
                    true => vec![
                        (reverse, "0".to_string()),
                        (start, "0".to_string()),
                        (end, "0".to_string()),
                    ],
                    false => vec![
                        (reverse, "0".to_string()),
                        (start, format!("calc({value} * var({reverse}))")),
                        (end, format!("calc({value} * calc(1 - var({reverse})))")),
                    ],
                },
                Shape::Between,
            ),
        });
    }
    None
}

/// A multiple of the spacing scale, as the value Tailwind gives it.
///
/// Three special cases, each of which the oracle said and none of which a person would have
/// written down: zero is `0px` rather than `0`, one is the variable itself rather than a `calc`
/// multiplying by one, and `px` is the literal pixel rather than a multiple of anything.
fn spacing(rest: &str) -> Option<String> {
    if rest == "px" {
        return Some("1px".to_string());
    }
    if !number(rest) {
        return None;
    }
    Some(match rest.parse::<f64>().ok()? {
        0.0 => "0px".to_string(),
        1.0 => "var(--spacing)".to_string(),
        _ => format!("calc(var(--spacing) * {rest})"),
    })
}

/// Utilities that are one name and take no argument.
const WORDS: &[(&str, &[(&str, &str)])] = &[
    ("flex", &[("display", "flex")]),
    ("inline-flex", &[("display", "inline-flex")]),
    ("grid", &[("display", "grid")]),
    ("inline-grid", &[("display", "inline-grid")]),
    ("block", &[("display", "block")]),
    ("inline-block", &[("display", "inline-block")]),
    ("inline", &[("display", "inline")]),
    ("hidden", &[("display", "none")]),
    ("contents", &[("display", "contents")]),
    ("flow-root", &[("display", "flow-root")]),
    ("table", &[("display", "table")]),
    ("static", &[("position", "static")]),
    ("relative", &[("position", "relative")]),
    ("absolute", &[("position", "absolute")]),
    ("fixed", &[("position", "fixed")]),
    ("sticky", &[("position", "sticky")]),
    ("italic", &[("font-style", "italic")]),
    ("not-italic", &[("font-style", "normal")]),
    ("underline", &[("text-decoration-line", "underline")]),
    ("overline", &[("text-decoration-line", "overline")]),
    ("line-through", &[("text-decoration-line", "line-through")]),
    ("no-underline", &[("text-decoration-line", "none")]),
    ("uppercase", &[("text-transform", "uppercase")]),
    ("lowercase", &[("text-transform", "lowercase")]),
    ("capitalize", &[("text-transform", "capitalize")]),
    ("normal-case", &[("text-transform", "none")]),
    (
        "truncate",
        &[
            ("overflow", "hidden"),
            ("text-overflow", "ellipsis"),
            ("white-space", "nowrap"),
        ],
    ),
    (
        "border",
        &[
            ("border-style", "var(--tw-border-style)"),
            ("border-width", "1px"),
        ],
    ),
    ("rounded", &[("border-radius", "0.25rem")]),
    (
        "sr-only",
        &[
            ("position", "absolute"),
            ("width", "1px"),
            ("height", "1px"),
            ("padding", "0"),
            ("margin", "-1px"),
            ("overflow", "hidden"),
            ("clip-path", "inset(50%)"),
            ("white-space", "nowrap"),
            ("border-width", "0"),
        ],
    ),
    (
        "outline-none",
        &[("--tw-outline-style", "none"), ("outline-style", "none")],
    ),
];

/// The families whose argument is a colour, and the property each sets.
const COLOURED: &[(&str, &str)] = &[
    ("bg-", "background-color"),
    ("fill-", "fill"),
    ("stroke-", "stroke"),
    ("ring-", "--tw-ring-color"),
    ("outline-", "outline-color"),
    ("decoration-", "text-decoration-color"),
];

/// The families whose argument is a length: the properties each sets, and whether `screen` is one
/// of its values.
///
/// Longest first, so `min-w-` is tried before `w-`.
const SIZED: &[(&str, &[&str], bool)] = &[
    ("size-", &["width", "height"], false),
    ("min-w-", &["min-width"], true),
    ("min-h-", &["min-height"], true),
    ("max-w-", &["max-width"], true),
    ("max-h-", &["max-height"], true),
    ("w-", &["width"], true),
    ("h-", &["height"], true),
];

/// What a spacing family sets, and therefore whether `auto` is one of its values.
enum Spaced {
    Padding {
        property: &'static str,
    },
    Margin {
        property: &'static str,
    },
    Between {
        reverse: &'static str,
        start: &'static str,
        end: &'static str,
    },
}

/// The families whose argument is a multiple of the spacing scale.
///
/// Longest first, so `gap-x` is tried before `gap` and `-x-2` is never read as a number.
const SPACED: &[(&str, Spaced)] = &[
    (
        "gap-x",
        Spaced::Padding {
            property: "column-gap",
        },
    ),
    (
        "gap-y",
        Spaced::Padding {
            property: "row-gap",
        },
    ),
    (
        "space-x",
        Spaced::Between {
            reverse: "--tw-space-x-reverse",
            start: "margin-inline-start",
            end: "margin-inline-end",
        },
    ),
    (
        "space-y",
        Spaced::Between {
            reverse: "--tw-space-y-reverse",
            start: "margin-block-start",
            end: "margin-block-end",
        },
    ),
    (
        "px",
        Spaced::Padding {
            property: "padding-inline",
        },
    ),
    (
        "py",
        Spaced::Padding {
            property: "padding-block",
        },
    ),
    (
        "pt",
        Spaced::Padding {
            property: "padding-top",
        },
    ),
    (
        "pr",
        Spaced::Padding {
            property: "padding-right",
        },
    ),
    (
        "pb",
        Spaced::Padding {
            property: "padding-bottom",
        },
    ),
    (
        "pl",
        Spaced::Padding {
            property: "padding-left",
        },
    ),
    (
        "ps",
        Spaced::Padding {
            property: "padding-inline-start",
        },
    ),
    (
        "pe",
        Spaced::Padding {
            property: "padding-inline-end",
        },
    ),
    (
        "mx",
        Spaced::Margin {
            property: "margin-inline",
        },
    ),
    (
        "my",
        Spaced::Margin {
            property: "margin-block",
        },
    ),
    (
        "mt",
        Spaced::Margin {
            property: "margin-top",
        },
    ),
    (
        "mr",
        Spaced::Margin {
            property: "margin-right",
        },
    ),
    (
        "mb",
        Spaced::Margin {
            property: "margin-bottom",
        },
    ),
    (
        "ml",
        Spaced::Margin {
            property: "margin-left",
        },
    ),
    (
        "ms",
        Spaced::Margin {
            property: "margin-inline-start",
        },
    ),
    (
        "me",
        Spaced::Margin {
            property: "margin-inline-end",
        },
    ),
    ("gap", Spaced::Padding { property: "gap" }),
    ("inset", Spaced::Margin { property: "inset" }),
    ("top", Spaced::Margin { property: "top" }),
    ("right", Spaced::Margin { property: "right" }),
    ("bottom", Spaced::Margin { property: "bottom" }),
    ("left", Spaced::Margin { property: "left" }),
    (
        "p",
        Spaced::Padding {
            property: "padding",
        },
    ),
    ("m", Spaced::Margin { property: "margin" }),
];

const TEXT_SIZES: &[&str] = &[
    "xs", "sm", "base", "lg", "xl", "2xl", "3xl", "4xl", "5xl", "6xl", "7xl", "8xl", "9xl",
];

const WEIGHTS: &[&str] = &[
    "thin",
    "extralight",
    "light",
    "normal",
    "medium",
    "semibold",
    "bold",
    "extrabold",
    "black",
];

/// The palette, as the names rather than the values: what a shade *is* belongs to [`THEME`].
const PALETTE: &[&str] = &[
    "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime",
    "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet", "purple", "fuchsia",
    "pink", "rose",
];

const SHADES: &[&str] = &[
    "50", "100", "200", "300", "400", "500", "600", "700", "800", "900", "950",
];

/// A colour as the value a declaration takes: a keyword, or a reference to a theme token.
///
/// The three keywords that are not tokens are not an inconsistency — `transparent` and `inherit`
/// are CSS's own and there is nothing to theme about them, and `current` is the spelling
/// difference between Tailwind's name and CSS's `currentcolor`.
fn colour(rest: &str) -> Option<String> {
    match rest {
        "transparent" => return Some("transparent".to_string()),
        "current" => return Some("currentcolor".to_string()),
        "inherit" => return Some("inherit".to_string()),
        "white" | "black" => return Some(format!("var(--color-{rest})")),
        _ => {}
    }
    let (name, shade) = rest.rsplit_once('-')?;
    match PALETTE.contains(&name) && SHADES.contains(&shade) {
        true => Some(format!("var(--color-{rest})")),
        false => None,
    }
}

/// A multiple of the spacing scale: a decimal number, because Tailwind 4's scale is multiplicative
/// rather than a list of steps.
fn number(rest: &str) -> bool {
    !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
        && rest.chars().filter(|c| *c == '.').count() <= 1
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        && rest.chars().last().is_some_and(|c| c.is_ascii_digit())
}

/// Every **closed** utility name this table knows, and every variant that can go in front of one.
///
/// The spacing scale is multiplicative and therefore open — `p-4` is here and `p-2.75` is not — so
/// this is the part of the table that can be listed. Its caller is the gate, which asks the oracle
/// about every one of them rather than about a list somebody wrote: a name accepted here and never
/// asked about is a page missing a rule with every gate green, which is how `size-screen`,
/// `max-w-auto` and fifteen `-auto` paddings survived a green run of the table's own differential.
pub fn enumerate() -> (Vec<String>, Vec<&'static str>) {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: String| {
        if rule(&name).is_some() {
            names.push(name);
        }
    };
    for (word, _) in WORDS {
        add(word.to_string());
    }
    let colours: Vec<String> = ["white", "black", "transparent", "current", "inherit"]
        .iter()
        .map(|k| k.to_string())
        .chain(
            PALETTE
                .iter()
                .flat_map(|p| SHADES.iter().map(move |s| format!("{p}-{s}"))),
        )
        .collect();
    for family in COLOURED.iter().map(|(f, _)| *f).chain(["text-", "border-"]) {
        for colour in &colours {
            add(format!("{family}{colour}"));
        }
    }
    for size in TEXT_SIZES {
        add(format!("text-{size}"));
    }
    for weight in WEIGHTS.iter().chain(&["sans", "serif", "mono"]) {
        add(format!("font-{weight}"));
    }
    for radius in [
        "none", "xs", "sm", "md", "lg", "xl", "2xl", "3xl", "4xl", "full",
    ] {
        add(format!("rounded-{radius}"));
    }
    for width in ["0", "2", "4", "8"] {
        add(format!("border-{width}"));
    }
    for (family, values) in [
        (
            "items-",
            &["start", "center", "end", "baseline", "stretch"][..],
        ),
        (
            "justify-",
            &[
                "start", "center", "end", "between", "around", "evenly", "stretch",
            ][..],
        ),
        (
            "flex-",
            &[
                "row",
                "row-reverse",
                "col",
                "col-reverse",
                "wrap",
                "nowrap",
                "wrap-reverse",
                "1",
                "auto",
                "initial",
                "none",
            ][..],
        ),
        (
            "overflow-",
            &["auto", "hidden", "clip", "visible", "scroll"][..],
        ),
    ] {
        for value in values {
            add(format!("{family}{value}"));
        }
    }
    // The closed arguments of the open families: their keywords, and **the scale Tailwind's own
    // documentation lists**. The scale is multiplicative and therefore infinite, so this is a
    // sample by construction — but it is the sample somebody typing `gap-` expects to be offered,
    // which is why it is the documented steps rather than three round numbers. Every one of them is
    // in `compiler/style/candidates.txt`, which is what
    // `style.rs::every_name_the_table_accepts_was_asked_about` holds.
    for (family, _, _) in SIZED {
        for value in ["full", "auto", "screen", "min", "max", "fit", "px"] {
            add(format!("{family}{value}"));
        }
        for step in SCALE {
            add(format!("{family}{step}"));
        }
    }
    for (family, _) in SPACED {
        for value in ["px", "auto"] {
            add(format!("{family}-{value}"));
        }
        for step in SCALE {
            add(format!("{family}-{step}"));
        }
    }
    (names, VARIANTS.to_vec())
}

/// The steps of the spacing scale Tailwind's documentation lists.
///
/// Not the scale — that is `calc(var(--spacing) * n)` for any `n` and has no end — but the part of
/// it a person browsing a completion list is looking for. [`rule`] accepts any number whether or
/// not it is here.
const SCALE: &[&str] = &[
    "0", "0.5", "1", "1.5", "2", "2.5", "3", "3.5", "4", "5", "6", "7", "8", "9", "10", "11", "12",
    "14", "16", "20", "24", "28", "32", "36", "40", "44", "48", "52", "56", "60", "64", "72", "80",
    "96",
];

/// Every variant this knows, in front of any utility.
const VARIANTS: &[&str] = &[
    "hover",
    "focus",
    "focus-visible",
    "focus-within",
    "active",
    "visited",
    "disabled",
    "checked",
    "first",
    "last",
    "odd",
    "even",
    "empty",
    "dark",
    "motion-safe",
    "motion-reduce",
    "print",
    "sm",
    "md",
    "lg",
    "xl",
    "2xl",
];

/// The condition guarding [`stylesheet`]'s fallback for browsers with no registered custom
/// properties. Tailwind's own, captured by `compiler/style/generate.sh` rather than transcribed.
const SUPPORTS: &str = "((-webkit-hyphens: none) and (not (margin-trim: inline))) or \
                        ((-moz-orient: inline) and (not (color:rgb(from red r g b))))";

/// One theme token's value, or `None` if the theme does not define it.
pub fn theme(token: &str) -> Option<&'static str> {
    THEME.iter().find(|(n, _)| *n == token).map(|(_, v)| *v)
}

/// Every token the theme defines, so the gate can hold each to Tailwind's own value.
pub fn theme_tokens() -> &'static [(&'static str, &'static str)] {
    THEME
}

/// Every registered custom property the utilities use, and what each is.
pub fn properties() -> &'static [(&'static str, &'static str)] {
    PROPERTIES
}

/// The condition guarding the sheet's fallback for browsers with no registered custom properties.
pub fn supports() -> &'static str {
    SUPPORTS
}

/// The theme, as Tailwind 4.3.3 defines it.
///
/// **Values rather than names**, which is the half [`is_utility`] never needed and a sheet cannot
/// do without. A ramp is not derivable from anything — `oklch(50.8% 0.118 165.612)` is a decade of
/// somebody's taste — so this is transcribed from the oracle's own output by
/// `compiler/style/generate.sh`, and `style.rs::the_theme_is_tailwinds` holds every entry against
/// it. That gate cannot catch the transcription that produced the table, only an edit to it
/// afterwards and a version that moves underneath it, and saying so is the point: it is a
/// regression gate rather than a derivation.
///
/// [`docs/08`](../../../../../docs/08-roadmap.md) §8.5.4's styling item 5 is what makes this a Beck
/// value a program can change; until then it is the default and the only one.
const THEME: &[(&str, &str)] = &[
    ("--color-amber-100", "oklch(96.2% 0.059 95.617)"),
    ("--color-amber-200", "oklch(92.4% 0.12 95.746)"),
    ("--color-amber-300", "oklch(87.9% 0.169 91.605)"),
    ("--color-amber-400", "oklch(82.8% 0.189 84.429)"),
    ("--color-amber-50", "oklch(98.7% 0.022 95.277)"),
    ("--color-amber-500", "oklch(76.9% 0.188 70.08)"),
    ("--color-amber-600", "oklch(66.6% 0.179 58.318)"),
    ("--color-amber-700", "oklch(55.5% 0.163 48.998)"),
    ("--color-amber-800", "oklch(47.3% 0.137 46.201)"),
    ("--color-amber-900", "oklch(41.4% 0.112 45.904)"),
    ("--color-amber-950", "oklch(27.9% 0.077 45.635)"),
    ("--color-black", "#000"),
    ("--color-blue-100", "oklch(93.2% 0.032 255.585)"),
    ("--color-blue-200", "oklch(88.2% 0.059 254.128)"),
    ("--color-blue-300", "oklch(80.9% 0.105 251.813)"),
    ("--color-blue-400", "oklch(70.7% 0.165 254.624)"),
    ("--color-blue-50", "oklch(97% 0.014 254.604)"),
    ("--color-blue-500", "oklch(62.3% 0.214 259.815)"),
    ("--color-blue-600", "oklch(54.6% 0.245 262.881)"),
    ("--color-blue-700", "oklch(48.8% 0.243 264.376)"),
    ("--color-blue-800", "oklch(42.4% 0.199 265.638)"),
    ("--color-blue-900", "oklch(37.9% 0.146 265.522)"),
    ("--color-blue-950", "oklch(28.2% 0.091 267.935)"),
    ("--color-cyan-100", "oklch(95.6% 0.045 203.388)"),
    ("--color-cyan-200", "oklch(91.7% 0.08 205.041)"),
    ("--color-cyan-300", "oklch(86.5% 0.127 207.078)"),
    ("--color-cyan-400", "oklch(78.9% 0.154 211.53)"),
    ("--color-cyan-50", "oklch(98.4% 0.019 200.873)"),
    ("--color-cyan-500", "oklch(71.5% 0.143 215.221)"),
    ("--color-cyan-600", "oklch(60.9% 0.126 221.723)"),
    ("--color-cyan-700", "oklch(52% 0.105 223.128)"),
    ("--color-cyan-800", "oklch(45% 0.085 224.283)"),
    ("--color-cyan-900", "oklch(39.8% 0.07 227.392)"),
    ("--color-cyan-950", "oklch(30.2% 0.056 229.695)"),
    ("--color-emerald-100", "oklch(95% 0.052 163.051)"),
    ("--color-emerald-200", "oklch(90.5% 0.093 164.15)"),
    ("--color-emerald-300", "oklch(84.5% 0.143 164.978)"),
    ("--color-emerald-400", "oklch(76.5% 0.177 163.223)"),
    ("--color-emerald-50", "oklch(97.9% 0.021 166.113)"),
    ("--color-emerald-500", "oklch(69.6% 0.17 162.48)"),
    ("--color-emerald-600", "oklch(59.6% 0.145 163.225)"),
    ("--color-emerald-700", "oklch(50.8% 0.118 165.612)"),
    ("--color-emerald-800", "oklch(43.2% 0.095 166.913)"),
    ("--color-emerald-900", "oklch(37.8% 0.077 168.94)"),
    ("--color-emerald-950", "oklch(26.2% 0.051 172.552)"),
    ("--color-fuchsia-100", "oklch(95.2% 0.037 318.852)"),
    ("--color-fuchsia-200", "oklch(90.3% 0.076 319.62)"),
    ("--color-fuchsia-300", "oklch(83.3% 0.145 321.434)"),
    ("--color-fuchsia-400", "oklch(74% 0.238 322.16)"),
    ("--color-fuchsia-50", "oklch(97.7% 0.017 320.058)"),
    ("--color-fuchsia-500", "oklch(66.7% 0.295 322.15)"),
    ("--color-fuchsia-600", "oklch(59.1% 0.293 322.896)"),
    ("--color-fuchsia-700", "oklch(51.8% 0.253 323.949)"),
    ("--color-fuchsia-800", "oklch(45.2% 0.211 324.591)"),
    ("--color-fuchsia-900", "oklch(40.1% 0.17 325.612)"),
    ("--color-fuchsia-950", "oklch(29.3% 0.136 325.661)"),
    ("--color-gray-100", "oklch(96.7% 0.003 264.542)"),
    ("--color-gray-200", "oklch(92.8% 0.006 264.531)"),
    ("--color-gray-300", "oklch(87.2% 0.01 258.338)"),
    ("--color-gray-400", "oklch(70.7% 0.022 261.325)"),
    ("--color-gray-50", "oklch(98.5% 0.002 247.839)"),
    ("--color-gray-500", "oklch(55.1% 0.027 264.364)"),
    ("--color-gray-600", "oklch(44.6% 0.03 256.802)"),
    ("--color-gray-700", "oklch(37.3% 0.034 259.733)"),
    ("--color-gray-800", "oklch(27.8% 0.033 256.848)"),
    ("--color-gray-900", "oklch(21% 0.034 264.665)"),
    ("--color-gray-950", "oklch(13% 0.028 261.692)"),
    ("--color-green-100", "oklch(96.2% 0.044 156.743)"),
    ("--color-green-200", "oklch(92.5% 0.084 155.995)"),
    ("--color-green-300", "oklch(87.1% 0.15 154.449)"),
    ("--color-green-400", "oklch(79.2% 0.209 151.711)"),
    ("--color-green-50", "oklch(98.2% 0.018 155.826)"),
    ("--color-green-500", "oklch(72.3% 0.219 149.579)"),
    ("--color-green-600", "oklch(62.7% 0.194 149.214)"),
    ("--color-green-700", "oklch(52.7% 0.154 150.069)"),
    ("--color-green-800", "oklch(44.8% 0.119 151.328)"),
    ("--color-green-900", "oklch(39.3% 0.095 152.535)"),
    ("--color-green-950", "oklch(26.6% 0.065 152.934)"),
    ("--color-indigo-100", "oklch(93% 0.034 272.788)"),
    ("--color-indigo-200", "oklch(87% 0.065 274.039)"),
    ("--color-indigo-300", "oklch(78.5% 0.115 274.713)"),
    ("--color-indigo-400", "oklch(67.3% 0.182 276.935)"),
    ("--color-indigo-50", "oklch(96.2% 0.018 272.314)"),
    ("--color-indigo-500", "oklch(58.5% 0.233 277.117)"),
    ("--color-indigo-600", "oklch(51.1% 0.262 276.966)"),
    ("--color-indigo-700", "oklch(45.7% 0.24 277.023)"),
    ("--color-indigo-800", "oklch(39.8% 0.195 277.366)"),
    ("--color-indigo-900", "oklch(35.9% 0.144 278.697)"),
    ("--color-indigo-950", "oklch(25.7% 0.09 281.288)"),
    ("--color-lime-100", "oklch(96.7% 0.067 122.328)"),
    ("--color-lime-200", "oklch(93.8% 0.127 124.321)"),
    ("--color-lime-300", "oklch(89.7% 0.196 126.665)"),
    ("--color-lime-400", "oklch(84.1% 0.238 128.85)"),
    ("--color-lime-50", "oklch(98.6% 0.031 120.757)"),
    ("--color-lime-500", "oklch(76.8% 0.233 130.85)"),
    ("--color-lime-600", "oklch(64.8% 0.2 131.684)"),
    ("--color-lime-700", "oklch(53.2% 0.157 131.589)"),
    ("--color-lime-800", "oklch(45.3% 0.124 130.933)"),
    ("--color-lime-900", "oklch(40.5% 0.101 131.063)"),
    ("--color-lime-950", "oklch(27.4% 0.072 132.109)"),
    ("--color-neutral-100", "oklch(97% 0 none)"),
    ("--color-neutral-200", "oklch(92.2% 0 none)"),
    ("--color-neutral-300", "oklch(87% 0 none)"),
    ("--color-neutral-400", "oklch(70.8% 0 none)"),
    ("--color-neutral-50", "oklch(98.5% 0 none)"),
    ("--color-neutral-500", "oklch(55.6% 0 none)"),
    ("--color-neutral-600", "oklch(43.9% 0 none)"),
    ("--color-neutral-700", "oklch(37.1% 0 none)"),
    ("--color-neutral-800", "oklch(26.9% 0 none)"),
    ("--color-neutral-900", "oklch(20.5% 0 none)"),
    ("--color-neutral-950", "oklch(14.5% 0 none)"),
    ("--color-orange-100", "oklch(95.4% 0.038 75.164)"),
    ("--color-orange-200", "oklch(90.1% 0.076 70.697)"),
    ("--color-orange-300", "oklch(83.7% 0.128 66.29)"),
    ("--color-orange-400", "oklch(75% 0.183 55.934)"),
    ("--color-orange-50", "oklch(98% 0.016 73.684)"),
    ("--color-orange-500", "oklch(70.5% 0.213 47.604)"),
    ("--color-orange-600", "oklch(64.6% 0.222 41.116)"),
    ("--color-orange-700", "oklch(55.3% 0.195 38.402)"),
    ("--color-orange-800", "oklch(47% 0.157 37.304)"),
    ("--color-orange-900", "oklch(40.8% 0.123 38.172)"),
    ("--color-orange-950", "oklch(26.6% 0.079 36.259)"),
    ("--color-pink-100", "oklch(94.8% 0.028 342.258)"),
    ("--color-pink-200", "oklch(89.9% 0.061 343.231)"),
    ("--color-pink-300", "oklch(82.3% 0.12 346.018)"),
    ("--color-pink-400", "oklch(71.8% 0.202 349.761)"),
    ("--color-pink-50", "oklch(97.1% 0.014 343.198)"),
    ("--color-pink-500", "oklch(65.6% 0.241 354.308)"),
    ("--color-pink-600", "oklch(59.2% 0.249 0.584)"),
    ("--color-pink-700", "oklch(52.5% 0.223 3.958)"),
    ("--color-pink-800", "oklch(45.9% 0.187 3.815)"),
    ("--color-pink-900", "oklch(40.8% 0.153 2.432)"),
    ("--color-pink-950", "oklch(28.4% 0.109 3.907)"),
    ("--color-purple-100", "oklch(94.6% 0.033 307.174)"),
    ("--color-purple-200", "oklch(90.2% 0.063 306.703)"),
    ("--color-purple-300", "oklch(82.7% 0.119 306.383)"),
    ("--color-purple-400", "oklch(71.4% 0.203 305.504)"),
    ("--color-purple-50", "oklch(97.7% 0.014 308.299)"),
    ("--color-purple-500", "oklch(62.7% 0.265 303.9)"),
    ("--color-purple-600", "oklch(55.8% 0.288 302.321)"),
    ("--color-purple-700", "oklch(49.6% 0.265 301.924)"),
    ("--color-purple-800", "oklch(43.8% 0.218 303.724)"),
    ("--color-purple-900", "oklch(38.1% 0.176 304.987)"),
    ("--color-purple-950", "oklch(29.1% 0.149 302.717)"),
    ("--color-red-100", "oklch(93.6% 0.032 17.717)"),
    ("--color-red-200", "oklch(88.5% 0.062 18.334)"),
    ("--color-red-300", "oklch(80.8% 0.114 19.571)"),
    ("--color-red-400", "oklch(70.4% 0.191 22.216)"),
    ("--color-red-50", "oklch(97.1% 0.013 17.38)"),
    ("--color-red-500", "oklch(63.7% 0.237 25.331)"),
    ("--color-red-600", "oklch(57.7% 0.245 27.325)"),
    ("--color-red-700", "oklch(50.5% 0.213 27.518)"),
    ("--color-red-800", "oklch(44.4% 0.177 26.899)"),
    ("--color-red-900", "oklch(39.6% 0.141 25.723)"),
    ("--color-red-950", "oklch(25.8% 0.092 26.042)"),
    ("--color-rose-100", "oklch(94.1% 0.03 12.58)"),
    ("--color-rose-200", "oklch(89.2% 0.058 10.001)"),
    ("--color-rose-300", "oklch(81% 0.117 11.638)"),
    ("--color-rose-400", "oklch(71.2% 0.194 13.428)"),
    ("--color-rose-50", "oklch(96.9% 0.015 12.422)"),
    ("--color-rose-500", "oklch(64.5% 0.246 16.439)"),
    ("--color-rose-600", "oklch(58.6% 0.253 17.585)"),
    ("--color-rose-700", "oklch(51.4% 0.222 16.935)"),
    ("--color-rose-800", "oklch(45.5% 0.188 13.697)"),
    ("--color-rose-900", "oklch(41% 0.159 10.272)"),
    ("--color-rose-950", "oklch(27.1% 0.105 12.094)"),
    ("--color-sky-100", "oklch(95.1% 0.026 236.824)"),
    ("--color-sky-200", "oklch(90.1% 0.058 230.902)"),
    ("--color-sky-300", "oklch(82.8% 0.111 230.318)"),
    ("--color-sky-400", "oklch(74.6% 0.16 232.661)"),
    ("--color-sky-50", "oklch(97.7% 0.013 236.62)"),
    ("--color-sky-500", "oklch(68.5% 0.169 237.323)"),
    ("--color-sky-600", "oklch(58.8% 0.158 241.966)"),
    ("--color-sky-700", "oklch(50% 0.134 242.749)"),
    ("--color-sky-800", "oklch(44.3% 0.11 240.79)"),
    ("--color-sky-900", "oklch(39.1% 0.09 240.876)"),
    ("--color-sky-950", "oklch(29.3% 0.066 243.157)"),
    ("--color-slate-100", "oklch(96.8% 0.007 247.896)"),
    ("--color-slate-200", "oklch(92.9% 0.013 255.508)"),
    ("--color-slate-300", "oklch(86.9% 0.022 252.894)"),
    ("--color-slate-400", "oklch(70.4% 0.04 256.788)"),
    ("--color-slate-50", "oklch(98.4% 0.003 247.858)"),
    ("--color-slate-500", "oklch(55.4% 0.046 257.417)"),
    ("--color-slate-600", "oklch(44.6% 0.043 257.281)"),
    ("--color-slate-700", "oklch(37.2% 0.044 257.287)"),
    ("--color-slate-800", "oklch(27.9% 0.041 260.031)"),
    ("--color-slate-900", "oklch(20.8% 0.042 265.755)"),
    ("--color-slate-950", "oklch(12.9% 0.042 264.695)"),
    ("--color-stone-100", "oklch(97% 0.001 106.424)"),
    ("--color-stone-200", "oklch(92.3% 0.003 48.717)"),
    ("--color-stone-300", "oklch(86.9% 0.005 56.366)"),
    ("--color-stone-400", "oklch(70.9% 0.01 56.259)"),
    ("--color-stone-50", "oklch(98.5% 0.001 106.423)"),
    ("--color-stone-500", "oklch(55.3% 0.013 58.071)"),
    ("--color-stone-600", "oklch(44.4% 0.011 73.639)"),
    ("--color-stone-700", "oklch(37.4% 0.01 67.558)"),
    ("--color-stone-800", "oklch(26.8% 0.007 34.298)"),
    ("--color-stone-900", "oklch(21.6% 0.006 56.043)"),
    ("--color-stone-950", "oklch(14.7% 0.004 49.25)"),
    ("--color-teal-100", "oklch(95.3% 0.051 180.801)"),
    ("--color-teal-200", "oklch(91% 0.096 180.426)"),
    ("--color-teal-300", "oklch(85.5% 0.138 181.071)"),
    ("--color-teal-400", "oklch(77.7% 0.152 181.912)"),
    ("--color-teal-50", "oklch(98.4% 0.014 180.72)"),
    ("--color-teal-500", "oklch(70.4% 0.14 182.503)"),
    ("--color-teal-600", "oklch(60% 0.118 184.704)"),
    ("--color-teal-700", "oklch(51.1% 0.096 186.391)"),
    ("--color-teal-800", "oklch(43.7% 0.078 188.216)"),
    ("--color-teal-900", "oklch(38.6% 0.063 188.416)"),
    ("--color-teal-950", "oklch(27.7% 0.046 192.524)"),
    ("--color-violet-100", "oklch(94.3% 0.029 294.588)"),
    ("--color-violet-200", "oklch(89.4% 0.057 293.283)"),
    ("--color-violet-300", "oklch(81.1% 0.111 293.571)"),
    ("--color-violet-400", "oklch(70.2% 0.183 293.541)"),
    ("--color-violet-50", "oklch(96.9% 0.016 293.756)"),
    ("--color-violet-500", "oklch(60.6% 0.25 292.717)"),
    ("--color-violet-600", "oklch(54.1% 0.281 293.009)"),
    ("--color-violet-700", "oklch(49.1% 0.27 292.581)"),
    ("--color-violet-800", "oklch(43.2% 0.232 292.759)"),
    ("--color-violet-900", "oklch(38% 0.189 293.745)"),
    ("--color-violet-950", "oklch(28.3% 0.141 291.089)"),
    ("--color-white", "#fff"),
    ("--color-yellow-100", "oklch(97.3% 0.071 103.193)"),
    ("--color-yellow-200", "oklch(94.5% 0.129 101.54)"),
    ("--color-yellow-300", "oklch(90.5% 0.182 98.111)"),
    ("--color-yellow-400", "oklch(85.2% 0.199 91.936)"),
    ("--color-yellow-50", "oklch(98.7% 0.026 102.212)"),
    ("--color-yellow-500", "oklch(79.5% 0.184 86.047)"),
    ("--color-yellow-600", "oklch(68.1% 0.162 75.834)"),
    ("--color-yellow-700", "oklch(55.4% 0.135 66.442)"),
    ("--color-yellow-800", "oklch(47.6% 0.114 61.907)"),
    ("--color-yellow-900", "oklch(42.1% 0.095 57.708)"),
    ("--color-yellow-950", "oklch(28.6% 0.066 53.813)"),
    ("--color-zinc-100", "oklch(96.7% 0.001 286.375)"),
    ("--color-zinc-200", "oklch(92% 0.004 286.32)"),
    ("--color-zinc-300", "oklch(87.1% 0.006 286.286)"),
    ("--color-zinc-400", "oklch(70.5% 0.015 286.067)"),
    ("--color-zinc-50", "oklch(98.5% 0 none)"),
    ("--color-zinc-500", "oklch(55.2% 0.016 285.938)"),
    ("--color-zinc-600", "oklch(44.2% 0.017 285.786)"),
    ("--color-zinc-700", "oklch(37% 0.013 285.805)"),
    ("--color-zinc-800", "oklch(27.4% 0.006 286.033)"),
    ("--color-zinc-900", "oklch(21% 0.006 285.885)"),
    ("--color-zinc-950", "oklch(14.1% 0.005 285.823)"),
    ("--default-font-family", "var(--font-sans)"),
    ("--default-mono-font-family", "var(--font-mono)"),
    ("--font-mono", "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, \"Liberation Mono\", \"Courier New\", monospace"),
    ("--font-sans", "-apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \"Helvetica Neue\", \"Noto Sans\", Arial, sans-serif, \"Apple Color Emoji\", \"Segoe UI Emoji\", \"Segoe UI Symbol\", \"Noto Color Emoji\""),
    ("--font-serif", "ui-serif, Georgia, Cambria, \"Times New Roman\", Times, serif"),
    ("--font-weight-black", "900"),
    ("--font-weight-bold", "700"),
    ("--font-weight-extrabold", "800"),
    ("--font-weight-extralight", "200"),
    ("--font-weight-light", "300"),
    ("--font-weight-medium", "500"),
    ("--font-weight-normal", "400"),
    ("--font-weight-semibold", "600"),
    ("--font-weight-thin", "100"),
    ("--radius-2xl", "1rem"),
    ("--radius-3xl", "1.5rem"),
    ("--radius-4xl", "2rem"),
    ("--radius-lg", "0.5rem"),
    ("--radius-md", "0.375rem"),
    ("--radius-sm", "0.25rem"),
    ("--radius-xl", "0.75rem"),
    ("--radius-xs", "0.125rem"),
    ("--spacing", "0.25rem"),
    ("--text-2xl", "1.5rem"),
    ("--text-2xl--line-height", "calc(2 / 1.5)"),
    ("--text-3xl", "1.875rem"),
    ("--text-3xl--line-height", "calc(2.25 / 1.875)"),
    ("--text-4xl", "2.25rem"),
    ("--text-4xl--line-height", "calc(2.5 / 2.25)"),
    ("--text-5xl", "3rem"),
    ("--text-5xl--line-height", "1"),
    ("--text-6xl", "3.75rem"),
    ("--text-6xl--line-height", "1"),
    ("--text-7xl", "4.5rem"),
    ("--text-7xl--line-height", "1"),
    ("--text-8xl", "6rem"),
    ("--text-8xl--line-height", "1"),
    ("--text-9xl", "8rem"),
    ("--text-9xl--line-height", "1"),
    ("--text-base", "1rem"),
    ("--text-base--line-height", "calc(1.5 / 1)"),
    ("--text-lg", "1.125rem"),
    ("--text-lg--line-height", "calc(1.75 / 1.125)"),
    ("--text-sm", "0.875rem"),
    ("--text-sm--line-height", "calc(1.25 / 0.875)"),
    ("--text-xl", "1.25rem"),
    ("--text-xl--line-height", "calc(1.75 / 1.25)"),
    ("--text-xs", "0.75rem"),
    ("--text-xs--line-height", "calc(1 / 0.75)"),
];

// -------------------------------------------------------------------------------------------
// The sheet
// -------------------------------------------------------------------------------------------

/// The stylesheet a program's pages need, and **nothing else**.
///
/// [`docs/104`](../../../../../docs/104-styling-and-the-component-library.md) §104.4's first
/// sentence, mechanised: "`beck build` walks the typed tree, collects the class strings that reach
/// a `class=`, and emits the sheet. No false positives, no false negatives across a module
/// boundary, and no configuration." [`classes`] is the walk and this is the sheet.
///
/// What it contains, in order:
///
/// 1. **A preflight.** Beck's own, and small — a browser's defaults disagree with every utility
///    that sets a margin. It is *not* Tailwind's, which is the delivery mechanism's opinionated
///    global sheet rather than the design system §104.4 takes, and §104.4 says which rules it has.
/// 2. **The theme tokens the rules read**, and only those: a page using one colour defines one
///    colour. This is the half [`docs/08`](../../../../../docs/08-roadmap.md) §8.5.4's styling item
///    5 makes a Beck value; here it is Tailwind's defaults.
/// 3. **`@property` for the internals the rules read**, with the fallback Tailwind ships for
///    browsers that do not register custom properties.
/// 4. **One rule per class the program can carry**, in name order — which puts `p-4` before
///    `px-2` because `-` sorts before a letter, so a shorthand loses to the longhand that follows
///    it, which is the order a reader expects and the order Tailwind's own sheet has.
///
/// A class the program carries that is **not** a utility contributes nothing: it is the program's
/// own name, and the compiler has nothing to say about what it should look like.
pub fn stylesheet(styles: &Styles) -> String {
    let mut rules: Vec<(Arc<str>, Rule)> = styles
        .classes
        .iter()
        .filter_map(|c| rule(c).map(|r| (c.clone(), r)))
        .collect();
    // Base rules before conditional ones, so a media query overrides what it narrows.
    rules.sort_by(|(a, x), (b, y)| x.at.len().cmp(&y.at.len()).then_with(|| a.cmp(b)));

    let mut tokens: BTreeSet<&'static str> = PREFLIGHT_TOKENS.iter().copied().collect();
    let mut internals: BTreeSet<&'static str> = BTreeSet::new();
    for (_, r) in &rules {
        r.tokens(&mut tokens);
        for (property, value) in &r.decls {
            for (name, _) in PROPERTIES {
                if property == name || value.contains(name) {
                    internals.insert(name);
                }
            }
        }
    }

    let mut out = String::with_capacity(2048);
    out.push_str("/* Written by `beck build` — one rule per class this program's pages can\n");
    out.push_str("   carry, and nothing else. docs/104 §104.4. */\n");
    out.push_str(PREFLIGHT);
    if !tokens.is_empty() {
        out.push_str(":root{");
        for token in &tokens {
            let value = THEME
                .iter()
                .find(|(n, _)| n == token)
                .map(|(_, v)| *v)
                .unwrap_or("");
            let _ = write!(out, "{token}:{value};");
        }
        out.push_str("}\n");
    }
    for (name, body) in PROPERTIES.iter().filter(|(n, _)| internals.contains(n)) {
        let _ = writeln!(out, "@property {name}{{{body}}}");
    }
    // The fallback for a browser that does not register custom properties: the same initial values,
    // set where a declaration would have found them. Its condition is Tailwind's own, captured
    // rather than transcribed — a browser-detection expression is the kind of string nobody can
    // check by reading it.
    let initial: Vec<&(&str, &str)> = PROPERTIES
        .iter()
        .filter(|(n, body)| internals.contains(n) && body.contains("initial-value"))
        .collect();
    if !initial.is_empty() {
        let _ = write!(out, "@supports {SUPPORTS}{{*,::before,::after,::backdrop{{");
        for (name, body) in initial {
            let value = body
                .rsplit_once("initial-value:")
                .map_or("", |(_, v)| v.trim());
            let _ = write!(out, "{name}:{value};");
        }
        out.push_str("}}\n");
    }
    for (_, rule) in &rules {
        for at in &rule.at {
            let _ = write!(out, "{at}{{");
        }
        let _ = write!(out, "{}{{", rule.selector);
        for (property, value) in &rule.decls {
            let _ = write!(out, "{property}:{value};");
        }
        out.push('}');
        out.push_str(&"}".repeat(rule.at.len()));
        out.push('\n');
    }
    out
}

/// Beck's preflight: what a browser has to be told before a utility means anything.
///
/// Nine rules, and each is here because a browser default fights a utility rather than because it
/// is a taste: `p-0` cannot win against a `ul`'s padding, `flex` cannot lay out an `li` carrying a
/// marker, and a `button` renders in the browser's font whatever `font-sans` says. Tailwind's own
/// preflight is four times this and is part of the *delivery mechanism* — an opinionated global
/// sheet that arrives with the tool — rather than the design system §104.4 takes.
const PREFLIGHT: &str = concat!(
    "*,::before,::after{box-sizing:border-box;margin:0;padding:0;border:0 solid}\n",
    "html{line-height:1.5;-webkit-text-size-adjust:100%;font-family:var(--default-font-family)}\n",
    "ul,ol{list-style:none}\n",
    "a{color:inherit;text-decoration:inherit}\n",
    "button,input,select,textarea{font:inherit;color:inherit;background:transparent}\n",
    "button{cursor:pointer}\n",
    "img,svg,video,canvas{display:block;max-width:100%}\n",
    "h1,h2,h3,h4,h5,h6{font-size:inherit;font-weight:inherit}\n",
    "table{border-collapse:collapse}\n",
);

/// The tokens [`PREFLIGHT`] reads, so a sheet defines them however few utilities a page uses.
const PREFLIGHT_TOKENS: &[&str] = &["--default-font-family", "--font-sans"];

/// The registered custom properties the utilities use, and what each is.
const PROPERTIES: &[(&str, &str)] = &[
    (
        "--tw-border-style",
        "syntax:\"*\";inherits:false;initial-value:solid",
    ),
    ("--tw-font-weight", "syntax:\"*\";inherits:false"),
    (
        "--tw-space-x-reverse",
        "syntax:\"*\";inherits:false;initial-value:0",
    ),
    (
        "--tw-space-y-reverse",
        "syntax:\"*\";inherits:false;initial-value:0",
    ),
];
