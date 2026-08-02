//! The printer — one `Node` tree, two surfaces.
//!
//! [`docs/02-syntax.md`](../../../../docs/02-syntax.md) §2.2: "`beck fmt --sexpr orders.beck`
//! emits the canonical Lisp form. `beck fmt --py orders.sx` emits the Python form." §2.8 asks for
//! it early because "every later phase uses it": macro expansion dumps, `beck ast`, and the error
//! renderer all print `Node`s.
//!
//! Round-tripping is lossless *modulo formatting*: `parse(print(parse(src)))` is structurally equal
//! to `parse(src)`, which is the property `tests/roundtrip.rs` asserts over the corpus.

use std::fmt::Write as _;

use crate::node::{sym, Head, Lit, Node};

/// Canonical S-expressions, one line. The notation the sketch is written in.
pub fn to_sexpr(n: &Node) -> String {
    let mut out = String::new();
    write_sexpr(&mut out, n);
    out
}

/// Canonical S-expressions, broken across lines where a form is long.
pub fn to_sexpr_pretty(n: &Node) -> String {
    let mut out = String::new();
    write_sexpr_pretty(&mut out, n, 0);
    out.push('\n');
    out
}

fn write_atom(out: &mut String, n: &Node) {
    match &n.head {
        Head::Sym(s) => {
            let _ = write!(out, "{}", s.name);
            // Hygiene scopes are printed only when present, so ordinary source round-trips
            // unchanged and expansion dumps stay legible.
            if !s.scopes.is_empty() {
                let _ = write!(out, "{:?}", s.scopes);
            }
        }
        Head::Lit(l) => write_lit(out, l),
    }
}

