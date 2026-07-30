//! The Python surface: recursive descent for statements, Pratt for expressions.
//!
//! [`docs/02-syntax.md`](../../../../docs/02-syntax.md) §2.8: "hand-written recursive descent +
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
            let name_span = self.span();
            let name = match self.cur().raw() {
                Some(Raw::Str(s)) => {
                    let s = s.clone();
                    self.bump();
                    s
                }
                _ => {
                    self.error("expected a test name in quotes");
                    return None;
                }
            };
            self.expect(&Raw::Colon, "`:`");
            let body = self.block()?;
            let span = start.to(body.span());
            return Some(Node::form(
                sym::TEST,
                vec![Node::lit(Lit::Str(name.into()), name_span), body],
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

        self.expect(&Raw::Colon, "`:` before the function body");
        let body = self.block()?;
        let span = start.to(body.span());
        Some(Node::form(
            sym::DEF,
            vec![Node::sym(name, name_span), params, returns, uses, body],
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
        self.expect(&Raw::Colon, "`:`");
        let mut fields = vec![Node::sym(name, name_span)];
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
        self.expect(&Raw::Colon, "`:`");
        let mut variants = vec![Node::sym(name, name_span)];
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

    fn impl_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // impl
        let (trait_name, tspan) = self.ident("a trait name")?;
        if !self.eat_kw("for") {
            self.error("expected `for` in an impl declaration");
            return None;
        }
        let ty = self.type_expr()?;
        self.expect(&Raw::Colon, "`:`");
        let body = self.block()?;
        let mut items = vec![Node::sym(trait_name, tspan), ty];
        items.extend(body.args);
        Some(Node::form(sym::IMPL, items, start))
    }

    fn type_item(&mut self) -> Option<Node> {
        let start = self.span();
        self.bump(); // type
        let (name, name_span) = self.ident("a type name")?;
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
                vec![Node::sym(name, name_span), inner],
                start,
            ));
        }
        let ty = self.type_expr()?;
        self.end_of_line();
        Some(Node::form(
            sym::TYPE,
            vec![Node::sym(name, name_span), ty],
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

    fn block(&mut self) -> Option<Node> {
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
        let span = start.to(self.span());
        Some(Node::form(sym::MATCH, arms, span))
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
                    items.push(self.expr()?);
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
            "(def toggle (params (: t Todo)) (returns Todo) (uses durable) (do (return t)))"
        );
    }

    #[test]
    fn the_doc_example_produces_the_documented_tree() {
        // §2.2's side-by-side, verbatim.
        assert_eq!(
            sx("def toggle(todos: Map[Id, Todo], e: Toggled) -> Map[Id, Todo]:\n\
                \x20   return todos.update(e.id, lambda t: t.with(done=not t.done))\n"),
            "(def toggle (params (: todos (Map Id Todo)) (: e Toggled)) (returns (Map Id Todo)) \
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
            "(decorate (on server) (def f (params) (returns Int) (uses) (do (return 1))))"
        );
    }

    #[test]
    fn models_unions_and_newtypes() {
        assert_eq!(
            sx("model Todo:\n    id: Id\n    done: Bool\n"),
            "(model Todo (field id Id) (field done Bool))"
        );
        assert_eq!(
            sx("union Event:\n    Added(id: Id, text: Str)\n    Toggled(id: Id)\n"),
            "(union Event (variant Added (field id Id) (field text Str)) (variant Toggled (field id Id)))"
        );
        assert_eq!(sx("type Id = newtype[Uuid]\n"), "(newtype Id Uuid)");
        assert_eq!(sx("type Ids = list[Id]\n"), "(type Ids (list Id))");
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
