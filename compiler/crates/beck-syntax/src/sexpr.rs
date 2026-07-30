//! The S-expression reader — the canonical surface.
//!
//! [`docs/02-syntax.md`](../../../../docs/02-syntax.md) §2.8: "The S-expression reader is ~300
//! lines and should exist from week one: it lets you write compiler tests against canonical ASTs
//! without depending on the Python surface being finished, and it is how you'll dump intermediate
//! state for the rest of the project's life."
//!
//! It reads the notation the original sketch is written in. `(def toggle (params (: t Todo)) ...)`
//! is a `Node` with head `def` and three arguments, and nothing is desugared on the way in — the
//! Python parser's job is to arrive at exactly the same tree.

use beck_diag::{Diagnostic, Diagnostics, FileId, Span};

use crate::node::{Head, Lit, Node, Symbol};

struct Reader<'a> {
    file: FileId,
    src: &'a [u8],
    text: &'a str,
    pos: usize,
}

/// Read every form in a source string.
pub fn read_all(file: FileId, src: &str, diags: &mut Diagnostics) -> Vec<Node> {
    let mut r = Reader {
        file,
        src: src.as_bytes(),
        text: src,
        pos: 0,
    };
    let mut out = Vec::new();
    loop {
        r.skip_trivia();
        if r.pos >= r.src.len() {
            break;
        }
        match r.form(diags) {
            Some(n) => out.push(n),
            None => break,
        }
    }
    out
}

/// Read exactly one form; anything after it is an error. Used by tests and by `beck ast`.
pub fn read_one(file: FileId, src: &str, diags: &mut Diagnostics) -> Option<Node> {
    let forms = read_all(file, src, diags);
    if forms.len() > 1 {
        let span = forms[1].span();
        diags.push(
            Diagnostic::error("B0110", "expected a single form", span)
                .with_primary_label("unexpected second form"),
        );
    }
    forms.into_iter().next()
}

impl<'a> Reader<'a> {
    fn span(&self, start: usize) -> Span {
        Span::new(self.file, start..self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_trivia(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' | b',' => self.pos += 1,
                b';' => {
                    while let Some(c) = self.peek() {
                        self.pos += 1;
                        if c == b'\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn form(&mut self, diags: &mut Diagnostics) -> Option<Node> {
        self.skip_trivia();
        let start = self.pos;
        let c = self.peek()?;
        match c {
            b'(' | b'[' | b'{' => self.list(c, diags),
            b')' | b']' | b'}' => {
                self.pos += 1;
                diags.push(
                    Diagnostic::error("B0111", "unbalanced closing delimiter", self.span(start))
                        .with_primary_label("no matching opening delimiter"),
                );
                None
            }
            b'"' => self.string(diags),
            b'\'' => {
                // `'form` is `(quote form)` — the reader's one piece of sugar, because dumping
                // quoted templates without it is unreadable.
                self.pos += 1;
                let inner = self.form(diags)?;
                Some(Node::form(
                    crate::node::sym::QUOTE,
                    vec![inner],
                    self.span(start),
                ))
            }
            _ => self.atom(diags),
        }
    }

    fn list(&mut self, open: u8, diags: &mut Diagnostics) -> Option<Node> {
        let start = self.pos;
        let close = match open {
            b'(' => b')',
            b'[' => b']',
            _ => b'}',
        };
        self.pos += 1;
        let mut items: Vec<Node> = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => {
                    diags.push(
                        Diagnostic::error("B0112", "unclosed list", self.span(start))
                            .with_primary_label("opened here, never closed"),
                    );
                    return None;
                }
                Some(c) if c == close => {
                    self.pos += 1;
                    break;
                }
                Some(c) if c == b')' || c == b']' || c == b'}' => {
                    diags.push(
                        Diagnostic::error(
                            "B0113",
                            "mismatched closing delimiter",
                            Span::new(self.file, self.pos..self.pos + 1),
                        )
                        .with_label(self.span(start), "opened here"),
                    );
                    self.pos += 1;
                    return None;
                }
                _ => items.push(self.form(diags)?),
            }
        }
        let span = self.span(start);

        // `[a b c]` is a list literal, `{...}` a record literal; only `(...)` is application.
        match open {
            b'[' => return Some(Node::form(crate::node::sym::LIST, items, span)),
            b'{' => return Some(Node::form(crate::node::sym::RECORD, items, span)),
            _ => {}
        }

        if items.is_empty() {
            diags.push(
                Diagnostic::error("B0114", "empty application", span)
                    .with_primary_label("`()` has no meaning")
                    .with_fix("write `unit` for the unit value"),
            );
            return None;
        }

        // The head of an application is its first element, hoisted out of `args` — that is what
        // makes `Node.head : Sym | Lit`. A computed callee cannot be hoisted, so it stays as an
        // argument of the reserved `call` head.
        let head = items.remove(0);
        match head.head {
            Head::Sym(s) if head.args.is_empty() => Some(Node::form_sym(s, items, span)),
            _ => {
                let mut args = vec![head];
                args.extend(items);
                Some(Node::form(crate::node::sym::CALL, args, span))
            }
        }
    }

    fn string(&mut self, diags: &mut Diagnostics) -> Option<Node> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => {
                    diags.push(
                        Diagnostic::error("B0115", "unclosed string", self.span(start))
                            .with_primary_label("opened here"),
                    );
                    return None;
                }
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let c = self.peek()?;
                    self.pos += 1;
                    let decoded = match c {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'0' => '\0',
                        b'\\' => '\\',
                        b'"' => '"',
                        other => {
                            // Unknown escapes survive verbatim, as in the Python surface.
                            out.push('\\');
                            other as char
                        }
                    };
                    out.push(decoded);
                }
                Some(_) => {
                    let ch = self.text[self.pos..].chars().next()?;
                    self.pos += ch.len_utf8();
                    out.push(ch);
                }
            }
        }
        Some(Node::lit(Lit::Str(out.into()), self.span(start)))
    }

    fn atom(&mut self, diags: &mut Diagnostics) -> Option<Node> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace()
                || matches!(
                    c,
                    b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b';' | b','
                )
            {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            self.pos += 1;
            diags.push(Diagnostic::error(
                "B0116",
                "unreadable character",
                self.span(start),
            ));
            return None;
        }
        let text = &self.text[start..self.pos];
        let span = self.span(start);
        Some(atom_node(text, span))
    }
}

