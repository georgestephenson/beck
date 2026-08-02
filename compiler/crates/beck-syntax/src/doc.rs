//! Doc comments: `##` in the Python surface, `;;` in the S-expression one.
//!
//! [`docs/16-packages-and-ecosystem.md`](../../../../docs/16-packages-and-ecosystem.md) §16.2 asks
//! for "documentation generated from types and doc-comments for every published version,
//! automatically". The types were already there — `beck_core::iface::Interface` has carried each
//! name's signature, effect row and placement since Phase 2. This module supplies the other half.
//!
//! # Why a side pass rather than a token
//!
//! An ordinary comment is skipped by the lexer, and layout treats a comment-only line as having no
//! indentation at all — that is what lets a comment sit at column zero inside an indented block
//! without closing it ([`crate::lexer`]). Lexing `##` as a real token would put that rule at risk
//! for every file, to serve a feature that only reads declarations.
//!
//! So doc comments are collected from the source text and attached to nodes afterwards, by
//! position: a run of `##` lines belongs to the declaration on the first line beneath it. The
//! token stream, the layout algorithm and the parser are untouched.
//!
//! # What attaches where
//!
//! A run attaches to the **outermost** node beginning that line, which is what makes
//!
//! ```text
//! ## The page the browser subscribes to.
//! @on(client)
//! def page(…) -> Html:
//! ```
//!
//! attach to the `decorate` form rather than to nothing: the decorator is part of the declaration,
//! and the doc comment is written above the whole thing.
//!
//! A doc comment is [`crate::Meta`], not a form, so it is not part of a node's identity: a
//! doc-only edit does not change [`crate::Node::structurally_eq`], does not invalidate a memo, and
//! does not move `beck_core::iface::Interface::digest` — documenting a function is not an API
//! change.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::node::Node;

/// The marker for the Python surface — `##`, one more `#` than a comment, as `///` is one more `/`.
pub const PY_MARKER: &str = "##";

/// The marker for the S-expression surface. `;;` is the Lisp convention for a comment about the
/// form beneath it, and `;` stays an ordinary comment.
pub const SEXPR_MARKER: &str = ";;";

/// The marker a file's extension implies.
pub fn marker_for(name: &str) -> &'static str {
    if name.ends_with(".sx") {
        SEXPR_MARKER
    } else {
        PY_MARKER
    }
}

/// Doc-comment runs in one source file, indexed by the line they document.
#[derive(Clone, Debug, Default)]
pub struct DocComments {
    /// Line index (0-based) of the first line *below* a run → the run's text.
    runs: BTreeMap<usize, Arc<str>>,
    /// Byte offset of the first character of each line, and of the first non-whitespace character.
    lines: Vec<(usize, usize)>,
}

impl DocComments {
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// Collect every doc-comment run in a file.
///
/// A line counts only when the marker is the first thing on it. That is what keeps a string
/// containing `##` from being read as documentation — and Beck string literals cannot span lines,
/// so scanning line by line needs no lexer state.
pub fn collect(src: &str, marker: &str) -> DocComments {
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut text: Vec<&str> = Vec::new();
    let mut at = 0usize;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        lines.push((at, at + (line.len() - trimmed.len())));
        text.push(trimmed.trim_end_matches(['\n', '\r']));
        at += line.len();
    }

    // Walk the runs backwards so a run is attributed to the line beneath the *last* of its
    // comments, and a blank line between the comment and the declaration breaks the run.
    let mut runs: BTreeMap<usize, Arc<str>> = BTreeMap::new();
    let mut i = 0usize;
    while i < text.len() {
        if !is_doc(text[i], marker) {
            i += 1;
            continue;
        }
        let start = i;
        while i < text.len() && is_doc(text[i], marker) {
            i += 1;
        }
        // `i` is now the first line that is not part of the run — the line being documented. A run
        // with nothing beneath it (end of file) documents nothing and is dropped.
        if i < text.len() && !text[i].is_empty() {
            let body: Vec<&str> = text[start..i].iter().map(|l| strip(l, marker)).collect();
            runs.insert(i, Arc::from(trim_blank_edges(&body).join("\n")));
        }
    }

    DocComments { runs, lines }
}

fn is_doc(line: &str, marker: &str) -> bool {
    line.starts_with(marker)
}

/// Strip the marker and, if present, exactly one space — so `## text` and `##text` both yield
/// `text`, and an indented continuation keeps its relative indentation.
fn strip<'a>(line: &'a str, marker: &str) -> &'a str {
    let rest = &line[marker.len()..];
    rest.strip_prefix(' ').unwrap_or(rest).trim_end()
}

fn trim_blank_edges<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map(|e| e + 1)
        .unwrap_or(start);
    lines[start..end].to_vec()
}

/// Attach every run to the node it documents.
///
/// Outermost first, and each run is claimed once: `@on(client)` above a `def` is one declaration,
/// and its doc comment belongs to the whole of it.
pub fn attach(node: &mut Node, docs: &DocComments) {
    if docs.is_empty() {
        return;
    }
    let mut claimed: Vec<usize> = Vec::new();
    walk(node, docs, &mut claimed);
}

fn walk(node: &mut Node, docs: &DocComments, claimed: &mut Vec<usize>) {
    let start = node.span().start as usize;
    if let Some(line) = line_starting_at(docs, start) {
        if let Some(text) = docs.runs.get(&line) {
            if !claimed.contains(&line) {
                claimed.push(line);
                node.meta.doc = Some(text.clone());
            }
        }
    }
    for a in &mut node.args {
        walk(a, docs, claimed);
    }
}

