//! The Python surface: recursive descent for statements, Pratt for expressions.
//!
//! [`docs/02-syntax.md`](../../../../../docs/02-syntax.md) §2.8: "hand-written recursive descent +
//! Pratt for expressions. **Not** a parser generator. Rationale: error messages and error
//! *recovery* are the top-two UX properties of a new language."
//!
//! The output is the *same* `Node` tree the S-expression reader produces — that equivalence is
//! asserted directly in `tests/surfaces.rs`, and it is the whole claim of §2.2. Two surface rules
//! carry most of the weight:
//!
//! * **The block rule** (§2.3): any call written `f(args):` followed by an indented block desugars
//!   to `f(args, do=quote(block))`. That single rule buys the entire Lisp special-form vocabulary
//!   with Python punctuation — `ui:`, `atomically:`, `retry(times=3):` are all ordinary calls.
//! * **Decorators are AST transforms** (§2.3): `@on(server)` before a `def` becomes
//!   `(decorate (on server) (def ...))`, so the decorator receives the definition's AST rather
//!   than a function object.

use beck_diag::depth::Nesting;
use beck_diag::{Diagnostic, Diagnostics, FileId, Span};

use crate::lexer::{lex, Raw, Tok, Token};
use crate::node::{sym, Head, Lit, Node, Symbol};

pub struct Parser<'a> {
    toks: Vec<Token>,
    pos: usize,
    diags: &'a mut Diagnostics,
    file: FileId,
    /// Set once the parser has bailed out of a construct, so a cascade of follow-on errors from
    /// one real mistake does not bury it.
    poisoned: bool,
    /// Set by [`Parser::attach_block`] so the statement parser knows the expression is finished.
    attached_block: bool,
    /// How deep the parser is inside brackets and indentation, against the ceiling every part of
    /// the front end shares. Unlike the `depth` locals in this file — which are balance counters
    /// serving error recovery and the layout algorithm — this one is a bound.
    nesting: Nesting,
    /// Non-zero inside a `test`/`property` body, where `given`, `when`, `expect` and `stub` are
    /// clause keywords. They are *not* reserved anywhere else: a program with a function called
    /// `expect` keeps working, and §21.2's construct does not cost the language four words.
    in_test: usize,
}

/// Parse a whole module.
pub fn parse_module(file: FileId, name: &str, src: &str, diags: &mut Diagnostics) -> Node {
    let toks = lex(file, src, diags);
    let mut p = Parser {
        toks,
        pos: 0,
        diags,
        file,
        poisoned: false,
        attached_block: false,
        nesting: Nesting::new(),
        in_test: 0,
    };
    let mut items = vec![Node::sym(name, Span::new(file, 0..0))];
    while !p.at_eof() {
        p.skip_newlines();
        if p.at_eof() {
            break;
        }
        match p.item() {
            Some(item) => items.push(item),
            None => p.recover_to_next_item(),
        }
    }
    Node::form(sym::MODULE, items, Span::new(file, 0..src.len()))
}

/// Parse a single expression — used by `beck ast` and by tests.
pub fn parse_expr_str(file: FileId, src: &str, diags: &mut Diagnostics) -> Option<Node> {
    let toks = lex(file, src, diags);
    let mut p = Parser {
        toks,
        pos: 0,
        diags,
        file,
        poisoned: false,
        attached_block: false,
        nesting: Nesting::new(),
        in_test: 0,
    };
    p.skip_newlines();
    p.expr()
}

impl<'a> Parser<'a> {
    // ---------------------------------------------------------------- token helpers

