//! What an editor asks the front end, and the answers, once.
//!
//! [`docs/04-compiler-architecture.md`](../../../../../docs/04-compiler-architecture.md) §4.6 fixes
//! the rule this module exists to keep: *"One binary serves `beck build`, `beck check`, `beck lsp`
//! and `beck explain`; there is no separate language server implementation to drift."* Until now
//! that was true of the *compiler* and not of the editor: `beck lsp` held the indexing, the
//! positions and the word-under-the-caret rule, and anything else wanting them — a playground with
//! a `<textarea>` in it ([`docs/98`](../../../../../docs/98-playground-report.md) §98.7) — had to
//! write them a second time.
//!
//! So the answers live here, where `beck-cli` and a `wasm32-unknown-unknown` module can both reach
//! them, and neither renders anything of its own:
//!
//! | Answer | What produces it |
//! |---|---|
//! | [`tokens`] — highlighting | [`beck_syntax::lexer::lex`] and [`beck_syntax::lexer::KEYWORDS`] |
//! | [`Editor::marks`] — inline diagnostics | the diagnostics the checker pushed |
//! | [`Editor::hover`] — a signature | [`crate::iface::render_item`], the one `beck iface` writes |
//! | [`Editor::completions`] — the names in scope | the checked program's own definition table |
//! | [`Editor::definition`] — where a name is declared | the span the checker recorded |
//!
//! # Why the whole file, every time
//!
//! An [`Editor`] is one compile. [`docs/64`](../../../../../docs/64-compile-speed-report.md) §64.6
//! is why that is defensible today: the worst file in this tree costs 4.7 ms through parse, expand
//! and check, and the median costs 0.75 ms. [`tokens`] deliberately does *not* need one — a file
//! being typed into is usually a file that does not compile, and highlighting that waited for a
//! clean parse would go out whenever it was most wanted.

use std::collections::BTreeMap;

use beck_diag::{Diagnostic, Diagnostics, Severity, SourceMap};
use beck_syntax::lexer::{lex, Raw, KEYWORDS};

use crate::iface::{render_item, Kind};
use crate::Placed;

// ---------------------------------------------------------------------------------------------
// Highlighting
// ---------------------------------------------------------------------------------------------

/// What a run of source *is*, for the purpose of colouring it.
///
/// Deliberately small. A theme with thirty categories needs a grammar of its own to feed it; these
/// eight are what the lexer already distinguishes, so every one of them is a fact rather than a
/// guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// A word the parser reads as syntax — [`beck_syntax::lexer::KEYWORDS`], and nothing else.
    Keyword,
    /// An identifier. Which ones are *bound* is a question for the checker, and an editor that
    /// coloured a name differently because it failed to resolve would be recolouring the file on
    /// every keystroke in the middle of a name.
    Name,
    /// `:done` — a keyword literal.
    Atom,
    Number,
    Str,
    Comment,
    /// `##` — documentation, which is metadata rather than a comment ([`beck_syntax::doc`]).
    Doc,
    Punct,
}

impl TokenKind {
    /// The name a stylesheet keys on, and the name a test names.
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Keyword => "keyword",
            TokenKind::Name => "name",
            TokenKind::Atom => "atom",
            TokenKind::Number => "number",
            TokenKind::Str => "string",
            TokenKind::Comment => "comment",
            TokenKind::Doc => "doc",
            TokenKind::Punct => "punct",
        }
    }

    /// The LSP semantic-token type this maps to, from the protocol's own list.
    ///
    /// The mapping is here rather than in `beck lsp` so that an editor's colours and the
    /// playground's are the same decision written once. `##` is a comment to a client that has no
    /// finer category for it, which is every client: the protocol has no `documentation` type.
    pub fn lsp_type(self) -> &'static str {
        match self {
            TokenKind::Keyword => "keyword",
            TokenKind::Name => "variable",
            TokenKind::Atom => "enumMember",
            TokenKind::Number => "number",
            TokenKind::Str => "string",
            TokenKind::Comment | TokenKind::Doc => "comment",
            TokenKind::Punct => "operator",
        }
    }

    /// The legend a `semanticTokens` capability publishes, in the order [`lsp_index`] counts.
    ///
    /// [`lsp_index`]: TokenKind::lsp_index
    pub fn legend() -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for kind in TokenKind::all() {
            let name = kind.lsp_type();
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// This kind's position in [`legend`](TokenKind::legend).
    pub fn lsp_index(self) -> u32 {
        TokenKind::legend()
            .iter()
            .position(|t| *t == self.lsp_type())
            .unwrap_or(0) as u32
    }

    fn all() -> [TokenKind; 8] {
        [
            TokenKind::Keyword,
            TokenKind::Name,
            TokenKind::Atom,
            TokenKind::Number,
            TokenKind::Str,
            TokenKind::Comment,
            TokenKind::Doc,
            TokenKind::Punct,
        ]
    }
}

