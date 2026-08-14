//! What an editor asks the front end, and the answers, once.
//!
//! [`docs/04-compiler-architecture.md`](../../../../../docs/04-compiler-architecture.md) §4.6 fixes
//! the rule this module exists to keep: *"One binary serves `beck build`, `beck check`, `beck lsp`
//! and `beck explain`; there is no separate language server implementation to drift."* Until now
//! that was true of the *compiler* and not of the editor: `beck lsp` held the indexing, the
//! positions and the word-under-the-caret rule, and anything else wanting them — a playground with
//! a `<textarea>` in it ([`docs/98`](../../../../../docs/98-playground-report.md) §98.9) — had to
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

use crate::check::Def;
use crate::core::{Core, CoreKind};
use crate::iface::{render_item, render_uses, Kind};
use crate::ty::Tier;
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
    /// The ` uses …` clause of that signature, alone — [`render_uses`], the half of it an inlay
    /// hint offers where the source did not write one. Empty for a name that performs nothing.
    pub uses: String,
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
    /// What the document is called, so a rename can re-analyse the text it proposes to write.
    name: String,
    text: String,
    /// Which file in [`Editor::map`] this document *is*.
    ///
    /// An [`Editor`] holds a whole linked project — the standard library included — so a span it
    /// can reach is not necessarily a span in this buffer, and two files' byte offsets overlap by
    /// construction. Every answer that turns a span into a range in *this* document checks it
    /// against this first. `None` for a document that did not get as far as being read, where
    /// there is nothing to answer anyway.
    file: Option<beck_diag::FileId>,
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
    /// resolving a relative path off a URI is a decision [`docs/65`](../../../../../docs/65-the-editor-report.md)
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
                uses: render_uses(&item.effects),
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

        // The root module is added to the map before anything it imports, so the first file under
        // this name is this document even when a library module happens to share it.
        let file = map.find(name);
        Editor {
            name: name.to_string(),
            text: text.to_string(),
            file,
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

    // -----------------------------------------------------------------------------------------
    // Occurrences, and the rename built on them
    // -----------------------------------------------------------------------------------------

    /// Every place the name under the caret appears in this document.
    ///
    /// Empty rather than partial when the two accounts of the document disagree — see
    /// [`occurrences`](Editor::occurrences), which is where that rule is.
    pub fn references(&self, offset: u32) -> Vec<Occurrence> {
        let Some(name) = word_at(&self.text, offset) else {
            return Vec::new();
        };
        self.occurrences(&name).unwrap_or_default()
    }

    /// Every place `name` appears, or `None` when this document's two accounts of it disagree.
    ///
    /// # The two accounts, and why both
    ///
    /// The **lexical** account is the token stream: every run of the text that reads
    /// `name`, keywords included. It is complete by construction — the lexer saw the whole file —
    /// and it knows nothing, so a local variable that happens to share the name is in it, and so is
    /// the `page` in `expect page contains "1"`, which is the grammar's word rather than a
    /// reference to the signal of that name.
    ///
    /// The **semantic** account is the checked program: a [`CoreKind::Global`] node per reference,
    /// resolved, so a local of the same name is *not* in it and a name reached through an import
    /// is. It knows everything and is not complete: a reference the checker rewrote — a trait
    /// method resolved to an impl, a macro's expansion — has a span that is a call site rather
    /// than an identifier.
    ///
    /// What makes an *edit* safe is the two agreeing: every semantic reference begins on a lexical
    /// identifier that reads `name`, and the only lexical identifier left over is the
    /// declaration's own. A file where that holds has no shadow, no unspanned mention and no
    /// rewritten reference, and the lexical ranges are then the whole truth about where the name
    /// is — which is also why the *edits* are the lexical ranges and never the spans. Where
    /// it does not hold this answers `None`, and both callers decline rather than edit — because
    /// the alternative to declining is a rename that silently changes what a program means, which
    /// is worse than a rename that does not happen.
    pub fn occurrences(&self, name: &str) -> Option<Vec<Occurrence>> {
        let placed = self.placed.as_ref()?;
        if self.stale {
            return None;
        }
        let symbol = self.names.get(name)?;

        let lexical = self.written(name);
        if lexical.is_empty() {
            return None;
        }

        let mut semantic: Vec<u32> = Vec::new();
        for core in self.own_expressions(placed) {
            self.globals_in(core, name, &mut semantic);
        }
        semantic.extend(self.static_mentions(placed, name));
        semantic.sort_unstable();
        semantic.dedup();
        // Both sides are in source order, so every membership question below is a binary search:
        // a name used a thousand times in a file is not a thousand scans of a thousand tokens.
        let is_reference = |at: &u32| semantic.binary_search(at).is_ok();
        if !semantic
            .iter()
            .all(|at| lexical.binary_search_by_key(at, |(s, _)| *s).is_ok())
        {
            return None;
        }

        // A `test` block's clauses are a grammar of their own — `expect page contains "1"`,
        // `when session("ana") sends …`, `expect state == fold_of […]` — and its words are
        // identifiers to the lexer. `page` there does not name the `page` signal: the runner finds
        // the page by its *type*, so renaming the signal leaves the clause saying what it said.
        // Nothing inside a clause is edited on the strength of a lexical match, and nothing inside
        // one refuses the rename either; the clause's actual expressions are in the semantic
        // account like any others, and get edited from there.
        let grammar: Vec<(u32, u32)> = placed
            .program
            .tests
            .iter()
            .filter(|t| self.owns(t.span))
            .flat_map(|t| t.clause_spans())
            .filter(|s| self.owns(*s))
            .map(|s| (s.start, s.end))
            .collect();
        let left: Vec<(u32, u32)> = lexical
            .iter()
            .copied()
            .filter(|(s, _)| !is_reference(s))
            .filter(|(s, e)| !grammar.iter().any(|(from, to)| s >= from && e <= to))
            .collect();
        // A name this document declares is written once more than it is referred to, and that once
        // is inside its own declaration. An imported name is not written here at all.
        let declaration = match symbol.span {
            Some((start, end)) => match left[..] {
                [only] if only.0 >= start && only.1 <= end => Some(only),
                _ => return None,
            },
            None => {
                if !left.is_empty() {
                    return None;
                }
                None
            }
        };

        let mut out: Vec<Occurrence> = lexical
            .into_iter()
            .filter(|(start, end)| is_reference(start) || Some((*start, *end)) == declaration)
            .map(|(start, end)| Occurrence {
                start,
                end,
                declaration: Some((start, end)) == declaration,
            })
            .collect();
        out.sort_unstable_by_key(|o| o.start);
        Some(out)
    }

    /// Where a rename would edit, or why it will not.
    ///
    /// The edits are [`occurrences`](Editor::occurrences)', and everything else here is a refusal.
    /// [`docs/03`](../../../../../docs/03-type-and-effect-system.md) §3.4's rule for placement — a
    /// compile error with a suggested annotation, never a silent guess — is the same rule this
    /// keeps for an edit: a refusal an author can read beats a rewrite they have to check.
    ///
    /// The last check is the expensive and the decisive one: the proposed text is **analysed**,
    /// and a rename that would not compile is not offered. That costs one more compile of one file
    /// — [`docs/64`](../../../../../docs/64-compile-speed-report.md) §64.6's 4.7 ms at the worst
    /// file in this tree — on a keystroke nobody types twice a minute, and it is what turns the
    /// reasoning above into a fact about the text rather than an argument about the IR.
    pub fn rename(&self, offset: u32, to: &str) -> Result<Vec<Occurrence>, Refusal> {
        if self.placed.is_none() || self.stale {
            return Err(Refusal::Broken);
        }
        let name = word_at(&self.text, offset).ok_or(Refusal::NotAName)?;
        let symbol = self.names.get(&name).ok_or(Refusal::NotAName)?;
        if !symbol.own || symbol.span.is_none() {
            return Err(Refusal::Imported(name));
        }
        if !is_name(to) {
            return Err(Refusal::NotAnIdentifier(to.to_string()));
        }
        // Anything already written under that name, whether or not the checker resolved it: a
        // top-level name, a parameter, a binding, a type. `occurrences` would notice the collision
        // for a global and could not for a local — a body's `let` keeps no name past the checker —
        // so the question asked here is the lexical one, which needs no resolution to answer.
        if self.names.contains_key(to) || !self.written(to).is_empty() {
            return Err(Refusal::Taken(to.to_string()));
        }
        let edits = self.occurrences(&name).ok_or(Refusal::Unaccounted(name))?;

        let mut proposed = self.text.clone();
        for edit in edits.iter().rev() {
            proposed.replace_range(edit.start as usize..edit.end as usize, to);
        }
        let after = Editor::of(&self.name, &proposed);
        if let Some(broken) = after
            .diagnostics()
            .iter()
            .find(|d| d.severity == Severity::Error)
        {
            return Err(Refusal::WouldNotCompile {
                code: broken.code.to_string(),
                message: broken.message.clone(),
            });
        }
        // Compiling is not enough on its own. A module with no merge point is a *library* rather
        // than an error ([`crate::project::slice_or_library`]), so a rename that cost a program its
        // page or its fold would pass the check above while quietly demoting an application to a
        // module that no longer runs.
        let kind = |e: &Editor| e.placed().map(|p| p.is_application());
        if kind(&after) != kind(self) {
            return Err(Refusal::WouldStopBeingAnApplication);
        }
        Ok(edits)
    }

    // -----------------------------------------------------------------------------------------
    // Inlay hints
    // -----------------------------------------------------------------------------------------

    /// What the compiler worked out that the source does not say, where it could be written down.
    ///
    /// The two inferred halves of a Beck signature, and only those: **where a definition runs**,
    /// which §3.4 makes a solved constraint rather than an annotation, and **what it performs**,
    /// which §3.6 makes an inferred row a boundary later has to declare. A name whose source
    /// already carries the annotation gets no hint for it — an inlay hint repeating what is on the
    /// line beside it is noise, and the point of these is that they are the part nobody wrote.
    ///
    /// Every label is what an author could paste in at the offset it carries, which is why the
    /// effect hint is rendered by [`render_uses`] and the tier hint reads `@on(...)`: a hint you
    /// can accept is worth more than a hint you have to translate.
    pub fn hints(&self) -> Vec<Hint> {
        let Some(placed) = self.placed.as_ref() else {
            return Vec::new();
        };
        let program = &placed.program;
        let lexed = tokens(&self.text);
        let mut out: Vec<Hint> = Vec::new();
        for name in &program.def_order {
            let Some(def) = program.defs.get(name) else {
                continue;
            };
            // A `.becki` line has no body to place and no row to infer: it is the declaration.
            if def.is_declaration || !self.owns(def.span) {
                continue;
            }
            if let Some(hint) = self.tier_hint(def.tier, def.tier_is_written, def.span) {
                out.push(hint);
            }
            if !def.row_is_declared {
                let uses = self
                    .names
                    .get(&**name)
                    .map(|s| s.uses.clone())
                    .unwrap_or_default();
                if let (false, Some(offset)) = (uses.is_empty(), self.signature_end(def, &lexed)) {
                    out.push(Hint {
                        offset,
                        label: uses,
                        kind: HintKind::Effects,
                    });
                }
            }
        }
        for signal in &program.signals {
            if let Some(hint) = self.tier_hint(signal.tier, signal.tier_is_written, signal.span) {
                out.push(hint);
            }
        }
        out.sort_by_key(|h| h.offset);
        out
    }

    /// The `@on(...)` a declaration did not write, where there is one to show.
    ///
    /// [`Tier::Any`] is not one. It is what §3.3 calls *unplaced* — pure code, compiled to every
    /// tier that needs it — so it is the absence of a placement rather than a placement, and a
    /// library whose every helper carried `@on(any)` would be a file of hints saying nothing. What
    /// this shows is the answer to "where does this end up", asked where that has an answer.
    fn tier_hint(&self, tier: Tier, written: bool, span: beck_diag::Span) -> Option<Hint> {
        (!written && tier != Tier::Any && self.owns(span)).then(|| Hint {
            offset: span.start,
            label: format!("@on({})", tier.name()),
            kind: HintKind::Tier,
        })
    }

    /// The colon that ends a definition's signature — where a `uses` clause would be written.
    ///
    /// A signature contains colons of its own, one per parameter, so it is the first colon at
    /// **bracket depth zero** rather than the first colon: `def f(x: Int) -> Int:` has two, and an
    /// offset at the earlier one would put the clause in the middle of the parameter list. Over
    /// the token stream rather than over the text, so a `:` inside a string or a comment is not
    /// one — the same reason [`tokens`] exists at all.
    ///
    /// Not found from the body's span, which is where this was first written and wrong: a body's
    /// first expression starts *after* the `return` that introduces it, so the text between the
    /// colon and the span is a keyword rather than whitespace and there is nothing there to
    /// recognise the colon by.
    ///
    /// The tokens are in source order, so the declaration's own are found by binary search and the
    /// scan stops at the colon. Filtering the whole stream per definition would have made hinting a
    /// file cost `definitions × tokens`, which is the quadratic this codebase keeps finding in
    /// exactly this shape ([`docs/64`](../../../../../docs/64-compile-speed-report.md) §64.2).
    fn signature_end(&self, def: &Def, tokens: &[Token]) -> Option<u32> {
        let from = tokens.partition_point(|t| t.start < def.span.start);
        let mut depth = 0i32;
        for token in tokens[from..].iter().take_while(|t| t.end <= def.span.end) {
            if token.kind != TokenKind::Punct {
                continue;
            }
            match self.text.get(token.start as usize..token.end as usize)? {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                ":" if depth == 0 => return Some(token.start),
                _ => {}
            }
        }
        None
    }

    /// Every run of this document that reads as the word `name`, in source order.
    ///
    /// A **keyword** counts, and that is not an oversight: `page`, `state`, `events` and `session`
    /// are words the parser reads as syntax inside a `test` block and perfectly ordinary names for
    /// a signal outside one — `page: Signal[Html] = per_session(count, view)` is in nearly every
    /// program in [`corpus/`](../../../../corpus). Reading only [`TokenKind::Name`] meant the most
    /// common name in the language had *no* occurrences at all and every question about it was
    /// declined. What separates the two uses is not the token, it is where it sits: the grammar's
    /// own words are inside a clause, and [`occurrences`](Editor::occurrences) drops those.
    fn written(&self, name: &str) -> Vec<(u32, u32)> {
        tokens(&self.text)
            .into_iter()
            .filter(|t| matches!(t.kind, TokenKind::Name | TokenKind::Keyword))
            .map(|t| (t.start, t.end))
            .filter(|(s, e)| self.text.get(*s as usize..*e as usize) == Some(name))
            .collect()
    }

    /// True for a span that is a range of *this* document.
    fn owns(&self, span: beck_diag::Span) -> bool {
        !span.is_none() && Some(span.file) == self.file
    }

    /// Every expression this document wrote: the bodies it declares, its signals, and its tests.
    ///
    /// Tests are in the list because a name used only by a `test` block is used — this is the
    /// walk [`docs/70`](../../../../../docs/70-the-evaluator-gets-fast-report.md) found three
    /// passes had been missing — and a rename blind to them would edit a program into one that no
    /// longer compiles.
    fn own_expressions<'a>(&'a self, placed: &'a Placed) -> Vec<&'a Core> {
        let program = &placed.program;
        let mut out: Vec<&Core> = Vec::new();
        for def in program.defs.values() {
            if self.owns(def.span) {
                out.push(&def.body);
            }
        }
        for signal in &program.signals {
            if self.owns(signal.span) {
                out.push(&signal.expr);
            }
        }
        for test in &program.tests {
            if self.owns(test.span) {
                out.extend(test.cores());
            }
        }
        out
    }

    /// Where a static expectation names `name` — `expect place(page) == client`.
    ///
    /// Not in [`own_expressions`](Editor::own_expressions), because it is not an expression:
    /// [`docs/21`](../../../../../docs/21-tests-in-beck-and-proof.md) §21.2's static assertions are
    /// answered from the placement table without running anything, so the name in one is a
    /// reference the checker resolves and keeps no [`Core`] node for. It is still a use of the
    /// name, and it was the one thing in the corpus that a rename could not account for: `page` is
    /// the name most Beck programs assert about, and 48 of them declined until this was here
    /// ([`docs/65`](../../../../../docs/65-the-editor-report.md) §65.6).
    fn static_mentions(&self, placed: &Placed, name: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for test in &placed.program.tests {
            if !self.owns(test.span) {
                continue;
            }
            for clause in &test.clauses {
                if let crate::testing::Clause::Expect {
                    what:
                        crate::testing::Expectation::Place {
                            what, what_span, ..
                        },
                    ..
                } = clause
                {
                    if &**what == name && self.owns(*what_span) {
                        out.push(what_span.start);
                    }
                }
            }
        }
        out
    }

    /// Where each reference to `name` in an expression tree **begins**.
    ///
    /// The start rather than the range, because a reference that is called carries the span of the
    /// *call*: `double(x)` is one node spanning the parentheses and their contents, and the name
    /// is its first token. Which token that is, is the lexical account's question — this one only
    /// says that the checker resolved a reference to `name` starting there.
    fn globals_in(&self, core: &Core, name: &str, out: &mut Vec<u32>) {
        if let CoreKind::Global(global) = &core.kind {
            if &**global == name && self.owns(core.span) {
                out.push(core.span.start);
            }
        }
        for child in crate::core::children(core) {
            self.globals_in(child, name, out);
        }
    }
}

