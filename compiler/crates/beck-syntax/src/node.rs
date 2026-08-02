//! `Node` — the canonical AST, and an ordinary value.
//!
//! [`docs/02-syntax.md`](../../../../docs/02-syntax.md) §2.2 fixes the shape:
//!
//! ```text
//! model Node:
//!     head: Sym | Lit
//!     args: list[Node]
//!     meta: Meta
//! ```
//!
//! Everything else is derived. Both surfaces — the Python one and the S-expression one — read to
//! *identical* `Node` trees; `beck fmt` prints either. That is the whole trick: "significant
//! whitespace is only hard if your macros do string concatenation. Ours cannot."
//!
//! One representational decision the doc leaves implicit: `head` is a symbol *or* a literal, so an
//! application whose callee is itself an expression has nowhere to put the callee. Those use the
//! reserved head [`sym::CALL`] — `(call (. f g) x)` — which keeps the common case, and therefore
//! the original sketch's notation, literal: `(update_at todos id ...)` is a symbol head with three
//! arguments, exactly as written.

use std::fmt;
use std::sync::Arc;

use beck_diag::Span;

/// A hygiene scope. Fresh scopes are minted by the macro expander; the set a symbol carries is
/// what decides which binding it refers to ([`crate::Symbol`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope(pub u32);

/// A set of hygiene scopes, kept sorted and deduplicated so that subset tests are a merge.
///
/// The empty set is the source program's own scope, which is why an ordinary top-level definition
/// is visible everywhere: `{} ⊆ S` for every `S`.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeSet(Arc<[Scope]>);

impl ScopeSet {
    pub fn empty() -> ScopeSet {
        ScopeSet(Arc::from([] as [Scope; 0]))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, s: Scope) -> bool {
        self.0.binary_search(&s).is_ok()
    }

    pub fn insert(&self, s: Scope) -> ScopeSet {
        if self.contains(s) {
            return self.clone();
        }
        let mut v = self.0.to_vec();
        v.push(s);
        v.sort_unstable();
        ScopeSet(Arc::from(v))
    }

    pub fn remove(&self, s: Scope) -> ScopeSet {
        if !self.contains(s) {
            return self.clone();
        }
        let v: Vec<Scope> = self.0.iter().copied().filter(|x| *x != s).collect();
        ScopeSet(Arc::from(v))
    }

    /// Add the scope if absent, remove it if present.
    ///
    /// This is the operation that makes hygiene work: the expander adds a fresh scope to a macro's
    /// *input* and flips it on the *output*, so identifiers that came from the call site come back
    /// to their original scopes while identifiers the template introduced acquire the new one.
    pub fn flip(&self, s: Scope) -> ScopeSet {
        if self.contains(s) {
            self.remove(s)
        } else {
            self.insert(s)
        }
    }

    /// `self ⊆ other`. A binding is a candidate for a reference exactly when this holds.
    pub fn is_subset_of(&self, other: &ScopeSet) -> bool {
        let (mut i, mut j) = (0, 0);
        while i < self.0.len() {
            if j >= other.0.len() {
                return false;
            }
            match self.0[i].cmp(&other.0[j]) {
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Less => return false,
            }
        }
        true
    }
}

impl fmt::Debug for ScopeSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (i, s) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}", s.0)?;
        }
        write!(f, "}}")
    }
}

/// An identifier, with the hygiene scopes it was written (or introduced) in.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol {
    pub name: Arc<str>,
    pub scopes: ScopeSet,
}

impl Symbol {
    pub fn new(name: impl AsRef<str>) -> Symbol {
        Symbol {
            name: Arc::from(name.as_ref()),
            scopes: ScopeSet::empty(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.name
    }

    pub fn with_scopes(&self, scopes: ScopeSet) -> Symbol {
        Symbol {
            name: self.name.clone(),
            scopes,
        }
    }
}

impl fmt::Debug for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scopes.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}{:?}", self.name, self.scopes)
        }
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// A literal. Floats compare by bit pattern so that `Lit` — and therefore `Node` — can be `Eq`:
/// two source files that differ only in `0.0` versus `-0.0` are different programs.
#[derive(Clone, Debug)]
pub enum Lit {
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Bool(bool),
    /// `:keyword` — a self-evaluating name, used for record field labels and enum-ish tags. The
    /// sketch writes `{:id id :text text}`, so keywords are in the core notation from the start.
    Keyword(Arc<str>),
}