/// One coloured run: a byte range and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub start: u32,
    pub end: u32,
    pub kind: TokenKind,
}

/// Every coloured run in `text`, in source order, non-overlapping.
///
/// The lexer *skips* comments — a comment is not a token, and making it one would put the layout
/// algorithm's "a comment-only line has no indentation" rule at risk ([`beck_syntax::doc`] says so
/// for `##`). So comments are recovered from the **gaps between the lexer's own spans**: whatever
/// the lexer did not claim is whitespace up to a `#`, and a comment from there to the end of the
/// line. That keeps one scanner rather than two — a second lexer written for highlighting is how
/// an editor ends up disagreeing with the compiler about where a string ends.
///
/// Diagnostics are discarded: a file with an unlexable character still produces a token stream
/// (that is what `lex` promises), and highlighting is wanted most exactly when the file is broken.
pub fn tokens(text: &str) -> Vec<Token> {
    let mut map = SourceMap::new();
    let file = map.add("editor.beck", text);
    let mut diags = Diagnostics::new();
    let lexed = lex(file, text, &mut diags);

    let mut out: Vec<Token> = Vec::new();
    let mut cursor = 0usize;
    for token in &lexed {
        // The synthetic tokens layout inserted (`INDENT`, `DEDENT`, `NEWLINE`, `EOF`) are not runs
        // of source, and some of them are zero-width. Only what the lexer actually read is
        // coloured.
        let Some(raw) = token.raw() else { continue };
        let (start, end) = (token.span.start as usize, token.span.end as usize);
        if start < cursor || end > text.len() {
            continue;
        }
        comments_in(text, cursor, start, &mut out);
        out.push(Token {
            start: start as u32,
            end: end as u32,
            kind: classify(raw),
        });
        cursor = end;
    }
    comments_in(text, cursor, text.len(), &mut out);
    out
}

/// The comments in a stretch of source the lexer claimed nothing in.
fn comments_in(text: &str, from: usize, to: usize, out: &mut Vec<Token>) {
    let mut at = from;
    while at < to {
        let Some(hash) = text[at..to].find('#') else {
            return;
        };
        let start = at + hash;
        let end = text[start..to].find('\n').map(|i| start + i).unwrap_or(to);
        out.push(Token {
            start: start as u32,
            end: end as u32,
            // `##` is documentation and `#` is a comment — one more `#`, as `///` is one more `/`.
            kind: if text[start..].starts_with(beck_syntax::doc::PY_MARKER) {
                TokenKind::Doc
            } else {
                TokenKind::Comment
            },
        });
        at = end + 1;
    }
}

fn classify(raw: &Raw) -> TokenKind {
    match raw {
        Raw::Ident(word) if KEYWORDS.contains(&word.as_str()) => TokenKind::Keyword,
        Raw::Ident(_) => TokenKind::Name,
        Raw::Keyword(_) => TokenKind::Atom,
        Raw::Int(_) | Raw::Float(_) => TokenKind::Number,
        Raw::Str(_) => TokenKind::Str,
        _ => TokenKind::Punct,
    }
}

// ---------------------------------------------------------------------------------------------
// The analysed document
// ---------------------------------------------------------------------------------------------

