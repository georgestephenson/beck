//! Macro expansion, hygienic from the first commit.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md) Phase 1: "Macro expander with hygiene —
//! **from the start** (§2.4); retrofitting hygiene is a rewrite." `docs/02-syntax.md` §2.4:
//! "Identifiers introduced inside a `quote` get a fresh hygiene scope in `Node.meta`; capture is
//! possible but must be explicit (`inject(name)`)."
//!
//! # The algorithm
//!
//! Flatt's *sets of scopes*, which is the model Racket settled on after, as §2.4 puts it, "Scheme's
//! 20-year history here". Each expansion step:
//!
//! 1. mints a fresh [`Scope`];
//! 2. adds it to the macro's **input** — every identifier the call site supplied;
//! 3. substitutes the arguments into the template;
//! 4. **flips** the scope over the whole result.
//!
//! The flip is what makes it work. Identifiers that came from the call site had the scope added in
//! step 2 and lose it again in step 4, so they mean what they meant where they were written.
//! Identifiers the template introduced never had it, so they gain it — and a binding carrying a
//! scope the call site does not have is invisible to the call site's references. Capture becomes
//! impossible in both directions rather than unlikely.
//!
//! Resolution itself lives in `beck-types`: a binding is a candidate for a reference exactly when
//! `binding.scopes ⊆ reference.scopes`, and the most specific candidate wins.
//!
//! # What Phase 1 supports
//!
//! Template macros: a body of `let`s and a final `return quote: …` with `$x` unquotes and `$*xs`
//! splices, expanded to a fixpoint. Macro bodies that need arbitrary compile-time *computation* —
//! §2.4's `derive`, which builds impls by iterating over a model's fields — are served here by
//! compiler-provided macros (`ui`) rather than by a general Beck interpreter running at compile
//! time. That interpreter is Phase 2 work; the hygiene machinery it will run on is not.

use std::collections::HashMap;
use std::sync::Arc;

use beck_diag::depth::Nesting;
use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{sym, Head, Lit, Node, Scope, Symbol};

mod ui;

pub use ui::expand_ui;

/// How deep a macro may expand before the expander decides it is not going to terminate.
///
/// This counts *expansions* — a macro whose output is another call to itself — and nothing else.
/// Walking into a form's arguments is not an expansion, and counting it here is what used to make
/// a 65-level-deep expression with no macros in it report that macro expansion had not terminated.
/// The structural walk is bounded by [`Nesting`] against the ceiling the whole front end shares.
const MAX_DEPTH: u32 = 64;

/// How many nodes a module's macros may **produce**, in total.
///
/// `MAX_DEPTH` and [`Nesting`] bound how deep expansion goes and neither bounds how much it makes,
/// which is [`14`](../../../../docs/14-review-findings.md)'s F17: *a macro that doubles its output
/// at each of a few levels is shallow, terminating, and enormous*. Eight nestings of a two-line
/// macro is 256 copies of its argument; sixty nestings is 10^18 of them, which is more nodes than a
/// machine has bytes — and every one of those programs is six lines long and passes every other
/// limit the front end has.
///
/// So the meter is what expansion **produces**, charged per node and shared by the whole module —
/// per module because that is what a compile is, and because a per-call budget would let a program
/// spend it as many times as it has calls
/// ([`84`](../../../../docs/84-a-quota-is-only-as-good-as-its-actor-report.md) §84.4 is the same
/// arithmetic one subsystem over).
///
/// The number is measured rather than declared. Across every program in this repository — the
/// corpus, both benchmark suites, both SICP chapters, the examples and the standard library — the
/// **largest total expansion is 138 nodes** (`sicp/ch3.beck`; `examples/todo.beck`'s page is 94), so
/// a hundred thousand is about 725× the biggest real one. It is also about seventeen nestings of a
/// doubling macro, which is a few dozen characters more source than a program that compiles — and
/// that is the room a limit wants when what it separates is *legitimate* from *absurd* rather than
/// big from small.
///
/// `macro_bomb.rs` is the gate, in both directions: the tree still compiles, and a doubling macro is
/// refused.
pub const MAX_EXPANSION: u64 = 100_000;

#[derive(Clone, Debug)]
struct MacroDef {
    name: Arc<str>,
    params: Vec<Arc<str>>,
    /// The `do` block of the macro's body.
    body: Node,
    span: Span,
}

