//! Tokens, and the layout algorithm that turns indentation into `INDENT`/`DEDENT`.
//!
//! [`docs/02-syntax.md`](../../../../docs/02-syntax.md) §2.8: "`logos` for tokens; a hand-written
//! layout algorithm producing explicit `INDENT`/`DEDENT`/`NEWLINE` tokens (Python's approach), with
//! brackets suppressing layout so multi-line calls work."
//!
//! Two rules that are easy to get subtly wrong and are therefore stated here:
//!
//! * **Brackets suppress layout.** Inside `(`/`[`/`{` no newline is significant, so a call may span
//!   lines without the parser ever seeing it.
//! * **Blank and comment-only lines have no indentation.** They emit nothing at all, so a comment
//!   at column 0 inside an indented block does not close the block.

use beck_diag::{Diagnostic, Diagnostics, FileId, Span};
use logos::Logos;

#[derive(Clone, Debug, PartialEq, Logos)]
#[logos(skip r"[ \t]+")]
// A comment runs to the end of its line; the greedy sweep is the meaning, so logos 0.16's
// unbounded-repetition lint is answered rather than suppressed.
#[logos(skip(r"#[^\n]*", allow_greedy = true))]
pub enum Raw {
    #[regex(r"\n")]
    Newline,

    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    // `:name` — a keyword literal. Written before the `:` operator so it wins the longest match.
    #[regex(r":[A-Za-z_][A-Za-z0-9_\-]*", |lex| lex.slice()[1..].to_string())]
    Keyword(String),

    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*", |lex| lex.slice().replace('_', "").parse::<f64>().ok())]
    Float(f64),

    #[regex(r"[0-9][0-9_]*", |lex| lex.slice().replace('_', "").parse::<i64>().ok())]
    Int(i64),

    #[regex(r#""([^"\\\n]|\\.)*""#, |lex| unescape(lex.slice()))]
    Str(String),

    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,

    #[token("->")]
    Arrow,
    #[token("==")]
    EqEq,
    #[token("!=")]
    NotEq,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    // `$*` splices, `$` unquotes. Longest match puts `$*` first.
    #[token("$*")]
    DollarStar,
    #[token("$")]
    Dollar,
    #[token("=")]
    Eq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token(".")]
    Dot,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token("@")]
    At,
    #[token("|")]
    Pipe,
    #[token("?")]
    Question,
}

fn unescape(raw: &str) -> Option<String> {
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            // Unknown escapes are kept verbatim rather than silently dropped, because `\d`
            // inside a `regex"..."` literal is a real thing to want (§2.5).
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Some(out)
}

/// A token after layout: the raw tokens plus the three synthetic ones.
#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Raw(Raw),
    Newline,
    Indent,
    Dedent,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

impl Token {
    pub fn raw(&self) -> Option<&Raw> {
        match &self.tok {
            Tok::Raw(r) => Some(r),
            _ => None,
        }
    }

    pub fn is_ident(&self, name: &str) -> bool {
        matches!(self.raw(), Some(Raw::Ident(s)) if s == name)
    }

    pub fn describe(&self) -> String {
        match &self.tok {
            Tok::Newline => "end of line".into(),
            Tok::Indent => "an indented block".into(),
            Tok::Dedent => "the end of a block".into(),
            Tok::Eof => "end of file".into(),
            Tok::Raw(r) => match r {
                Raw::Ident(s) => format!("`{s}`"),
                Raw::Keyword(s) => format!("`:{s}`"),
                Raw::Int(n) => format!("`{n}`"),
                Raw::Float(n) => format!("`{n}`"),
                Raw::Str(_) => "a string".into(),
                Raw::Newline => "end of line".into(),
                other => format!("`{}`", punct(other)),
            },
        }
    }
}

fn punct(r: &Raw) -> &'static str {
    match r {
        Raw::LParen => "(",
        Raw::RParen => ")",
        Raw::LBracket => "[",
        Raw::RBracket => "]",
        Raw::LBrace => "{",
        Raw::RBrace => "}",
        Raw::Arrow => "->",
        Raw::EqEq => "==",
        Raw::NotEq => "!=",
        Raw::LtEq => "<=",
        Raw::GtEq => ">=",
        Raw::DollarStar => "$*",
        Raw::Dollar => "$",
        Raw::Eq => "=",
        Raw::Lt => "<",
        Raw::Gt => ">",
        Raw::Plus => "+",
        Raw::Minus => "-",
        Raw::Star => "*",
        Raw::Slash => "/",
        Raw::Percent => "%",
        Raw::Dot => ".",
        Raw::Comma => ",",
        Raw::Colon => ":",
        Raw::At => "@",
        Raw::Pipe => "|",
        Raw::Question => "?",
        _ => "?",
    }
}

/// Lex and lay out one file.
///
/// Errors are reported rather than thrown: a file with an unlexable character still produces a
/// token stream, so the parser can carry on and report more than one problem per run.
pub fn lex(file: FileId, src: &str, diags: &mut Diagnostics) -> Vec<Token> {
    let mut lexed: Vec<Token> = Vec::new();
    let mut lx = Raw::lexer(src);
    while let Some(res) = lx.next() {
        let span = Span::new(file, lx.span());
        match res {
            Ok(r) => lexed.push(Token {
                tok: Tok::Raw(r),
                span,
            }),
            Err(()) => diags.push(
                Diagnostic::error("B0100", "unrecognised character", span)
                    .with_primary_label("not a Beck token"),
            ),
        }
    }
    layout(file, src, lexed, diags)
}