impl PartialEq for Lit {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Lit::Int(a), Lit::Int(b)) => a == b,
            (Lit::Float(a), Lit::Float(b)) => a.to_bits() == b.to_bits(),
            (Lit::Str(a), Lit::Str(b)) => a == b,
            (Lit::Bool(a), Lit::Bool(b)) => a == b,
            (Lit::Keyword(a), Lit::Keyword(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Lit {}

impl Lit {
    pub fn type_name(&self) -> &'static str {
        match self {
            Lit::Int(_) => "int",
            Lit::Float(_) => "float",
            Lit::Str(_) => "str",
            Lit::Bool(_) => "bool",
            Lit::Keyword(_) => "keyword",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Head {
    Sym(Symbol),
    Lit(Lit),
}

/// Everything a `Node` knows about itself beyond its shape.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub span: Span,
    /// The macro expansion chain this node came out of, innermost last. Empty for source code.
    pub expansion: Vec<(Arc<str>, Span)>,
}

impl Meta {
    pub fn at(span: Span) -> Meta {
        Meta {
            span,
            expansion: Vec::new(),
        }
    }
}

/// Structural equality, ignoring spans and expansion chains.
///
/// Salsa needs `Eq` to decide whether a re-executed query actually produced a different value, and
/// *formatting is explicitly not part of a `Node`'s identity* (§2.2) — so equality is exactly
/// [`Node::structurally_eq`], and a re-parse that moved a span does not invalidate anything
/// downstream.
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.structurally_eq(other)
    }
}

impl Eq for Node {}

#[derive(Clone, Debug)]
pub struct Node {
    pub head: Head,
    pub args: Vec<Node>,
    /// Whether this node was *written as an application*.
    ///
    /// §2.2's model has `args: list[Node]`, which leaves `(params)` and `params` indistinguishable
    /// — and an empty parameter list is not the same thing as a reference to a variable called
    /// `params`. Elixir solves this by giving a variable `nil` args where a call has a list; this
    /// is the same distinction with a cheaper representation.
    pub applied: bool,
    pub meta: Meta,
}

/// The reserved heads. Named constants rather than string literals scattered through the compiler:
/// a typo in `"paramss"` would otherwise be a silently-unmatched form.
pub mod sym {
    pub const MODULE: &str = "module";
    pub const DEF: &str = "def";
    pub const PARAMS: &str = "params";
    /// A `def`'s type parameters — `def map[T, U](…)`. Always present on a `def`, empty when the
    /// definition is monomorphic, so that the form has one shape (`docs/29` §29.7).
    pub const TYPARAMS: &str = "typarams";
    pub const RETURNS: &str = "returns";
    pub const ANNOT: &str = ":";
    pub const FN: &str = "fn";
    pub const CALL: &str = "call";
    pub const DOT: &str = ".";
    pub const IF: &str = "if";
    pub const LET: &str = "let";
    pub const VAR: &str = "var";
    pub const SET: &str = "set";
    pub const DO: &str = "do";
    pub const RETURN: &str = "return";
    pub const MATCH: &str = "match";
    pub const CASE: &str = "case";
    pub const FOR: &str = "for";
    pub const WHILE: &str = "while";
    pub const LIST: &str = "list";
    pub const MAP: &str = "map-lit";
    pub const RECORD: &str = "record";
    pub const MODEL: &str = "model";
    pub const UNION: &str = "union";
    pub const VARIANT: &str = "variant";
    pub const FIELD: &str = "field";
    pub const TYPE: &str = "type";
    pub const NEWTYPE: &str = "newtype";
    pub const TRAIT: &str = "trait";
    pub const IMPL: &str = "impl";
    pub const IMPORT: &str = "import";
    pub const MACRO: &str = "macro";
    pub const QUOTE: &str = "quote";
    pub const UNQUOTE: &str = "unquote";
    pub const SPLICE: &str = "unquote-splicing";
    pub const DECORATE: &str = "decorate";
    pub const ON: &str = "on";
    pub const UI: &str = "ui";
    pub const KW_ARG: &str = "kw";
    pub const WILDCARD: &str = "_";
    pub const SERVICE: &str = "service";
    pub const STYLES: &str = "styles";
    pub const DOCUMENT: &str = "document";

    // ---- §21.2's test construct. A test is a log, a command and an expectation, so each of the
    // three is a form of its own rather than a call the checker would have to recognise by name.
    pub const TEST: &str = "test";
    /// `(property "name" (params …) (do …))` — §11.10's generated-input sibling of `test`.
    pub const PROPERTY: &str = "property";
    /// `(given <list[Event]> <actor?>)` — the state, as the log that reaches it.
    pub const GIVEN: &str = "given";
    /// `(when <session|_> <command> …)` — proposals through the real `validate`.
    pub const WHEN: &str = "when";
    /// `(expect <Bool>)`.
    pub const EXPECT: &str = "expect";
    /// `(expect-contains <Str> <actor?>)` — `expect page contains "milk"`. The subject is always
    /// the rendered page, for the actor named or the test's default one.
    pub const EXPECT_CONTAINS: &str = "expect-contains";
    /// `(expect-fold <list[Event]> <actor?>)` — `expect state == fold_of [ … ]`.
    pub const EXPECT_FOLD: &str = "expect-fold";
    /// `(expect-place <name> <tier>)` — answered without running anything.
    pub const EXPECT_PLACE: &str = "expect-place";
    /// `(expect-flow <Type> <tier>)`.
    pub const EXPECT_FLOW: &str = "expect-flow";
    /// `(expect-wire "previous.becki")`.
    pub const EXPECT_WIRE: &str = "expect-wire";
    /// `(expect-effect "<atom>" (none|once|times <n>|with <expr>))` — §21.3 rule 4: verification is
    /// a query over what happened, not an expectation set in advance.
    pub const EXPECT_EFFECT: &str = "expect-effect";
    /// `(stub "<atom>" <value>)` — §21.3 rule 2: name the effect, not the shape.
    pub const STUB: &str = "stub";
    /// `(stub "<atom>" (arms (case …) …))` — §21.3 rule 3. The arms have no scrutinee written
    /// because only the checker knows what performs the effect, and therefore what its argument is.
    pub const STUB_ARMS: &str = "arms";

    /// Names the checker matches as *forms* before it resolves anything.
    ///
    /// A definition called one of these would be shadowed by the form and never called — silently,
    /// because `record(x)` is a well-formed record literal whatever `record` is bound to. The
    /// checker rejects such a definition by name rather than letting it be quietly unreachable.
    pub const RESERVED_FORMS: &[&str] = &[
        CALL, DO, FN, IF, LIST, MAP, MATCH, QUOTE, RECORD, RETURN, SET, UNQUOTE, SPLICE, KW_ARG,
        DOT,
    ];
}

impl Node {
    pub fn sym(name: impl AsRef<str>, span: Span) -> Node {
        Node {
            head: Head::Sym(Symbol::new(name)),
            args: Vec::new(),
            applied: false,
            meta: Meta::at(span),
        }
    }

    pub fn symbol(sym: Symbol, span: Span) -> Node {
        Node {
            head: Head::Sym(sym),
            args: Vec::new(),
            applied: false,
            meta: Meta::at(span),
        }
    }

    pub fn lit(lit: Lit, span: Span) -> Node {
        Node {
            head: Head::Lit(lit),
            args: Vec::new(),
            applied: false,
            meta: Meta::at(span),
        }
    }

    pub fn form(head: impl AsRef<str>, args: Vec<Node>, span: Span) -> Node {
        Node {
            head: Head::Sym(Symbol::new(head)),
            args,
            applied: true,
            meta: Meta::at(span),
        }
    }

    pub fn form_sym(head: Symbol, args: Vec<Node>, span: Span) -> Node {
        Node {
            head: Head::Sym(head),
            args,
            applied: true,
            meta: Meta::at(span),
        }
    }

    pub fn span(&self) -> Span {
        self.meta.span
    }

    pub fn head_sym(&self) -> Option<&Symbol> {
        match &self.head {
            Head::Sym(s) => Some(s),
            Head::Lit(_) => None,
        }
    }

    pub fn head_name(&self) -> Option<&str> {
        self.head_sym().map(|s| s.as_str())
    }

    pub fn as_lit(&self) -> Option<&Lit> {
        match &self.head {
            Head::Lit(l) if !self.applied => Some(l),
            _ => None,
        }
    }

    pub fn as_str_lit(&self) -> Option<&str> {
        match self.as_lit() {
            Some(Lit::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_keyword(&self) -> Option<&str> {
        match self.as_lit() {
            Some(Lit::Keyword(k)) => Some(k),
            _ => None,
        }
    }

    /// A bare identifier: symbol head, not applied.
    pub fn as_var(&self) -> Option<&Symbol> {
        match &self.head {
            Head::Sym(s) if !self.applied => Some(s),
            _ => None,
        }
    }

    /// An application with this exact head name — `(params)` counts, a bare `params` does not.
    pub fn is_form(&self, head: &str) -> bool {
        self.applied && self.head_name() == Some(head)
    }

    /// `(head ...)` or a bare `head`.
    pub fn has_head(&self, head: &str) -> bool {
        self.head_name() == Some(head)
    }

    pub fn arg(&self, i: usize) -> Option<&Node> {
        self.args.get(i)
    }

    /// Structural equality ignoring spans and expansion chains — what tests and the round-trip
    /// property compare, since formatting is explicitly not part of a `Node`'s identity.
    pub fn structurally_eq(&self, other: &Node) -> bool {
        let heads = match (&self.head, &other.head) {
            (Head::Sym(a), Head::Sym(b)) => a.name == b.name && a.scopes == b.scopes,
            (Head::Lit(a), Head::Lit(b)) => a == b,
            _ => false,
        };
        heads
            && self.applied == other.applied
            && self.args.len() == other.args.len()
            && self
                .args
                .iter()
                .zip(&other.args)
                .all(|(a, b)| a.structurally_eq(b))
    }

    /// Rewrite every symbol in the tree. The expander's workhorse.
    pub fn map_symbols(&self, f: &mut impl FnMut(&Symbol) -> Symbol) -> Node {
        let head = match &self.head {
            Head::Sym(s) => Head::Sym(f(s)),
            Head::Lit(l) => Head::Lit(l.clone()),
        };
        Node {
            head,
            args: self.args.iter().map(|a| a.map_symbols(f)).collect(),
            applied: self.applied,
            meta: self.meta.clone(),
        }
    }

    /// Add a scope to every symbol in the tree.
    pub fn add_scope(&self, s: Scope) -> Node {
        self.map_symbols(&mut |sym| sym.with_scopes(sym.scopes.insert(s)))
    }

    /// Flip a scope on every symbol in the tree (see [`ScopeSet::flip`]).
    pub fn flip_scope(&self, s: Scope) -> Node {
        self.map_symbols(&mut |sym| sym.with_scopes(sym.scopes.flip(s)))
    }

    /// Record that this subtree came out of a macro, for §4.5's expansion chain in diagnostics.
    pub fn with_expansion(&self, name: Arc<str>, at: Span) -> Node {
        let mut meta = self.meta.clone();
        meta.expansion.push((name.clone(), at));
        Node {
            head: self.head.clone(),
            args: self
                .args
                .iter()
                .map(|a| a.with_expansion(name.clone(), at))
                .collect(),
            applied: self.applied,
            meta,
        }
    }

    pub fn node_count(&self) -> usize {
        1 + self.args.iter().map(Node::node_count).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_sets_are_sorted_sets_with_subset_and_flip() {
        let e = ScopeSet::empty();
        let a = e.insert(Scope(3)).insert(Scope(1)).insert(Scope(3));
        assert_eq!(a.len(), 2);
        assert!(e.is_subset_of(&a));
        assert!(!a.is_subset_of(&e));
        assert!(a.is_subset_of(&a));

        let flipped = a.flip(Scope(1));
        assert!(!flipped.contains(Scope(1)));
        assert!(flipped.flip(Scope(1)).is_subset_of(&a) && a.is_subset_of(&flipped.flip(Scope(1))));

        let b = e.insert(Scope(2));
        assert!(!b.is_subset_of(&a));
        assert!(!a.is_subset_of(&b));
    }

    #[test]
    fn flipping_a_scope_over_a_tree_is_an_involution() {
        let n = Node::form(
            "def",
            vec![Node::sym("x", Span::NONE), Node::sym("y", Span::NONE)],
            Span::NONE,
        );
        assert!(n
            .flip_scope(Scope(7))
            .flip_scope(Scope(7))
            .structurally_eq(&n));
        assert!(!n.flip_scope(Scope(7)).structurally_eq(&n));
    }

    #[test]
    fn structural_equality_ignores_spans() {
        let mut map = beck_diag::SourceMap::new();
        let f = map.add("a", "xy");
        let a = Node::sym("x", Span::new(f, 0..1));
        let b = Node::sym("x", Span::new(f, 1..2));
        assert!(a.structurally_eq(&b));
    }
}