pub struct Expander<'a> {
    macros: HashMap<Arc<str>, MacroDef>,
    next_scope: u32,
    /// What is left of [`MAX_EXPANSION`], in nodes, for the whole module.
    fuel: u64,
    /// Whether the budget ran out, so the diagnostic is reported once rather than at every call
    /// that would have expanded afterwards.
    spent: bool,
    /// How deep into the tree this walk is. Separate from the expansion depth above, because they
    /// bound different things and only one of them means a macro is misbehaving.
    nesting: Nesting,
    diags: &'a mut Diagnostics,
}

/// Expand every macro in a module to a fixpoint.
pub fn expand_module(module: &Node, diags: &mut Diagnostics) -> Node {
    let mut ex = Expander {
        macros: HashMap::new(),
        next_scope: 1,
        fuel: MAX_EXPANSION,
        spent: false,
        nesting: Nesting::new(),
        diags,
    };
    ex.collect_macros(module);

    let mut items = Vec::with_capacity(module.args.len());
    for (i, item) in module.args.iter().enumerate() {
        // args[0] is the module name.
        if i == 0 {
            items.push(item.clone());
            continue;
        }
        // A `macro` definition is consumed by the expander and does not survive into the program.
        if item.is_form(sym::MACRO) {
            continue;
        }
        items.push(ex.expand(item, 0));
    }
    Node::form_sym(
        module
            .head_sym()
            .cloned()
            .unwrap_or_else(|| Symbol::new(sym::MODULE)),
        items,
        module.span(),
    )
}

impl<'a> Expander<'a> {
    fn collect_macros(&mut self, module: &Node) {
        for item in &module.args {
            if !item.is_form(sym::MACRO) || item.args.len() < 3 {
                continue;
            }
            let Some(name) = item.args[0].as_var() else {
                continue;
            };
            let params: Vec<Arc<str>> = item.args[1]
                .args
                .iter()
                .filter_map(|p| {
                    let target = if p.is_form(sym::ANNOT) { &p.args[0] } else { p };
                    target.as_var().map(|s| s.name.clone())
                })
                .collect();
            let def = MacroDef {
                name: name.name.clone(),
                params,
                body: item.args[2].clone(),
                span: item.span(),
            };
            if self.macros.insert(name.name.clone(), def).is_some() {
                self.diags.push(
                    Diagnostic::error(
                        "B0200",
                        format!("macro `{name}` is defined twice"),
                        item.span(),
                    )
                    .with_primary_label("a later definition would silently win"),
                );
            }
        }
    }

    fn fresh_scope(&mut self) -> Scope {
        let s = Scope(self.next_scope);
        self.next_scope += 1;
        s
    }

    /// Charge what one expansion produced against the module's budget.
    ///
    /// **Iterative**, with its own stack, for the reason the walk counts at all: the tree being
    /// measured is one a macro just built, so a recursive count would be a claim about the host's
    /// stack rather than about the program
    /// ([`93`](../../../../docs/93-the-native-backends-report.md) §93.9 is the same defect one
    /// subsystem over). It also stops the moment the budget does, so the *accounting* is bounded by
    /// the budget it is accounting for — a macro that produced a billion nodes is refused after a
    /// hundred thousand of them have been counted, not after a billion.
    fn charge(&mut self, out: &Node, span: Span) -> bool {
        let mut stack = vec![out];
        while let Some(node) = stack.pop() {
            if self.fuel == 0 {
                if !self.spent {
                    self.spent = true;
                    self.diags.push(
                        Diagnostic::error("B0214", "macro expansion produced too much", span)
                            .with_primary_label("the expander stopped here")
                            .with_note(format!(
                                "the budget is {MAX_EXPANSION} nodes for the whole module, and \
                                 expansion is bounded by what it *produces* rather than by how \
                                 deep it goes: a macro that doubles its output is shallow, \
                                 terminates, and is enormous"
                            )),
                    );
                }
                return false;
            }
            self.fuel -= 1;
            stack.extend(node.args.iter());
        }
        true
    }