/// The line this offset begins, if the offset *is* that line's first non-whitespace character.
///
/// The restriction is what makes attachment unambiguous: a node in the middle of a line is not the
/// thing a comment above the line was written about.
fn line_starting_at(docs: &DocComments, offset: usize) -> Option<usize> {
    let idx = docs
        .lines
        .binary_search_by(|(start, _)| start.cmp(&offset))
        .unwrap_or_else(|i| i.saturating_sub(1));
    let (_, first) = *docs.lines.get(idx)?;
    (first == offset).then_some(idx)
}

/// Render a doc comment back into source, one `## ` line each, at the given indentation.
pub fn render(doc: &str, marker: &str, indent: &str) -> String {
    let mut out = String::new();
    for line in doc.split('\n') {
        out.push_str(indent);
        out.push_str(marker);
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_diag::{Diagnostics, SourceMap};

    fn parse(src: &str) -> Node {
        let mut map = SourceMap::new();
        let file = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        let n = crate::parse_file(file, "t.beck", src, &mut d);
        assert!(!d.has_errors(), "{}", d.render(&map));
        n
    }

    fn doc_of(n: &Node, name: &str) -> Option<String> {
        for item in n.args.iter().skip(1) {
            let mut inner = item;
            while inner.is_form(crate::sym::DECORATE) {
                inner = &inner.args[1];
            }
            let matches = inner
                .args
                .first()
                .and_then(|a| a.as_var())
                .map(|s| s.as_str() == name)
                .unwrap_or(false);
            if matches {
                return item.meta.doc.as_ref().map(|d| d.to_string());
            }
        }
        None
    }

    #[test]
    fn a_run_of_doc_lines_attaches_to_the_declaration_beneath_it() {
        let n = parse("## Adds two numbers.\n## Both of them.\ndef add(a: Int, b: Int) -> Int:\n    return a\n");
        assert_eq!(
            doc_of(&n, "add").as_deref(),
            Some("Adds two numbers.\nBoth of them.")
        );
    }

    #[test]
    fn a_doc_comment_above_a_decorator_documents_the_whole_declaration() {
        let n = parse("## The page.\n@on(client)\ndef page() -> Int:\n    return 1\n");
        assert_eq!(doc_of(&n, "page").as_deref(), Some("The page."));
    }

    #[test]
    fn a_blank_line_ends_a_run_so_a_file_header_documents_nothing() {
        let n = parse("## A file header, about the module.\n\ndef f() -> Int:\n    return 1\n");
        assert_eq!(doc_of(&n, "f"), None);
    }

    #[test]
    fn an_ordinary_comment_is_still_an_ordinary_comment() {
        let n = parse("# not documentation\ndef f() -> Int:\n    return 1\n");
        assert_eq!(doc_of(&n, "f"), None);
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_doc_comment() {
        let docs = collect("x = \"## not a doc\"\n", PY_MARKER);
        assert!(docs.is_empty());
    }

    #[test]
    fn a_doc_comment_does_not_change_what_a_program_means() {
        // Structural equality ignores `Meta`, so documenting a definition cannot invalidate a
        // memo or move an interface digest.
        let plain = parse("def f() -> Int:\n    return 1\n");
        let documented = parse("## Documented.\ndef f() -> Int:\n    return 1\n");
        assert_eq!(plain, documented);
        assert!(doc_of(&documented, "f").is_some());
    }

    /// Every doc comment in the tree, keyed by the path of node indices that reaches it — so the
    /// comparison is about *where* a comment landed as well as what it says.
    fn all_docs(n: &Node) -> Vec<(Vec<usize>, String)> {
        fn go(n: &Node, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, String)>) {
            if let Some(d) = &n.meta.doc {
                out.push((path.clone(), d.to_string()));
            }
            for (i, a) in n.args.iter().enumerate() {
                path.push(i);
                go(a, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(n, &mut Vec::new(), &mut out);
        out
    }

    fn reparse(name: &str, src: &str) -> Node {
        let mut map = SourceMap::new();
        let file = map.add(name, src);
        let mut d = Diagnostics::new();
        let n = crate::parse_file(file, name, src, &mut d);
        assert!(!d.has_errors(), "{}\n--- source ---\n{src}", d.render(&map));
        n
    }

    const DOCUMENTED: &str = "\
## The identifier of a todo.
type Id = newtype[Str]

## One item on the list.
model Todo:
    ## Stable for the life of the item.
    id: Id
    ## What the user typed.
    text: Str

## What may happen to the list.
union Event:
    Added(id: Id)
    ## Toggling is idempotent in the fold.
    Toggled(id: Id)

## Adds two numbers, and is documented about it.
@on(any)
def add(a: Int, b: Int) -> Int:
    return a
";

    #[test]
    fn doc_comments_survive_printing_and_reparsing_in_both_surfaces() {
        let original = reparse("t.beck", DOCUMENTED);
        let docs = all_docs(&original);
        assert_eq!(docs.len(), 7, "{docs:#?}");

        let py = crate::print::to_python(&original);
        assert_eq!(all_docs(&reparse("t.beck", &py)), docs, "python:\n{py}");

        let sx = crate::print::to_sexpr_pretty(&original);
        assert_eq!(all_docs(&reparse("t.sx", &sx)), docs, "sexpr:\n{sx}");
    }

    #[test]
    fn formatting_a_documented_module_is_idempotent() {
        let once = crate::print::to_python(&reparse("t.beck", DOCUMENTED));
        let twice = crate::print::to_python(&reparse("t.beck", &once));
        assert_eq!(once, twice, "once:\n{once}\ntwice:\n{twice}");
    }

    #[test]
    fn model_fields_are_documented_too() {
        let n = parse("model Todo:\n    ## What it says.\n    text: Str\n");
        let model = &n.args[1];
        let field = &model.args[1];
        assert_eq!(field.meta.doc.as_deref(), Some("What it says."));
    }
}
