//! Diagnostics as structured values, and the one renderer that prints them.
//!
//! [`docs/04-compiler-architecture.md`](../../../../docs/04-compiler-architecture.md) §4.5: "for a
//! language whose main feature is inference, error quality *is* the product". Concretely that means
//! three things, and this crate is where all three live:
//!
//! * every diagnostic is a **value** (code, primary span, secondary spans, notes, fix-its), never a
//!   formatted string thrown at stderr;
//! * one renderer is shared by the CLI, the snapshot suite and (later) the LSP, so the two cannot
//!   drift;
//! * macro-generated code carries its **expansion chain**, so a type error inside `derive(Json)`
//!   says where the derive was written.

pub mod depth;
pub mod index;

use std::fmt::Write as _;
use std::ops::Range;

/// A file in the compilation. Interned by [`SourceMap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

/// A byte range within a file.
///
/// Every `Node` carries one, every `Core` node carries provenance back to a `Node`; that chain is
/// what lets a placement error point at the annotation the programmer actually wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Default for Span {
    /// A node with no source position — macro-generated code that chose not to borrow one.
    fn default() -> Span {
        Span::NONE
    }
}

impl Span {
    pub const NONE: Span = Span {
        file: FileId(u32::MAX),
        start: 0,
        end: 0,
    };

    pub fn new(file: FileId, range: Range<usize>) -> Span {
        Span {
            file,
            start: range.start as u32,
            end: range.end as u32,
        }
    }

    pub fn is_none(&self) -> bool {
        self.file.0 == u32::MAX
    }

    /// The smallest span covering both. Used when a parser builds a node from several tokens.
    pub fn to(self, other: Span) -> Span {
        if self.is_none() {
            return other;
        }
        if other.is_none() || other.file != self.file {
            return self;
        }
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

#[derive(Clone, Debug)]
struct SourceFile {
    name: String,
    text: String,
    /// Byte offset of the start of each line, for O(log n) offset → line/col.
    line_starts: Vec<u32>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let (name, text) = (name.into(), text.into());
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        self.files.push(SourceFile {
            name,
            text,
            line_starts,
        });
        FileId(self.files.len() as u32 - 1)
    }

    pub fn name(&self, file: FileId) -> &str {
        &self.files[file.0 as usize].name
    }

    /// The file this map holds under `name`, if it holds one.
    ///
    /// For a caller that has a span and needs to know **which file it is in** rather than what it
    /// says: an editor answering a question about one document holds spans from every module the
    /// project linked, and two files' byte offsets overlap by construction. Compares whole names
    /// rather than suffixes, because a `FileId` that is nearly right is a span pointing into the
    /// wrong buffer.
    pub fn find(&self, name: &str) -> Option<FileId> {
        self.files
            .iter()
            .position(|f| f.name == name)
            .map(|i| FileId(i as u32))
    }

    pub fn text(&self, file: FileId) -> &str {
        &self.files[file.0 as usize].text
    }

    pub fn snippet(&self, span: Span) -> &str {
        if span.is_none() {
            return "";
        }
        let text = self.text(span.file);
        let (s, e) = (span.start as usize, (span.end as usize).min(text.len()));
        text.get(s..e).unwrap_or("")
    }

    /// 1-based line and column (column counted in characters, not bytes).
    ///
    /// A span from a file this map does not hold reports `1:1` rather than panicking. That is a
    /// compiler defect wherever it happens — a diagnostic that cannot be located is a diagnostic
    /// nobody can act on — but the failure belongs in the message, not in a crash during rendering.
    pub fn line_col(&self, file: FileId, offset: u32) -> (usize, usize) {
        let Some(f) = self.files.get(file.0 as usize) else {
            return (1, 1);
        };
        let line = f.line_starts.partition_point(|&s| s <= offset).max(1) - 1;
        let start = f.line_starts[line] as usize;
        let col = f.text[start..(offset as usize).min(f.text.len())]
            .chars()
            .count();
        (line + 1, col + 1)
    }

    fn line_text(&self, file: FileId, line: usize) -> &str {
        let f = &self.files[file.0 as usize];
        let start = f.line_starts[line - 1] as usize;
        let end = f
            .line_starts
            .get(line)
            .map(|&e| e as usize - 1)
            .unwrap_or(f.text.len());
        f.text[start..end.min(f.text.len())].trim_end_matches('\r')
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

/// A secondary span: somewhere else the reader needs to look.
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// One step of a macro expansion chain — "in `derive(Json)` expanded at orders.beck:12" (§4.5).
#[derive(Clone, Debug)]
pub struct ExpansionStep {
    pub macro_name: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// A stable code (`B0101`) so that tests, docs and the eventual error index agree.
    pub code: &'static str,
    pub message: String,
    pub primary: Span,
    pub primary_label: Option<String>,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    /// A suggested edit. Placement ambiguity is "a compile error with a suggested annotation —
    /// never a silent guess" (§3.4), and this is where that suggestion rides.
    pub fix: Option<String>,
    pub expansion: Vec<ExpansionStep>,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: impl Into<String>, primary: Span) -> Diagnostic {
        Diagnostic {
            severity: Severity::Error,
            code,
            message: message.into(),
            primary,
            primary_label: None,
            labels: Vec::new(),
            notes: Vec::new(),
            fix: None,
            expansion: Vec::new(),
        }
    }

    pub fn warning(code: &'static str, message: impl Into<String>, primary: Span) -> Diagnostic {
        Diagnostic {
            severity: Severity::Warning,
            ..Diagnostic::error(code, message, primary)
        }
    }

    pub fn with_primary_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.primary_label = Some(label.into());
        self
    }

    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Diagnostic {
        self.fix = Some(fix.into());
        self
    }

    pub fn with_expansion(mut self, steps: Vec<ExpansionStep>) -> Diagnostic {
        self.expansion = steps;
        self
    }
}

/// Diagnostics accumulated by one compilation.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Diagnostics {
        Diagnostics::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    /// Sorted by position, so a snapshot does not depend on the order checks happened to run in.
    pub fn sorted(&self) -> Vec<&Diagnostic> {
        let mut out: Vec<&Diagnostic> = self.items.iter().collect();
        out.sort_by_key(|d| (d.primary.file, d.primary.start, d.code));
        out
    }