    /// Expand a node bottom-up, then re-expand if the node itself was a macro call.
    fn expand(&mut self, n: &Node, depth: u32) -> Node {
        // Once the budget is gone nothing else is expanded: the module is not going to compile, and
        // carrying on would be spending the rest of the compile on a program already refused.
        if self.spent {
            return n.clone();
        }
        if depth > MAX_DEPTH {
            self.diags.push(
                Diagnostic::error("B0201", "macro expansion did not terminate", n.span())
                    .with_primary_label("expanded past the depth limit")
                    .with_note(format!("the limit is {MAX_DEPTH} nested expansions")),
            );
            return n.clone();
        }

        // A `quote` is data: its contents are a template, not code to expand. Unquotes inside it
        // *are* code, and are substituted by `instantiate` when the template is used.
        if n.is_form(sym::QUOTE) {
            return n.clone();
        }

        if !self.nesting.enter() {
            if self.nesting.should_report() {
                let note = self.nesting.note();
                self.diags.push(
                    Diagnostic::error("B0213", "the form nests too deep to expand", n.span())
                        .with_primary_label("the expander gave up here")
                        .with_note(note),
                );
            }
            return n.clone();
        }
        let expanded_args: Vec<Node> = n.args.iter().map(|a| self.expand(a, depth)).collect();
        self.nesting.leave();
        let here = Node {
            head: n.head.clone(),
            args: expanded_args,
            applied: n.applied,
            meta: n.meta.clone(),
        };

        // Compiler-provided macros. `ui` is one because its expansion is a *recursive rewrite of a
        // block's structure*, which template macros cannot express (§2.4's typed macros and a
        // compile-time interpreter are what generalise this, in Phase 2).
        if here.is_form(sym::UI) {
            // The same four-part dance as a user macro: add a fresh scope to the input, expand,
            // flip. Without the *add*, the flip would scope the user's own identifiers — `todos`
            // inside a `for` would stop referring to the `todos` the caller wrote.
            let s = self.fresh_scope();
            let out = ui::expand_ui(&here.add_scope(s), self.diags);
            if !self.charge(&out, here.span()) {
                return here;
            }
            return self.expand(&out.flip_scope(s), depth + 1);
        }

        if !here.applied {
            return here;
        }
        let Some(name) = here.head_name().map(|s| s.to_string()) else {
            return here;
        };
        let Some(def) = self.macros.get(name.as_str()).cloned() else {
            return here;
        };

        match self.apply_macro(&def, &here) {
            Some(out) if self.charge(&out, here.span()) => self.expand(&out, depth + 1),
            _ => here,
        }
    }

    /// One expansion step: the four-part dance described at the top of this module.
    fn apply_macro(&mut self, def: &MacroDef, call: &Node) -> Option<Node> {
        let scope = self.fresh_scope();

        // Keyword arguments bind by name — which is how the block rule's `do=` reaches a macro
        // parameter called `do` (§2.3).
        let mut positional: Vec<Node> = Vec::new();
        let mut named: HashMap<Arc<str>, Node> = HashMap::new();
        for a in &call.args {
            if a.is_form(sym::KW_ARG) && a.args.len() == 2 {
                if let Some(k) = a.args[0].as_var() {
                    named.insert(k.name.clone(), unquote_arg(&a.args[1]).add_scope(scope));
                    continue;
                }
            }
            positional.push(unquote_arg(a).add_scope(scope));
        }

        let mut env: HashMap<Arc<str>, Node> = HashMap::new();
        let mut pos = positional.into_iter();
        for p in &def.params {
            let bound = named.remove(p).or_else(|| pos.next());
            match bound {
                Some(v) => {
                    env.insert(p.clone(), v);
                }
                None => {
                    self.diags.push(
                        Diagnostic::error(
                            "B0202",
                            format!("macro `{}` expects an argument for `{p}`", def.name),
                            call.span(),
                        )
                        .with_label(def.span, "defined here"),
                    );
                    return None;
                }
            }
        }
        if pos.next().is_some() || !named.is_empty() {
            self.diags.push(
                Diagnostic::error(
                    "B0203",
                    format!("too many arguments for macro `{}`", def.name),
                    call.span(),
                )
                .with_label(def.span, "defined here"),
            );
            return None;
        }

        let template = self.macro_result(def, &env)?;
        let out = self.instantiate(&template, &env);
        // The flip: call-site identifiers lose the scope they gained, template identifiers gain it.
        Some(
            out.flip_scope(scope)
                .with_expansion(def.name.clone(), call.span()),
        )
    }

