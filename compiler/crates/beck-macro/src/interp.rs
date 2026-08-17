//! The macro interpreter: Beck, evaluated at compile time.
//!
//! [`docs/02-syntax.md`](../../../../../docs/02-syntax.md) §2.4: "Macro bodies run at compile time
//! in the compiler's own Beck interpreter, with a *capability-restricted* environment: pure
//! computation and reads of the declared module graph, no ambient filesystem or network.
//! Non-negotiable — build reproducibility and the 'compile once, deploy many' model depend on it,
//! and it closes a real supply-chain hole that Rust `build.rs` and npm `postinstall` leave open."
//!
//! This is that interpreter. A macro body is ordinary Beck — bindings, `if`, `for`, `while`,
//! lambdas, calls to the pure part of the prelude and to the module's own `def`s — and `quote:`
//! is the one form whose value is *syntax* rather than a number or a string.
//!
//! # Why it is a second interpreter rather than `beck-eval`
//!
//! Untyped macros expand **before** the checker runs ([`docs/02`](../../../../../docs/02-syntax.md)
//! §2.4: "`macro` (untyped AST in, AST out, expands before type checking)"), so there is no
//! `Core` IR and no type for a macro body to be evaluated against — only [`Node`]. `beck-eval`
//! evaluates `Core`, and `beck-core` (which lowers to it) depends on this crate, so the dependency
//! could not run the other way even if the IR existed. The two are held together by a
//! differential rather than by sharing code: `beck-cli/tests/macro_interp.rs` computes the same
//! pure expressions at compile time and at run time and fails when the answers differ, which is
//! the same instrument [`docs/04`](../../../../../docs/04-compiler-architecture.md) §4.8 points at
//! the backends.
//!
//! Where an operation is somebody else's table — case mapping, substring replacement — this calls
//! `beck-prim`, the crate the evaluator and a compiled program already call
//! ([`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.12). Agreement there is
//! not a property of two implementations being careful; there is one implementation.
//!
//! # The sandbox
//!
//! The environment is a **whitelist**: a name resolves to a local, to one of the module's own
//! `def`s, or to one of the pure builtins in [`BUILTINS`] — and to nothing else. There is no name
//! for opening a file, reading the environment, starting a process or fetching a URL, because
//! nothing here defines one. The prelude's effectful names are refused *by name* rather than left
//! to fall out of the whitelist ([`RESTRICTED`]), so that a macro reaching for `now()` is told what
//! it did wrong instead of being told the name does not exist.
//!
//! That refusal is a claim, so it is a gate: `beck-cli/tests/macro_sandbox.rs` enumerates the
//! prelude and fails if an effectful primitive is missing from [`RESTRICTED`] or reachable from a
//! macro body.
//!
//! # What it does not have
//!
//! - **Unions**, and therefore no `Option`: the prelude's `str_to_int`, `str_index_of` and
//!   `list_get` return one, so they are not compile-time builtins. Indexing (`xs[i]`) is, and
//!   refuses out of range rather than answering `None`.
//! - **`match`**, for the same reason: its patterns are about variants.
//! - **The transcendentals.** `sqrt`, `sin` and `cos` would make the compiler's answer depend on
//!   the host's libm, which is F9's open question ([`docs/35`](../../../../../docs/35-standards-landscape.md)
//!   §35.5) and not a thing to prejudge from here.
//! - **Types.** A `typed macro` receives the AST with inferred types attached (§2.4) and needs the
//!   checker to have run; this interpreter is the untyped half.

use std::collections::HashMap;
use std::sync::Arc;

use beck_diag::depth::Nesting;
use beck_diag::{Diagnostic, Diagnostics, Span};
use beck_syntax::{print, sym, Head, Lit, Node, Symbol};

/// How many steps one module's macro bodies may take, in total.
///
/// The same shape as [`crate::MAX_EXPANSION`] and for the same reason: per module, because that is
/// what a compile is, and a per-call budget would let a program spend it once per call site.
///
/// A step is an expression evaluated or a statement run, so the number is a bound on *compile
/// time* rather than on a program's size — which is what makes it the answer to `while true:` in a
/// macro body. `macro_interp.rs::a_macro_body_that_does_not_terminate_is_refused` is the gate, and
/// two measurements set the size: `the_step_budget_is_far_above_what_a_real_macro_spends`
/// **prints** what the most expensive macro body here costs — **84 steps**, so a million is about
/// 12,000× the largest real one — and exhausting the whole budget costs under a second of
/// `beck check` in an unoptimised build. It is the room a limit wants when what it separates is
/// *legitimate* from *absurd* rather than big from small.
pub const MAX_STEPS: u64 = 1_000_000;

/// The prelude names a macro body may not call, with the effect atom each performs.
///
/// Reading the tree's own list rather than inventing one: these are the primitives whose scheme
/// carries an atom in `beck_core::prelude`, plus `http_fetch`, whose `net(host)` atom is derived
/// at the call site from the host it names
/// ([`adr/0013`](../../../../../docs/adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md))
/// rather than written in its scheme.
///
/// Nothing here is *reachable* — the interpreter's environment is a whitelist and none of these is
/// on it — so this list buys a diagnostic rather than a control. It is the difference between "a
/// macro may not read the clock" and "there is no name `now`", and only the first is true.
pub const RESTRICTED: &[(&str, &str)] = &[
    ("awareness", "cap.presence"),
    ("digest_keyed", "cap.sign"),
    ("durable", "durable"),
    ("http_fetch", "net(host)"),
    ("merge_clients", "ingress"),
    ("now", "nondet"),
    ("presence", "cap.presence"),
    ("reveal", "cap.internal"),
    ("secret_env", "env"),
    ("uuid", "nondet"),
];

/// A compile-time value.
///
/// [`Val::Syntax`] is the one that makes this a macro interpreter rather than a calculator: a
/// `quote:` block evaluates to one, `$e` inside a template puts one back, and every other variant
/// has a [reflection](Interp::reflect) into syntax so that `$n` where `n` is `3` is the literal
/// `3`.
#[derive(Clone, Debug)]
pub enum Val {
    Unit,
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Bool(bool),
    Keyword(Arc<str>),
    List(Arc<Vec<Val>>),
    Record(Arc<Vec<(Arc<str>, Val)>>),
    Syntax(Node),
    Fun(Arc<Lambda>),
}

impl Val {
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::Unit => "unit",
            Val::Int(_) => "Int",
            Val::Float(_) => "Float",
            Val::Str(_) => "Str",
            Val::Bool(_) => "Bool",
            Val::Keyword(_) => "keyword",
            Val::List(_) => "list",
            Val::Record(_) => "record",
            Val::Syntax(_) => "syntax",
            Val::Fun(_) => "function",
        }
    }

    fn str_(s: impl AsRef<str>) -> Val {
        Val::Str(Arc::from(s.as_ref()))
    }

    fn list(xs: Vec<Val>) -> Val {
        Val::List(Arc::new(xs))
    }
}

