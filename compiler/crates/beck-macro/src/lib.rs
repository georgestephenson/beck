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
//! # What a macro body may do
//!
//! Anything a pure Beck function may do. [`interp`] is the compile-time interpreter §2.4 calls for
//! — bindings, `if`, `for`, `while`, lambdas, calls to the module's own `def`s and to the pure
//! part of the prelude — in a **capability-restricted** environment: there is no name for a file,
//! a socket, a clock or a process, and the prelude's effectful primitives are refused by name so
//! that reaching for one is a diagnostic rather than a spelling mistake.
//!
//! `quote:` is the form whose value is *syntax*, and `$e` inside one is an ordinary expression
//! whose value is reflected back into the template — so `$x` where `x` is a parameter is the
//! caller's code, and `$(n * 2)` is a literal. That is the whole difference from the template
//! expander this used to be: a `let` in a macro body now *computes* rather than substituting.
//!
//! And `refuse("…")` is how a body says it has no rule for what it was given — a code generator
//! that meets something it cannot write for otherwise emits code that fails to check somewhere
//! else, with a message about lines the reader never wrote.
//!
//! # The two phases
//!
//! An ordinary `macro` is expanded here, before anything has been checked. A `typed macro` is left
//! exactly as written by this pass and expanded by the **checker**, because its body asks what its
//! arguments were inferred to be; [`typed`] is that half, and it is the same interpreter with one
//! more name in scope.

use std::collections::HashMap;
use std::sync::Arc;

use beck_diag::depth::Nesting;
use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{sym, Lit, Node, Scope, Symbol};

pub mod interp;
pub mod typed;
mod ui;
pub mod vocabulary;

pub use interp::{Val, BUILTINS, MAX_STEPS, RESTRICTED};
pub use typed::{DeclInfo, Fields, TyKind, TyRepr, TypeEnv, TypedExpander, Variants};
pub use ui::expand_ui;

/// How deep a macro may expand before the expander decides it is not going to terminate.
///
/// This counts *expansions* — a macro whose output is another call to itself — and nothing else.
/// Walking into a form's arguments is not an expansion, and counting it here is what used to make
/// a 65-level-deep expression with no macros in it report that macro expansion had not terminated.
/// The structural walk is bounded by [`Nesting`] against the ceiling the whole front end shares.
pub const MAX_DEPTH: u32 = 64;

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
/// ([`docs/82`](../../../../docs/82-the-edge-report.md) §82.5 is the same
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

/// The expansion budget's refusal, in one place because two callers report it: the meter itself,
/// and a checker whose probe threw the first report away ([`typed::TypedExpander::exhausted`]).
pub(crate) fn too_much(span: Span) -> Diagnostic {
    Diagnostic::error("B0214", "macro expansion produced too much", span)
        .with_primary_label("the expander stopped here")
        .with_note(format!(
            "the budget is {MAX_EXPANSION} nodes for the whole module, and expansion is bounded by \
             what it *produces* rather than by how deep it goes: a macro that doubles its output is \
             shallow, terminates, and is enormous"
        ))
}

#[derive(Clone, Debug)]
struct MacroDef {
    name: Arc<str>,
    params: Vec<Arc<str>>,
    /// The `do` block of the macro's body.
    body: Node,
    span: Span,
    /// `typed macro` — expanded by the checker rather than here (§2.4, [`typed`]). Collected in
    /// the same table as an untyped one so that the flat namespace is one namespace: two macros of
    /// a name collide whichever kind they are, and `B0200` is what says so.
    typed: bool,
}