fn write_lit(out: &mut String, l: &Lit) {
    match l {
        Lit::Int(v) => {
            let _ = write!(out, "{v}");
        }
        Lit::Float(v) => {
            if v.fract() == 0.0 && v.is_finite() {
                let _ = write!(out, "{v:.1}");
            } else {
                let _ = write!(out, "{v}");
            }
        }
        Lit::Bool(v) => {
            let _ = write!(out, "{v}");
        }
        Lit::Keyword(k) => {
            let _ = write!(out, ":{k}");
        }
        Lit::Str(s) => write_string(out, s),
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

fn write_sexpr(out: &mut String, n: &Node) {
    if !n.applied {
        write_atom(out, n);
        return;
    }
    out.push('(');
    write_atom(out, n);
    for a in &n.args {
        out.push(' ');
        write_sexpr(out, a);
    }
    out.push(')');
}

fn write_sexpr_pretty(out: &mut String, n: &Node, indent: usize) {
    let flat = to_sexpr(n);
    // A documented argument forces the broken form: a comment needs a line of its own, so a form
    // short enough to print flat still has to be broken to keep its documentation.
    let documented = n.args.iter().any(|a| a.meta.doc.is_some());
    if !n.applied || (flat.len() + indent <= 96 && !documented) {
        out.push_str(&flat);
        return;
    }
    out.push('(');
    write_atom(out, n);
    let inner = indent + 2;
    let pad = " ".repeat(inner);
    for a in &n.args {
        if let Some(doc) = &a.meta.doc {
            for line in crate::doc::render(doc, crate::doc::SEXPR_MARKER, &pad).lines() {
                out.push('\n');
                out.push_str(line);
            }
        }
        out.push('\n');
        out.push_str(&pad);
        write_sexpr_pretty(out, a, inner);
    }
    out.push(')');
}

/// The Python surface. This is what `beck fmt` writes, and §2.6's style rules ("`snake_case`
/// values, `PascalCase` types … enforced by `beck fmt`") are applied by the *formatter*, not here:
/// this function prints faithfully, so that printing an already-renamed tree is idempotent.
pub fn to_python(n: &Node) -> String {
    let mut p = Py {
        out: String::new(),
        indent: 0,
    };
    if n.is_form(sym::MODULE) {
        for (i, item) in n.args.iter().skip(1).enumerate() {
            if i > 0 {
                p.out.push('\n');
            }
            p.item(item);
        }
    } else {
        p.item(n);
    }
    p.out
}

struct Py {
    out: String,
    indent: usize,
}

impl Py {
    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// Emit the node's doc comment, if it has one, at the current indentation.
    ///
    /// Ordinary comments are dropped by the lexer and so cannot survive `beck fmt`; a doc comment
    /// is [`crate::Meta`], so it can, and formatting a documented module has to give it back.
    fn docs(&mut self, n: &Node) {
        let Some(doc) = n.meta.doc.clone() else {
            return;
        };
        let indent = "    ".repeat(self.indent);
        self.out
            .push_str(&crate::doc::render(&doc, crate::doc::PY_MARKER, &indent));
    }

    fn item(&mut self, n: &Node) {
        self.docs(n);
        match n.head_name() {
            Some(sym::DECORATE) => {
                let deco = self.expr(&n.args[0]);
                self.line(&format!("@{deco}"));
                self.item(&n.args[1]);
            }
            Some(sym::DEF) => self.def(n),
            Some(sym::MACRO) => {
                let name = self.expr(&n.args[0]);
                let params = self.params(&n.args[1]);
                self.line(&format!("macro {name}({params}):"));
                self.body(&n.args[2]);
            }
            Some(sym::MODEL) => {
                let name = self.expr(&n.args[0]);
                let typarams = self.typarams(n);
                self.line(&format!("model {name}{typarams}:"));
                self.indent += 1;
                if n.args.len() == 2 {
                    self.line("pass");
                }
                for f in &n.args[2..] {
                    self.docs(f);
                    let fname = self.expr(&f.args[0]);
                    let fty = self.type_expr(&f.args[1]);
                    self.line(&format!("{fname}: {fty}"));
                }
                self.indent -= 1;
            }
            Some(sym::UNION) => {
                let name = self.expr(&n.args[0]);
                let typarams = self.typarams(n);
                self.line(&format!("union {name}{typarams}:"));
                self.indent += 1;
                for v in &n.args[2..] {
                    self.docs(v);
                    let vname = self.expr(&v.args[0]);
                    if v.args.len() == 1 {
                        self.line(&vname);
                    } else {
                        let fields: Vec<String> = v.args[1..]
                            .iter()
                            .map(|f| {
                                format!("{}: {}", self.expr(&f.args[0]), self.type_expr(&f.args[1]))
                            })
                            .collect();
                        self.line(&format!("{vname}({})", fields.join(", ")));
                    }
                }
                self.indent -= 1;
            }
            Some(sym::TRAIT) => {
                let name = self.expr(&n.args[0]);
                self.line(&format!("trait {name}:"));
                self.indent += 1;
                for m in &n.args[1..] {
                    self.item(m);
                }
                self.indent -= 1;
            }
            Some(sym::IMPL) => {
                let name = self.expr(&n.args[0]);
                let typarams = self.typarams(n);
                let ty = self.type_expr(&n.args[2]);
                self.line(&format!("impl{typarams} {name} for {ty}:"));
                self.indent += 1;
                for m in &n.args[3..] {
                    self.item(m);
                }
                self.indent -= 1;
            }
            Some(sym::TYPE) => {
                let name = self.expr(&n.args[0]);
                let typarams = self.typarams(n);
                let ty = self.type_expr(&n.args[2]);
                self.line(&format!("type {name}{typarams} = {ty}"));
            }
            Some(sym::NEWTYPE) => {
                let name = self.expr(&n.args[0]);
                let typarams = self.typarams(n);
                let ty = self.type_expr(&n.args[2]);
                self.line(&format!("type {name}{typarams} = newtype[{ty}]"));
            }
            Some(sym::IMPORT) => {
                let path = self.expr(&n.args[0]);
                self.line(&format!("import {path}"));
            }
            Some(sym::TEST) => {
                let name = self.expr(&n.args[0]);
                self.line(&format!("test {name}:"));
                self.body(&n.args[1]);
            }
            Some(sym::PROPERTY) if n.args.len() == 3 => {
                let name = self.expr(&n.args[0]);
                let params = self.params(&n.args[1]);
                self.line(&format!("property {name}({params}):"));
                self.body(&n.args[2]);
            }
            _ => self.stmt(n),
        }
    }

    /// `[T, U]` or `[T: Show + Eq]` from the list at `args[1]`, or the empty string when there is
    /// nothing to quantify.
    fn typarams(&mut self, n: &Node) -> String {
        let Some(t) = n.args.get(1).filter(|t| !t.args.is_empty()) else {
            return String::new();
        };
        let names: Vec<String> = t
            .args
            .clone()
            .iter()
            .map(|a| {
                if !a.is_form(sym::ANNOT) || a.args.len() < 2 {
                    return self.expr(a);
                }
                let bounds: Vec<String> = a.args[1..].iter().map(|b| self.expr(b)).collect();
                format!("{}: {}", self.expr(&a.args[0]), bounds.join(" + "))
            })
            .collect();
        format!("[{}]", names.join(", "))
    }

    fn def(&mut self, n: &Node) {
        let name = self.expr(&n.args[0]);
        let typarams = self.typarams(n);
        let params = self.params(&n.args[2]);
        let ret = n
            .args
            .get(3)
            .filter(|r| !r.args.is_empty())
            .map(|r| format!(" -> {}", self.type_expr(&r.args[0])))
            .unwrap_or_default();
        let uses = n
            .args
            .get(4)
            .filter(|u| !u.args.is_empty())
            .map(|u| {
                let items: Vec<String> = u.args.iter().map(|e| self.expr(e)).collect();
                format!(" uses {}", items.join(", "))
            })
            .unwrap_or_default();
        match n.args.get(5) {
            Some(body) => {
                self.line(&format!("def {name}{typarams}({params}){ret}{uses}:"));
                self.body(body);
            }
            // A declaration: a trait's method signature, or a line of a `.becki` interface (§3.6).
            // It prints without a colon, which is what it parses back from.
            None => self.line(&format!("def {name}{typarams}({params}){ret}{uses}")),
        }
    }

    fn params(&self, n: &Node) -> String {
        n.args
            .iter()
            .map(|p| {
                if p.is_form(sym::ANNOT) {
                    format!("{}: {}", self.expr(&p.args[0]), self.type_expr(&p.args[1]))
                } else {
                    self.expr(p)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn body(&mut self, n: &Node) {
        self.indent += 1;
        if n.args.is_empty() {
            self.line("pass");
        }
        for s in &n.args {
            self.stmt(s);
        }
        self.indent -= 1;
    }

    fn stmt(&mut self, n: &Node) {
        match n.head_name() {
            Some(sym::DO) => {
                for s in &n.args {
                    self.stmt(s);
                }
            }
            Some(sym::RETURN) => match n.args.first() {
                // `return ui:` + block. The block rule applies in final position (§2.7 only
                // forbids a block-form call as a *non-final argument*), so the printer has to
                // reproduce it — printing `return ui(do=quote(...))` would not re-parse, because
                // `quote` is a block form and `;`-joined statements are not surface syntax.
                // `return quote:` + template — the shape every macro body has (§2.4).
                Some(e) if e.is_form(sym::QUOTE) && e.args.len() == 1 => {
                    self.line("return quote:");
                    let body = &e.args[0];
                    if body.is_form(sym::DO) {
                        self.body(body);
                    } else {
                        self.indent += 1;
                        self.stmt(body);
                        self.indent -= 1;
                    }
                }
                Some(e) => match split_block_call(e) {
                    Some((head, args, block)) => {
                        let rendered = self.call_text(head, &args);
                        self.line(&format!("return {rendered}:"));
                        self.body(&block);
                    }
                    None => {
                        let e = self.expr(e);
                        self.line(&format!("return {e}"));
                    }
                },
                None => self.line("return"),
            },
            Some(sym::LET) | Some(sym::VAR)
                if n.args.len() == 2 && split_block_call(&n.args[1]).is_some() =>
            {
                let (head, args, block) =
                    split_block_call(&n.args[1]).expect("checked by the guard");
                let target = self.expr(&n.args[0]);
                let keyword = if n.is_form(sym::VAR) { "var " } else { "" };
                let rendered = self.call_text(head, &args);
                self.line(&format!("{keyword}{target} = {rendered}:"));
                self.body(&block);
            }
            Some(sym::LET) if n.args.len() == 2 => {
                let t = &n.args[0];
                let target = if t.is_form(sym::ANNOT) {
                    format!("{}: {}", self.expr(&t.args[0]), self.type_expr(&t.args[1]))
                } else {
                    self.expr(t)
                };
                let v = self.expr(&n.args[1]);
                self.line(&format!("{target} = {v}"));
            }
            Some(sym::VAR) if n.args.len() == 2 => {
                let t = &n.args[0];
                let target = if t.is_form(sym::ANNOT) {
                    format!("{}: {}", self.expr(&t.args[0]), self.type_expr(&t.args[1]))
                } else {
                    self.expr(t)
                };
                let v = self.expr(&n.args[1]);
                self.line(&format!("var {target} = {v}"));
            }
            Some(sym::IF) if n.args.len() >= 2 && n.args[1].is_form(sym::DO) => {
                let c = self.expr(&n.args[0]);
                self.line(&format!("if {c}:"));
                self.body(&n.args[1]);
                if let Some(alt) = n.args.get(2) {
                    // `elif` is an `else` whose only statement is another `if`.
                    if alt.args.len() == 1 && alt.args[0].is_form(sym::IF) {
                        let inner = &alt.args[0];
                        let mut s = String::new();
                        std::mem::swap(&mut self.out, &mut s);
                        self.stmt(inner);
                        std::mem::swap(&mut self.out, &mut s);
                        let pad = "    ".repeat(self.indent);
                        let rewritten = s.replacen(&format!("{pad}if "), &format!("{pad}elif "), 1);
                        self.out.push_str(&rewritten);
                    } else {
                        self.line("else:");
                        self.body(alt);
                    }
                }
            }
            Some(sym::FOR) if n.args.len() == 3 => {
                let v = self.expr(&n.args[0]);
                let seq = self.expr(&n.args[1]);
                self.line(&format!("for {v} in {seq}:"));
                self.body(&n.args[2]);
            }
            Some(sym::WHILE) if n.args.len() == 2 => {
                let c = self.expr(&n.args[0]);
                self.line(&format!("while {c}:"));
                self.body(&n.args[1]);
            }
            Some(sym::MATCH) if !n.args.is_empty() => {
                let s = self.expr(&n.args[0]);
                self.line(&format!("match {s}:"));
                self.indent += 1;
                for arm in &n.args[1..] {
                    let pat = self.expr(&arm.args[0]);
                    self.line(&format!("case {pat}:"));
                    self.body(&arm.args[1]);
                }
                self.indent -= 1;
            }
            Some(sym::DEF | sym::MACRO | sym::MODEL | sym::UNION | sym::TYPE | sym::NEWTYPE)
            | Some(sym::TRAIT | sym::IMPL | sym::IMPORT | sym::DECORATE | sym::TEST)
            | Some(sym::PROPERTY) => self.item(n),

            // ---- §21.2's clauses. Each prints back as the line it was written as, because
            // `parse(print(parse(src))) == parse(src)` is asserted over the corpus and a test block
            // is part of the corpus now.
            Some(sym::GIVEN) if !n.args.is_empty() => {
                let events = self.expr(&n.args[0]);
                match n.args.get(1) {
                    Some(actor) => {
                        let a = self.expr(actor);
                        self.line(&format!("given {events} by {a}"));
                    }
                    None => self.line(&format!("given {events}")),
                }
            }
            Some(sym::WHEN) if n.args.len() >= 2 => {
                let cmds: Vec<String> = n.args[1..].iter().map(|a| self.expr(a)).collect();
                let cmds = cmds.join(", ");
                match n.args[0].as_str_lit() {
                    Some(actor) => self.line(&format!("when session(\"{actor}\") sends {cmds}")),
                    None => self.line(&format!("when {cmds}")),
                }
            }
            Some(sym::EXPECT) if n.args.len() == 1 => {
                // `expect Ok(…)`/`expect Err(…)` parsed as `result == …`; printing the desugared
                // form is what makes the round-trip a fixed point rather than an oscillation.
                let e = self.expr(&n.args[0]);
                self.line(&format!("expect {e}"));
            }
            Some(sym::EXPECT_CONTAINS) if !n.args.is_empty() => {
                let needle = self.expr(&n.args[0]);
                match n.args.get(1).and_then(|a| a.as_str_lit()) {
                    Some(actor) => self.line(&format!(
                        "expect page(session(\"{actor}\")) contains {needle}"
                    )),
                    None => self.line(&format!("expect page contains {needle}")),
                }
            }
            Some(sym::EXPECT_FOLD) if !n.args.is_empty() => {
                let events = self.expr(&n.args[0]);
                match n.args.get(1).and_then(|a| a.as_str_lit()) {
                    Some(actor) => {
                        self.line(&format!("expect state == fold_of {events} by \"{actor}\""))
                    }
                    None => self.line(&format!("expect state == fold_of {events}")),
                }
            }
            Some(sym::EXPECT_PLACE) if n.args.len() == 2 => {
                let what = self.expr(&n.args[0]);
                let tier = self.expr(&n.args[1]);
                self.line(&format!("expect place({what}) == {tier}"));
            }
            Some(sym::EXPECT_FLOW) if n.args.len() == 2 => {
                let ty = self.expr(&n.args[0]);
                let tier = self.expr(&n.args[1]);
                self.line(&format!("expect flow({ty}) reaches nothing on {tier}"));
            }
            Some(sym::EXPECT_WIRE) if n.args.len() == 1 => {
                let path = self.expr(&n.args[0]);
                self.line(&format!("expect wire_compatible_with {path}"));
            }
            Some(sym::EXPECT_EFFECT) if n.args.len() == 2 => {
                let atom = n.args[0].as_str_lit().unwrap_or_default().to_string();
                let how = &n.args[1];
                match how.head_name() {
                    Some("times") if how.args.len() == 1 => {
                        match how.args[0].as_lit() {
                            Some(Lit::Int(1)) => self.line(&format!("expect {atom} once")),
                            Some(Lit::Int(k)) => self.line(&format!("expect {atom} times {k}")),
                            _ => self.line(&format!("expect {atom} once")),
                        };
                    }
                    Some("with") if how.args.len() == 1 => {
                        let v = self.expr(&how.args[0]);
                        self.line(&format!("expect {atom} with {v}"));
                    }
                    _ => self.line(&format!("expect no {atom}")),
                }
            }
            Some(sym::STUB) if n.args.len() == 2 => {
                let atom = n.args[0].as_str_lit().unwrap_or_default().to_string();
                let body = &n.args[1];
                if body.is_form(sym::STUB_ARMS) {
                    self.line(&format!("stub {atom}:"));
                    self.indent += 1;
                    for arm in &body.args {
                        let pat = self.expr(&arm.args[0]);
                        self.line(&format!("case {pat}:"));
                        self.body(&arm.args[1]);
                    }
                    self.indent -= 1;
                } else if body.is_form(sym::DO) {
                    self.line(&format!("stub {atom}:"));
                    self.body(body);
                } else {
                    let v = self.expr(body);
                    self.line(&format!("stub {atom}: {v}"));
                }
            }
            _ => {
                // A call with a `do=` block prints back in block form; anything else is an
                // expression statement.
                if let Some((head, args, block)) = split_block_call(n) {
                    let rendered = self.call_text(head, &args);
                    self.line(&format!("{rendered}:"));
                    self.body(&block);
                } else {
                    let e = self.expr(n);
                    self.line(&e);
                }
            }
        }
    }

    fn call_text(&self, head: &str, args: &[Node]) -> String {
        if args.is_empty() {
            return head.to_string();
        }
        let rendered: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
        format!("{head}({})", rendered.join(", "))
    }

    fn type_expr(&self, n: &Node) -> String {
        match n.head_name() {
            Some("fn-type") if n.args.len() >= 2 => {
                let params: Vec<String> = n.args[..n.args.len() - 1]
                    .iter()
                    .map(|a| self.type_expr(a))
                    .collect();
                format!(
                    "({}) -> {}",
                    params.join(", "),
                    self.type_expr(&n.args[n.args.len() - 1])
                )
            }
            _ if !n.applied => {
                let mut s = String::new();
                write_atom(&mut s, n);
                s
            }
            _ => {
                let mut head = String::new();
                write_atom(&mut head, n);
                let args: Vec<String> = n.args.iter().map(|a| self.type_expr(a)).collect();
                format!("{head}[{}]", args.join(", "))
            }
        }
    }

    fn expr(&self, n: &Node) -> String {
        if !n.applied {
            let mut s = String::new();
            write_atom(&mut s, n);
            return s;
        }
        let head = n.head_name().unwrap_or("");
        match head {
            "not" if n.args.len() == 1 => format!("not {}", self.expr(&n.args[0])),
            "negate" if n.args.len() == 1 => format!("-{}", self.expr(&n.args[0])),
            sym::UNQUOTE if n.args.len() == 1 => format!("${}", self.expr(&n.args[0])),
            sym::SPLICE if n.args.len() == 1 => format!("$*{}", self.expr(&n.args[0])),
            "and" | "or" | "==" | "!=" | "<" | "<=" | ">" | ">=" | "+" | "-" | "*" | "/" | "%"
                if n.args.len() == 2 =>
            {
                format!(
                    "({} {head} {})",
                    self.expr(&n.args[0]),
                    self.expr(&n.args[1])
                )
            }
            "contains" if n.args.len() == 2 => {
                format!("({} in {})", self.expr(&n.args[0]), self.expr(&n.args[1]))
            }
            "index" if n.args.len() == 2 => {
                format!("{}[{}]", self.expr(&n.args[0]), self.expr(&n.args[1]))
            }
            sym::IF if n.args.len() == 3 && !n.args[1].is_form(sym::DO) => format!(
                "{} if {} else {}",
                self.expr(&n.args[1]),
                self.expr(&n.args[0]),
                self.expr(&n.args[2])
            ),
            sym::DOT if n.args.len() == 2 => {
                format!("{}.{}", self.expr(&n.args[0]), self.expr(&n.args[1]))
            }
            sym::DOT if n.args.len() > 2 => {
                let args: Vec<String> = n.args[2..].iter().map(|a| self.expr(a)).collect();
                format!(
                    "{}.{}({})",
                    self.expr(&n.args[0]),
                    self.expr(&n.args[1]),
                    args.join(", ")
                )
            }
            sym::KW_ARG if n.args.len() == 2 => {
                format!("{}={}", self.expr(&n.args[0]), self.expr(&n.args[1]))
            }
            sym::LIST => {
                let items: Vec<String> = n.args.iter().map(|a| self.expr(a)).collect();
                format!("[{}]", items.join(", "))
            }
            sym::REST if n.args.len() == 1 => format!("*{}", self.expr(&n.args[0])),
            sym::RECORD => {
                let mut parts = Vec::new();
                for pair in n.args.chunks(2) {
                    if pair.len() == 2 {
                        let k = pair[0]
                            .as_keyword()
                            .map(str::to_string)
                            .unwrap_or_else(|| self.expr(&pair[0]));
                        parts.push(format!("{k}: {}", self.expr(&pair[1])));
                    }
                }
                format!("{{{}}}", parts.join(", "))
            }
            sym::MAP => {
                let mut parts = Vec::new();
                for pair in n.args.chunks(2) {
                    if pair.len() == 2 {
                        parts.push(format!("{}: {}", self.expr(&pair[0]), self.expr(&pair[1])));
                    }
                }
                format!("{{{}}}", parts.join(", "))
            }
            sym::FN if n.args.len() == 2 => {
                let params = self.params(&n.args[0]);
                let body = &n.args[1];
                let b = if body.args.len() == 1 {
                    self.expr(&body.args[0])
                } else {
                    self.expr(body)
                };
                format!("lambda {params}: {b}")
            }
            sym::QUOTE if n.args.len() == 1 => {
                // A quoted block prints as `quote:` + body; the statement printer handles the
                // block-call case before reaching here.
                format!("quote({})", self.expr(&n.args[0]))
            }
            sym::CALL if !n.args.is_empty() => {
                let callee = self.expr(&n.args[0]);
                let args: Vec<String> = n.args[1..].iter().map(|a| self.expr(a)).collect();
                format!("{callee}({})", args.join(", "))
            }
            sym::DO => {
                let items: Vec<String> = n.args.iter().map(|a| self.expr(a)).collect();
                items.join("; ")
            }
            _ => {
                let mut h = String::new();
                write_atom(&mut h, n);
                let args: Vec<String> = n.args.iter().map(|a| self.expr(a)).collect();
                format!("{h}({})", args.join(", "))
            }
        }
    }
}

/// Recognise `f(args, do=quote(block))` so it can print back as `f(args):` + block.
fn split_block_call(n: &Node) -> Option<(&str, Vec<Node>, Node)> {
    let head = n.head_name()?;
    let last = n.args.last()?;
    if !last.is_form(sym::KW_ARG) || last.args.len() != 2 {
        return None;
    }
    if last.args[0].as_var().map(|s| s.as_str()) != Some("do") {
        return None;
    }
    let quoted = &last.args[1];
    if !quoted.is_form(sym::QUOTE) || quoted.args.len() != 1 {
        return None;
    }
    let block = quoted.args[0].clone();
    if !block.has_head(sym::DO) {
        return None;
    }
    Some((head, n.args[..n.args.len() - 1].to_vec(), block))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser, sexpr};
    use beck_diag::{Diagnostics, SourceMap};

    fn roundtrip_python(src: &str) -> String {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let n = parser::parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
        to_python(&n)
    }

    fn parse(src: &str) -> Node {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let n = parser::parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
        n
    }

    #[test]
    fn printing_python_is_idempotent_and_reparses_to_the_same_tree() {
        let src = "\
def total(items: list[Int], base: Int) -> Int:
    var acc = base
    for i in items:
        acc = (acc + i)
    if (acc > 10):
        return acc
    elif (acc > 5):
        return 5
    else:
        return 0
";
        let once = roundtrip_python(src);
        let twice = roundtrip_python(&once);
        assert_eq!(once, twice, "fmt must be idempotent");
        assert!(parse(src).structurally_eq(&parse(&once)));
    }

    #[test]
    fn block_calls_print_back_as_blocks() {
        let src = "ui:\n    main:\n        h1(class=\"t\"):\n            \"todos\"\n";
        let out = roundtrip_python(src);
        assert!(out.contains("ui:"), "{out}");
        assert!(out.contains("h1(class=\"t\"):"), "{out}");
        assert!(parse(src).structurally_eq(&parse(&out)));
    }

    #[test]
    fn the_two_surfaces_are_the_same_language() {
        // §2.2's claim, mechanised: both readers produce identical `Node` trees.
        let py = "def toggle(todos: Map[Id, Todo], e: Toggled) -> Map[Id, Todo]:\n\
                  \x20   return todos.update(e.id, lambda t: t.with(done=not t.done))\n";
        let mut map = SourceMap::new();
        let f = map.add("t.beck", py);
        let mut d = Diagnostics::new();
        let from_py = parser::parse_module(f, "t", py, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));

        let sx_src = "(def toggle
                        (params (: todos (Map Id Todo)) (: e Toggled))
                        (returns (Map Id Todo))
                        (uses)
                        (do (return (. todos update (. e id)
                             (fn (params t) (do (. t with (kw done (not (. t done)))))))))) ";
        let g = map.add("t.sx", sx_src);
        let from_sx = sexpr::read_one(g, sx_src, &mut d).unwrap();
        assert!(!d.has_errors(), "{}", d.render(&map));

        assert!(
            from_py.args[1].structurally_eq(&from_sx),
            "python:\n{}\nsexpr:\n{}",
            to_sexpr(&from_py.args[1]),
            to_sexpr(&from_sx)
        );
    }

    #[test]
    fn sexpr_pretty_breaks_only_long_forms() {
        let n = parse("def f() -> Int:\n    return 1\n");
        let pretty = to_sexpr_pretty(&n.args[1]);
        assert_eq!(pretty.lines().count(), 1, "{pretty}");
    }
}

/// The round-trip property, over the corpus.
///
/// §4.8: "Round-trip property: `parse(print(parse(src))) == parse(src)`". It is a property of the
/// *printer* rather than of any one construct, so it is asserted over whole files — the example
/// program is the corpus Phase 1 has, and it exercises every surface form the language ships.
#[cfg(test)]
mod roundtrip {
    use super::*;
    use crate::parser;
    use beck_diag::{Diagnostics, SourceMap};

    fn parse(name: &str, src: &str) -> Node {
        let mut map = SourceMap::new();
        let f = map.add(name, src);
        let mut d = Diagnostics::new();
        let n = parser::parse_module(f, "t", src, &mut d);
        assert!(!d.has_errors(), "{name}:\n{}", d.render(&map));
        n
    }

    fn corpus() -> Vec<(&'static str, &'static str)> {
        vec![
            ("example", include_str!("../../../examples/todo.beck")),
            (
                "macros",
                "macro unless(cond, do):\n    return quote:\n        if not $cond:\n            $do\n",
            ),
            (
                "control",
                "def f(xs: list[Int]) -> Int:\n    var acc = 0\n    if (acc > 1):\n        return 1\n    elif (acc > 0):\n        return 2\n    else:\n        return 3\n",
            ),
            (
                "types",
                "type Id = newtype[Str]\n\nmodel M:\n    a: Int\n\nunion U:\n    A(x: Int)\n    B\n",
            ),
            // §21.2's clauses are part of the surface now, so they are part of the property that
            // says the surface is a fixed point of printing.
            (
                "tests",
                "test \"a\":\n    given [Added(id=\"1\")] by \"ana\"\n    when session(\"ana\") sends Add(id=\"1\"), Toggle(id=\"1\")\n    stub net.out(payments.example.com): Declined\n    stub net.out(a.example.com):\n        case Charge(amount):\n            return Declined\n        case _:\n            return Approved\n    stub net.out(b.example.com):\n        x = 1\n        return Approved\n    expect page contains \"milk\"\n    expect page(session(\"bo\")) contains \"milk\"\n    expect state == fold_of []\n    expect state == fold_of [Added(id=\"1\")] by \"ana\"\n    expect place(view) == client\n    expect flow(ApiKey) reaches nothing on client\n    expect wire_compatible_with \"o.becki\"\n    expect no net.out\n    expect net.out(h.example.com) once\n    expect net.out(h.example.com) times 3\n    expect net.out(h.example.com) with Charge(amount=1)\n    expect Err(error=BlankText)\n\nproperty \"p\"(events: list[Event]):\n    given events\n    expect list_len(events) >= 0\n",
            ),
        ]
    }

    #[test]
    fn printing_the_python_surface_round_trips() {
        for (name, src) in corpus() {
            let first = parse(name, src);
            let printed = to_python(&first);
            let leaked: &'static str = Box::leak(printed.clone().into_boxed_str());
            let second = parse(name, leaked);
            assert!(
                first.structurally_eq(&second),
                "{name} did not round-trip.\n--- printed ---\n{printed}\n--- as sexpr ---\n{}\n--- was ---\n{}",
                to_sexpr(&second),
                to_sexpr(&first)
            );
        }
    }

    #[test]
    fn formatting_is_idempotent() {
        for (name, src) in corpus() {
            let once = to_python(&parse(name, src));
            let leaked: &'static str = Box::leak(once.clone().into_boxed_str());
            let twice = to_python(&parse(name, leaked));
            assert_eq!(once, twice, "{name}: fmt is not idempotent");
        }
    }
}