/// A lambda, with the frame it closed over.
#[derive(Debug)]
pub struct Lambda {
    params: Vec<Arc<str>>,
    body: Node,
    captured: HashMap<Arc<str>, Val>,
}

/// A module-level `def`, callable from a macro body.
///
/// The body is the one the parser produced, **before** expansion: a compile-time call runs the
/// definition as written. A `def` whose body calls a macro is therefore not callable at compile
/// time, which is honest — the alternative is expansion order that depends on who calls what.
#[derive(Clone, Debug)]
pub struct FnDef {
    pub params: Vec<Arc<str>>,
    pub body: Node,
    pub span: Span,
}

/// Evaluation stopped and a diagnostic has already been reported.
#[derive(Debug)]
pub struct Halt;

type Eval<T> = Result<T, Halt>;

/// What a statement did.
enum Flow {
    Fell,
    Returned(Val),
}

pub struct Interp<'a> {
    defs: &'a HashMap<Arc<str>, FnDef>,
    diags: &'a mut Diagnostics,
    /// What is left of [`MAX_STEPS`] for the whole module.
    pub steps: u64,
    /// Whether the budget ran out, so the diagnostic is reported once rather than at every macro
    /// that would have run afterwards.
    pub exhausted: bool,
    /// Host recursion, counted at the site the way
    /// [`adr/0012`](../../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md) asks:
    /// one counter over *every* recursive entry rather than one per grammar rule, so a deep
    /// expression and a deep chain of compile-time calls spend the same budget.
    nesting: Nesting,
}