pub struct Expander<'a> {
    macros: HashMap<Arc<str>, MacroDef>,
    /// The module's own `def`s, callable from a macro body (§2.4's "reads of the declared module
    /// graph").
    defs: HashMap<Arc<str>, interp::FnDef>,
    next_scope: u32,
    /// What is left of [`interp::MAX_STEPS`] for the whole module, and whether it ran out.
    steps: u64,
    steps_spent: bool,
    /// What is left of [`MAX_EXPANSION`], in nodes, for the whole module.
    fuel: u64,
    /// Whether the budget ran out, so the diagnostic is reported once rather than at every call
    /// that would have expanded afterwards.
    spent: bool,
    /// How deep into the tree this walk is. Separate from the expansion depth above, because they
    /// bound different things and only one of them means a macro is misbehaving.
    nesting: Nesting,
    /// What the checker inferred about the call being expanded — `Some` only while a
    /// [`typed::TypedExpander`] is driving, which is the whole difference between the two phases.
    types: Option<&'a TypeEnv>,
    diags: &'a mut Diagnostics,
}

/// Expand every macro in a module to a fixpoint.
pub fn expand_module(module: &Node, diags: &mut Diagnostics) -> Node {
    expand_module_measured(module, diags).0
}

/// The same, with the macros of the modules this one **imports** in scope.
///
/// [`docs/02`](../../../../docs/02-syntax.md) §2.4: a macro is a declaration like any other, and
/// a module that imports another gets its declarations. Until this existed a macro was usable in
/// the file that declared it and nowhere else — not refused, simply absent — which is what kept
/// §2.4's `derive` and §2.5's `sql"…"` out of `lib/` and made every macro an example rather than a
/// facility.
///
/// `imported` are the **parsed** modules, in any order: a macro body is compile-time callable as it
/// was *written*, before expansion, so what a macro needs from
/// another module is its source and not its interface. That is also the limit — an import that is
/// an interface and no implementation publishes signatures, and a macro has none.
///
/// Names are merged flat, which is the language's own model rather than a shortcut here: Beck links
/// modules into one namespace with no qualified reference (`B0601`), so a macro imported from one
/// module and a macro declared in this one collide exactly as two `def`s of one name do, and
/// `B0200` is what says so.
pub fn expand_module_with(module: &Node, imported: &[&Node], diags: &mut Diagnostics) -> Node {
    expand_module_inner(module, imported, diags).0
}

/// Expand a module, and say how much of the interpreter's step budget is left.
///
/// The second half is what makes [`interp::MAX_STEPS`]'s doc comment a measurement rather than an
/// assertion: `macro_interp.rs` expands the most expensive macro body here and prints what it
/// cost. Nothing in the compiler reads it.
pub fn expand_module_measured(module: &Node, diags: &mut Diagnostics) -> (Node, u64) {
    expand_module_inner(module, &[], diags)
}

fn expand_module_inner(module: &Node, imported: &[&Node], diags: &mut Diagnostics) -> (Node, u64) {
    let mut ex = Expander::collecting(diags);
    // The imports first, so a macro this module declares shadows nothing silently: `B0200` fires on
    // the second definition of a name, and the second one is this module's.
    //
    // "Are there macros here at all" is asked of **every** module in play, this one included. Asked
    // of the imports alone it made whether a macro body could call an imported `def` depend on
    // whether that other module happened to declare a macro of its own — so adding an unused macro
    // to the imported file was the difference between `B0208` and a compile, and `B0208` states a
    // rule ("a `def` in this module") that was not the one being applied.
    let brings_macros = declares_a_macro(module) || imported.iter().any(|m| declares_a_macro(m));
    for m in imported {
        ex.collect_macros_from(m, brings_macros);
    }
    ex.collect_macros_from(module, brings_macros);

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
        // A `typed macro` survives this phase untouched, and is consumed by the checker. Its body
        // is a template like any macro's, so expanding *into* it would expand code that has not
        // been called yet.
        if item.is_form(sym::TYPED_MACRO) {
            items.push(item.clone());
            continue;
        }
        let expanded = ex.expand(item, 0);
        // `splice([…])` at the top of a module is several items where one was written — §2.4's
        // `derive` returns the definition it decorated *and* the impls it generated.
        //
        // Flattened all the way down rather than one level: `derive` is handed a **block**, which
        // is already a `do`, and returns it beside what it generated — so the answer is a `do`
        // holding a `do`, and stopping at the first would leave a block where an item belongs.
        flatten_into(&expanded, &mut items);
    }
    let steps_left = ex.steps;
    (
        Node::form_sym(
            module
                .head_sym()
                .cloned()
                .unwrap_or_else(|| Symbol::new(sym::MODULE)),
            items,
            module.span(),
        ),
        steps_left,
    )
}