    /// Run the macro body far enough to reach the `quote` it returns.
    ///
    /// Phase 1's macro bodies are `let`-then-`return`; a `let` binds a name to a template fragment,
    /// which covers the shapes §2.4 shows and is honest about the rest.
    fn macro_result(&mut self, def: &MacroDef, env: &HashMap<Arc<str>, Node>) -> Option<Node> {
        let mut local = env.clone();
        for stmt in &def.body.args {
            if stmt.is_form(sym::LET) && stmt.args.len() == 2 {
                if let Some(name) = stmt.args[0].as_var() {
                    let value = self.instantiate(&stmt.args[1], &local);
                    local.insert(name.name.clone(), value);
                    continue;
                }
            }
            if stmt.is_form(sym::RETURN) {
                let Some(returned) = stmt.args.first() else {
                    self.diags.push(Diagnostic::error(
                        "B0204",
                        format!("macro `{}` returns nothing", def.name),
                        stmt.span(),
                    ));
                    return None;
                };
                // `return quote: …` yields the template; `return $x` yields a fragment directly.
                if returned.is_form(sym::QUOTE) && returned.args.len() == 1 {
                    let mut body = returned.args[0].clone();
                    // A one-statement `quote:` block is that statement, not a block.
                    if body.is_form(sym::DO) && body.args.len() == 1 {
                        body = body.args[0].clone();
                    }
                    return Some(body);
                }
                return Some(returned.clone());
            }
            self.diags.push(
                Diagnostic::error(
                    "B0205",
                    format!("unsupported statement in the body of macro `{}`", def.name),
                    stmt.span(),
                )
                .with_note(
                    "Phase 1 macro bodies are `let` bindings and a final `return quote: …`; \
                     arbitrary compile-time computation arrives with the macro interpreter",
                ),
            );
            return None;
        }
        self.diags.push(Diagnostic::error(
            "B0204",
            format!("macro `{}` returns nothing", def.name),
            def.span,
        ));
        None
    }

    /// Substitute `$x` and `$*xs` inside a template.
    fn instantiate(&mut self, template: &Node, env: &HashMap<Arc<str>, Node>) -> Node {
        if template.is_form(sym::UNQUOTE) && template.args.len() == 1 {
            let inner = &template.args[0];
            if let Some(v) = inner.as_var() {
                if let Some(bound) = env.get(&v.name) {
                    return bound.clone();
                }
                self.diags.push(
                    Diagnostic::error(
                        "B0206",
                        format!("`${v}` is not a macro parameter"),
                        template.span(),
                    )
                    .with_primary_label("unquoting an unbound name"),
                );
                return template.clone();
            }
            return self.instantiate(inner, env);
        }

        let mut args = Vec::with_capacity(template.args.len());
        for a in &template.args {
            // `$*xs` splices a list's elements into the surrounding form. Like `$xs`, the operand
            // is a *macro parameter*, so it is looked up rather than instantiated.
            if a.is_form(sym::SPLICE) && a.args.len() == 1 {
                let spliced = match a.args[0].as_var().and_then(|v| env.get(&v.name)) {
                    Some(bound) => bound.clone(),
                    None => self.instantiate(&a.args[0], env),
                };
                if spliced.is_form(sym::LIST) || spliced.is_form(sym::DO) {
                    args.extend(spliced.args.iter().cloned());
                } else {
                    args.push(spliced);
                }
                continue;
            }
            args.push(self.instantiate(a, env));
        }

        // A quoted `$f(...)` whose head is itself unquoted.
        let head = match &template.head {
            Head::Sym(s) => match env.get(&s.name) {
                Some(bound) if template.applied && !bound.applied => match &bound.head {
                    Head::Sym(bs) => Head::Sym(bs.clone()),
                    _ => Head::Sym(s.clone()),
                },
                _ => Head::Sym(s.clone()),
            },
            Head::Lit(l) => Head::Lit(l.clone()),
        };

        Node {
            head,
            args,
            applied: template.applied,
            meta: template.meta.clone(),
        }
    }
}