    fn cur(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn at_eof(&self) -> bool {
        matches!(self.cur().tok, Tok::Eof)
    }

    fn span(&self) -> Span {
        self.cur().span
    }

    fn bump(&mut self) -> Token {
        let t = self.cur().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn at(&self, r: &Raw) -> bool {
        self.cur().raw() == Some(r)
    }

    fn at_kw(&self, name: &str) -> bool {
        self.cur().is_ident(name)
    }

    fn eat(&mut self, r: &Raw) -> bool {
        if self.at(r) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, name: &str) -> bool {
        if self.at_kw(name) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, r: &Raw, what: &str) -> bool {
        if self.eat(r) {
            return true;
        }
        self.error(format!("expected {what}, found {}", self.cur().describe()));
        false
    }

    fn error(&mut self, msg: impl Into<String>) {
        if self.poisoned {
            return;
        }
        self.poisoned = true;
        let span = self.span();
        self.diags.push(
            Diagnostic::error("B0120", msg.into(), span).with_primary_label("unexpected here"),
        );
    }

    fn skip_newlines(&mut self) {
        while matches!(self.cur().tok, Tok::Newline) {
            self.bump();
        }
    }

    /// Skip forward to something that can start a top-level item, so one bad line does not make
    /// the rest of the file unparseable.
    fn recover_to_next_item(&mut self) {
        self.poisoned = false;
        let mut depth = 0i32;
        loop {
            match &self.cur().tok {
                Tok::Eof => return,
                Tok::Indent => {
                    depth += 1;
                    self.bump();
                }
                Tok::Dedent => {
                    depth -= 1;
                    self.bump();
                    if depth <= 0 {
                        return;
                    }
                }
                Tok::Newline if depth <= 0 => {
                    self.bump();
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn ident(&mut self, what: &str) -> Option<(String, Span)> {
        let span = self.span();
        match self.cur().raw() {
            Some(Raw::Ident(s)) => {
                let s = s.clone();
                self.bump();
                Some((s, span))
            }
            _ => {
                self.error(format!("expected {what}, found {}", self.cur().describe()));
                None
            }
        }
    }

    // ---------------------------------------------------------------- items

    fn item(&mut self) -> Option<Node> {
        if self.at(&Raw::At) {
            return self.decorated();
        }
        let start = self.span();
        if self.at_kw("def") {
            return self.def_item();
        }
        // `row Failure = raises(FormError), log` — Koka's community supplies the argument for this
        // being in the design from the start rather than added when rows get long (`docs/38`
        // §38.4): five- and six-label rows are ordinary, and a language that makes you write them
        // out is a language whose signatures nobody reads.
        if self.at_kw("row") {
            self.bump();
            let (name, name_span) = self.ident("a row name")?;
            self.expect(&Raw::Eq, "`=`");
            let mut atoms = Vec::new();
            loop {
                atoms.push(self.expr()?);
                if !self.eat(&Raw::Comma) {
                    break;
                }
            }
            let span = start.to(atoms.last().map(|a| a.span()).unwrap_or(name_span));
            self.end_of_line();
            let mut items = vec![Node::sym(name, name_span)];
            items.extend(atoms);
            return Some(Node::form(sym::ROW, items, span));
        }
        if self.at_kw("macro") {
            return self.macro_item();
        }
        if self.at_kw("model") {
            return self.model_item();
        }
        if self.at_kw("union") {
            return self.union_item();
        }
        if self.at_kw("trait") {
            return self.trait_item();
        }
        if self.at_kw("impl") {
            return self.impl_item();
        }
        if self.at_kw("type") {
            return self.type_item();
        }
        if self.at_kw("import") {
            self.bump();
            let (name, s) = self.ident("a module name")?;
            let mut path = name;
            let mut span = start.to(s);
            while self.at(&Raw::Dot) {
                self.bump();
                let (seg, s2) = self.ident("a module name")?;
                path.push('.');
                path.push_str(&seg);
                span = span.to(s2);
            }
            self.end_of_line();
            return Some(Node::form(sym::IMPORT, vec![Node::sym(path, span)], span));
        }
        if self.at_kw("test") {
            self.bump();
            let (name, name_span) = self.quoted_name("a test name")?;
            self.expect(&Raw::Colon, "`:`");
            let body = self.test_body()?;
            let span = start.to(body.span());
            return Some(Node::form(
                sym::TEST,
                vec![Node::lit(Lit::Str(name.into()), name_span), body],
                span,
            ));
        }
        // `property "…" (events: list[Event]):` — §11.10. The same clauses as a `test`, with the
        // parameters supplied by the generator instead of written out.
        if self.at_kw("property") && matches!(self.peek_raw(1), Some(Raw::Str(_))) {
            self.bump();
            let (name, name_span) = self.quoted_name("a property name")?;
            let params = self.params()?;
            self.expect(&Raw::Colon, "`:`");
            let body = self.test_body()?;
            let span = start.to(body.span());
            return Some(Node::form(
                sym::PROPERTY,
                vec![Node::lit(Lit::Str(name.into()), name_span), params, body],
                span,
            ));
        }
        // Anything else at top level is a statement — a module-level `let`, or an expression.
        self.statement()
    }

    fn decorated(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // @
        let deco = self.postfix(false)?;
        self.end_of_line();
        self.skip_newlines();
        let inner = self.item()?;
        let span = start.to(inner.span());
        Some(Node::form(sym::DECORATE, vec![deco, inner], span))
    }

    /// `def name(params) -> Ret uses eff, eff:` + block
    fn def_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // def
        let (name, name_span) = self.ident("a function name")?;
        // `def map[T, U](…)` — §3.1's "full inference inside bodies; mandatory annotations on
        // public signatures", which means a *user's* abstraction says what it is polymorphic in
        // rather than having it guessed (`docs/32` §32.7).
        let typarams = self.typarams(name_span);
        let params = self.params()?;
        let returns = if self.eat(&Raw::Arrow) {
            let t = self.type_expr()?;
            let s = t.span();
            Node::form(sym::RETURNS, vec![t], s)
        } else {
            Node::form(sym::RETURNS, vec![], name_span)
        };

        // §2.9: effect and placement annotations read better as signature clauses than as
        // decorators, and they are part of the published module interface (§3.6).
        let mut uses = Vec::new();
        if self.eat_kw("uses") {
            loop {
                uses.push(self.expr()?);
                if !self.eat(&Raw::Comma) {
                    break;
                }
            }
        }
        let uses_span = uses.first().map(|n| n.span()).unwrap_or(name_span);
        let uses = Node::form("uses", uses, uses_span);

        // A `def` with no body is a **declaration**: a signature with nothing behind it. It is
        // what a `.becki` interface file is made of (§3.6), and it is an error in an ordinary
        // module — but that is `check`'s judgement to make, not the parser's, because the parser
        // does not know which kind of file it is reading.
        if !self.at(&Raw::Colon) {
            let span = start.to(uses.span());
            return Some(Node::form(
                sym::DEF,
                vec![Node::sym(name, name_span), typarams, params, returns, uses],
                span,
            ));
        }
        self.expect(&Raw::Colon, "`:` before the function body");
        let body = self.block()?;
        let span = start.to(body.span());
        Some(Node::form(
            sym::DEF,
            vec![
                Node::sym(name, name_span),
                typarams,
                params,
                returns,
                uses,
                body,
            ],
            span,
        ))
    }

    fn macro_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // macro
        let (name, name_span) = self.ident("a macro name")?;
        let params = self.params()?;
        self.expect(&Raw::Colon, "`:` before the macro body");
        let body = self.block()?;
        let span = start.to(body.span());
        Some(Node::form(
            sym::MACRO,
            vec![Node::sym(name, name_span), params, body],
            span,
        ))
    }

    /// `[T, U]` or `[T: Show + Eq, U]`, or nothing.
    ///
    /// The same list follows the name of a `def`, a `model`, a `union` and a `type`, so a
    /// declaration and a definition are quantified by the same notation.
    ///
    /// A **bound** says which traits the parameter's argument must implement, and it is what lets a
    /// generic body call a trait method: `[T: Show]` reads as `(: T Show)`, and an unbounded
    /// parameter stays a bare symbol so that every form that never carries one is unchanged.
    fn typarams(&mut self, at: Span) -> Node {
        let start = self.span();
        if !self.at(&Raw::LBracket) {
            return Node::form(sym::TYPARAMS, Vec::new(), at);
        }
        self.bump();
        let mut out = Vec::new();
        while !self.at(&Raw::RBracket) && !self.at_eof() {
            let Some((name, span)) = self.ident("a type parameter") else {
                break;
            };
            if self.eat(&Raw::Colon) {
                let mut parts = vec![Node::sym(name, span)];
                while let Some((t, tspan)) = self.ident("a trait name") {
                    parts.push(Node::sym(t, tspan));
                    if !self.eat(&Raw::Plus) {
                        break;
                    }
                }
                let end = self.span();
                out.push(Node::form(sym::ANNOT, parts, span.to(end)));
            } else {
                out.push(Node::sym(name, span));
            }
            if !self.eat(&Raw::Comma) {
                break;
            }
        }
        let end = self.span();
        self.expect(&Raw::RBracket, "`]`");
        Node::form(sym::TYPARAMS, out, start.to(end))
    }

    fn params(&mut self) -> Option<Node> {
        let start = self.span();
        self.expect(&Raw::LParen, "`(`");
        let mut out = Vec::new();
        while !self.at(&Raw::RParen) && !self.at_eof() {
            let (name, name_span) = self.ident("a parameter name")?;
            let ty = if self.eat(&Raw::Colon) {
                Some(self.type_expr()?)
            } else {
                None
            };
            let span = ty
                .as_ref()
                .map(|t| name_span.to(t.span()))
                .unwrap_or(name_span);
            out.push(match ty {
                Some(t) => Node::form(sym::ANNOT, vec![Node::sym(name, name_span), t], span),
                None => Node::sym(name, name_span),
            });
            if !self.eat(&Raw::Comma) {
                break;
            }
        }
        let end = self.span();
        self.expect(&Raw::RParen, "`)`");
        Some(Node::form(sym::PARAMS, out, start.to(end)))
    }

    fn model_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // model
        let (name, name_span) = self.ident("a model name")?;
        let typarams = self.typarams(name_span);
        self.expect(&Raw::Colon, "`:`");
        let mut fields = vec![Node::sym(name, name_span), typarams];
        for line in self.indented_lines()? {
            let mut p = self.sub(line);
            if let Some(f) = p.field_decl() {
                fields.push(f);
            }
        }
        Some(Node::form(sym::MODEL, fields, start))
    }

    fn union_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // union
        let (name, name_span) = self.ident("a union name")?;
        let typarams = self.typarams(name_span);
        self.expect(&Raw::Colon, "`:`");
        let mut variants = vec![Node::sym(name, name_span), typarams];
        for line in self.indented_lines()? {
            let mut p = self.sub(line);
            if let Some(v) = p.variant_decl() {
                variants.push(v);
            }
        }
        Some(Node::form(sym::UNION, variants, start))
    }

    fn trait_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // trait
        let (name, name_span) = self.ident("a trait name")?;
        self.expect(&Raw::Colon, "`:`");
        let body = self.block()?;
        let mut items = vec![Node::sym(name, name_span)];
        items.extend(body.args);
        Some(Node::form(sym::TRAIT, items, start))
    }

    /// `impl[T] Show for Tree[T]:` — the list binds the names the *target* is written in terms of.
    ///
    /// It goes after `impl` rather than after the trait name because that is what it quantifies:
    /// `Tree[T]` is one impl covering every `T`, and `Show` is not parameterised at all.
    fn impl_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // impl
        let typarams = self.typarams(start);
        let (trait_name, tspan) = self.ident("a trait name")?;
        if !self.eat_kw("for") {
            self.error("expected `for` in an impl declaration");
            return None;
        }
        let ty = self.type_expr()?;
        let mut items = vec![Node::sym(trait_name, tspan), typarams, ty];
        // `impl Priced for Item` with nothing after it is a *declaration*, which is what a `.becki`
        // publishes: an importing module needs to know the implementation exists and what its
        // signature is, and the bodies stay in the module that wrote them.
        if self.eat(&Raw::Colon) {
            let body = self.block()?;
            items.extend(body.args);
        } else {
            self.end_of_line();
        }
        Some(Node::form(sym::IMPL, items, start))
    }