/// Classify an atom. Shared with the Python surface so that `true` means the same thing in both.
pub fn atom_node(text: &str, span: Span) -> Node {
    if let Some(kw) = text.strip_prefix(':') {
        if !kw.is_empty() {
            return Node::lit(Lit::Keyword(kw.into()), span);
        }
    }
    match text {
        "true" | "True" => return Node::lit(Lit::Bool(true), span),
        "false" | "False" => return Node::lit(Lit::Bool(false), span),
        _ => {}
    }
    if let Ok(n) = text.parse::<i64>() {
        return Node::lit(Lit::Int(n), span);
    }
    if text.contains('.') && !text.starts_with('.') {
        if let Ok(n) = text.parse::<f64>() {
            return Node::lit(Lit::Float(n), span);
        }
    }
    Node::symbol(Symbol::new(text), span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::sym;

    fn read(src: &str) -> Node {
        let mut map = beck_diag::SourceMap::new();
        let f = map.add("t.sx", src);
        let mut d = Diagnostics::new();
        let n = read_one(f, src, &mut d).expect("reads");
        assert!(!d.has_errors(), "{}", d.render(&map));
        n
    }

    #[test]
    fn the_sketchs_notation_reads_as_written() {
        let n = read("(def apply-event (fn [todos e] (assoc todos id 1)))");
        assert_eq!(n.head_name(), Some("def"));
        assert_eq!(n.args.len(), 2);
        assert_eq!(n.args[0].as_var().unwrap().as_str(), "apply-event");
        let f = &n.args[1];
        assert_eq!(f.head_name(), Some("fn"));
        assert_eq!(f.args[0].head_name(), Some(sym::LIST));
        assert_eq!(f.args[1].head_name(), Some("assoc"));
    }

    #[test]
    fn keywords_and_record_literals() {
        let n = read("{:id id :text text}");
        assert_eq!(n.head_name(), Some(sym::RECORD));
        assert_eq!(n.args[0].as_keyword(), Some("id"));
        assert_eq!(n.args[1].as_var().unwrap().as_str(), "id");
    }

    #[test]
    fn a_computed_callee_becomes_the_call_head() {
        let n = read("((. f g) x)");
        assert_eq!(n.head_name(), Some(sym::CALL));
        assert_eq!(n.args.len(), 2);
        assert_eq!(n.args[0].head_name(), Some("."));
    }

    #[test]
    fn literals_are_classified_not_stringly_typed() {
        assert_eq!(read("42").as_lit(), Some(&Lit::Int(42)));
        assert_eq!(read("4.5").as_lit(), Some(&Lit::Float(4.5)));
        assert_eq!(read("true").as_lit(), Some(&Lit::Bool(true)));
        assert_eq!(read(r#""hi""#).as_str_lit(), Some("hi"));
        assert_eq!(read(":done").as_keyword(), Some("done"));
    }

    #[test]
    fn quote_sugar_and_comments() {
        let n = read("; a comment\n'(a b)");
        assert_eq!(n.head_name(), Some(sym::QUOTE));
        assert_eq!(n.args[0].head_name(), Some("a"));
    }

    #[test]
    fn unbalanced_input_reports_rather_than_panics() {
        let mut map = beck_diag::SourceMap::new();
        let src = "(def a";
        let f = map.add("t.sx", src);
        let mut d = Diagnostics::new();
        assert!(read_one(f, src, &mut d).is_none());
        assert!(d.has_errors());
    }
}