/// Python's layout algorithm: an indent stack, brackets suppressing significance.
fn layout(file: FileId, src: &str, lexed: Vec<Token>, diags: &mut Diagnostics) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut stack: Vec<usize> = vec![0];
    let mut depth: i32 = 0;
    let mut at_line_start = true;
    let mut line_has_content = false;

    let mut i = 0;
    while i < lexed.len() {
        let t = &lexed[i];

        if matches!(t.tok, Tok::Raw(Raw::Newline)) {
            if depth == 0 && line_has_content {
                out.push(Token {
                    tok: Tok::Newline,
                    span: t.span,
                });
                line_has_content = false;
                at_line_start = true;
            }
            i += 1;
            continue;
        }

        if at_line_start && depth == 0 {
            let col = indent_width(src, t.span.start as usize);
            let top = *stack.last().expect("indent stack is never empty");
            if col > top {
                stack.push(col);
                out.push(Token {
                    tok: Tok::Indent,
                    span: t.span,
                });
            } else if col < top {
                while *stack.last().expect("indent stack is never empty") > col {
                    stack.pop();
                    out.push(Token {
                        tok: Tok::Dedent,
                        span: t.span,
                    });
                }
                if *stack.last().expect("indent stack is never empty") != col {
                    diags.push(
                        Diagnostic::error("B0101", "inconsistent indentation", t.span)
                            .with_primary_label("this line does not match any enclosing block")
                            .with_note("indentation is significant: spaces only, four per level"),
                    );
                    stack.push(col);
                }
            }
            at_line_start = false;
        }

        match &t.tok {
            Tok::Raw(Raw::LParen | Raw::LBracket | Raw::LBrace) => depth += 1,
            Tok::Raw(Raw::RParen | Raw::RBracket | Raw::RBrace) => depth -= 1,
            _ => {}
        }
        line_has_content = true;
        out.push(t.clone());
        i += 1;
    }

    let end = Span::new(file, src.len()..src.len());
    if line_has_content {
        out.push(Token {
            tok: Tok::Newline,
            span: end,
        });
    }
    while stack.len() > 1 {
        stack.pop();
        out.push(Token {
            tok: Tok::Dedent,
            span: end,
        });
    }
    out.push(Token {
        tok: Tok::Eof,
        span: end,
    });
    out
}

/// How far into the line the first token starts, counting a tab as one column.
///
/// §2.6 fixes indentation as "spaces only, 4"; a tab is therefore a lint, not a width question,
/// and counting it as one keeps the layout deterministic either way.
fn indent_width(src: &str, offset: usize) -> usize {
    let line_start = src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    src[line_start..offset].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        let mut map = beck_diag::SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let out = lex(f, src, &mut d).into_iter().map(|t| t.tok).collect();
        assert!(!d.has_errors(), "{}", d.render(&map));
        out
    }

    #[test]
    fn indentation_becomes_indent_and_dedent() {
        let t = toks("def f():\n    return 1\n");
        let shape: Vec<&str> = t
            .iter()
            .map(|t| match t {
                Tok::Indent => "INDENT",
                Tok::Dedent => "DEDENT",
                Tok::Newline => "NL",
                Tok::Eof => "EOF",
                Tok::Raw(_) => "tok",
            })
            .collect();
        assert_eq!(
            shape,
            [
                "tok", "tok", "tok", "tok", "tok", "NL", "INDENT", "tok", "tok", "NL", "DEDENT",
                "EOF"
            ]
        );
    }

    #[test]
    fn brackets_suppress_layout_so_calls_may_span_lines() {
        let t = toks("f(\n    1,\n    2,\n)\n");
        assert_eq!(t.iter().filter(|t| matches!(t, Tok::Indent)).count(), 0);
        assert_eq!(t.iter().filter(|t| matches!(t, Tok::Newline)).count(), 1);
    }

    #[test]
    fn blank_and_comment_lines_do_not_close_a_block() {
        let t = toks("def f():\n    a\n\n# a comment at column zero\n    b\n");
        assert_eq!(t.iter().filter(|t| matches!(t, Tok::Dedent)).count(), 1);
        assert_eq!(t.iter().filter(|t| matches!(t, Tok::Indent)).count(), 1);
    }

    #[test]
    fn keywords_beat_the_colon_operator_and_dollar_star_beats_dollar() {
        assert!(matches!(
            &toks(":id x")[0],
            Tok::Raw(Raw::Keyword(k)) if k == "id"
        ));
        assert!(matches!(&toks("$*xs")[0], Tok::Raw(Raw::DollarStar)));
        assert!(matches!(&toks("$x")[0], Tok::Raw(Raw::Dollar)));
    }

    #[test]
    fn strings_unescape() {
        assert!(matches!(
            &toks(r#""a\nb\"c""#)[0],
            Tok::Raw(Raw::Str(s)) if s == "a\nb\"c"
        ));
    }

    #[test]
    fn nested_dedents_all_close_at_once() {
        let t = toks("if a:\n    if b:\n        c\nd\n");
        let dedents = t.iter().filter(|t| matches!(t, Tok::Dedent)).count();
        assert_eq!(dedents, 2);
    }
}