/// One name an editor can ask about.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Where the declaration is **in this document**, or `None` for an imported name — whose
    /// declaration is in a `.becki` this editor is not showing. A jump-to-definition that landed on
    /// a byte range of the wrong file is worse than one that declines.
    pub span: Option<(u32, u32)>,
    /// The signature as `beck iface` would publish it — [`render_item`], not a second renderer.
    pub signature: String,
    pub kind: SymbolKind,
    pub tier: String,
    /// The `##` comment attached to the declaration, if it has one.
    pub doc: Option<String>,
    /// True for a name this module declares, false for one it imported.
    pub own: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Signal,
}

impl SymbolKind {
    /// `12` (Function) or `13` (Variable) in LSP's `SymbolKind`.
    pub fn lsp_symbol(self) -> u32 {
        match self {
            SymbolKind::Function => 12,
            SymbolKind::Signal => 13,
        }
    }

    /// `3` (Function) or `6` (Variable) in LSP's `CompletionItemKind`.
    pub fn lsp_completion(self) -> u32 {
        match self {
            SymbolKind::Function => 3,
            SymbolKind::Signal => 6,
        }
    }
}

/// One thing an editor can offer to finish the word being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// The signature, for a name; empty for a keyword.
    pub detail: String,
    pub kind: CompletionKind,
    pub doc: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Function,
    Signal,
}

impl CompletionKind {
    /// LSP's `CompletionItemKind`: `14` is Keyword.
    pub fn lsp(self) -> u32 {
        match self {
            CompletionKind::Keyword => 14,
            CompletionKind::Function => SymbolKind::Function.lsp_completion(),
            CompletionKind::Signal => SymbolKind::Signal.lsp_completion(),
        }
    }
}

/// One diagnostic, as an editor draws it: a byte range, a code and the text a terminal would print.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub start: u32,
    pub end: u32,
    pub error: bool,
    pub code: String,
    /// The message, the notes and the fix — everything the terminal renderer would have put under
    /// the span. An editor that showed only the first line would be dropping the suggestion §3.4
    /// insists a diagnostic carries.
    pub message: String,
}

/// The names an analysis found, kept without the analysis.
///
/// Strings and spans, so a server can hold one per open document. See
/// [`Editor::completing_from`] for what it is for.
#[derive(Clone, Debug, Default)]
pub struct Index {
    names: BTreeMap<String, Symbol>,
}