/// One place a name appears in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurrence {
    pub start: u32,
    pub end: u32,
    /// True for the one that declares it, so a client can mark it as the write among the reads.
    pub declaration: bool,
}

/// Why a rename will not happen.
///
/// A variant per reason rather than one string, because the caller renders them: a language server
/// puts them in an error response and a browser tab puts them beside the box somebody typed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The document does not currently compile, so there is no program to rename in.
    Broken,
    /// There is no name under the caret.
    NotAName,
    /// The name is declared in another module, and this editor is not showing that file.
    Imported(String),
    /// The new name is not one the lexer would read as an identifier.
    NotAnIdentifier(String),
    /// Something in this document is already written under the new name.
    Taken(String),
    /// The name is used in a way this document's analysis cannot account for — see
    /// [`Editor::occurrences`].
    Unaccounted(String),
    /// The edit was made and the result does not compile.
    WouldNotCompile { code: String, message: String },
    /// The edit was made, the result compiles, and it is no longer an application.
    WouldStopBeingAnApplication,
}

impl Refusal {
    /// The sentence a person reads, in the terms they typed in.
    pub fn message(&self) -> String {
        match self {
            Refusal::Broken => {
                "this file does not compile, so there is nothing to rename in it yet".to_string()
            }
            Refusal::NotAName => "there is no name under the cursor".to_string(),
            Refusal::Imported(name) => {
                format!("`{name}` is declared in another module, which this file cannot edit")
            }
            Refusal::NotAnIdentifier(to) => format!("`{to}` is not a name Beck can read"),
            Refusal::Taken(to) => format!("`{to}` is already used in this file"),
            Refusal::Unaccounted(name) => format!(
                "`{name}` is used somewhere this rename cannot account for — a local of the same \
                 name, or a mention the checker keeps no position for, such as `expect place({name})`"
            ),
            Refusal::WouldNotCompile { code, message } => {
                format!("the renamed file would not compile: {code}: {message}")
            }
            Refusal::WouldStopBeingAnApplication => {
                "the renamed file would still compile, as a library rather than as the \
                 application it is now"
                    .to_string()
            }
        }
    }
}