    pub fn render(&self, map: &SourceMap) -> String {
        let mut out = String::new();
        for d in self.sorted() {
            out.push_str(&render(d, map));
            out.push('\n');
        }
        out
    }
}

/// The renderer. Modelled on rustc/Elm: severity line, location, the source line with a caret
/// span, then secondary labels, notes and the fix-it.
pub fn render(d: &Diagnostic, map: &SourceMap) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}[{}]: {}", d.severity.label(), d.code, d.message);

    if !d.primary.is_none() {
        let (line, col) = map.line_col(d.primary.file, d.primary.start);
        let _ = writeln!(out, "  --> {}:{}:{}", map.name(d.primary.file), line, col);
        render_snippet(&mut out, map, d.primary, d.primary_label.as_deref());
    }

    for label in &d.labels {
        if label.span.is_none() {
            continue;
        }
        let (line, col) = map.line_col(label.span.file, label.span.start);
        let _ = writeln!(out, "  --> {}:{}:{}", map.name(label.span.file), line, col);
        render_snippet(&mut out, map, label.span, Some(&label.message));
    }

    for step in &d.expansion {
        if step.span.is_none() {
            let _ = writeln!(out, "  = in `{}`", step.macro_name);
        } else {
            let (line, col) = map.line_col(step.span.file, step.span.start);
            let _ = writeln!(
                out,
                "  = in `{}` expanded at {}:{}:{}",
                step.macro_name,
                map.name(step.span.file),
                line,
                col
            );
        }
    }

    for note in &d.notes {
        let _ = writeln!(out, "  = note: {note}");
    }
    if let Some(fix) = &d.fix {
        let _ = writeln!(out, "  = help: {fix}");
    }
    out
}

fn render_snippet(out: &mut String, map: &SourceMap, span: Span, label: Option<&str>) {
    let (line, col) = map.line_col(span.file, span.start);
    let text = map.line_text(span.file, line);
    let gutter = line.to_string();
    let pad = " ".repeat(gutter.len());

    let _ = writeln!(out, "{pad} |");
    let _ = writeln!(out, "{gutter} | {text}");

    // A span that runs past the end of its line is clamped: multi-line spans underline their
    // first line and say so, rather than drawing a caret run across a line break.
    let line_end = span.start + (text.chars().count() as u32 - (col as u32 - 1));
    let end = span.end.min(line_end);
    let width = map
        .text(span.file)
        .get(span.start as usize..end as usize)
        .map(|s| s.chars().count())
        .unwrap_or(1)
        .max(1);

    let _ = write!(out, "{pad} | {}{}", " ".repeat(col - 1), "^".repeat(width));
    match label {
        Some(l) => {
            let _ = writeln!(out, " {l}");
        }
        None => {
            let _ = writeln!(out);
        }
    }
    let _ = writeln!(out, "{pad} |");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based_and_counts_characters() {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", "def f():\n    return é + 1\n");
        assert_eq!(map.line_col(f, 0), (1, 1));
        assert_eq!(map.line_col(f, 9), (2, 1));
        // `é` is two bytes; the column after it must be 13, not 14.
        let idx = map.text(f).find('é').unwrap() as u32;
        assert_eq!(map.line_col(f, idx), (2, 12));
        assert_eq!(map.line_col(f, idx + 2), (2, 13));
    }

    #[test]
    fn a_rendered_diagnostic_points_at_the_right_column() {
        let mut map = SourceMap::new();
        let src = "def f():\n    return g(1)\n";
        let f = map.add("t.beck", src);
        let span = Span::new(f, 20..21);
        assert_eq!(&src[20..21], "g");
        let d = Diagnostic::error("B0001", "cannot find `g` in this scope", span)
            .with_primary_label("not found")
            .with_note("`g` is not defined in this module")
            .with_fix("did you mean `f`?");
        let rendered = render(&d, &map);
        assert!(rendered.contains("error[B0001]: cannot find `g` in this scope"));
        assert!(rendered.contains("--> t.beck:2:12"));
        assert!(rendered.contains("           ^ not found"));
        assert!(rendered.contains("= help: did you mean `f`?"));
    }

    #[test]
    fn sorting_makes_output_independent_of_check_order() {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", "aaa\nbbb\n");
        let mut ds = Diagnostics::new();
        ds.push(Diagnostic::error("B0002", "second", Span::new(f, 4..7)));
        ds.push(Diagnostic::error("B0001", "first", Span::new(f, 0..3)));
        let codes: Vec<_> = ds.sorted().iter().map(|d| d.code).collect();
        assert_eq!(codes, ["B0001", "B0002"]);
        assert!(ds.has_errors());
    }
}
