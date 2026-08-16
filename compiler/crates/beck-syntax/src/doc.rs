//! Doc comments: `##` in the Python surface, `;;` in the S-expression one.
//!
//! [`docs/16-packages-and-ecosystem.md`](../../../../../docs/16-packages-and-ecosystem.md) §16.2 asks
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
//!
//! # Ordinary comments are collected by the same pass, for the same reason
//!
//! `beck fmt` prints from the tree, so a comment the tree does not carry is one the formatter
//! deletes — and a formatter an editor runs on save must not delete what somebody wrote. That was
//! `DEFECTS.md::fmt-comments`, and it is why `textDocument/formatting` was deliberately not
//! offered.
//!
//! One pass rather than two, because **what separates the two kinds is one decision**: a line
//! beginning `##` is documentation and a line beginning `#` is a comment. Collected apart, that
//! rule would be written twice and the copies would disagree about `###`.
//!
//! Three positions, and each attaches differently:
//!
//! * **Above a node**, as [`crate::Comments::before`] — a run of full-line comments, claimed by
//!   the outermost node beginning the first line beneath it.
//! * **At the end of a node's own line**, as [`crate::Comments::trailing`]. Finding it means
//!   skipping string literals, because `"a # b"` is not a comment.
//! * **Below a node with nothing after it**, as [`crate::Comments::after`] — the end of a body or
//!   of the file. These attach *backwards*, to the last node that began a line above them, because
//!   there is nothing beneath to attach forwards to. Without this case the comment at the end of a
//!   function would move to whatever came next and out of the block it was written in.

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

/// Every comment in one source file, indexed by the line it belongs to.
#[derive(Clone, Debug, Default)]
pub struct DocComments {
    /// Line index (0-based) of the first line *below* a doc run → the run's text.
    runs: BTreeMap<usize, Arc<str>>,
    /// Line index of the line below an ordinary run → its lines, in source order.
    before: BTreeMap<usize, Vec<Arc<str>>>,
    /// Line index → the comment that ends that line.
    trailing: BTreeMap<usize, Arc<str>>,
    /// Line index of the last line that began a node above the run → the run's lines. Where a run
    /// has nothing beneath it in its block.
    after: BTreeMap<usize, Vec<Arc<str>>>,
    /// Byte offset of the first character of each line, and of the first non-whitespace character.
    lines: Vec<(usize, usize)>,
}

impl DocComments {
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
            && self.before.is_empty()
            && self.trailing.is_empty()
            && self.after.is_empty()
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
        // `i` is now the first line that is not part of the run — the line being documented, once
        // any *ordinary* comment lines between the two are stepped over. That case is rare and it
        // used to lose the documentation outright: the run attached to a line no node begins, and
        // nothing claimed it. A blank line still breaks the association, which is the difference
        // between "this documents that" and "this is a note that happens to be above it".
        let mut target = i;
        while target < text.len() && text[target].starts_with('#') && !is_doc(text[target], marker)
        {
            target += 1;
        }
        if target < text.len() && !text[target].is_empty() {
            let body: Vec<&str> = text[start..i].iter().map(|l| strip(l, marker)).collect();
            runs.insert(target, Arc::from(trim_blank_edges(&body).join("\n")));
        }
    }

    let indents: Vec<usize> = lines.iter().map(|(start, first)| first - start).collect();
    let (before, trailing, after) = ordinary(&text, &indents, marker);
    DocComments {
        runs,
        before,
        trailing,
        after,
        lines,
    }
}

/// Every ordinary comment, in the three positions of this module's third section.
///
/// `text` is the file's lines, trimmed of indentation and line endings, and `marker` is what makes
/// a line documentation rather than a comment.
type Ordinary = (
    BTreeMap<usize, Vec<Arc<str>>>,
    BTreeMap<usize, Arc<str>>,
    BTreeMap<usize, Vec<Arc<str>>>,
);