impl<'a> Interp<'a> {
    pub fn new(
        defs: &'a HashMap<Arc<str>, FnDef>,
        diags: &'a mut Diagnostics,
        steps: u64,
        exhausted: bool,
    ) -> Interp<'a> {
        Interp {
            defs,
            diags,
            steps,
            exhausted,
            nesting: Nesting::new(),
        }
    }

    /// Run a macro body, given its parameters already bound to the syntax they were called with.
    ///
    /// The result is whatever the body returned, reflected into syntax — so `return quote: …` is
    /// a template and `return 6 * 7` is the literal `42`.
    pub fn run_body(
        &mut self,
        name: &str,
        body: &Node,
        env: HashMap<Arc<str>, Val>,
        def_span: Span,
    ) -> Option<Node> {
        let mut frame = env;
        match self.block(body, &mut frame) {
            Ok(Flow::Returned(v)) => {
                let span = body.span();
                self.reflect(&v, span).ok()
            }
            Ok(Flow::Fell) => {
                self.diags.push(Diagnostic::error(
                    "B0204",
                    format!("macro `{name}` returns nothing"),
                    def_span,
                ));
                None
            }
            Err(Halt) => None,
        }
    }

    // ---------------------------------------------------------------------------- the machinery

    fn step(&mut self, span: Span) -> Eval<()> {
        if self.steps == 0 {
            if !self.exhausted {
                self.exhausted = true;
                self.diags.push(
                    Diagnostic::error("B0215", "a macro body ran too long", span)
                        .with_primary_label("the interpreter stopped here")
                        .with_note(format!(
                            "the budget is {MAX_STEPS} steps for the whole module — a bound on how \
                             long a compile takes, which is what answers a macro body that does \
                             not terminate"
                        )),
                );
            }
            return Err(Halt);
        }
        self.steps -= 1;
        Ok(())
    }

    fn enter(&mut self, span: Span) -> Eval<()> {
        if self.nesting.enter() {
            return Ok(());
        }
        if self.nesting.should_report() {
            let note = self.nesting.note();
            self.diags.push(
                Diagnostic::error("B0216", "a macro body recursed too deep", span)
                    .with_primary_label("the interpreter gave up here")
                    .with_note(note),
            );
        }
        Err(Halt)
    }

    fn wrong(&mut self, msg: impl Into<String>, span: Span) -> Halt {
        self.diags.push(
            Diagnostic::error("B0209", msg, span)
                .with_primary_label("computed while expanding a macro"),
        );
        Halt
    }

    // ------------------------------------------------------------------------------- statements

    fn block(&mut self, block: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Flow> {
        self.enter(block.span())?;
        let out = self.block_inner(block, frame);
        self.nesting.leave();
        out
    }

    fn block_inner(&mut self, block: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Flow> {
        let stmts: &[Node] = if block.is_form(sym::DO) {
            &block.args
        } else {
            std::slice::from_ref(block)
        };
        for stmt in stmts {
            match self.stmt(stmt, frame)? {
                Flow::Fell => {}
                done => return Ok(done),
            }
        }
        Ok(Flow::Fell)
    }

    fn stmt(&mut self, s: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Flow> {
        self.step(s.span())?;

        if (s.is_form(sym::LET) || s.is_form(sym::VAR)) && s.args.len() == 2 {
            let target = &s.args[0];
            let name = if target.is_form(sym::ANNOT) {
                target.args.first().and_then(Node::as_var)
            } else {
                target.as_var()
            };
            let Some(name) = name.map(|v| v.name.clone()) else {
                return Err(self.wrong("a macro body binds a name, not a pattern", s.span()));
            };
            let v = self.eval(&s.args[1], frame)?;
            frame.insert(name, v);
            return Ok(Flow::Fell);
        }

        if s.is_form(sym::RETURN) {
            let v = match s.args.first() {
                Some(e) => self.eval(e, frame)?,
                None => Val::Unit,
            };
            return Ok(Flow::Returned(v));
        }

        if s.is_form(sym::IF) && s.args.len() >= 2 {
            let cond = self.eval(&s.args[0], frame)?;
            return if self.truth(&cond, s.args[0].span())? {
                self.block(&s.args[1], frame)
            } else if let Some(alt) = s.args.get(2) {
                self.block(alt, frame)
            } else {
                Ok(Flow::Fell)
            };
        }

        if s.is_form(sym::FOR) && s.args.len() == 3 {
            let Some(binder) = s.args[0].as_var().map(|v| v.name.clone()) else {
                return Err(self.wrong("a `for` binds one name", s.args[0].span()));
            };
            let seq = self.eval(&s.args[1], frame)?;
            let items = match &seq {
                Val::List(xs) => xs.as_ref().clone(),
                Val::Syntax(n) if n.is_form(sym::LIST) || n.is_form(sym::DO) => {
                    n.args.iter().cloned().map(Val::Syntax).collect()
                }
                other => {
                    let msg = format!("a `for` walks a list, not {}", other.type_name());
                    return Err(self.wrong(msg, s.args[1].span()));
                }
            };
            for item in items {
                self.step(s.span())?;
                frame.insert(binder.clone(), item);
                match self.block(&s.args[2], frame)? {
                    Flow::Fell => {}
                    done => return Ok(done),
                }
            }
            return Ok(Flow::Fell);
        }

        if s.is_form(sym::WHILE) && s.args.len() == 2 {
            loop {
                self.step(s.span())?;
                let cond = self.eval(&s.args[0], frame)?;
                if !self.truth(&cond, s.args[0].span())? {
                    return Ok(Flow::Fell);
                }
                match self.block(&s.args[1], frame)? {
                    Flow::Fell => {}
                    done => return Ok(done),
                }
            }
        }

        if s.is_form(sym::DO) {
            return self.block(s, frame);
        }

        // A statement whose value is discarded — a call written for its result and then ignored.
        self.eval(s, frame)?;
        Ok(Flow::Fell)
    }

    /// The forms that belong to the program rather than to the expander.
    ///
    /// Refused by name so the message is about the form: without this a `raise` in a macro body
    /// would be an applied node whose head resolves to nothing, and the diagnostic would say
    /// `raise` cannot be found — which is true of the environment and false about the language.
    fn refuse_program_form(&mut self, n: &Node) -> Eval<()> {
        let Some(head) = PROGRAM_ONLY.iter().find(|f| n.is_form(f)) else {
            return Ok(());
        };
        self.diags.push(
            Diagnostic::error(
                "B0205",
                format!("`{head}` is not available in a macro body"),
                n.span(),
            )
            .with_primary_label("this belongs to the program the macro expands to")
            .with_note(
                "a macro body is pure compile-time computation: bindings, `if`, `for`, `while`, \
                 lambdas, calls and `quote:`. Failure, declarations and pattern matching on \
                 variants are the program's, not the expander's",
            )
            .with_fix("put it inside the `quote:` the macro returns"),
        );
        Err(Halt)
    }

    fn truth(&mut self, v: &Val, span: Span) -> Eval<bool> {
        match v {
            Val::Bool(b) => Ok(*b),
            other => {
                let msg = format!("a condition is a Bool, not {}", other.type_name());
                Err(self.wrong(msg, span))
            }
        }
    }

    // ------------------------------------------------------------------------------ expressions

    fn eval(&mut self, e: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Val> {
        self.step(e.span())?;
        self.enter(e.span())?;
        let out = self.eval_inner(e, frame);
        self.nesting.leave();
        out
    }

    fn eval_inner(&mut self, e: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Val> {
        self.refuse_program_form(e)?;

        // A literal.
        if let Some(l) = e.as_lit() {
            return Ok(match l {
                Lit::Int(n) => Val::Int(*n),
                Lit::Float(f) => Val::Float(*f),
                Lit::Str(s) => Val::Str(s.clone()),
                Lit::Bool(b) => Val::Bool(*b),
                Lit::Keyword(k) => Val::Keyword(k.clone()),
            });
        }

        // A name.
        if let Some(v) = e.as_var() {
            if let Some(bound) = frame.get(&v.name) {
                return Ok(bound.clone());
            }
            if v.name.as_ref() == "unit" {
                return Ok(Val::Unit);
            }
            if let Some(def) = self.defs.get(&v.name) {
                // A `def` used as a value is a function of its parameters.
                return Ok(Val::Fun(Arc::new(Lambda {
                    params: def.params.clone(),
                    body: def.body.clone(),
                    captured: HashMap::new(),
                })));
            }
            return Err(self.unbound(&v.name, e.span()));
        }

        // `quote:` — the form whose value is syntax.
        if e.is_form(sym::QUOTE) && e.args.len() == 1 {
            let body = self.template(&e.args[0], frame)?;
            return Ok(Val::Syntax(unwrap_block(body)));
        }
        if e.is_form(sym::UNQUOTE) || e.is_form(sym::SPLICE) {
            let head = if e.is_form(sym::UNQUOTE) { "$" } else { "$*" };
            let msg = format!("`{head}` is only meaningful inside a `quote:`");
            return Err(self.wrong(msg, e.span()));
        }

        // Literal collections.
        if e.is_form(sym::LIST) {
            let mut out = Vec::with_capacity(e.args.len());
            for a in &e.args {
                out.push(self.eval(a, frame)?);
            }
            return Ok(Val::list(out));
        }
        if e.is_form(sym::RECORD) {
            let mut fields: Vec<(Arc<str>, Val)> = Vec::new();
            let mut i = 0;
            while i + 1 < e.args.len() {
                let Some(k) = e.args[i].as_keyword() else {
                    return Err(self.wrong("a record field is a name", e.args[i].span()));
                };
                let v = self.eval(&e.args[i + 1], frame)?;
                fields.push((Arc::from(k), v));
                i += 2;
            }
            return Ok(Val::Record(Arc::new(fields)));
        }

        // The conditional expression, `a if c else b`.
        if e.is_form(sym::IF) && e.args.len() == 3 {
            let c = self.eval(&e.args[0], frame)?;
            return if self.truth(&c, e.args[0].span())? {
                self.eval(&e.args[1], frame)
            } else {
                self.eval(&e.args[2], frame)
            };
        }

        // `and` and `or` short-circuit, which is why they are forms rather than builtins
        // (`docs/53` §53.2 is where that was found out about the *program's* half).
        if (e.is_form("and") || e.is_form("or")) && e.args.len() == 2 {
            let left = self.eval(&e.args[0], frame)?;
            let left = self.truth(&left, e.args[0].span())?;
            if e.is_form("and") && !left {
                return Ok(Val::Bool(false));
            }
            if e.is_form("or") && left {
                return Ok(Val::Bool(true));
            }
            let right = self.eval(&e.args[1], frame)?;
            return Ok(Val::Bool(self.truth(&right, e.args[1].span())?));
        }

        // A lambda closes over a copy of the frame: a macro body is a compile-time computation
        // and nothing it builds outlives the expansion, so there is nothing for a shared cell to
        // be observed by.
        if e.is_form(sym::FN) && e.args.len() == 2 {
            let params = e.args[0]
                .args
                .iter()
                .filter_map(|p| {
                    let target = if p.is_form(sym::ANNOT) { &p.args[0] } else { p };
                    target.as_var().map(|s| s.name.clone())
                })
                .collect();
            return Ok(Val::Fun(Arc::new(Lambda {
                params,
                body: e.args[1].clone(),
                captured: frame.clone(),
            })));
        }

        // `xs[i]`.
        if e.is_form("index") && e.args.len() == 2 {
            let subject = self.eval(&e.args[0], frame)?;
            let idx = self.eval(&e.args[1], frame)?;
            return self.index(&subject, &idx, e.span());
        }

        // `r.field`, and — the one method-shaped form — nothing else.
        if e.is_form(sym::DOT) && e.args.len() == 2 {
            let subject = self.eval(&e.args[0], frame)?;
            let Some(field) = e.args[1].as_var().map(|s| s.name.clone()) else {
                return Err(self.wrong("a field is a name", e.args[1].span()));
            };
            let Val::Record(fields) = &subject else {
                let msg = format!("{} has no fields", subject.type_name());
                return Err(self.wrong(msg, e.span()));
            };
            return match fields.iter().find(|(k, _)| *k == field) {
                Some((_, v)) => Ok(v.clone()),
                None => {
                    let msg = format!("this record has no field `{field}`");
                    Err(self.wrong(msg, e.args[1].span()))
                }
            };
        }
        if e.is_form(sym::DOT) {
            return Err(self.wrong(
                "a macro body calls functions, not methods — the compile-time environment has no \
                 traits",
                e.span(),
            ));
        }

        // A call: `(name args…)` or `(call callee args…)`.
        if e.applied {
            let (callee, args): (Option<Node>, &[Node]) = if e.is_form(sym::CALL) {
                (e.args.first().cloned(), &e.args[1..])
            } else {
                (None, &e.args[..])
            };

            // The callee is checked *before* the arguments are evaluated: `http_fetch(url, req)`
            // should say what is wrong with `http_fetch` rather than what is wrong with `req`.
            if let Some(name) = e.head_sym().filter(|_| callee.is_none()) {
                if !frame.contains_key(&name.name)
                    && !self.defs.contains_key(&name.name)
                    && RESTRICTED.iter().any(|(n, _)| *n == name.name.as_ref())
                {
                    return Err(self.unbound(&name.name.clone(), e.span()));
                }
            }

            let mut values = Vec::with_capacity(args.len());
            for a in args {
                // A keyword argument in a macro body is bound by name at the call, which the
                // compile-time environment does not do: parameters are positional here.
                if a.is_form(sym::KW_ARG) {
                    return Err(self.wrong(
                        "a compile-time call passes its arguments by position",
                        a.span(),
                    ));
                }
                values.push(self.eval(a, frame)?);
            }

            if let Some(callee) = callee {
                let f = self.eval(&callee, frame)?;
                return self.apply(&f, values, e.span());
            }

            let Some(name) = e.head_sym().map(|s| s.name.clone()) else {
                return Err(self.wrong("a call needs a callee", e.span()));
            };

            // A local — a lambda in a binding — before a `def`, and a `def` before a builtin, so
            // that a module can name a function of its own after one of the prelude's and have
            // the compile-time environment agree with the program about which one it means.
            if let Some(bound) = frame.get(&name).cloned() {
                return self.apply(&bound, values, e.span());
            }
            if let Some(def) = self.defs.get(&name).cloned() {
                return self.call_def(&name, &def, values, e.span());
            }
            if let Some(out) = self.builtin(&name, &values, e.span()) {
                return out;
            }
            return Err(self.unbound(&name, e.span()));
        }

        let head = e.head_name().unwrap_or("this form").to_string();
        Err(self.wrong(format!("`{head}` has no value at compile time"), e.span()))
    }

    fn unbound(&mut self, name: &str, span: Span) -> Halt {
        if let Some((_, atom)) = RESTRICTED.iter().find(|(n, _)| *n == name) {
            self.diags.push(
                Diagnostic::error(
                    "B0207",
                    format!("`{name}` may not be called while expanding a macro"),
                    span,
                )
                .with_primary_label(format!("performs `{atom}`"))
                .with_note(
                    "macro expansion is capability-restricted (`docs/02` §2.4): it is pure \
                     computation over the module's own definitions, so that what a compile \
                     produces depends on the source and on nothing else",
                )
                .with_fix("compute this in the program the macro expands to, not in the macro"),
            );
            return Halt;
        }
        self.diags.push(
            Diagnostic::error(
                "B0208",
                format!("cannot find `{name}` at compile time"),
                span,
            )
            .with_primary_label("not a local, a `def` in this module, or a compile-time builtin")
            .with_note(
                "the macro interpreter's environment is deliberately small: the pure part of the \
                 prelude, this module's own definitions, and the `node_*` reflection over syntax",
            ),
        );
        Halt
    }

    fn index(&mut self, subject: &Val, idx: &Val, span: Span) -> Eval<Val> {
        let Val::Int(i) = idx else {
            let msg = format!("an index is an Int, not {}", idx.type_name());
            return Err(self.wrong(msg, span));
        };
        let items: Vec<Val> = match subject {
            Val::List(xs) => xs.as_ref().clone(),
            Val::Syntax(n) if n.is_form(sym::LIST) || n.is_form(sym::DO) => {
                n.args.iter().cloned().map(Val::Syntax).collect()
            }
            other => {
                let msg = format!("{} is not indexable", other.type_name());
                return Err(self.wrong(msg, span));
            }
        };
        match usize::try_from(*i).ok().and_then(|i| items.get(i)) {
            Some(v) => Ok(v.clone()),
            None => {
                let msg = format!("index {i} is outside a list of {}", items.len());
                Err(self.wrong(msg, span))
            }
        }
    }

    fn apply(&mut self, f: &Val, args: Vec<Val>, span: Span) -> Eval<Val> {
        let Val::Fun(lambda) = f else {
            let msg = format!("{} is not a function", f.type_name());
            return Err(self.wrong(msg, span));
        };
        if lambda.params.len() != args.len() {
            let msg = format!(
                "this function takes {} argument(s) and got {}",
                lambda.params.len(),
                args.len()
            );
            return Err(self.wrong(msg, span));
        }
        let mut frame = lambda.captured.clone();
        for (p, a) in lambda.params.iter().zip(args) {
            frame.insert(p.clone(), a);
        }
        self.enter(span)?;
        let out = self.block(&lambda.body, &mut frame);
        self.nesting.leave();
        match out? {
            Flow::Returned(v) => Ok(v),
            // `lambda t: e` parses as a block holding one expression, so a body that falls off the
            // end has one statement whose value is the answer.
            Flow::Fell => self.last_value(&lambda.body.clone(), &mut frame),
        }
    }

    fn last_value(&mut self, body: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Val> {
        let last = if body.is_form(sym::DO) {
            body.args.last()
        } else {
            Some(body)
        };
        match last {
            Some(e) => self.eval(e, frame),
            None => Ok(Val::Unit),
        }
    }

    fn call_def(&mut self, name: &str, def: &FnDef, args: Vec<Val>, span: Span) -> Eval<Val> {
        if def.params.len() != args.len() {
            let msg = format!(
                "`{name}` takes {} argument(s) and got {}",
                def.params.len(),
                args.len()
            );
            return Err(self.wrong(msg, span));
        }
        let mut frame: HashMap<Arc<str>, Val> = HashMap::new();
        for (p, a) in def.params.iter().zip(args) {
            frame.insert(p.clone(), a);
        }
        self.enter(span)?;
        let out = self.block(&def.body, &mut frame);
        self.nesting.leave();
        match out? {
            Flow::Returned(v) => Ok(v),
            Flow::Fell => Ok(Val::Unit),
        }
    }

    // -------------------------------------------------------------------------------- templates

    /// Walk a `quote`d template, replacing `$e` with the syntax of `e`'s value.
    ///
    /// Everything else is carried through unchanged — a template is data, so a `for` inside a
    /// `quote:` is a loop in the *program being built*, not one the interpreter runs.
    fn template(&mut self, t: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Node> {
        self.step(t.span())?;
        self.enter(t.span())?;
        let out = self.template_inner(t, frame);
        self.nesting.leave();
        out
    }

    fn template_inner(&mut self, t: &Node, frame: &mut HashMap<Arc<str>, Val>) -> Eval<Node> {
        if t.is_form(sym::UNQUOTE) && t.args.len() == 1 {
            // `$x` where `x` is bound nowhere is worth its own code: the mistake is almost always
            // a parameter that was renamed, and the span to point at is the `$`.
            if let Some(v) = t.args[0].as_var() {
                if !frame.contains_key(&v.name) && !self.defs.contains_key(&v.name) {
                    self.diags.push(
                        Diagnostic::error(
                            "B0206",
                            format!("`${v}` is not bound in this macro"),
                            t.span(),
                        )
                        .with_primary_label("unquoting an unbound name")
                        .with_note("`$e` evaluates `e` in the macro body's own environment"),
                    );
                    return Err(Halt);
                }
            }
            let v = self.eval(&t.args[0], frame)?;
            return self.reflect(&v, t.span());
        }

        let mut args = Vec::with_capacity(t.args.len());
        for a in &t.args {
            if a.is_form(sym::SPLICE) && a.args.len() == 1 {
                let v = self.eval(&a.args[0], frame)?;
                for piece in self.spliced(&v, a.span())? {
                    args.push(piece);
                }
                continue;
            }
            args.push(self.template(a, frame)?);
        }

        // A template head that names a bound piece of syntax is that syntax: `$f(x)` is written
        // `f(x)` inside a quote, because a head is a symbol and `$` takes an expression.
        let head = match &t.head {
            Head::Sym(s) => match frame.get(&s.name) {
                Some(Val::Syntax(bound)) if t.applied && !bound.applied => match &bound.head {
                    Head::Sym(bs) => Head::Sym(bs.clone()),
                    _ => Head::Sym(s.clone()),
                },
                _ => Head::Sym(s.clone()),
            },
            Head::Lit(l) => Head::Lit(l.clone()),
        };

        Ok(Node {
            head,
            args,
            applied: t.applied,
            meta: t.meta.clone(),
        })
    }

    /// What `$*xs` puts into the surrounding form.
    fn spliced(&mut self, v: &Val, span: Span) -> Eval<Vec<Node>> {
        match v {
            Val::List(xs) => {
                let mut out = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    out.push(self.reflect(x, span)?);
                }
                Ok(out)
            }
            Val::Syntax(n) if n.is_form(sym::LIST) || n.is_form(sym::DO) => Ok(n.args.clone()),
            other => Ok(vec![self.reflect(other, span)?]),
        }
    }

    /// A value's syntax.
    ///
    /// Every value has one except a function: a closure is a thing the compile-time environment
    /// holds, and there is no expression that denotes it in the program being built.
    pub fn reflect(&mut self, v: &Val, span: Span) -> Eval<Node> {
        Ok(match v {
            Val::Unit => Node::sym("unit", span),
            Val::Int(n) => Node::lit(Lit::Int(*n), span),
            Val::Float(f) => Node::lit(Lit::Float(*f), span),
            Val::Str(s) => Node::lit(Lit::Str(s.clone()), span),
            Val::Bool(b) => Node::lit(Lit::Bool(*b), span),
            Val::Keyword(k) => Node::lit(Lit::Keyword(k.clone()), span),
            Val::Syntax(n) => n.clone(),
            Val::List(xs) => {
                let mut args = Vec::with_capacity(xs.len());
                for x in xs.iter() {
                    args.push(self.reflect(x, span)?);
                }
                Node::form(sym::LIST, args, span)
            }
            Val::Record(fields) => {
                let mut args = Vec::with_capacity(fields.len() * 2);
                for (k, val) in fields.iter() {
                    args.push(Node::lit(Lit::Keyword(k.clone()), span));
                    args.push(self.reflect(val, span)?);
                }
                Node::form(sym::RECORD, args, span)
            }
            Val::Fun(_) => {
                return Err(self.wrong(
                    "a function has no syntax — a macro returns the code that builds one",
                    span,
                ))
            }
        })
    }

    // --------------------------------------------------------------------------------- builtins

    /// The whitelist. `None` means the name is not a builtin at all.
    fn builtin(&mut self, name: &str, args: &[Val], span: Span) -> Option<Eval<Val>> {
        if !is_builtin(name) {
            return None;
        }
        Some(self.builtin_inner(name, args, span))
    }

    fn builtin_inner(&mut self, name: &str, args: &[Val], span: Span) -> Eval<Val> {
        let arity = |want: usize, this: &mut Self| -> Eval<()> {
            if args.len() == want {
                Ok(())
            } else {
                let msg = format!("`{name}` takes {want} argument(s) and got {}", args.len());
                Err(this.wrong(msg, span))
            }
        };

        macro_rules! s {
            ($i:expr) => {{
                match &args[$i] {
                    Val::Str(s) => s.clone(),
                    other => {
                        let msg = format!("`{name}` expects a Str, not {}", other.type_name());
                        return Err(self.wrong(msg, span));
                    }
                }
            }};
        }
        macro_rules! i {
            ($i:expr) => {{
                match &args[$i] {
                    Val::Int(n) => *n,
                    other => {
                        let msg = format!("`{name}` expects an Int, not {}", other.type_name());
                        return Err(self.wrong(msg, span));
                    }
                }
            }};
        }
        macro_rules! l {
            ($i:expr) => {{
                match &args[$i] {
                    Val::List(xs) => xs.as_ref().clone(),
                    other => {
                        let msg = format!("`{name}` expects a list, not {}", other.type_name());
                        return Err(self.wrong(msg, span));
                    }
                }
            }};
        }
        macro_rules! n {
            ($i:expr) => {{
                match &args[$i] {
                    Val::Syntax(n) => n.clone(),
                    other => {
                        let msg = format!("`{name}` expects syntax, not {}", other.type_name());
                        return Err(self.wrong(msg, span));
                    }
                }
            }};
        }

        match name {
            // ---- text. The tables are `beck-prim`'s, so a macro folding a letter and a program
            // folding the same letter reach one implementation (`docs/93` §93.12).
            "str" => {
                arity(1, self)?;
                Ok(Val::str_(display(&args[0])))
            }
            "str_len" => {
                arity(1, self)?;
                Ok(Val::Int(s!(0).chars().count() as i64))
            }
            "str_is_empty" => {
                arity(1, self)?;
                Ok(Val::Bool(s!(0).is_empty()))
            }
            "str_trim" => {
                arity(1, self)?;
                Ok(Val::str_(s!(0).trim()))
            }
            "str_upper" => {
                arity(1, self)?;
                Ok(Val::str_(beck_prim::text::upper(&s!(0))))
            }
            "str_lower" => {
                arity(1, self)?;
                Ok(Val::str_(beck_prim::text::lower(&s!(0))))
            }
            "str_contains" => {
                arity(2, self)?;
                Ok(Val::Bool(s!(0).contains(s!(1).as_ref())))
            }
            "str_starts_with" => {
                arity(2, self)?;
                Ok(Val::Bool(s!(0).starts_with(s!(1).as_ref())))
            }
            "str_ends_with" => {
                arity(2, self)?;
                Ok(Val::Bool(s!(0).ends_with(s!(1).as_ref())))
            }
            "str_slice" => {
                arity(3, self)?;
                let (start, len) = (i!(1).max(0) as usize, i!(2).max(0) as usize);
                let out: String = s!(0).chars().skip(start).take(len).collect();
                Ok(Val::str_(out))
            }
            "str_replace" => {
                arity(3, self)?;
                Ok(Val::str_(beck_prim::text::replace(&s!(0), &s!(1), &s!(2))))
            }
            "str_repeat" => {
                arity(2, self)?;
                let n = i!(1).clamp(0, 1_000_000) as usize;
                Ok(Val::str_(s!(0).repeat(n)))
            }
            "str_chars" => {
                arity(1, self)?;
                Ok(Val::list(
                    s!(0).chars().map(|c| Val::str_(c.to_string())).collect(),
                ))
            }
            "str_split" => {
                arity(2, self)?;
                let (hay, sep) = (s!(0), s!(1));
                let parts: Vec<Val> = if sep.is_empty() {
                    hay.chars().map(|c| Val::str_(c.to_string())).collect()
                } else {
                    hay.split(sep.as_ref()).map(Val::str_).collect()
                };
                Ok(Val::list(parts))
            }
            "str_join" => {
                arity(2, self)?;
                let xs = l!(0);
                let sep = s!(1);
                let parts: Vec<String> = xs.iter().map(display).collect();
                Ok(Val::str_(parts.join(sep.as_ref())))
            }

            // ---- lists
            "list_len" => {
                arity(1, self)?;
                Ok(Val::Int(l!(0).len() as i64))
            }
            "list_is_empty" => {
                arity(1, self)?;
                Ok(Val::Bool(l!(0).is_empty()))
            }
            "list_append" => {
                arity(2, self)?;
                let mut xs = l!(0);
                xs.push(args[1].clone());
                Ok(Val::list(xs))
            }
            "list_reverse" => {
                arity(1, self)?;
                let mut xs = l!(0);
                xs.reverse();
                Ok(Val::list(xs))
            }
            "list_contains" => {
                arity(2, self)?;
                Ok(Val::Bool(l!(0).iter().any(|x| val_eq(x, &args[1]))))
            }
            "list_take" => {
                arity(2, self)?;
                let n = i!(1).max(0) as usize;
                Ok(Val::list(l!(0).into_iter().take(n).collect()))
            }
            "list_drop" => {
                arity(2, self)?;
                let n = i!(1).max(0) as usize;
                Ok(Val::list(l!(0).into_iter().skip(n).collect()))
            }
            "list_slice" => {
                arity(3, self)?;
                let (start, len) = (i!(1).max(0) as usize, i!(2).max(0) as usize);
                Ok(Val::list(l!(0).into_iter().skip(start).take(len).collect()))
            }
            // One argument, a list *of* lists — the prelude's shape, not `a + b`.
            "concat_lists" => {
                arity(1, self)?;
                let mut out = Vec::new();
                for group in l!(0) {
                    match group {
                        Val::List(ys) => out.extend(ys.as_ref().clone()),
                        other => {
                            let msg = format!(
                                "`concat_lists` takes a list of lists, and found {}",
                                other.type_name()
                            );
                            return Err(self.wrong(msg, span));
                        }
                    }
                }
                Ok(Val::list(out))
            }
            "map_list" => {
                arity(2, self)?;
                let xs = l!(0);
                let f = args[1].clone();
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    out.push(self.apply(&f, vec![x], span)?);
                }
                Ok(Val::list(out))
            }
            "filter_list" => {
                arity(2, self)?;
                let xs = l!(0);
                let f = args[1].clone();
                let mut out = Vec::new();
                for x in xs {
                    let keep = self.apply(&f, vec![x.clone()], span)?;
                    if self.truth(&keep, span)? {
                        out.push(x);
                    }
                }
                Ok(Val::list(out))
            }
            "list_fold" => {
                arity(3, self)?;
                let xs = l!(0);
                let mut acc = args[1].clone();
                let f = args[2].clone();
                for x in xs {
                    acc = self.apply(&f, vec![acc, x], span)?;
                }
                Ok(acc)
            }
            "list_all" | "list_any" => {
                arity(2, self)?;
                let xs = l!(0);
                let f = args[1].clone();
                let all = name == "list_all";
                for x in xs {
                    let got = self.apply(&f, vec![x], span)?;
                    if self.truth(&got, span)? != all {
                        return Ok(Val::Bool(!all));
                    }
                }
                Ok(Val::Bool(all))
            }
            "list_flat_map" => {
                arity(2, self)?;
                let xs = l!(0);
                let f = args[1].clone();
                let mut out = Vec::new();
                for x in xs {
                    match self.apply(&f, vec![x], span)? {
                        Val::List(ys) => out.extend(ys.as_ref().clone()),
                        other => {
                            let msg =
                                format!("`list_flat_map` expects lists, not {}", other.type_name());
                            return Err(self.wrong(msg, span));
                        }
                    }
                }
                Ok(Val::list(out))
            }

            // ---- numbers. No `sqrt`, `sin` or `cos`: they would make what the compiler produces
            // depend on the host's libm, which is F9's question and not one to prejudge here.
            "abs" => {
                arity(1, self)?;
                match &args[0] {
                    Val::Int(n) => Ok(Val::Int(n.abs())),
                    Val::Float(f) => Ok(Val::Float(f.abs())),
                    other => {
                        let msg = format!("`abs` expects a number, not {}", other.type_name());
                        Err(self.wrong(msg, span))
                    }
                }
            }
            "float" => {
                arity(1, self)?;
                Ok(Val::Float(i!(0) as f64))
            }
            "trunc" => {
                arity(1, self)?;
                match &args[0] {
                    Val::Float(f) => Ok(Val::Int(f.trunc() as i64)),
                    other => {
                        let msg = format!("`trunc` expects a Float, not {}", other.type_name());
                        Err(self.wrong(msg, span))
                    }
                }
            }

            // ---- syntax. §2.2's `Node` is "an ordinary Beck value"; this is the part of that
            // sentence a macro can reach today — read a node, build a node.
            "node_head" => {
                arity(1, self)?;
                match &n!(0).head {
                    Head::Sym(s) => Ok(Val::str_(s.as_str())),
                    Head::Lit(_) => Err(self.wrong(
                        "this node is a literal and has no head symbol — ask `node_is_lit` first",
                        span,
                    )),
                }
            }
            "node_args" => {
                arity(1, self)?;
                Ok(Val::list(
                    n!(0).args.iter().cloned().map(Val::Syntax).collect(),
                ))
            }
            "node_is_call" => {
                arity(1, self)?;
                Ok(Val::Bool(n!(0).applied))
            }
            "node_is_lit" => {
                arity(1, self)?;
                Ok(Val::Bool(n!(0).as_lit().is_some()))
            }
            "node_sym" => {
                arity(1, self)?;
                Ok(Val::Syntax(Node::sym(s!(0).as_ref(), span)))
            }
            "node_form" => {
                arity(2, self)?;
                let head = s!(0);
                let mut items = Vec::new();
                for a in l!(1) {
                    items.push(self.reflect(&a, span)?);
                }
                Ok(Val::Syntax(Node::form(head.as_ref(), items, span)))
            }
            "node_str" => {
                arity(1, self)?;
                Ok(Val::str_(print::to_sexpr(&n!(0))))
            }
            // `splice([a, b])` is several forms where one is expected — the shape §2.4's `derive`
            // returns, and the reason `expand_module` flattens a `do` at the top of a module.
            "splice" => {
                arity(1, self)?;
                let mut items = Vec::new();
                for a in l!(0) {
                    items.push(self.reflect(&a, span)?);
                }
                Ok(Val::Syntax(Node::form(sym::DO, items, span)))
            }

            // ---- operators
            "+" | "-" | "*" | "/" | "%" => {
                arity(2, self)?;
                self.arith(name, &args[0], &args[1], span)
            }
            "negate" => {
                arity(1, self)?;
                match &args[0] {
                    Val::Int(n) => Ok(Val::Int(-n)),
                    Val::Float(f) => Ok(Val::Float(-f)),
                    other => {
                        let msg = format!("`-` expects a number, not {}", other.type_name());
                        Err(self.wrong(msg, span))
                    }
                }
            }
            "==" => {
                arity(2, self)?;
                Ok(Val::Bool(val_eq(&args[0], &args[1])))
            }
            "!=" => {
                arity(2, self)?;
                Ok(Val::Bool(!val_eq(&args[0], &args[1])))
            }
            "<" | "<=" | ">" | ">=" => {
                arity(2, self)?;
                self.compare(name, &args[0], &args[1], span)
            }
            "not" => {
                arity(1, self)?;
                let b = self.truth(&args[0], span)?;
                Ok(Val::Bool(!b))
            }
            other => unreachable!("`{other}` is listed as a builtin and not implemented"),
        }
    }

    fn arith(&mut self, op: &str, a: &Val, b: &Val, span: Span) -> Eval<Val> {
        match (a, b) {
            (Val::Int(x), Val::Int(y)) => {
                if matches!(op, "/" | "%") && *y == 0 {
                    return Err(self.wrong("division by zero while expanding a macro", span));
                }
                Ok(Val::Int(match op {
                    "+" => x.wrapping_add(*y),
                    "-" => x.wrapping_sub(*y),
                    "*" => x.wrapping_mul(*y),
                    "/" => x.wrapping_div(*y),
                    _ => x.wrapping_rem(*y),
                }))
            }
            (Val::Float(x), Val::Float(y)) => Ok(Val::Float(match op {
                "+" => x + y,
                "-" => x - y,
                "*" => x * y,
                "/" => x / y,
                _ => x % y,
            })),
            // `+` concatenates strings, which is how the checker resolves it for a program too.
            (Val::Str(x), Val::Str(y)) if op == "+" => Ok(Val::str_(format!("{x}{y}"))),
            (Val::List(x), Val::List(y)) if op == "+" => {
                let mut out = x.as_ref().clone();
                out.extend(y.as_ref().clone());
                Ok(Val::list(out))
            }
            _ => {
                let msg = format!(
                    "`{op}` does not apply to {} and {}",
                    a.type_name(),
                    b.type_name()
                );
                Err(self.wrong(msg, span))
            }
        }
    }

    fn compare(&mut self, op: &str, a: &Val, b: &Val, span: Span) -> Eval<Val> {
        let ord = match (a, b) {
            (Val::Int(x), Val::Int(y)) => x.cmp(y),
            (Val::Float(x), Val::Float(y)) => match x.partial_cmp(y) {
                Some(o) => o,
                None => return Err(self.wrong("a NaN has no order", span)),
            },
            (Val::Str(x), Val::Str(y)) => x.as_ref().cmp(y.as_ref()),
            _ => {
                let msg = format!(
                    "`{op}` does not compare {} with {}",
                    a.type_name(),
                    b.type_name()
                );
                return Err(self.wrong(msg, span));
            }
        };
        Ok(Val::Bool(match op {
            "<" => ord.is_lt(),
            "<=" => ord.is_le(),
            ">" => ord.is_gt(),
            _ => ord.is_ge(),
        }))
    }
}

/// A one-statement `quote:` block is that statement, not a block.
fn unwrap_block(n: Node) -> Node {
    if n.is_form(sym::DO) && n.args.len() == 1 {
        return n.args[0].clone();
    }
    n
}

/// Forms that are the *program's* and are refused with `B0205` rather than falling through to a
/// "cannot find" about their head.
const PROGRAM_ONLY: &[&str] = &[
    sym::MATCH,
    sym::TRY,
    sym::RAISE,
    sym::PARALLEL,
    sym::DEF,
    sym::MODEL,
    sym::UNION,
    sym::TRAIT,
    sym::IMPL,
    sym::TYPE,
    sym::NEWTYPE,
    sym::IMPORT,
    sym::TEST,
    sym::PROPERTY,
    sym::SERVICE,
    sym::UI,
];

/// Every name the compile-time environment defines.
///
/// A single list so that "is this a builtin" and "what does it do" cannot disagree — the `match`
/// in `Interp::builtin_inner` has an `unreachable!` arm that fires if a name is listed here and
/// not implemented, and `every_builtin_is_implemented` runs it.
pub const BUILTINS: &[&str] = &[
    "+",
    "-",
    "*",
    "/",
    "%",
    "==",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "abs",
    "concat_lists",
    "filter_list",
    "float",
    "list_all",
    "list_any",
    "list_append",
    "list_contains",
    "list_drop",
    "list_flat_map",
    "list_fold",
    "list_is_empty",
    "list_len",
    "list_reverse",
    "list_slice",
    "list_take",
    "map_list",
    "negate",
    "node_args",
    "node_form",
    "node_head",
    "node_is_call",
    "node_is_lit",
    "node_str",
    "node_sym",
    "not",
    "splice",
    "str",
    "str_chars",
    "str_contains",
    "str_ends_with",
    "str_is_empty",
    "str_join",
    "str_len",
    "str_lower",
    "str_repeat",
    "str_replace",
    "str_slice",
    "str_split",
    "str_starts_with",
    "str_trim",
    "str_upper",
    "trunc",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTINS.contains(&name)
}

/// What `str(x)` renders, which is also what `str_join` uses for a non-string element.
fn display(v: &Val) -> String {
    match v {
        Val::Unit => "unit".to_string(),
        Val::Int(n) => n.to_string(),
        Val::Float(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{f:.1}")
            } else {
                f.to_string()
            }
        }
        Val::Str(s) => s.to_string(),
        Val::Bool(b) => b.to_string(),
        Val::Keyword(k) => format!(":{k}"),
        Val::List(xs) => {
            let parts: Vec<String> = xs.iter().map(display).collect();
            format!("[{}]", parts.join(", "))
        }
        Val::Record(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", display(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Val::Syntax(n) => print::to_sexpr(n),
        Val::Fun(_) => "<function>".to_string(),
    }
}

fn val_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Unit, Val::Unit) => true,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Float(x), Val::Float(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Keyword(x), Val::Keyword(y)) => x == y,
        (Val::List(x), Val::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| val_eq(a, b))
        }
        (Val::Record(x), Val::Record(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y.iter())
                    .all(|((ka, va), (kb, vb))| ka == kb && val_eq(va, vb))
        }
        (Val::Syntax(x), Val::Syntax(y)) => x.structurally_eq(y),
        _ => false,
    }
}