/// One thing the compiler worked out and the source does not say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// Where it belongs, as a byte offset into the document.
    pub offset: u32,
    /// The label, which is also what could be written at `offset` to say the same thing.
    pub label: String,
    pub kind: HintKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    /// `@on(server)` — where the solver put this definition (§3.4).
    Tier,
    /// ` uses net.out(…)` — the row the checker inferred for it (§3.6).
    Effects,
}

/// True for a word the lexer would read as one identifier.
///
/// The lexer rather than a rule written here, because "what is a name" already has an answer with
/// a Unicode profile behind it ([`beck_syntax::security`], `docs/44` §44.5), and a rename that
/// accepted a name the compiler would refuse — a confusable, a bidirectional control, a keyword —
/// would be a second definition of an identifier in a project that has spent a report on having
/// one.
fn is_name(text: &str) -> bool {
    if text.is_empty() || KEYWORDS.contains(&text) {
        return false;
    }
    let mut map = SourceMap::new();
    let file = map.add("rename.beck", text);
    let mut diags = Diagnostics::new();
    let lexed = lex(file, text, &mut diags);
    if diags.has_errors() {
        return false;
    }
    let mut words = lexed.iter().filter_map(|t| t.raw());
    matches!(
        (words.next(), words.next()),
        (Some(Raw::Ident(word)), None) if word.as_str() == text
    )
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

    // ------------------------------------------------------------------------------------------
    // Occurrences, rename and hints
    // ------------------------------------------------------------------------------------------

    const USED_TWICE: &str = "\
def double(x: Int) -> Int:
    return x * 2

def quadruple(x: Int) -> Int:
    return double(double(x))
";

    fn caret(text: &str, at: &str) -> u32 {
        text.find(at).expect("it is there") as u32
    }

    #[test]
    fn every_use_of_a_name_is_found_and_the_declaration_is_marked() {
        let editor = Editor::of("t.beck", USED_TWICE);
        let found = editor.references(caret(USED_TWICE, "double"));
        assert_eq!(found.len(), 3, "{found:?}");
        // The one inside `def double(…)` is the declaration; the two inside `quadruple` are not.
        assert!(found[0].declaration);
        assert!(found[1..].iter().all(|o| !o.declaration));
        for occurrence in &found {
            assert_eq!(
                &USED_TWICE[occurrence.start as usize..occurrence.end as usize],
                "double"
            );
        }
        // Asked from a use rather than from the declaration, the answer is the same set.
        let from_use = editor.references(caret(USED_TWICE, "double(double"));
        assert_eq!(from_use, found);
    }

    #[test]
    fn a_rename_edits_every_use_and_the_declaration() {
        let editor = Editor::of("t.beck", USED_TWICE);
        let edits = editor
            .rename(caret(USED_TWICE, "double"), "twice")
            .expect("a plain rename");
        let mut renamed = USED_TWICE.to_string();
        for edit in edits.iter().rev() {
            renamed.replace_range(edit.start as usize..edit.end as usize, "twice");
        }
        assert_eq!(
            renamed,
            "def twice(x: Int) -> Int:\n    return x * 2\n\ndef quadruple(x: Int) -> Int:\n    \
             return twice(twice(x))\n"
        );
        // And the thing the edits are *for*: the file still compiles.
        let after = Editor::of("t.beck", &renamed);
        assert!(
            !after.diagnostics().has_errors(),
            "{}",
            after.diagnostics().render(after.source_map())
        );
    }

    #[test]
    fn a_name_used_only_by_a_test_is_still_renamed() {
        // The walk `docs/70` found three passes had been missing. A rename that missed it would
        // edit every definition and leave the `test` block calling a name that no longer exists —
        // and the verification step is what turns that into a refusal rather than a broken file,
        // so this asserts the *edit*, which is the outcome the refusal would have hidden.
        let source = "\
def limit() -> Int:
    return 3

def under(n: Int) -> Bool:
    return n < limit()

test \"the limit holds\":
    expect under(limit() - 1)
";
        let editor = Editor::of("t.beck", source);
        assert!(
            !editor.diagnostics().has_errors(),
            "{}",
            editor.diagnostics().render(editor.source_map())
        );
        let edits = editor
            .rename(caret(source, "limit"), "ceiling")
            .expect("a name a test uses is renameable");
        assert_eq!(edits.len(), 3, "{edits:?}");
        assert!(
            edits.iter().any(|e| e.start > caret(source, "test ")),
            "the use inside the test block is edited too: {edits:?}"
        );
    }

    #[test]
    fn a_local_of_the_same_name_stops_the_rename_rather_than_capturing_it() {
        // The failure this rules out is silent: `total` the parameter and `total` the definition
        // are different bindings, the lexer cannot tell them apart, and an edit that renamed both
        // would change what the body means while still compiling.
        let source = "\
def total(x: Int) -> Int:
    return x + 1

def report(total: Int) -> Int:
    return total + 1
";
        let editor = Editor::of("t.beck", source);
        assert!(
            !editor.diagnostics().has_errors(),
            "{}",
            editor.diagnostics().render(editor.source_map())
        );
        assert_eq!(
            editor.rename(caret(source, "total"), "amount"),
            Err(Refusal::Unaccounted("total".to_string()))
        );
        // And references declines for the same reason rather than reporting the shadow as a use.
        assert!(editor.references(caret(source, "total")).is_empty());
    }

    #[test]
    fn a_rename_onto_a_name_that_is_taken_is_refused() {
        let editor = Editor::of("t.beck", USED_TWICE);
        assert_eq!(
            editor.rename(caret(USED_TWICE, "double"), "quadruple"),
            Err(Refusal::Taken("quadruple".to_string()))
        );
        // Including a name that is only a *parameter* — which is not in the name table, so the
        // check that catches it is the lexical one.
        assert_eq!(
            editor.rename(caret(USED_TWICE, "double"), "x"),
            Err(Refusal::Taken("x".to_string()))
        );
    }

    #[test]
    fn a_new_name_is_one_the_lexer_would_read() {
        let editor = Editor::of("t.beck", USED_TWICE);
        for bad in ["", "2fast", "with space", "def", "a-b", "🎈"] {
            assert!(
                matches!(
                    editor.rename(caret(USED_TWICE, "double"), bad),
                    Err(Refusal::NotAnIdentifier(_))
                ),
                "`{bad}` is not a name Beck can read"
            );
        }
        assert!(editor
            .rename(caret(USED_TWICE, "double"), "twice_over")
            .is_ok());
    }

    #[test]
    fn an_imported_name_and_a_broken_file_are_both_refused() {
        let source = "import text\n\ndef size(s: Str) -> Int:\n    return word_count(s)\n";
        let editor = Editor::of("t.beck", source);
        assert_eq!(
            editor.rename(caret(source, "word_count"), "words"),
            Err(Refusal::Imported("word_count".to_string()))
        );

        let broken = Editor::of("t.beck", "def f(x: Int) -> Str:\n    return x\n");
        assert_eq!(
            broken.rename(caret("def f(", "f"), "g"),
            Err(Refusal::Broken)
        );
    }

    #[test]
    fn a_hint_is_the_annotation_nobody_wrote() {
        let source = "\
def key() -> secret[Str]:
    return secret_env(\"API_KEY\")

@on(server)
def other() -> secret[Str]:
    return secret_env(\"OTHER_KEY\")

def pure(x: Int) -> Int:
    return x + 1
";
        let editor = Editor::of("t.beck", source);
        assert!(
            !editor.diagnostics().has_errors(),
            "{}",
            editor.diagnostics().render(editor.source_map())
        );
        let tiers: Vec<Hint> = editor
            .hints()
            .into_iter()
            .filter(|h| h.kind == HintKind::Tier)
            .collect();
        // `key` is placed and does not say so. `other` says so. `pure` is unplaced, and "anywhere"
        // is not a placement worth writing on the line.
        assert_eq!(tiers.len(), 1, "{tiers:?}");
        assert_eq!(tiers[0].offset, caret(source, "def key"));
        assert_eq!(tiers[0].label, "@on(server)");

        // And it is the annotation the source would have carried: written in, it still compiles,
        // and it no longer hints.
        let mut written = source.to_string();
        written.insert_str(tiers[0].offset as usize, "@on(server)\n");
        let after = Editor::of("t.beck", &written);
        assert!(
            !after.diagnostics().has_errors(),
            "{written}\n{}",
            after.diagnostics().render(after.source_map())
        );
        assert!(after.hints().iter().all(|h| h.kind != HintKind::Tier));
    }

    #[test]
    fn an_inferred_row_is_hinted_where_the_signature_would_carry_it() {
        // Two properties in one, and the second is the interesting one: the offset is the colon
        // that ends the signature, so pasting the label in writes a signature that parses.
        let source = "\
def stamp() -> Int uses nondet:
    return now()

def later() -> Int:
    return stamp() + 1000
";
        let editor = Editor::of("t.beck", source);
        assert!(
            !editor.diagnostics().has_errors(),
            "{}",
            editor.diagnostics().render(editor.source_map())
        );
        let hint = editor
            .hints()
            .into_iter()
            .find(|h| h.kind == HintKind::Effects)
            .expect("`later` performs what `stamp` performs and does not say so");
        assert_eq!(&source[hint.offset as usize..hint.offset as usize + 1], ":");
        assert!(hint.label.starts_with(" uses "), "{}", hint.label);

        let mut written = source.to_string();
        written.insert_str(hint.offset as usize, &hint.label);
        let after = Editor::of("t.beck", &written);
        assert!(
            !after.diagnostics().has_errors(),
            "a hint you can paste in:\n{written}\n{}",
            after.diagnostics().render(after.source_map())
        );
        // And `stamp`, which declares its row, is not hinted about it a second time.
        assert!(editor
            .hints()
            .iter()
            .all(|h| h.kind != HintKind::Effects || h.offset > caret(source, "def later")));
    }
}