fn ordinary(text: &[&str], indents: &[usize], marker: &str) -> Ordinary {
    let mut before: BTreeMap<usize, Vec<Arc<str>>> = BTreeMap::new();
    let mut trailing: BTreeMap<usize, Arc<str>> = BTreeMap::new();
    let mut after: BTreeMap<usize, Vec<Arc<str>>> = BTreeMap::new();

    let is_ordinary = |l: &str| l.starts_with('#') && !is_doc(l, marker);
    // Every line that carried code, with its indentation. An `after` run hangs on the last one
    // **at or above its own level**: a comment at column zero at the end of a file belongs to the
    // declaration it follows, not to the last statement of that declaration's innermost block.
    let mut code: Vec<(usize, usize)> = Vec::new();

    let mut i = 0usize;
    while i < text.len() {
        let line = text[i];
        if is_ordinary(line) {
            let start = i;
            // **A preamble is one block, blank lines included.** A file header, a blank line and a
            // section rule above the first declaration are three things a reader sees as one, and
            // collecting them as separate runs leaves the first with only a comment beneath it —
            // nothing to attach to, so it would travel to the end of the file. Interior blanks are
            // kept as empty entries so the block prints back with its own shape.
            // A block continues through blank lines, but only into comments at its **own
            // indentation**: the note at the end of a function body and the note above the next
            // declaration are two blocks with a blank line between them, and joining them would
            // print one of them in the other's place.
            let mut run: Vec<Arc<str>> = Vec::new();
            while i < text.len()
                && (text[i].is_empty() || (is_ordinary(text[i]) && indents[i] == indents[start]))
            {
                run.push(Arc::from(text[i]));
                i += 1;
            }
            // A block that ran on through blank lines and stopped at something else gives the
            // blanks back, so that `i` is where the scan continues from.
            while run.last().is_some_and(|l| l.is_empty()) {
                run.pop();
                i -= 1;
            }
            // The line the run is about: the next one that carries something. Blank lines are
            // stepped over rather than breaking the run, because a comment separated from its
            // declaration by a blank line is still that declaration's — and dropping it, which is
            // what a doc run does, would delete it.
            // The line the run is about: the next one carrying something that is not itself a
            // comment. Blank lines are stepped over rather than breaking the run — a comment
            // separated from its declaration by a blank line is still that declaration's, and
            // dropping it, which is what a doc run does, would delete it. A **doc** run beneath is
            // stepped over too: `# note` above `## documentation` above `def` is all one preamble
            // and belongs to the same declaration.
            let mut target = i;
            while target < text.len() && (text[target].is_empty() || is_doc(text[target], marker)) {
                target += 1;
            }
            // **Which way it attaches is decided by indentation.** A run indented further than
            // the line beneath it is the end of the block it sits in — the comment at the bottom
            // of a function body — and attaching it forwards would move it out of that block and
            // print it against the next declaration. There is nothing beneath it *in its own
            // block*, so it hangs backwards on the last line that began one.
            let deeper = target < text.len() && indents[start] > indents[target];
            let hangs_on = || {
                code.iter()
                    .rev()
                    .find(|(indent, _)| *indent <= indents[start])
                    .map(|(_, line)| *line)
            };
            match (target < text.len() && !deeper, hangs_on()) {
                (true, _) => before.entry(target).or_default().extend(run),
                (false, Some(line)) => {
                    let entry = after.entry(line).or_default();
                    // The blank line above it is part of how a tail comment reads, and there is
                    // nothing else left to supply it: the printer's own spacing goes *between*
                    // items, and this is inside one.
                    if entry.is_empty() && start > 0 && text[start - 1].is_empty() {
                        entry.push(Arc::from(""));
                    }
                    entry.extend(run);
                }
                // Nothing above it at its level and nothing below it at all — a file that is only
                // comments. `attach` puts it on the root rather than dropping it.
                (false, None) => {}
            }
            continue;
        }
        if line.is_empty() || is_doc(line, marker) {
            i += 1;
            continue;
        }
        if let Some(text) = comment_ending(line) {
            trailing.insert(i, Arc::from(text));
        }
        code.push((indents[i], i));
        i += 1;
    }
    (before, trailing, after)
}

/// The comment that ends this line, if it has one.
///
/// A `#` inside a string literal is not a comment, so this walks the line rather than searching
/// it. Beck's strings cannot span lines, so no state carries between lines and the walk is exact.
fn comment_ending(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'#' if !in_string => return Some(line[i..].trim_end()),
            _ => {}
        }
        i += 1;
    }
    None
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
    let mut lines: Vec<usize> = Vec::new();
    walk(node, docs, &mut claimed, &mut lines);

    // **Nothing is dropped, even when nothing wanted it.** A comment can sit on a line no node
    // begins — a continuation inside a bracketed call, say — and the formatter prints from the
    // tree, so a comment the tree does not carry is deleted. Anything unclaimed goes to the end of
    // the root, which is bad placement and not a lost line;
    // `roundtrip.rs::formatting_keeps_every_comment` is what says the difference matters.
    let mut orphans: Vec<Arc<str>> = Vec::new();
    for (line, run) in docs.before.iter().chain(docs.after.iter()) {
        if !lines.contains(line) {
            orphans.extend(run.iter().cloned());
        }
    }
    for (line, text) in &docs.trailing {
        if !lines.contains(line) {
            orphans.push(text.clone());
        }
    }
    if !orphans.is_empty() {
        node.meta
            .comments
            .get_or_insert_with(Default::default)
            .after
            .extend(orphans);
    }
}

fn walk(node: &mut Node, docs: &DocComments, claimed: &mut Vec<usize>, lines: &mut Vec<usize>) {
    let start = node.span().start as usize;
    if let Some(line) = line_starting_at(docs, start) {
        if !claimed.contains(&line) {
            claimed.push(line);
            lines.push(line);
            if let Some(text) = docs.runs.get(&line) {
                node.meta.doc = Some(text.clone());
            }
            // The outermost node beginning a line takes that line's comments, for the reason it
            // takes the doc run: a comment above `@on(client)` is about the declaration under it,
            // not about the annotation.
            let before = docs.before.get(&line);
            let trailing = docs.trailing.get(&line);
            let after = docs.after.get(&line);
            if before.is_some() || trailing.is_some() || after.is_some() {
                let c = node.meta.comments.get_or_insert_with(Default::default);
                c.before = before.cloned().unwrap_or_default();
                c.trailing = trailing.cloned();
                c.after = after.cloned().unwrap_or_default();
            }
        }
    }
    for a in &mut node.args {
        walk(a, docs, claimed, lines);
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
        let field = &model.args[2];
        assert_eq!(field.meta.doc.as_deref(), Some("What it says."));
    }
}