/// The parameter list of a `def` or a `macro`, as names.
pub fn param_names(params: &Node) -> Vec<Arc<str>> {
    params
        .args
        .iter()
        .filter_map(|p| {
            let target = if p.is_form(sym::ANNOT) { &p.args[0] } else { p };
            target.as_var().map(|s: &Symbol| s.name.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_is_implemented() {
        // The `unreachable!` arm in `builtin_inner` is only unreachable if the two lists agree,
        // and "agree" is what this asserts: every listed name is dispatched, with an arity error
        // rather than a panic when it is called with nothing.
        let defs = HashMap::new();
        for name in BUILTINS {
            let mut diags = Diagnostics::new();
            let mut interp = Interp::new(&defs, &mut diags, MAX_STEPS, false);
            let _ = interp.builtin(name, &[], Span::NONE);
        }
    }

    /// The interpreter's ceiling fits the stack the front end declares.
    ///
    /// `beck-syntax` and `beck-core` each have one of these and
    /// [`beck_diag::depth::STACK_BYTES`] names them, because a count is only a *bound* if the
    /// stack it implies is a stack that exists. This crate recurses now too, and its frames are
    /// its own: an interpreter's `eval` carries a value and an environment where a parser's
    /// carries a token cursor.
    ///
    /// Measured at the ceiling rather than extrapolated from a per-level cost: a compile-time
    /// recursion with no base case is refused at exactly [`Nesting`]'s limit, so what it spends
    /// *is* the worst case, and no arithmetic stands between the measurement and the conclusion.
    #[test]
    fn the_interpreters_ceiling_fits_the_declared_stack() {
        // Measured on a stack far larger than the one whose adequacy is being concluded, so the
        // measurement is never the thing that overflows.
        let spent = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(|| {
                let src = "\
def down(n: Int) -> Int:
    return down(n + 1)

macro deep(x):
    y = down(1)
    return quote:
        $x

def f() -> Int:
    return deep(1)
";
                beck_diag::depth::probe::stack_spent(|| {
                    let mut map = beck_diag::SourceMap::new();
                    let file = map.add("probe.beck", src);
                    let mut diags = Diagnostics::new();
                    let parsed = beck_syntax::parser::parse_module(file, "probe", src, &mut diags);
                    let out = crate::expand_module(&parsed, &mut diags);
                    assert!(
                        diags.iter().any(|d| d.code == "B0216"),
                        "the probe must reach the ceiling for this to be measuring it"
                    );
                    out
                })
            })
            .expect("a thread")
            .join()
            .expect("the probe expands");

        println!(
            "the macro interpreter spends {spent} bytes reaching its ceiling of {} levels",
            beck_diag::depth::MAX_NESTING
        );
        // Twice over, as the parser's and the evaluator's are: whoever drives expansion has as
        // much stack again above the ceiling as the ceiling itself needs.
        assert!(
            spent * 2 < beck_diag::depth::STACK_BYTES,
            "reaching the ceiling costs {spent} bytes, and {} with the margin, against a declared \
             STACK_BYTES of {} — raise the declaration or lower the ceiling",
            spent * 2,
            beck_diag::depth::STACK_BYTES
        );
    }

    #[test]
    fn the_restricted_list_is_sorted_and_unique() {
        let names: Vec<&str> = RESTRICTED.iter().map(|(n, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted, "keep `RESTRICTED` sorted and duplicate-free");
        let mut builtins = BUILTINS.to_vec();
        builtins.sort_unstable();
        for (name, _) in RESTRICTED {
            assert!(
                !builtins.contains(name),
                "`{name}` is both restricted and a builtin"
            );
        }
    }
}