/// Whether a parsed module declares a **typed** macro — the question the checker asks before
/// collecting anything, since only a typed macro is its to expand.
pub(crate) fn declares_a_typed_macro(module: &Node) -> bool {
    module.args.iter().any(|i| i.is_form(sym::TYPED_MACRO))
}

/// Whether a parsed module declares a macro of either kind — the one question worth asking before
/// copying anything out of it.
pub(crate) fn declares_a_macro(module: &Node) -> bool {
    module
        .args
        .iter()
        .any(|i| i.is_form(sym::MACRO) || i.is_form(sym::TYPED_MACRO))
}

/// Every item a macro's answer stands for, with the `do`s it is wrapped in taken off.
///
/// One `do` is `splice([…])`; two is `splice([do, impl])` where `do` is the block the macro was
/// given, which is what §2.4's `derive` returns. Neither is a construct a program wrote at module
/// level, so both are unwrapped, and a `do` that a *program* wrote there was already refused as an
/// unsupported top-level item.
fn flatten_into(node: &Node, out: &mut Vec<Node>) {
    match node.is_form(sym::DO) {
        true => node.args.iter().for_each(|a| flatten_into(a, out)),
        false => out.push(node.clone()),
    }
}

impl<'a> Expander<'a> {
    /// An expander with nothing collected and the module's budgets full.
    pub(crate) fn collecting(diags: &'a mut Diagnostics) -> Expander<'a> {
        Expander {
            macros: HashMap::new(),
            defs: HashMap::new(),
            next_scope: 1,
            steps: interp::MAX_STEPS,
            steps_spent: false,
            fuel: MAX_EXPANSION,
            spent: false,
            nesting: Nesting::new(),
            types: None,
            diags,
        }
    }

    /// The macros and compile-time-callable `def`s of one module.
    ///
    /// `elsewhere` says whether some *other* module in scope declares a macro. A module with no
    /// macros of its own pays nothing for the interpreter — collecting the `def`s copies a body
    /// each, which is proportional to the whole module, and the overwhelming majority of modules
    /// have nothing that could ever call one. That guard has to widen by exactly one word once
    /// macros are importable: a module with no macros that *imports* one still has to hand over its
    /// definitions, because the imported macro's body may call them.
    pub(crate) fn collect_macros_from(&mut self, module: &Node, elsewhere: bool) {
        let has_macros = elsewhere || declares_a_macro(module);

        for item in &module.args {
            // A `def` is callable from a macro body, as the definition was *written*: expansion
            // has not run yet, so a `def` whose body calls a macro is not compile-time callable.
            // The alternative would be an expansion order that depends on who calls what.
            if has_macros
                && item.is_form(sym::DEF)
                && item.args.len() >= 6
                && item.args[2].is_form(sym::PARAMS)
            {
                if let (Some(name), Some(body)) = (item.args[0].as_var(), item.args.last()) {
                    self.defs.insert(
                        name.name.clone(),
                        interp::FnDef {
                            params: interp::param_names(&item.args[2]),
                            body: body.clone(),
                            span: item.span(),
                        },
                    );
                }
                continue;
            }
            let typed = item.is_form(sym::TYPED_MACRO);
            if !(item.is_form(sym::MACRO) || typed) || item.args.len() < 3 {
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
                typed,
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

    /// A scope no other expansion has, **in either phase**.
    ///
    /// Two expanders run over one module — this one before the checker and
    /// [`typed::TypedExpander`] inside it — and a scope both of them minted would make a binding one
    /// introduced visible to a reference the other introduced. That is hygiene failing, and it is
    /// the one way it can fail *silently*, so the two are kept apart by **parity** rather than by an
    /// argument about how many scopes either can spend: this one counts the odd numbers and the
    /// typed expander counts the even ones. A bound would have had to hold for expansions that
    /// mint a scope and then *fail* — an arity error mints one and charges nothing — and the number
    /// of those is bounded only by how many macro calls a source file can hold.
    fn fresh_scope(&mut self) -> Scope {
        let s = Scope(self.next_scope);
        self.next_scope += 2;
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
    pub(crate) fn charge(&mut self, out: &Node, span: Span) -> bool {
        let mut stack = vec![out];
        while let Some(node) = stack.pop() {
            if self.fuel == 0 {
                if !self.spent {
                    self.spent = true;
                    self.diags.push(too_much(span));
                }
                return false;
            }
            self.fuel -= 1;
            stack.extend(node.args.iter());
        }
        true
    }

    /// Expand a node bottom-up, then re-expand if the node itself was a macro call.
    pub(crate) fn expand(&mut self, n: &Node, depth: u32) -> Node {
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
        // A typed macro is the checker's to expand: its body asks what its arguments were inferred
        // to be, and nothing has been inferred yet. Left exactly as written, so the call site the
        // checker reports against is the one somebody typed.
        if def.typed {
            return here;
        }

        match self.apply_macro(&def, &here) {
            Some(out) if self.charge(&out, here.span()) => self.expand(&out, depth + 1),
            // The macro was found and did not produce code — it refused, ran out of budget, or was
            // called wrongly, and each of those has already been reported. Leaving the call here
            // would have the checker report a *second* thing, that it cannot find the name.
            _ => Node::form(sym::REFUSED, Vec::new(), here.span()),
        }
    }

    /// One expansion step: the four-part dance described at the top of this module.
    pub(crate) fn apply_macro(&mut self, def: &MacroDef, call: &Node) -> Option<Node> {
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

        let mut env: HashMap<Arc<str>, Val> = HashMap::new();
        let mut pos = positional.into_iter();
        for p in &def.params {
            let bound = named.remove(p).or_else(|| pos.next());
            match bound {
                Some(v) => {
                    env.insert(p.clone(), Val::Syntax(v));
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

        let out = self.macro_result(def, env, call.span())?;
        // The flip: call-site identifiers lose the scope they gained, template identifiers gain it.
        Some(
            out.flip_scope(scope)
                .with_expansion(def.name.clone(), call.span()),
        )
    }

    /// Run the macro body, and take the syntax it returned.
    ///
    /// The body is ordinary Beck ([`interp`]); the module's step budget is threaded through here
    /// rather than owned by the interpreter, because a module compiles once and its macros share
    /// what that compile is allowed to cost.
    fn macro_result(
        &mut self,
        def: &MacroDef,
        env: HashMap<Arc<str>, Val>,
        call: Span,
    ) -> Option<Node> {
        let mut interp =
            interp::Interp::new(&self.defs, &mut *self.diags, self.steps, self.steps_spent)
                .knowing(self.types)
                .called_at(call);
        let out = interp.run_body(&def.name, &def.body, env, def.span);
        self.steps = interp.steps;
        self.steps_spent = interp.exhausted;
        out
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
    fn a_let_in_a_macro_body_computes_rather_than_substituting() {
        // The one semantic change the interpreter made to a body that already worked: a `let`
        // whose right-hand side is not a `quote` used to be instantiated as a *template*, so
        // `n = 2 + 3` bound the syntax `2 + 3`. It now binds `5`, and `$n` is the literal.
        let (out, d, map) = expand(
            "macro five(x):\n\
             \x20   n = 2 + 3\n\
             \x20   return quote:\n\
             \x20       $n + $x\n\
             \n\
             def f() -> Int:\n\
             \x20   return five(1)\n",
        );
        assert!(!d.has_errors(), "{}", d.render(&map));
        assert!(crate::ui::tests_strip(&out).contains("(+ 5 1)"), "{out}");
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