    fn type_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // type
        let (name, name_span) = self.ident("a type name")?;
        let typarams = self.typarams(name_span);
        self.expect(&Raw::Eq, "`=`");
        // `type CustomerId = newtype[u64]` — §3.1's zero-cost nominal newtype.
        if self.at_kw("newtype") {
            self.bump();
            self.expect(&Raw::LBracket, "`[`");
            let inner = self.type_expr()?;
            self.expect(&Raw::RBracket, "`]`");
            self.end_of_line();
            return Some(Node::form(
                sym::NEWTYPE,
                vec![Node::sym(name, name_span), typarams, inner],
                start,
            ));
        }
        let ty = self.type_expr()?;
        self.end_of_line();
        Some(Node::form(
            sym::TYPE,
            vec![Node::sym(name, name_span), typarams, ty],
            start,
        ))
    }

    fn field_decl(&mut self) -> Option<Node> {
        let (name, name_span) = self.ident("a field name")?;
        self.expect(&Raw::Colon, "`:`");
        let ty = self.type_expr()?;
        let span = name_span.to(ty.span());
        Some(Node::form(
            sym::FIELD,
            vec![Node::sym(name, name_span), ty],
            span,
        ))
    }

    fn variant_decl(&mut self) -> Option<Node> {
        let (name, name_span) = self.ident("a variant name")?;
        let mut items = vec![Node::sym(name, name_span)];
        if self.eat(&Raw::LParen) {
            while !self.at(&Raw::RParen) && !self.at_eof() {
                items.push(self.field_decl()?);
                if !self.eat(&Raw::Comma) {
                    break;
                }
            }
            self.expect(&Raw::RParen, "`)`");
        }
        Some(Node::form(sym::VARIANT, items, name_span))
    }

    // ---------------------------------------------------------------- statements

    fn peek_raw(&self, n: usize) -> Option<&Raw> {
        self.toks
            .get((self.pos + n).min(self.toks.len() - 1))?
            .raw()
    }

    fn quoted_name(&mut self, what: &str) -> Option<(String, Span)> {
        let span = self.span();
        match self.cur().raw() {
            Some(Raw::Str(s)) => {
                let s = s.clone();
                self.bump();
                Some((s, span))
            }
            _ => {
                self.error(format!("expected {what} in quotes"));
                None
            }
        }
    }

    /// A `test`/`property` body: an ordinary block, parsed with the four clause keywords live.
    fn test_body(&mut self) -> Option<Node> {
        self.in_test += 1;
        let out = self.block();
        self.in_test -= 1;
        out
    }

    // ------------------------------------------------------------ §21.2's clauses
    //
    // A test names a log, an input and an expectation, so each is a clause rather than a call: the
    // checker binds `state`, `events`, `result` and `page` around them, and none of the four words
    // is reserved outside a test body.

    /// `given <list[Event]>` or `given <list[Event]> by "actor"`.
    fn given_clause(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // given
        let events = self.expr()?;
        let mut args = vec![events];
        if self.eat_kw("by") {
            let (actor, span) = self.quoted_name("an actor name")?;
            args.push(Node::lit(Lit::Str(actor.into()), span));
        }
        let span = start.to(args.last().map(|a| a.span()).unwrap_or(start));
        self.end_of_line();
        Some(Node::form(sym::GIVEN, args, span))
    }

    /// `when c1, c2` or `when session("ana") sends c1, c2`.
    ///
    /// The session slot is always present — `_` when the test did not name one — so the form has
    /// one shape and the printer has one case. It holds the *actor*, a string literal, rather than
    /// a `Session` expression: a session is minted by the identity subsystem (§3.7) and a test that
    /// could build one out of an expression would be a way to forge one.
    fn when_clause(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // when
                     // `session("ana") sends c` — look ahead for `sends` rather than committing, so that a
                     // command called `session` is still a command.
        let session = if self.at_kw("session") && self.line_has_ident("sends") {
            let (actor, span) = self.session_actor()?;
            if !self.eat_kw("sends") {
                self.error("expected `sends` after the session");
                return None;
            }
            Node::lit(Lit::Str(actor.into()), span)
        } else {
            Node::sym(sym::WILDCARD, start)
        };
        let mut args = vec![session];
        loop {
            args.push(self.expr()?);
            if !self.eat(&Raw::Comma) {
                break;
            }
        }
        let span = start.to(args.last().map(|a| a.span()).unwrap_or(start));
        self.end_of_line();
        Some(Node::form(sym::WHEN, args, span))
    }

    /// `stub <effect atom>: <value>`, or a block that answers from the call's arguments.
    ///
    /// §21.3 rule 2 is the one-line form; rule 3 is the block:
    ///
    /// ```text
    /// stub net.out(payments.example.com):
    ///     case Charge(amount): Declined
    ///     case _: Approved
    /// ```
    ///
    /// A block of `case` arms matches on the stubbed definition's parameter — "ordinary Beck
    /// pattern matching … there is nothing to learn, nothing that composes differently from the
    /// rest of the language, and no `Expression<Func<…>>` to satisfy". A block of anything else is
    /// an ordinary body with those parameters in scope, which is the general form the `case` sugar
    /// is a case of.
    fn stub_clause(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // stub
        let (atom, atom_span) = self.effect_atom()?;
        self.expect(&Raw::Colon, "`:`");

        let value = if matches!(self.cur().tok, Tok::Newline) {
            // `case` directly under `stub` is the doc's notation and has no scrutinee written: the
            // checker supplies it, because only the checker knows what performs the effect.
            if self.block_starts_with("case") {
                self.skip_newlines();
                self.bump(); // INDENT
                let arms = self.case_arms()?;
                Node::form(sym::STUB_ARMS, arms, start.to(self.span()))
            } else {
                self.block()?
            }
        } else {
            let e = self.expr()?;
            self.end_of_line();
            e
        };

        let span = start.to(value.span());
        Some(Node::form(
            sym::STUB,
            vec![Node::lit(Lit::Str(atom.into()), atom_span), value],
            span,
        ))
    }

    /// The six shapes of `expect`. Five are decided by a leading keyword; the sixth is an ordinary
    /// `Bool` expression, optionally followed by `contains`.
    fn expect_clause(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // expect

        // `expect no net.out` — §21.3 rule 4.
        if self.at_kw("no") {
            self.bump();
            let (atom, _) = self.effect_atom()?;
            let span = start.to(self.span());
            self.end_of_line();
            return Some(Node::form(
                sym::EXPECT_EFFECT,
                vec![
                    Node::lit(Lit::Str(atom.into()), span),
                    Node::sym("none", span),
                ],
                span,
            ));
        }

        // `expect wire_compatible_with "orders.v1.becki"` — answered from `beck check --wire-compat`'s
        // own data, without running anything.
        if self.at_kw("wire_compatible_with") {
            self.bump();
            let (path, pspan) = self.quoted_name("a `.becki` path")?;
            let span = start.to(pspan);
            self.end_of_line();
            return Some(Node::form(
                sym::EXPECT_WIRE,
                vec![Node::lit(Lit::Str(path.into()), pspan)],
                span,
            ));
        }

        // `expect place(charge) == server` — §3.4's assertability guardrail, beside the code.
        if self.at_kw("place") && matches!(self.peek_raw(1), Some(Raw::LParen)) {
            self.bump();
            self.expect(&Raw::LParen, "`(`");
            let (name, nspan) = self.ident("a definition or signal name")?;
            self.expect(&Raw::RParen, "`)`");
            if !self.eat(&Raw::EqEq) {
                self.error("expected `==` and a tier");
                return None;
            }
            let (tier, tspan) = self.ident("a tier")?;
            let span = start.to(tspan);
            self.end_of_line();
            return Some(Node::form(
                sym::EXPECT_PLACE,
                vec![Node::sym(name, nspan), Node::sym(tier, tspan)],
                span,
            ));
        }

        // `expect flow(ApiKey) reaches nothing on client`.
        if self.at_kw("flow") && matches!(self.peek_raw(1), Some(Raw::LParen)) {
            self.bump();
            self.expect(&Raw::LParen, "`(`");
            let (name, nspan) = self.ident("a type name")?;
            self.expect(&Raw::RParen, "`)`");
            if !(self.eat_kw("reaches") && self.eat_kw("nothing") && self.eat_kw("on")) {
                self.error("expected `reaches nothing on <tier>`");
                return None;
            }
            let (tier, tspan) = self.ident("a tier")?;
            let span = start.to(tspan);
            self.end_of_line();
            return Some(Node::form(
                sym::EXPECT_FLOW,
                vec![Node::sym(name, nspan), Node::sym(tier, tspan)],
                span,
            ));
        }

        // `expect net.out(h) once` / `… times 2` / `… with Charge(amount=2000)`.
        if self.at_effect_atom() {
            let (atom, aspan) = self.effect_atom()?;
            let how = if self.eat_kw("once") {
                Node::form(
                    "times",
                    vec![Node::lit(Lit::Int(1), self.span())],
                    self.span(),
                )
            } else if self.eat_kw("times") {
                let span = self.span();
                match self.cur().raw() {
                    Some(Raw::Int(n)) => {
                        let n = *n;
                        self.bump();
                        Node::form("times", vec![Node::lit(Lit::Int(n), span)], span)
                    }
                    _ => {
                        self.error("expected a count after `times`");
                        return None;
                    }
                }
            } else if self.eat_kw("with") {
                let e = self.expr()?;
                let s = e.span();
                Node::form("with", vec![e], s)
            } else {
                self.error("expected `once`, `times <n>` or `with <value>` after an effect atom");
                return None;
            };
            let span = start.to(how.span());
            self.end_of_line();
            return Some(Node::form(
                sym::EXPECT_EFFECT,
                vec![Node::lit(Lit::Str(atom.into()), aspan), how],
                span,
            ));
        }

        // `expect page contains "milk"` / `expect page(session("bo")) contains "milk"`. The page is
        // the subject rather than an expression because rendering one is `per_session(state, view)`
        // applied — a role the runtime drives, not a function the test scope can hold.
        if self.at_kw("page") {
            self.bump();
            let mut args = Vec::new();
            if self.at(&Raw::LParen) {
                self.bump();
                if !self.eat_kw("session") {
                    self.error("expected `session(\"actor\")`");
                    return None;
                }
                let (actor, aspan) = self.parenthesised_string("an actor name")?;
                self.expect(&Raw::RParen, "`)`");
                args.push(Node::lit(Lit::Str(actor.into()), aspan));
            }
            if !self.eat_kw("contains") {
                self.error("expected `contains` and a string");
                return None;
            }
            let needle = self.expr()?;
            let span = start.to(needle.span());
            self.end_of_line();
            args.insert(0, needle);
            return Some(Node::form(sym::EXPECT_CONTAINS, args, span));
        }

        // `expect state == fold_of [ … ]` — §21.2's identity test. Folding a log is what the data
        // tier does, so the comparison names the log and lets the harness fold it.
        if self.at_kw("state")
            && matches!(self.peek_raw(1), Some(Raw::EqEq))
            && matches!(self.peek_raw(2), Some(Raw::Ident(s)) if s == "fold_of")
        {
            self.bump(); // state
            self.bump(); // ==
            self.bump(); // fold_of
            let events = self.expr()?;
            let mut args = vec![events];
            if self.eat_kw("by") {
                let (actor, span) = self.quoted_name("an actor name")?;
                args.push(Node::lit(Lit::Str(actor.into()), span));
            }
            let span = start.to(args.last().map(|a| a.span()).unwrap_or(start));
            self.end_of_line();
            return Some(Node::form(sym::EXPECT_FOLD, args, span));
        }

        // The ordinary case: a `Bool` expression, in a scope where `state`, `events` and `result`
        // are bound. `expect Ok(…)`/`expect Err(…)` is shorthand for `result == …`.
        let e = self.expr()?;
        let e = match e.head_name() {
            Some("Ok" | "Err") if e.applied => {
                let span = e.span();
                Node::form("==", vec![Node::sym("result", span), e], span)
            }
            _ => e,
        };
        let span = start.to(e.span());
        self.end_of_line();
        Some(Node::form(sym::EXPECT, vec![e], span))
    }

    /// The heads an effect atom can start with. Deliberately a closed list: it is what makes
    /// `expect net.out(h) once` and `expect is_done(state)` decidable without backtracking.
    const EFFECT_HEADS: &'static [&'static str] = &[
        "ingress", "durable", "dom", "nondet", "net", "fs", "env", "spawn", "cap", "partial",
        "external", "log", "metrics",
    ];

    fn at_effect_atom(&self) -> bool {
        match self.cur().raw() {
            Some(Raw::Ident(s)) => Self::EFFECT_HEADS.contains(&s.as_str()),
            _ => false,
        }
    }

    /// `net.out(payments.example.com)`, `cap.session`, `fs(/tmp)`, `env`.
    ///
    /// Reassembled from tokens rather than sliced from the source, because the parser does not hold
    /// the source; the atom vocabulary is small enough that this is exact.
    fn effect_atom(&mut self) -> Option<(String, Span)> {
        let start = self.span();
        let (head, _) = self.ident("an effect atom")?;
        let mut out = head;
        while self.at(&Raw::Dot) {
            self.bump();
            let (seg, _) = self.ident("an effect atom")?;
            out.push('.');
            out.push_str(&seg);
        }
        let mut end = self.span();
        if self.at(&Raw::LParen) {
            self.bump();
            out.push('(');
            let mut depth = 1;
            loop {
                match self.cur().raw() {
                    Some(Raw::LParen) => depth += 1,
                    Some(Raw::RParen) => {
                        depth -= 1;
                        if depth == 0 {
                            end = self.span();
                            self.bump();
                            break;
                        }
                    }
                    None => {
                        self.error("unterminated effect atom");
                        return None;
                    }
                    _ => {}
                }
                out.push_str(&token_text(self.cur()));
                self.bump();
            }
            out.push(')');
        }
        Some((out, start.to(end)))
    }

    /// Does the block about to be parsed — newlines, then `INDENT` — open with this keyword?
    ///
    /// Lookahead without consuming, because the caller may still want [`Parser::block`] to handle
    /// the layout tokens itself.
    fn block_starts_with(&self, kw: &str) -> bool {
        let mut i = self.pos;
        while matches!(self.toks.get(i).map(|t| &t.tok), Some(Tok::Newline)) {
            i += 1;
        }
        if !matches!(self.toks.get(i).map(|t| &t.tok), Some(Tok::Indent)) {
            return false;
        }
        matches!(self.toks.get(i + 1).and_then(|t| t.raw()), Some(Raw::Ident(s)) if s == kw)
    }

    /// `session("ana")`, already known to be there.
    fn session_actor(&mut self) -> Option<(String, Span)> {
        self.bump(); // session
        self.parenthesised_string("an actor name")
    }

    /// `("ana")` — the parentheses and the string inside them.
    fn parenthesised_string(&mut self, what: &str) -> Option<(String, Span)> {
        self.expect(&Raw::LParen, "`(`");
        let out = self.quoted_name(what)?;
        self.expect(&Raw::RParen, "`)`");
        Some(out)
    }

    /// Is `name` an identifier on the rest of this logical line, outside brackets?
    fn line_has_ident(&self, name: &str) -> bool {
        let mut depth = 0i32;
        for t in &self.toks[self.pos..] {
            match &t.tok {
                Tok::Newline | Tok::Indent | Tok::Dedent | Tok::Eof if depth == 0 => return false,
                Tok::Raw(Raw::LParen | Raw::LBracket | Raw::LBrace) => depth += 1,
                Tok::Raw(Raw::RParen | Raw::RBracket | Raw::RBrace) => depth -= 1,
                Tok::Raw(Raw::Ident(s)) if depth == 0 && s == name => return true,
                _ => {}
            }
        }
        false
    }

    /// Descend one level of user-chosen structure, or refuse.
    ///
    /// `false` means the ceiling is reached: the caller returns `None` without recursing and
    /// without leaving, and the parser's ordinary recovery takes it from there. The two callers are
    /// [`Parser::block`] and [`Parser::primary`] — the two places the parser re-enters itself, one
    /// per level of indentation and one per level of brackets.
    fn enter(&mut self) -> bool {
        if self.nesting.enter() {
            return true;
        }
        if self.nesting.should_report() {
            let span = self.span();
            let note = self.nesting.note();
            self.diags.push(
                Diagnostic::error("B0121", "nesting is too deep to read", span)
                    .with_primary_label("the parser gave up here")
                    .with_note(note),
            );
        }
        self.poisoned = true;
        false
    }

    fn block(&mut self) -> Option<Node> {
        if !self.enter() {
            return None;
        }
        let out = self.block_inner();
        self.nesting.leave();
        out
    }

    fn block_inner(&mut self) -> Option<Node> {
        let start = self.span();
        // `f(x): expr` — the single-line form of the block rule (§2.3).
        if !matches!(self.cur().tok, Tok::Newline) {
            let e = self.statement()?;
            let s = e.span();
            return Some(Node::form(sym::DO, vec![e], start.to(s)));
        }
        self.skip_newlines();
        if !matches!(self.cur().tok, Tok::Indent) {
            self.error("expected an indented block");
            return None;
        }
        self.bump(); // INDENT
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            match self.cur().tok {
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                Tok::Eof => break,
                _ => match self.statement() {
                    Some(s) => stmts.push(s),
                    None => {
                        self.recover_in_block();
                        if matches!(self.cur().tok, Tok::Dedent) {
                            self.bump();
                            break;
                        }
                        if self.at_eof() {
                            break;
                        }
                    }
                },
            }
        }
        let end = self.span();
        Some(Node::form(sym::DO, stmts, start.to(end)))
    }

    fn recover_in_block(&mut self) {
        self.poisoned = false;
        let mut depth = 0i32;
        loop {
            match &self.cur().tok {
                Tok::Eof => return,
                Tok::Indent => {
                    depth += 1;
                    self.bump();
                }
                Tok::Dedent if depth > 0 => {
                    depth -= 1;
                    self.bump();
                }
                Tok::Dedent => return,
                Tok::Newline if depth == 0 => {
                    self.bump();
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Collect the raw token runs of an indented block's lines. Used by `model`/`union`, whose
    /// bodies are declarations rather than expressions.
    fn indented_lines(&mut self) -> Option<Vec<Vec<Token>>> {
        self.skip_newlines();
        if !matches!(self.cur().tok, Tok::Indent) {
            self.error("expected an indented block");
            return None;
        }
        self.bump();
        let mut lines = Vec::new();
        let mut cur: Vec<Token> = Vec::new();
        let mut depth = 0i32;
        loop {
            match &self.cur().tok {
                Tok::Eof => break,
                Tok::Dedent if depth == 0 => {
                    self.bump();
                    break;
                }
                Tok::Dedent => {
                    depth -= 1;
                    cur.push(self.bump());
                }
                Tok::Indent => {
                    depth += 1;
                    cur.push(self.bump());
                }
                Tok::Newline if depth == 0 => {
                    self.bump();
                    if !cur.is_empty() {
                        lines.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(self.bump()),
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        Some(lines)
    }

    /// A sub-parser over a captured token run, sharing the diagnostics sink.
    fn sub(&mut self, mut toks: Vec<Token>) -> Parser<'_> {
        let end = toks.last().map(|t| t.span).unwrap_or(Span::NONE);
        toks.push(Token {
            tok: Tok::Eof,
            span: end,
        });
        Parser {
            toks,
            pos: 0,
            diags: self.diags,
            file: self.file,
            poisoned: false,
            attached_block: false,
            nesting: self.nesting.resumed(),
            in_test: self.in_test,
        }
    }

    /// Does the current logical line contain a binding `=` outside brackets?
    ///
    /// `=` inside brackets is a keyword argument (`f(x=1)`), not a binding, so bracket depth is
    /// tracked; `==` is a separate token and never matches.
    fn line_has_assignment(&self) -> bool {
        let mut depth = 0i32;
        for t in &self.toks[self.pos..] {
            match &t.tok {
                Tok::Newline | Tok::Indent | Tok::Dedent | Tok::Eof if depth == 0 => return false,
                Tok::Raw(Raw::LParen | Raw::LBracket | Raw::LBrace) => depth += 1,
                Tok::Raw(Raw::RParen | Raw::RBracket | Raw::RBrace) => depth -= 1,
                Tok::Raw(Raw::Eq) if depth == 0 => return true,
                _ => {}
            }
        }
        false
    }

    fn end_of_line(&mut self) {
        if matches!(self.cur().tok, Tok::Newline) {
            self.bump();
        }
    }

    fn statement(&mut self) -> Option<Node> {
        let start = self.span();

        // §21.2's clauses, live only inside a `test`/`property` body.
        if self.in_test > 0 {
            if self.at_kw("given") {
                return self.given_clause();
            }
            if self.at_kw("when") {
                return self.when_clause();
            }
            if self.at_kw("expect") {
                return self.expect_clause();
            }
            if self.at_kw("stub") {
                return self.stub_clause();
            }
        }

        if self.at(&Raw::At) {
            return self.decorated();
        }
        if self.at_kw("def") {
            return self.def_item();
        }
        if self.at_kw("return") {
            self.bump();
            if matches!(self.cur().tok, Tok::Newline | Tok::Dedent | Tok::Eof) {
                self.end_of_line();
                return Some(Node::form(sym::RETURN, vec![], start));
            }
            let e = self.expr_stmt()?;
            let span = start.to(e.span());
            self.end_of_line();
            return Some(Node::form(sym::RETURN, vec![e], span));
        }
        if self.at_kw("if") {
            return self.if_stmt();
        }
        if self.at_kw("for") {
            self.bump();
            let (name, name_span) = self.ident("a loop variable")?;
            if !self.eat_kw("in") {
                self.error("expected `in`");
                return None;
            }
            let seq = self.expr()?;
            self.expect(&Raw::Colon, "`:`");
            let body = self.block()?;
            let span = start.to(body.span());
            return Some(Node::form(
                sym::FOR,
                vec![Node::sym(name, name_span), seq, body],
                span,
            ));
        }
        if self.at_kw("while") {
            self.bump();
            let c = self.expr()?;
            self.expect(&Raw::Colon, "`:`");
            let body = self.block()?;
            let span = start.to(body.span());
            return Some(Node::form(sym::WHILE, vec![c, body], span));
        }
        if self.at_kw("match") {
            return self.match_stmt();
        }
        if self.at_kw("var") {
            self.bump();
            let (name, name_span) = self.ident("a variable name")?;
            let ty = if self.eat(&Raw::Colon) {
                Some(self.type_expr()?)
            } else {
                None
            };
            self.expect(&Raw::Eq, "`=`");
            let e = self.expr_stmt()?;
            let span = start.to(e.span());
            self.end_of_line();
            let target = match ty {
                Some(t) => Node::form(sym::ANNOT, vec![Node::sym(name, name_span), t], name_span),
                None => Node::sym(name, name_span),
            };
            return Some(Node::form(sym::VAR, vec![target, e], span));
        }
        if self.at_kw("quote") {
            // A bare `quote:` block is an expression statement; fall through to `expr`.
        }

        // Assignment or bare expression. `x = e` and `x: T = e` both bind.
        //
        // The lookahead is decided *before* committing, by scanning the logical line for a
        // top-level `=`. Without that, `h1: "todos"` — a block call in the `ui:` vocabulary —
        // enters the annotated-binding path and reports a bogus "expected a type".
        let save = self.pos;
        if self.line_has_assignment() {
            if let Some(Raw::Ident(name)) = self.cur().raw().cloned() {
                let name_span = self.span();
                self.bump();
                let ty = if self.at(&Raw::Colon) {
                    self.bump();
                    match self.type_expr() {
                        Some(t) => Some(t),
                        None => {
                            self.pos = save;
                            self.poisoned = false;
                            None
                        }
                    }
                } else {
                    None
                };
                if self.at(&Raw::Eq) {
                    self.bump();
                    let e = self.expr_stmt()?;
                    let span = start.to(e.span());
                    self.end_of_line();
                    let target = match ty {
                        Some(t) => {
                            Node::form(sym::ANNOT, vec![Node::sym(&name, name_span), t], name_span)
                        }
                        None => Node::sym(&name, name_span),
                    };
                    return Some(Node::form(sym::LET, vec![target, e], span));
                }
                self.pos = save;
                self.poisoned = false;
            }
        }

        let e = self.expr_stmt()?;
        self.end_of_line();
        Some(e)
    }

    fn if_stmt(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // if / elif
        let cond = self.expr()?;
        self.expect(&Raw::Colon, "`:`");
        let then = self.block()?;
        self.skip_newlines();
        let mut args = vec![cond, then];
        if self.at_kw("elif") {
            let e = self.if_stmt()?;
            let s = e.span();
            args.push(Node::form(sym::DO, vec![e], s));
        } else if self.at_kw("else") {
            self.bump();
            self.expect(&Raw::Colon, "`:`");
            args.push(self.block()?);
        }
        let span = start.to(args.last().map(|n| n.span()).unwrap_or(start));
        Some(Node::form(sym::IF, args, span))
    }

    fn match_stmt(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // match
        let scrutinee = self.expr()?;
        self.expect(&Raw::Colon, "`:`");
        self.skip_newlines();
        if !matches!(self.cur().tok, Tok::Indent) {
            self.error("expected an indented block of `case` arms");
            return None;
        }
        self.bump();
        let mut arms = vec![scrutinee];
        arms.extend(self.case_arms()?);
        let span = start.to(self.span());
        Some(Node::form(sym::MATCH, arms, span))
    }

    /// The `case` arms of a block whose `INDENT` has already been consumed.
    ///
    /// Shared by `match` and by §21.3 rule 3's `stub`, so a stub's arms are the language's own
    /// pattern matching rather than a second, drifting notation.
    fn case_arms(&mut self) -> Option<Vec<Node>> {
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            match self.cur().tok {
                Tok::Dedent => {
                    self.bump();
                    break;
                }
                Tok::Eof => break,
                _ => {}
            }
            let arm_start = self.span();
            if !self.eat_kw("case") {
                self.error("expected `case`");
                self.recover_in_block();
                continue;
            }
            // Patterns are ordinary `Node`s: `Added(id, text)` is the form `(Added id text)`,
            // `_` is the wildcard symbol, a literal is a literal. Nothing new to represent.
            let pat = self.expr()?;
            self.expect(&Raw::Colon, "`:`");
            let body = self.block()?;
            let span = arm_start.to(body.span());
            arms.push(Node::form(sym::CASE, vec![pat, body], span));
        }
        Some(arms)
    }

    // ---------------------------------------------------------------- expressions (Pratt)

    pub fn expr(&mut self) -> Option<Node> {
        self.expr_bp(0)
    }

    /// An expression in *statement* position, where the block rule applies.
    ///
    /// §2.7's fourth honest loss is "a trailing-block ambiguity when a call with a block is itself
    /// an argument to another call", mitigated by "a hard syntax rule — a block-form call may not
    /// appear as a non-final argument". That rule is enforced here by construction: `:` only opens
    /// a block on the outermost call of a statement, so `for t in todos:` parses its sequence as an
    /// ordinary expression rather than swallowing the loop body.
    fn expr_stmt(&mut self) -> Option<Node> {
        if self.at_kw("not")
            || self.at(&Raw::Minus)
            || self.at(&Raw::Dollar)
            || self.at(&Raw::DollarStar)
        {
            return self.expr();
        }
        let first = self.postfix(true)?;
        if self.attached_block {
            self.attached_block = false;
            return Some(first);
        }
        self.expr_bp_from(first, 0)
    }

    fn expr_bp(&mut self, min_bp: u8) -> Option<Node> {
        let lhs = self.unary()?;
        self.expr_bp_from(lhs, min_bp)
    }

    fn expr_bp_from(&mut self, lhs: Node, min_bp: u8) -> Option<Node> {
        let mut lhs = lhs;

        loop {
            // `a if c else b` — Python's conditional expression, at the lowest precedence, which
            // is what makes `x = if c: 1 else: 2` (§2.6) expressible without a statement.
            if self.at_kw("if") && min_bp == 0 {
                self.bump();
                let cond = self.expr_bp(1)?;
                if !self.eat_kw("else") {
                    self.error("expected `else` in a conditional expression");
                    return None;
                }
                let alt = self.expr_bp(0)?;
                let span = lhs.span().to(alt.span());
                lhs = Node::form(sym::IF, vec![cond, lhs, alt], span);
                continue;
            }

            let (op, lbp, rbp) = match self.infix_op() {
                Some(x) => x,
                None => break,
            };
            if lbp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.expr_bp(rbp)?;
            let span = lhs.span().to(rhs.span());
            lhs = Node::form(op, vec![lhs, rhs], span);
        }
        Some(lhs)
    }

    /// §2.6: "Fixed precedence table; user-defined operators allowed but only at existing
    /// precedence levels."
    fn infix_op(&self) -> Option<(&'static str, u8, u8)> {
        if self.at_kw("or") {
            return Some(("or", 1, 2));
        }
        if self.at_kw("and") {
            return Some(("and", 3, 4));
        }
        if self.at_kw("in") {
            return Some(("contains", 5, 6));
        }
        let r = self.cur().raw()?;
        Some(match r {
            Raw::EqEq => ("==", 5, 6),
            Raw::NotEq => ("!=", 5, 6),
            Raw::Lt => ("<", 5, 6),
            Raw::LtEq => ("<=", 5, 6),
            Raw::Gt => (">", 5, 6),
            Raw::GtEq => (">=", 5, 6),
            Raw::Plus => ("+", 7, 8),
            Raw::Minus => ("-", 7, 8),
            Raw::Star => ("*", 9, 10),
            Raw::Slash => ("/", 9, 10),
            Raw::Percent => ("%", 9, 10),
            _ => return None,
        })
    }

    fn unary(&mut self) -> Option<Node> {
        let start = self.span();
        if self.at_kw("not") {
            self.bump();
            let e = self.unary()?;
            let span = start.to(e.span());
            return Some(Node::form("not", vec![e], span));
        }
        if self.at(&Raw::Minus) {
            self.bump();
            let e = self.unary()?;
            let span = start.to(e.span());
            return Some(Node::form("negate", vec![e], span));
        }
        if self.at(&Raw::Dollar) {
            self.bump();
            let e = self.unary()?;
            let span = start.to(e.span());
            return Some(Node::form(sym::UNQUOTE, vec![e], span));
        }
        if self.at(&Raw::DollarStar) {
            self.bump();
            let e = self.unary()?;
            let span = start.to(e.span());
            return Some(Node::form(sym::SPLICE, vec![e], span));
        }
        self.postfix(false)
    }

    fn postfix(&mut self, allow_block: bool) -> Option<Node> {
        let mut e = self.primary()?;
        loop {
            if self.at(&Raw::Dot) {
                self.bump();
                let (name, name_span) = self.ident("a field or method name")?;
                let span = e.span().to(name_span);
                // `(. obj name)` is a field read; `(. obj name args...)` a method call — exactly
                // the notation §2.2 prints.
                if self.at(&Raw::LParen) {
                    let (args, aspan) = self.call_args()?;
                    let mut items = vec![e, Node::sym(name, name_span)];
                    items.extend(args);
                    e = Node::form(sym::DOT, items, span.to(aspan));
                } else {
                    e = Node::form(sym::DOT, vec![e, Node::sym(name, name_span)], span);
                }
                continue;
            }
            if self.at(&Raw::LParen) {
                let (args, aspan) = self.call_args()?;
                let span = e.span().to(aspan);
                e = match e.head {
                    // A plain name applies directly: `(update_at todos id ...)`, as written.
                    Head::Sym(s) if e.args.is_empty() => Node::form_sym(s, args, span),
                    _ => {
                        let mut items = vec![e];
                        items.extend(args);
                        Node::form(sym::CALL, items, span)
                    }
                };
                // The block rule (§2.3): a call directly followed by `:` takes the indented block
                // as a quoted `do=` argument.
                if allow_block && self.at(&Raw::Colon) {
                    e = self.attach_block(e)?;
                    break;
                }
                continue;
            }
            if self.at(&Raw::LBracket) {
                self.bump();
                let idx = self.expr()?;
                let end = self.span();
                self.expect(&Raw::RBracket, "`]`");
                let span = e.span().to(end);
                e = Node::form("index", vec![e, idx], span);
                continue;
            }
            // A bare name followed by `:` and a block is also a call: `main:` is `main(do=...)`.
            if allow_block && self.at(&Raw::Colon) && e.as_var().is_some() {
                e = self.attach_block(e)?;
                break;
            }
            break;
        }
        Some(e)
    }

    /// `f(x):` + block  ⇒  `f(x, do=quote(block))`.
    fn attach_block(&mut self, callee: Node) -> Option<Node> {
        let colon = self.span();
        self.bump(); // :
        let body = self.block()?;
        let bspan = body.span();
        let quoted = Node::form(sym::QUOTE, vec![body], bspan);
        let kw = Node::form(
            sym::KW_ARG,
            vec![Node::sym("do", colon), quoted],
            colon.to(bspan),
        );
        let span = callee.span().to(bspan);
        let mut n = callee;
        n.args.push(kw);
        n.applied = true;
        n.meta.span = span;
        self.attached_block = true;
        Some(n)
    }

    fn call_args(&mut self) -> Option<(Vec<Node>, Span)> {
        let start = self.span();
        self.expect(&Raw::LParen, "`(`");
        let mut args = Vec::new();
        while !self.at(&Raw::RParen) && !self.at_eof() {
            // `name=value` — a keyword argument, which is also how the block rule passes `do`.
            let save = self.pos;
            if let Some(Raw::Ident(name)) = self.cur().raw().cloned() {
                let nspan = self.span();
                self.bump();
                if self.at(&Raw::Eq) {
                    self.bump();
                    let v = self.expr()?;
                    let span = nspan.to(v.span());
                    args.push(Node::form(
                        sym::KW_ARG,
                        vec![Node::sym(&name, nspan), v],
                        span,
                    ));
                    if !self.eat(&Raw::Comma) {
                        break;
                    }
                    continue;
                }
                self.pos = save;
            }
            args.push(self.expr()?);
            if !self.eat(&Raw::Comma) {
                break;
            }
        }
        let end = self.span();
        self.expect(&Raw::RParen, "`)`");
        Some((args, start.to(end)))
    }

    fn primary(&mut self) -> Option<Node> {
        if !self.enter() {
            return None;
        }
        let out = self.primary_inner();
        self.nesting.leave();
        out
    }

    fn primary_inner(&mut self) -> Option<Node> {
        let span = self.span();
        if self.at_kw("lambda") {
            self.bump();
            let mut params = Vec::new();
            while !self.at(&Raw::Colon) && !self.at_eof() {
                let (name, nspan) = self.ident("a parameter name")?;
                params.push(Node::sym(name, nspan));
                if !self.eat(&Raw::Comma) {
                    break;
                }
            }
            self.expect(&Raw::Colon, "`:`");
            let body = self.expr()?;
            let bspan = body.span();
            return Some(Node::form(
                sym::FN,
                vec![
                    Node::form(sym::PARAMS, params, span),
                    Node::form(sym::DO, vec![body], bspan),
                ],
                span.to(bspan),
            ));
        }
        // `raise e` and `try: block` are expressions, not statements: `x = try: f()` is the form
        // that makes a `Result` out of a failure, and a `raise` in the middle of an expression is
        // exactly where a fallible branch wants to be.
        if self.at_kw("raise") {
            self.bump();
            let e = self.expr()?;
            let espan = e.span();
            return Some(Node::form(sym::RAISE, vec![e], span.to(espan)));
        }
        if self.at_kw("try") {
            self.bump();
            self.expect(&Raw::Colon, "`:` after `try`");
            let body = self.block()?;
            let bspan = body.span();
            return Some(Node::form(sym::TRY, vec![body], span.to(bspan)));
        }
        if self.at_kw("quote") {
            self.bump();
            self.expect(&Raw::Colon, "`:` after `quote`");
            let body = self.block()?;
            let bspan = body.span();
            return Some(Node::form(sym::QUOTE, vec![body], span.to(bspan)));
        }

        match self.cur().raw().cloned() {
            Some(Raw::Int(n)) => {
                self.bump();
                Some(Node::lit(Lit::Int(n), span))
            }
            Some(Raw::Float(n)) => {
                self.bump();
                Some(Node::lit(Lit::Float(n), span))
            }
            Some(Raw::Str(s)) => {
                self.bump();
                Some(Node::lit(Lit::Str(s.into()), span))
            }
            Some(Raw::Keyword(k)) => {
                self.bump();
                Some(Node::lit(Lit::Keyword(k.into()), span))
            }
            Some(Raw::Ident(name)) => {
                self.bump();
                match name.as_str() {
                    "True" | "true" => Some(Node::lit(Lit::Bool(true), span)),
                    "False" | "false" => Some(Node::lit(Lit::Bool(false), span)),
                    _ => Some(Node::symbol(Symbol::new(&name), span)),
                }
            }
            Some(Raw::LParen) => {
                self.bump();
                if self.at(&Raw::RParen) {
                    let end = self.span();
                    self.bump();
                    return Some(Node::sym("unit", span.to(end)));
                }
                let e = self.expr()?;
                self.expect(&Raw::RParen, "`)`");
                Some(e)
            }
            Some(Raw::LBracket) => {
                self.bump();
                let mut items = Vec::new();
                while !self.at(&Raw::RBracket) && !self.at_eof() {
                    // `*rest` — only meaningful in a `case` pattern, and parsed here rather than in
                    // a separate pattern grammar because §2.6's patterns *are* expressions:
                    // "`Added(id, text)` is the form `(Added id text)` … Nothing new to
                    // represent". The checker is what refuses it outside a pattern
                    // (`docs/33` §33.5).
                    if self.at(&Raw::Star) {
                        let star = self.span();
                        self.bump();
                        let e = self.postfix(false)?;
                        let sp = star.to(e.span());
                        items.push(Node::form(sym::REST, vec![e], sp));
                    } else {
                        items.push(self.expr()?);
                    }
                    if !self.eat(&Raw::Comma) {
                        break;
                    }
                }
                let end = self.span();
                self.expect(&Raw::RBracket, "`]`");
                Some(Node::form(sym::LIST, items, span.to(end)))
            }
            Some(Raw::LBrace) => {
                self.bump();
                // `{name: value}` is a record literal; `{key_expr: value}` a map literal. The
                // discriminator is whether the key is a bare identifier, which is exactly how the
                // sketch's `{:id id :text text}` reads in the S-expression surface.
                let mut items = Vec::new();
                let mut is_record = true;
                while !self.at(&Raw::RBrace) && !self.at_eof() {
                    let key_span = self.span();
                    let key = match self.cur().raw().cloned() {
                        Some(Raw::Ident(name))
                            if self.toks.get(self.pos + 1).map(|t| t.raw())
                                == Some(Some(&Raw::Colon)) =>
                        {
                            self.bump();
                            Node::lit(Lit::Keyword(name.into()), key_span)
                        }
                        _ => {
                            is_record = false;
                            self.expr()?
                        }
                    };
                    self.expect(&Raw::Colon, "`:`");
                    let value = self.expr()?;
                    items.push(key);
                    items.push(value);
                    if !self.eat(&Raw::Comma) {
                        break;
                    }
                }
                let end = self.span();
                self.expect(&Raw::RBrace, "`}`");
                let head = if is_record { sym::RECORD } else { sym::MAP };
                Some(Node::form(head, items, span.to(end)))
            }
            _ => {
                self.error(format!(
                    "expected an expression, found {}",
                    self.cur().describe()
                ));
                None
            }
        }
    }

    // ---------------------------------------------------------------- types

    /// A type is a name, a generic application `Map[K, V]`, or a function type `(A, B) -> R`.
    /// It reads to the same `Node` shape as any other application: `(Map K V)`.
    fn type_expr(&mut self) -> Option<Node> {
        let start = self.span();
        if self.at(&Raw::LParen) {
            self.bump();
            let mut params = Vec::new();
            while !self.at(&Raw::RParen) && !self.at_eof() {
                params.push(self.type_expr()?);
                if !self.eat(&Raw::Comma) {
                    break;
                }
            }
            self.expect(&Raw::RParen, "`)`");
            if self.eat(&Raw::Arrow) {
                let ret = self.type_expr()?;
                let span = start.to(ret.span());
                let mut items = params;
                items.push(ret);
                return Some(Node::form("fn-type", items, span));
            }
            // A parenthesised type with one member is just that type.
            if params.len() == 1 {
                return params.pop();
            }
            let span = start.to(self.span());
            return Some(Node::form("tuple-type", params, span));
        }

        let (name, name_span) = self.ident("a type")?;
        let mut node = Node::sym(&name, name_span);
        if self.at(&Raw::LBracket) {
            self.bump();
            let mut args = Vec::new();
            while !self.at(&Raw::RBracket) && !self.at_eof() {
                args.push(self.type_expr()?);
                if !self.eat(&Raw::Comma) {
                    break;
                }
            }
            let end = self.span();
            self.expect(&Raw::RBracket, "`]`");
            node = Node::form(&name, args, name_span.to(end));
        }
        // `T -> U` for a one-argument function type.
        if self.eat(&Raw::Arrow) {
            let ret = self.type_expr()?;
            let span = node.span().to(ret.span());
            return Some(Node::form("fn-type", vec![node, ret], span));
        }
        Some(node)
    }
}

/// A token's source text, for the one place the parser has to reassemble it: the inside of an
/// effect atom's parentheses, where `payments.example.com` is three tokens and one host name.
fn token_text(t: &Token) -> String {
    match &t.tok {
        Tok::Raw(Raw::Ident(s)) => s.clone(),
        Tok::Raw(Raw::Str(s)) => s.clone(),
        Tok::Raw(Raw::Keyword(s)) => format!(":{s}"),
        Tok::Raw(Raw::Int(n)) => n.to_string(),
        Tok::Raw(Raw::Float(n)) => n.to_string(),
        Tok::Raw(Raw::Dot) => ".".into(),
        Tok::Raw(Raw::Slash) => "/".into(),
        Tok::Raw(Raw::Minus) => "-".into(),
        Tok::Raw(Raw::Star) => "*".into(),
        Tok::Raw(Raw::Comma) => ",".into(),
        Tok::Raw(Raw::Colon) => ":".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print;

    fn parse(src: &str) -> (Node, beck_diag::SourceMap) {
        let mut map = beck_diag::SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let n = parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
        (n, map)
    }

    fn sx(src: &str) -> String {
        let (n, _) = parse(src);
        // Drop the `(module t ...)` wrapper for readability in assertions.
        print::to_sexpr(&n.args[1])
    }

    #[test]
    fn a_def_carries_params_returns_and_effects() {
        assert_eq!(
            sx("def toggle(t: Todo) -> Todo uses durable:\n    return t\n"),
            "(def toggle (typarams) (params (: t Todo)) (returns Todo) (uses durable) (do (return t)))"
        );
    }

    #[test]
    fn the_doc_example_produces_the_documented_tree() {
        // §2.2's side-by-side, verbatim.
        assert_eq!(
            sx("def toggle(todos: Map[Id, Todo], e: Toggled) -> Map[Id, Todo]:\n\
                \x20   return todos.update(e.id, lambda t: t.with(done=not t.done))\n"),
            "(def toggle (typarams) (params (: todos (Map Id Todo)) (: e Toggled)) (returns (Map Id Todo)) \
             (uses) (do (return (. todos update (. e id) (fn (params t) (do (. t with (kw done (not (. t done)))))))))) "
                .trim_end()
        );
    }

    #[test]
    fn precedence_is_conventional() {
        let (n, _) = parse("x = 1 + 2 * 3 < 4 and not b\n");
        assert_eq!(
            print::to_sexpr(&n.args[1]),
            "(let x (and (< (+ 1 (* 2 3)) 4) (not b)))"
        );
    }

    #[test]
    fn the_block_rule_passes_the_body_as_a_quoted_argument() {
        assert_eq!(
            sx("retry(times=3):\n    charge(card)\n"),
            "(retry (kw times 3) (kw do (quote (do (charge card)))))"
        );
        // Bare-name form: `main:` is a call too.
        assert_eq!(
            sx("main:\n    h1: \"todos\"\n"),
            "(main (kw do (quote (do (h1 (kw do (quote (do \"todos\"))))))))"
        );
    }

    #[test]
    fn decorators_receive_the_definitions_ast() {
        assert_eq!(
            sx("@on(server)\ndef f() -> Int:\n    return 1\n"),
            "(decorate (on server) (def f (typarams) (params) (returns Int) (uses) (do (return 1))))"
        );
    }

    #[test]
    fn models_unions_and_newtypes() {
        assert_eq!(
            sx("model Todo:\n    id: Id\n    done: Bool\n"),
            "(model Todo (typarams) (field id Id) (field done Bool))"
        );
        assert_eq!(
            sx("union Event:\n    Added(id: Id, text: Str)\n    Toggled(id: Id)\n"),
            "(union Event (typarams) (variant Added (field id Id) (field text Str)) \
             (variant Toggled (field id Id)))"
        );
        assert_eq!(
            sx("type Id = newtype[Uuid]\n"),
            "(newtype Id (typarams) Uuid)"
        );
        assert_eq!(
            sx("type Ids = list[Id]\n"),
            "(type Ids (typarams) (list Id))"
        );
    }

    #[test]
    fn match_arms_are_ordinary_nodes() {
        assert_eq!(
            sx("match e:\n    case Added(id, text):\n        return 1\n    case _:\n        return 2\n"),
            "(match e (case (Added id text) (do (return 1))) (case _ (do (return 2))))"
        );
    }

    #[test]
    fn conditional_expressions_and_collections() {
        assert_eq!(sx("x = 1 if c else 2\n"), "(let x (if c 1 2))");
        assert_eq!(sx("x = [1, 2]\n"), "(let x (list 1 2))");
        assert_eq!(sx("x = {id: 1}\n"), "(let x (record :id 1))");
        assert_eq!(sx("x = {k: 1}[k]\n"), "(let x (index (record :k 1) k))");
    }

    #[test]
    fn quote_and_unquote() {
        assert_eq!(
            sx("macro unless(cond, do):\n    return quote:\n        if not $cond:\n            $do\n"),
            "(macro unless (params cond do) (do (return (quote (do (if (not (unquote cond)) (do (unquote do))))))))"
        );
    }

    #[test]
    fn errors_recover_so_later_items_still_parse() {
        // A missing comma between parameters. Note the mistake keeps brackets balanced: an
        // *unclosed* bracket suppresses layout for the rest of the file, so there are no line
        // boundaries left to recover to — the same failure mode Python has, and not one a parser
        // can paper over.
        let src = "def a(x: Int y: Int) -> Int:\n    return 1\n\ndef b() -> Int:\n    return 2\n";
        let mut map = beck_diag::SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let n = parse_module(f, "t", src, &mut d);
        assert!(d.has_errors());
        let names: Vec<String> = n
            .args
            .iter()
            .skip(1)
            .filter(|i| i.is_form(sym::DEF))
            .map(|i| i.args[0].as_var().unwrap().as_str().to_string())
            .collect();
        assert!(
            names.contains(&"b".to_string()),
            "the definition after the error must still be parsed, got {names:?}"
        );
    }
}

#[cfg(test)]
mod test_clause_tests {
    use super::*;

    fn sx(src: &str) -> String {
        let mut map = beck_diag::SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let n = parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
        crate::print::to_sexpr(&n.args[1])
    }

    #[test]
    fn the_four_clauses_read_as_forms() {
        assert_eq!(
            sx("test \"x\":\n    given []\n"),
            "(test \"x\" (do (given (list))))"
        );
        assert_eq!(
            sx("test \"x\":\n    given [a] by \"ana\"\n"),
            "(test \"x\" (do (given (list a) \"ana\")))"
        );
        assert_eq!(
            sx("test \"x\":\n    when A(id=1), B(id=2)\n"),
            "(test \"x\" (do (when _ (A (kw id 1)) (B (kw id 2)))))"
        );
        assert_eq!(
            sx("test \"x\":\n    when session(\"ana\") sends A(id=1)\n"),
            "(test \"x\" (do (when \"ana\" (A (kw id 1)))))"
        );
        assert_eq!(
            sx("test \"x\":\n    stub net.out(payments.example.com): Declined\n"),
            "(test \"x\" (do (stub \"net.out(payments.example.com)\" Declined)))"
        );
    }

    #[test]
    fn expect_has_six_shapes_and_they_are_decided_without_backtracking() {
        assert_eq!(
            sx("test \"x\":\n    expect page contains \"milk\"\n"),
            "(test \"x\" (do (expect-contains \"milk\")))"
        );
        assert_eq!(
            sx("test \"x\":\n    expect place(charge) == server\n"),
            "(test \"x\" (do (expect-place charge server)))"
        );
        assert_eq!(
            sx("test \"x\":\n    expect flow(ApiKey) reaches nothing on client\n"),
            "(test \"x\" (do (expect-flow ApiKey client)))"
        );
        assert_eq!(
            sx("test \"x\":\n    expect wire_compatible_with \"o.becki\"\n"),
            "(test \"x\" (do (expect-wire \"o.becki\")))"
        );
        assert_eq!(
            sx("test \"x\":\n    expect no net.out\n"),
            "(test \"x\" (do (expect-effect \"net.out\" none)))"
        );
        assert_eq!(
            sx("test \"x\":\n    expect net.out(h.example.com) once\n"),
            "(test \"x\" (do (expect-effect \"net.out(h.example.com)\" (times 1))))"
        );
        // …and the ordinary case is an ordinary expression.
        assert_eq!(
            sx("test \"x\":\n    expect list_len(events) == 1\n"),
            "(test \"x\" (do (expect (== (list_len events) 1))))"
        );
        // `expect Err(...)` is shorthand for `result == Err(...)`.
        assert_eq!(
            sx("test \"x\":\n    expect Err(error=BlankText)\n"),
            "(test \"x\" (do (expect (== result (Err (kw error BlankText))))))"
        );
    }

    #[test]
    fn the_clause_keywords_are_not_reserved_outside_a_test() {
        // A program with a definition called `expect` still parses as a call, because the four
        // words are live only inside a `test` body.
        assert_eq!(
            sx("def f() -> Int:\n    return expect(1)\n"),
            "(def f (typarams) (params) (returns Int) (uses) (do (return (expect 1))))"
        );
    }

    #[test]
    fn a_property_carries_its_generated_parameters() {
        assert_eq!(
            sx("property \"p\"(events: list[Event]):\n    given events\n"),
            "(property \"p\" (params (: events (list Event))) (do (given events)))"
        );
    }
}

/// The front end's recursion bound, from the outside: what a program past the ceiling gets, and
/// whether the stack the ceiling is declared to need actually covers it.
///
/// `docs/42` §42.2 is what these are about — an ~7.6 KB file that aborted `beck check` in a debug
/// build, on the 64 MiB stack `adr/0007` declared for a *different* recursive consumer of it.
#[cfg(test)]
mod nesting_tests {
    use super::*;
    use beck_diag::depth::{MAX_NESTING, STACK_BYTES};

    /// `(((…1…)))`, nested `n` deep, as a whole module.
    fn nested_parens(n: usize) -> String {
        format!(
            "def f() -> Int:\n    return {}1{}\n",
            "(".repeat(n),
            ")".repeat(n)
        )
    }

    /// On the stack the front end declares it needs — which is the whole contract: the ceiling is
    /// only a bound if somebody guarantees it is reachable.
    fn diagnose(src: &str) -> Vec<String> {
        beck_diag::depth::on_the_front_end_stack(|| {
            let mut map = beck_diag::SourceMap::new();
            let f = map.add("deep.beck", src);
            let mut d = Diagnostics::new();
            let _ = parse_module(f, "deep", src, &mut d);
            d.iter().map(|x| x.code.to_string()).collect()
        })
    }

    #[test]
    fn one_level_past_the_ceiling_is_a_diagnostic_rather_than_an_abort() {
        // Two levels of the ceiling are spent on the module and the `def`'s block before the
        // expression starts, so "one past" is stated as a wide margin rather than as arithmetic
        // about the parser's own frames.
        let codes = diagnose(&nested_parens(MAX_NESTING as usize + 8));
        assert!(
            codes.contains(&"B0121".to_string()),
            "a program past the ceiling should be refused with B0121, got {codes:?}"
        );
    }

    #[test]
    fn the_refusal_is_one_diagnostic_and_not_one_per_level() {
        let codes = diagnose(&nested_parens(MAX_NESTING as usize + 8));
        assert_eq!(
            codes.iter().filter(|c| *c == "B0121").count(),
            1,
            "got {codes:?}"
        );
    }

    #[test]
    fn nesting_a_person_would_write_is_still_read() {
        let mut map = beck_diag::SourceMap::new();
        let src = nested_parens(64);
        let f = map.add("ok.beck", &src);
        let mut d = Diagnostics::new();
        let _ = parse_module(f, "ok", &src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
    }

    /// The `beck-eval` pair, for the reader: measure what one level costs and hold the declaration
    /// to it, rather than trusting a number somebody wrote down once.
    #[test]
    fn the_ceiling_fits_the_declared_stack() {
        const PROBE_DEPTH: usize = 200;
        // Measured on a stack far larger than the one whose adequacy is being concluded, so the
        // measurement is never the thing that overflows.
        let spent = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(|| {
                let src = nested_parens(PROBE_DEPTH);
                beck_diag::depth::probe::stack_spent(|| {
                    let mut map = beck_diag::SourceMap::new();
                    let f = map.add("probe.beck", &src);
                    let mut d = Diagnostics::new();
                    parse_module(f, "probe", &src, &mut d)
                })
            })
            .expect("a thread")
            .join()
            .expect("the probe parses");

        let per_level = spent / PROBE_DEPTH;
        println!("parser: {spent} bytes for {PROBE_DEPTH} levels ({per_level} per level)");
        // Twice over, as the evaluator's does: whoever drives the parser has as much stack again
        // above the ceiling as the ceiling itself needs.
        let needed = MAX_NESTING as usize * per_level * 2;
        assert!(
            needed < STACK_BYTES,
            "a ceiling of {MAX_NESTING} levels at {per_level} bytes each needs {needed} bytes \
             with the margin, against a declared STACK_BYTES of {STACK_BYTES} — raise the \
             declaration or lower the ceiling"
        );
    }
}