/// Strip the `quote` the block rule wraps a body in before handing it to a macro.
///
/// §2.3: "If the callee is a macro, it receives the AST. If it is a function, it receives a
/// thunk." `f(x):` desugars to `f(x, do=quote(block))`, so the quote is the *marker* that this
/// argument is syntax — a macro parameter should be bound to the block itself, not to a `quote`
/// node it would then have to unwrap by hand.
fn unquote_arg(n: &Node) -> Node {
    if n.is_form(sym::QUOTE) && n.args.len() == 1 {
        return n.args[0].clone();
    }
    n.clone()
}

/// Build a string literal node — shared with the `ui` builtin.
pub(crate) fn str_lit(s: impl AsRef<str>, span: Span) -> Node {
    Node::lit(Lit::Str(s.as_ref().into()), span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_diag::SourceMap;
    use beck_syntax::{parser, print};

    fn expand(src: &str) -> (String, Diagnostics, SourceMap) {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let module = parser::parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "parse: {}", d.render(&map));
        let out = expand_module(&module, &mut d);
        (print::to_sexpr(&out), d, map)
    }

    #[test]
    fn a_template_macro_expands() {
        let (out, d, map) = expand(
            "macro unless(cond, do):\n\
             \x20   return quote:\n\
             \x20       if not $cond:\n\
             \x20           $do\n\
             \n\
             def f() -> Int:\n\
             \x20   unless(ready):\n\
             \x20       wait()\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(
            crate::ui::tests_strip(&out).contains("(if (not ready)"),
            "{out}"
        );
        assert!(out.contains("(wait)"), "{out}");
        // The macro definition itself does not survive into the program.
        assert!(!out.contains("(macro"), "{out}");
    }

    #[test]
    fn hygiene_prevents_a_macro_binding_from_capturing_user_code() {
        // The macro introduces `tmp`; the caller's block also mentions `tmp`. The two must not be
        // the same binding — the caller's `tmp` refers to whatever `tmp` meant at the call site.
        let (out, d, map) = expand(
            "macro twice(do):\n\
             \x20   return quote:\n\
             \x20       tmp = 1\n\
             \x20       $do\n\
             \n\
             def f() -> Int:\n\
             \x20   tmp = 99\n\
             \x20   twice():\n\
             \x20       return tmp\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&map));
        // The macro-introduced binder carries a scope; the user's reference does not. Core form
        // heads are identifiers too and are scoped along with everything else, exactly as in
        // Racket — harmless, because forms are matched by name.
        assert!(
            out.contains("(let{1} tmp{1} 1)"),
            "macro binder should be scoped: {out}"
        );
        assert!(
            out.contains("(let tmp 99)"),
            "the user's own binding must stay unscoped: {out}"
        );
        assert!(
            out.contains("(return tmp)"),
            "the user's reference must stay unscoped: {out}"
        );
    }

    #[test]
    fn call_site_identifiers_come_back_to_their_own_scopes() {
        // Everything the caller passed in must print exactly as written: the scope added on the
        // way in is removed by the flip on the way out.
        let (out, d, _) = expand(
            "macro id(x):\n\
             \x20   return quote:\n\
             \x20       $x\n\
             \n\
             def f() -> Int:\n\
             \x20   return id(hello)\n",
        );
        assert!(!d.has_errors());
        assert!(
            crate::ui::tests_strip(&out).contains("(return hello)"),
            "{out}"
        );
    }

    #[test]
    fn splicing_inlines_a_list() {
        let (out, d, map) = expand(
            "macro all(items):\n\
             \x20   return quote:\n\
             \x20       total($*items)\n\
             \n\
             def f() -> Int:\n\
             \x20   return all([1, 2, 3])\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(
            crate::ui::tests_strip(&out).contains("(total 1 2 3)"),
            "{out}"
        );
    }

    #[test]
    fn a_nonterminating_macro_is_reported_rather_than_hanging() {
        let (_, d, _) = expand(
            "macro loopy(x):\n\
             \x20   return quote:\n\
             \x20       loopy($x)\n\
             \n\
             def f() -> Int:\n\
             \x20   return loopy(1)\n",
        );
        assert!(d.iter().any(|x| x.code == "B0201"));
    }

    #[test]
    fn arity_errors_name_the_macro_and_its_definition() {
        let (_, d, _) = expand(
            "macro two(a, b):\n\
             \x20   return quote:\n\
             \x20       pair($a, $b)\n\
             \n\
             def f() -> Int:\n\
             \x20   return two(1)\n",
        );
        assert!(d.iter().any(|x| x.code == "B0202"));
    }
}