impl Index {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// One document, analysed: what the front end made of the text the editor last sent.
pub struct Editor {
    text: String,
    placed: Option<Placed>,
    diagnostics: Diagnostics,
    map: SourceMap,
    /// Every name an editor can ask about, own and imported.
    ///
    /// A `BTreeMap` so that document symbols come out in a stable order whatever order the checker
    /// resolved them in — the same reason [`crate::iface::Interface`] keeps its types in
    /// declaration order.
    names: BTreeMap<String, Symbol>,
    /// True when [`Editor::completing_from`] supplied the names, because this text does not check.
    stale: bool,
}

impl Editor {
    /// Parse, expand, check, place and secure one document, and index what an editor can ask about.
    ///
    /// A **project** rather than a lone module, with the standard library as the only other place
    /// modules come from: a file being edited may `import bignum`, and an editor that answered
    /// "cannot find `add_big`" for every name in it would be answering about a program the compiler
    /// does not have. There is no directory here — a browser tab has none, and a language server
    /// resolving a relative path off a URI is a decision [`docs/65`](../../../../../docs/65-lsp-report.md)
    /// did not take — so the loader serves this text as the root and nothing else, and
    /// [`crate::stdlib`] answers the rest.
    ///
    /// A library is analysed as a library, not refused: most files being edited are, and
    /// [`crate::project::slice_or_library`] is the same entry `beck check` uses to say so.
    pub fn of(name: &str, text: &str) -> Editor {
        let mut map = SourceMap::new();
        let mut diagnostics = Diagnostics::new();
        // The module name a file has, which is what an `import` names it by.
        let root = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(name)
            .split('.')
            .next()
            .unwrap_or(name)
            .to_string();
        let text_of_root = text.to_string();
        let display = name.to_string();
        let loader = |want: &str| {
            (want == root).then(|| crate::project::Sources {
                module: Some(text_of_root.clone()),
                interface: None,
                path: Some(display.clone()),
            })
        };
        let project =
            crate::project::check_project(&root, &loader, None, &mut map, &mut diagnostics);
        // The root module's own contract, kept before the project is sliced: it is what separates
        // the names this file declares from the ones it imported, and after linking the program is
        // one namespace in which that difference is no longer visible.
        let published: Vec<String> = project
            .as_ref()
            .map(|p| {
                p.interface
                    .items
                    .iter()
                    .map(|i| i.name.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let placed = project
            .and_then(|p| crate::project::slice_or_library(p, &mut diagnostics))
            .filter(|_| !diagnostics.has_errors());

        let mut names = BTreeMap::new();
        if let Some(placed) = placed.as_ref() {
            let program = &placed.program;
            let symbol = |item: &crate::iface::Item, span: Option<(u32, u32)>, own: bool| Symbol {
                span,
                signature: render_item(item).trim_end().to_string(),
                kind: match item.kind {
                    Kind::Signal { .. } => SymbolKind::Signal,
                    Kind::Function { .. } => SymbolKind::Function,
                },
                tier: item.tier.name().to_string(),
                doc: program.docs.get(&*item.name).map(|d| d.to_string()),
                own,
            };

            // Every name the linked program has, described by the signature `beck iface` would
            // publish for it — so an editor cannot show a signature the compiler would not. The
            // ones this file *declares* carry the span the checker recorded; an imported name's
            // declaration is in another module, and a jump that landed on a byte range of this
            // document would point at the wrong file.
            for item in &crate::iface::Interface::of(program).items {
                let own = published.iter().any(|n| *n == *item.name);
                let span = own
                    .then(|| {
                        program
                            .defs
                            .get(&item.name)
                            .map(|d| (d.span.start, d.span.end))
                            .or_else(|| {
                                program
                                    .signals
                                    .iter()
                                    .find(|s| s.name == item.name)
                                    .map(|s| (s.span.start, s.span.end))
                            })
                    })
                    .flatten();
                names.insert(item.name.to_string(), symbol(item, span, own));
            }
        }

        Editor {
            text: text.to_string(),
            placed,
            diagnostics,
            map,
            names,
            stale: false,
        }
    }

    /// The name table alone, for a caller that wants to keep it and not the analysis.
    ///
    /// An [`Editor`] holds a checked program; an [`Index`] holds strings. A server keeping one per
    /// open document keeps this one.
    pub fn index(&self) -> Index {
        Index {
            names: self.names.clone(),
        }
    }

    /// Borrow the previous analysis's names, for text that does not check.
    ///
    /// A half-typed name is an unresolved name, so the most common state of a file being written
    /// in is the state that has no program and therefore no name table. An editor that answered
    /// nothing there would answer nothing exactly when it was being asked, so a consumer keeps its
    /// last analysis and hands it here.
    ///
    /// What this is **not** is a stale answer presented as a current one: the names are marked
    /// [`stale`](Editor::stale), the diagnostics are always this text's, and nothing else is
    /// carried over. [`docs/98`](../../../../../docs/98-playground-report.md) §98.1's rule — a
    /// stale table beside a red error teaches somebody something false — is about *derived
    /// answers* like a placement table, and this is a completion list; the difference is that the
    /// consumer is told.
    pub fn completing_from(mut self, previous: &Index) -> Editor {
        if self.names.is_empty() && !previous.names.is_empty() {
            self.names.clone_from(&previous.names);
            self.stale = true;
        }
        self
    }

    /// True when the names came from an earlier text than the one being shown.
    pub fn stale(&self) -> bool {
        self.stale
    }

    /// The checked program, for a caller that wants more than an editor's questions.
    pub fn placed(&self) -> Option<&Placed> {
        self.placed.as_ref()
    }

    pub fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }

    pub fn source_map(&self) -> &SourceMap {
        &self.map
    }

    /// Every diagnostic, as a range and the text a terminal would print.
    pub fn marks(&self) -> Vec<Mark> {
        marks(&self.diagnostics)
    }

    /// The names this document declares, in name order — `documentSymbol`'s answer.
    ///
    /// Imported names are excluded: they are not symbols *of* this file, and an outline listing
    /// them would list the standard library under every module that used it.
    pub fn symbols(&self) -> impl Iterator<Item = (&str, &Symbol)> {
        self.names
            .iter()
            .filter(|(_, s)| s.own)
            .map(|(n, s)| (n.as_str(), s))
    }

    pub fn symbol(&self, name: &str) -> Option<&Symbol> {
        self.names.get(name)
    }

    /// What to say about the name under the caret.
    pub fn hover(&self, offset: u32) -> Option<&Symbol> {
        self.names.get(&word_at(&self.text, offset)?)
    }

    /// Where the name under the caret is declared, in this document.
    pub fn definition(&self, offset: u32) -> Option<(u32, u32)> {
        self.hover(offset)?.span
    }

    /// What could finish the word being typed, best-first.
    ///
    /// Names before keywords, because a name is specific to this program and a keyword is not, and
    /// within each group the order is the name order the index already keeps. A caret that is not
    /// in a word offers everything — which is what an editor's "show me what is here" gesture
    /// asks for.
    pub fn completions(&self, offset: u32) -> Vec<Completion> {
        let prefix = prefix_at(&self.text, offset);
        let mut out: Vec<Completion> = Vec::new();
        for (name, symbol) in &self.names {
            if !name.starts_with(&prefix) {
                continue;
            }
            out.push(Completion {
                label: name.clone(),
                detail: symbol.signature.clone(),
                kind: match symbol.kind {
                    SymbolKind::Function => CompletionKind::Function,
                    SymbolKind::Signal => CompletionKind::Signal,
                },
                doc: symbol.doc.clone(),
            });
        }
        for keyword in KEYWORDS {
            if keyword.starts_with(&prefix) {
                out.push(Completion {
                    label: (*keyword).to_string(),
                    detail: String::new(),
                    kind: CompletionKind::Keyword,
                    doc: None,
                });
            }
        }
        out
    }

    /// The word a completion would replace, so a client can send an edit rather than a guess.
    pub fn prefix(&self, offset: u32) -> String {
        prefix_at(&self.text, offset)
    }
}

/// Diagnostics as an editor draws them, for a caller that has the diagnostics and not an
/// [`Editor`] — the playground's analysis has already compiled the text and is not going to do it
/// twice.
///
/// A zero-width span is widened to one character: a caret with nothing under it is a squiggle
/// nobody can see, and "the compiler pointed at the end of the file" is a thing that happens.
pub fn marks(diagnostics: &Diagnostics) -> Vec<Mark> {
    diagnostics
        .iter()
        .map(|d| Mark {
            start: d.primary.start,
            end: d.primary.end.max(d.primary.start + 1),
            error: d.severity == Severity::Error,
            code: d.code.to_string(),
            message: message_of(d),
        })
        .collect()
}

/// The message an editor shows, with the notes the terminal renderer would have printed.
///
/// A `B0350` that says only "cannot find `foo`" is a worse diagnostic in an editor than in a
/// terminal, because the editor drops everything the terminal put underneath it. The notes carry
/// the fix suggestion §3.4 insists on, so they travel.
pub fn message_of(d: &Diagnostic) -> String {
    let mut out = d.message.clone();
    for note in &d.notes {
        out.push_str("\n\nnote: ");
        out.push_str(note);
    }
    if let Some(fix) = &d.fix {
        out.push_str("\n\nhelp: ");
        out.push_str(fix);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------------------------

/// A byte offset as a zero-based line and **UTF-16** character offset.
///
/// UTF-16 because that is what LSP specifies by default, and getting it wrong is invisible until
/// somebody writes an emoji in a string literal — which `beck-syntax`'s own security tests say they
/// will. `SourceMap::line_col` counts *characters* and is one-based, so it is the wrong unit twice
/// over and is deliberately not used here.
pub fn utf16_position(text: &str, offset: u32) -> (u32, u32) {
    let offset = (offset as usize).min(text.len());
    let mut line = 0u32;
    let mut character = 0u32;
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            character = 0;
        } else {
            character += c.len_utf16() as u32;
        }
    }
    (line, character)
}

/// A byte offset as a **UTF-16** offset from the start of the text.
///
/// The flat version of [`utf16_position`], for an editor that works in offsets rather than in
/// lines: a browser's `<textarea>` counts its value in UTF-16 code units, so a span the compiler
/// gave in bytes lands in the wrong place the first time somebody writes an emoji in a string.
/// Neither side guesses — the module that has the text does the conversion.
pub fn utf16_offset(text: &str, byte: u32) -> u32 {
    let byte = (byte as usize).min(text.len());
    text[..byte].chars().map(|c| c.len_utf16() as u32).sum()
}

/// The inverse: a UTF-16 offset back to a byte offset.
pub fn byte_of_utf16(text: &str, utf16: u32) -> u32 {
    let mut counted = 0u32;
    for (i, c) in text.char_indices() {
        if counted >= utf16 {
            return i as u32;
        }
        counted += c.len_utf16() as u32;
    }
    text.len() as u32
}

/// The inverse of [`utf16_position`]: a line and UTF-16 character back to a byte offset.
pub fn byte_offset(text: &str, line: u32, character: u32) -> Option<u32> {
    let mut at_line = 0u32;
    let mut utf16 = 0u32;
    for (i, c) in text.char_indices() {
        if at_line == line && utf16 == character {
            return Some(i as u32);
        }
        if c == '\n' {
            if at_line == line {
                // The position is past the end of its line, which a client may legitimately send
                // when the cursor sits after the last character.
                return Some(i as u32);
            }
            at_line += 1;
            utf16 = 0;
        } else if at_line == line {
            utf16 += c.len_utf16() as u32;
        }
    }
    (at_line == line).then_some(text.len() as u32)
}

/// The identifier the cursor is inside or immediately after.
///
/// "Immediately after" matters: an editor sends the position of the caret, and a caret at the end
/// of `total` is one byte past the `l`. A server that only looked at the byte under the caret would
/// answer nothing for the most common way of asking.
pub fn word_at(text: &str, offset: u32) -> Option<String> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let bytes = text.as_bytes();
    let mut at = (offset as usize).min(bytes.len());
    if at > 0 && (at == bytes.len() || !is_word(text[at..].chars().next()?)) {
        at -= 1;
    }
    if !text.is_char_boundary(at) || !is_word(text[at..].chars().next()?) {
        return None;
    }
    let mut start = at;
    while start > 0 {
        let prev = text[..start].char_indices().next_back()?;
        if !is_word(prev.1) {
            break;
        }
        start = prev.0;
    }
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map(|(i, _)| at + i)
        .unwrap_or(text.len());
    Some(text[start..end].to_string())
}

/// The word characters immediately *before* the caret — what a completion is filtering on.
///
/// Not [`word_at`]: a caret in the middle of `to|tal` completes on `to`, because the rest of the
/// word is what the person is about to replace, and offering only names that start with `total`
/// would answer a question they have not finished asking.
fn prefix_at(text: &str, offset: u32) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut end = (offset as usize).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let Some(prev) = text[..start].char_indices().next_back() else {
            break;
        };
        if !is_word(prev.1) {
            break;
        }
        start = prev.0;
    }
    text[start..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_utf16_and_round_trip() {
        // The byte, character and UTF-16 counts all differ on this line, which is the only way to
        // tell a correct implementation from one that happens to agree on ASCII.
        let text = "def f() -> Str:\n    return \"🎈 x\"\n";
        let balloon = text.find('🎈').expect("the emoji is there") as u32;
        assert_eq!(utf16_position(text, balloon), (1, 12));
        assert_eq!(byte_offset(text, 1, 12), Some(balloon));
        // One position past the emoji is *two* UTF-16 units later, not one.
        assert_eq!(byte_offset(text, 1, 14), Some(balloon + 4));

        // And the flat form a `<textarea>` counts in. The balloon is four bytes and two units, so
        // the two numbers differ from here on — which is the whole reason this conversion exists.
        let flat = utf16_offset(text, balloon);
        assert_eq!(flat, text[..balloon as usize].chars().count() as u32);
        assert_eq!(byte_of_utf16(text, flat), balloon);
        assert_eq!(byte_of_utf16(text, flat + 2), balloon + 4);
        assert_eq!(utf16_offset(text, balloon + 4), flat + 2);
    }

    #[test]
    fn a_caret_at_either_end_of_a_name_finds_it() {
        let text = "def total(x: Int) -> Int:\n    return x\n";
        let at = text.find("total").expect("it is there") as u32;
        assert_eq!(word_at(text, at).as_deref(), Some("total"));
        assert_eq!(word_at(text, at + 2).as_deref(), Some("total"));
        // The caret sits *after* the last character, which is where an editor puts it when you
        // finish typing a name.
        assert_eq!(word_at(text, at + 5).as_deref(), Some("total"));
        // The same rule read from the other side: a caret in the space before `total` is a caret
        // just past `def`, and answering `def` is what "immediately after" means.
        assert_eq!(word_at(text, at - 1).as_deref(), Some("def"));
        // Somewhere no identifier touches on either side finds nothing rather than guessing.
        let arrow = text.find("-> Int").expect("it is there") as u32;
        assert_eq!(word_at(text, arrow + 1), None);
    }

    #[test]
    fn a_prefix_is_what_is_behind_the_caret_and_not_the_whole_word() {
        let text = "def total(x: Int) -> Int:\n    return tot\n";
        let at = text.rfind("tot").expect("it is there") as u32;
        assert_eq!(prefix_at(text, at + 3), "tot");
        assert_eq!(prefix_at(text, at + 1), "t");
        // A caret against whitespace completes on nothing, which is how "show me everything" is
        // asked for. `at` itself is that position: the space before `tot`.
        assert_eq!(prefix_at(text, at), "");
        // And a caret at the end of the word before it completes on *that* word, which is the
        // same rule read from the other side.
        assert_eq!(prefix_at(text, at - 1), "return");
    }

    #[test]
    fn a_comment_is_coloured_even_though_the_lexer_skips_it() {
        let text = "## The answer.\ndef f() -> Int:\n    return 1  # a comment\n";
        let out = tokens(text);
        let kinds: Vec<&str> = out.iter().map(|t| t.kind.name()).collect();
        assert_eq!(kinds.first(), Some(&"doc"), "{kinds:?}");
        assert!(kinds.contains(&"comment"), "{kinds:?}");
        // The `##` line is one token covering exactly that line, and the trailing comment starts
        // at the `#` rather than at the space before it.
        let doc = out[0];
        assert_eq!(
            &text[doc.start as usize..doc.end as usize],
            "## The answer."
        );
        let comment = out
            .iter()
            .find(|t| t.kind == TokenKind::Comment)
            .expect("the comment");
        assert_eq!(
            &text[comment.start as usize..comment.end as usize],
            "# a comment"
        );
        // And `def` is a keyword while `f` is not, which is the whole of what a highlighter is.
        let def = out
            .iter()
            .find(|t| &text[t.start as usize..t.end as usize] == "def");
        assert_eq!(def.map(|t| t.kind), Some(TokenKind::Keyword));
        let f = out
            .iter()
            .find(|t| &text[t.start as usize..t.end as usize] == "f");
        assert_eq!(f.map(|t| t.kind), Some(TokenKind::Name));
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        // The gap scanner never sees it, because the string is a token and the scanner only reads
        // what the lexer left. This is the assertion that says so.
        let text = "def f() -> Str:\n    return \"# not a comment\"\n";
        let out = tokens(text);
        assert!(
            !out.iter().any(|t| t.kind == TokenKind::Comment),
            "{:?}",
            out.iter()
                .map(|t| (&text[t.start as usize..t.end as usize], t.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_token_is_ordered_and_covers_its_own_bytes() {
        let text = "## doc\ndef add(a: Int, b: Int) -> Int:\n    return a + b  # sum\n";
        let out = tokens(text);
        let mut at = 0u32;
        for token in &out {
            assert!(token.start >= at, "{out:?}");
            assert!(token.end > token.start, "{out:?}");
            assert!(token.end as usize <= text.len(), "{out:?}");
            at = token.end;
        }
    }

    #[test]
    fn hover_and_completion_are_the_signature_beck_iface_publishes() {
        let editor = Editor::of(
            "t.beck",
            "## Adds two numbers.\ndef add(a: Int, b: Int) -> Int:\n    return a + b\n",
        );
        assert!(!editor.diagnostics().has_errors());
        let at = "## Adds two numbers.\ndef ad".len() as u32;
        let symbol = editor.hover(at).expect("the name under the caret");
        assert_eq!(symbol.signature, "def add(a: Int, b: Int) -> Int");
        assert_eq!(symbol.doc.as_deref(), Some("Adds two numbers."));
        assert!(symbol.own);
        assert!(symbol.span.is_some());

        // Completion filters on what is behind the caret, and offers the same signature.
        let source = "def add(a: Int, b: Int) -> Int:\n    return add(a, b)\n";
        let editor = Editor::of("t.beck", source);
        let caret = source.rfind("add").expect("it is there") as u32 + 2;
        let offered = editor.completions(caret);
        assert_eq!(
            offered
                .first()
                .map(|c| (c.label.as_str(), c.detail.as_str())),
            Some(("add", "def add(a: Int, b: Int) -> Int"))
        );
    }

    /// The state a file is in while somebody types into it: half a name, and no program.
    ///
    /// Without [`Editor::completing_from`] the answer here is nothing at all, which is the answer
    /// an editor gives exactly when it is being asked. The diagnostics stay this text's — only the
    /// names are borrowed, and they say so.
    #[test]
    fn a_half_typed_name_still_completes_from_the_last_analysis() {
        let good = Editor::of(
            "t.beck",
            "def add(a: Int, b: Int) -> Int:\n    return a + b\n",
        );
        let source =
            "def add(a: Int, b: Int) -> Int:\n    return a + b\ndef g() -> Int:\n    return ad\n";
        let mid_edit = Editor::of("t.beck", source);
        assert!(mid_edit.diagnostics().has_errors());
        // Keywords are a property of the language and are offered whatever the text says; a *name*
        // is what there is nothing to offer from.
        assert!(mid_edit
            .completions(source.len() as u32)
            .iter()
            .all(|c| c.kind == CompletionKind::Keyword));

        let mid_edit = mid_edit.completing_from(&good.index());
        assert!(mid_edit.stale());
        assert!(
            mid_edit.diagnostics().has_errors(),
            "the errors are this text's"
        );
        let caret = source.rfind("ad").expect("it is there") as u32 + 2;
        assert_eq!(
            mid_edit.completions(caret).first().map(|c| c.label.clone()),
            Some("add".to_string())
        );
    }

    #[test]
    fn an_imported_name_is_offered_and_described_but_not_jumped_to() {
        // `word_count` is not declared here, so it has no span in this document — and a jump that
        // landed on a byte range of the interface it came from would point at the wrong file.
        let source = "import text\n\ndef size(s: Str) -> Int:\n    return word_count(s)\n";
        let editor = Editor::of("t.beck", source);
        assert!(
            !editor.diagnostics().has_errors(),
            "{}",
            editor.diagnostics().render(editor.source_map())
        );
        let symbol = editor
            .symbol("word_count")
            .expect("an imported name is indexed");
        assert!(!symbol.own);
        assert_eq!(symbol.span, None);
        assert_eq!(symbol.signature, "def word_count(text: Str) -> Int");
        // It is offered as a completion — which is the point of indexing it at all.
        let caret = source.rfind("word_c").expect("it is there") as u32 + 6;
        assert!(editor
            .completions(caret)
            .iter()
            .any(|c| c.label == "word_count"));
        // And it is not in this file's outline.
        assert!(editor.symbols().all(|(name, _)| name != "word_count"));
    }

    #[test]
    fn a_mark_carries_the_notes_the_terminal_would_have_printed() {
        let editor = Editor::of("x.beck", "def f(x: Int) -> Str:\n    return x\n");
        let marks = editor.marks();
        let mark = marks.first().expect("it does not compile");
        assert!(mark.error);
        assert!(mark.message.contains("expected"), "{}", mark.message);
        assert!(mark.end > mark.start);
    }
}
