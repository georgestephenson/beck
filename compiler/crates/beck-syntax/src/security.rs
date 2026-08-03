//! What a source file may contain, before anything tries to read it.
//!
//! [`docs/35-standards-landscape.md`](../../../../../docs/35-standards-landscape.md) §35.5 item 2:
//! "pin the Unicode version per release; add UTS #39's security profile with conformance vectors".
//! [`docs/08-roadmap.md`](../../../../../docs/08-roadmap.md) §8.5.2 classes it **R** — a retrofit that
//! becomes expensive "the moment identifiers exist in published packages", which is before the
//! registry and therefore now.
//!
//! # The profile Beck adopts
//!
//! UTS #39 defines *restriction levels* for identifiers. Beck is at **Level 1, ASCII-Only**, and it
//! is there by construction rather than by filtering: the Python surface's identifier production is
//! `[A-Za-z_][A-Za-z0-9_]*` and always has been. That is the strictest level in the report, and it
//! makes the two attacks UTS #39 is mostly about — confusables and mixed-script identifiers —
//! *unrepresentable* rather than checked. §12.7's vocabulary for that distinction is
//! "unrepresentable by construction", and `identifiers.rs` is the negative test proving it.
//!
//! What ASCII-only identifiers do **not** close is the other half of UTS #39 §4: **bidirectional
//! confusion**, where a file renders in an editor differently from how it compiles. That is
//! Trojan Source (CVE-2021-42574), it works through comments and string literals rather than
//! identifiers, and no restriction on identifiers touches it. [`scan`] is that check.
//!
//! # Why the version pin is one line
//!
//! The compiler carries no Unicode tables: an ASCII-only profile needs none, and the character
//! classes below are stable properties of characters that were assigned decades ago. [`UNICODE`] is
//! therefore a statement of *which version these rules were written against* rather than a
//! dependency — and the day Beck accepts a non-ASCII identifier, that constant stops being a note
//! and starts being a thing with tables behind it.

use beck_diag::{Diagnostic, Diagnostics, FileId, Span};

/// The Unicode version this file's rules are stated against.
///
/// Pinned per release, per §35.5 item 2. See the module note: today this is a statement, not a
/// dependency, and the difference is worth keeping visible.
pub const UNICODE: &str = "17.0";

/// The bidirectional formatting characters, refused anywhere in a source file.
///
/// These are the twelve from UTS #39 §4.1 and from the Trojan Source paper. Every one of them
/// changes how following text is *displayed* without changing what it *is*, which is exactly the
/// property that lets a reviewer read one program and a compiler read another.
///
/// They are refused in string literals too, and `\u{...}` is how a program that genuinely needs one
/// writes it — a legitimate use is a runtime *value*, and a value spelled with an escape is a value
/// a reviewer can see.
const BIDI: &[(char, &str)] = &[
    ('\u{202A}', "LEFT-TO-RIGHT EMBEDDING"),
    ('\u{202B}', "RIGHT-TO-LEFT EMBEDDING"),
    ('\u{202C}', "POP DIRECTIONAL FORMATTING"),
    ('\u{202D}', "LEFT-TO-RIGHT OVERRIDE"),
    ('\u{202E}', "RIGHT-TO-LEFT OVERRIDE"),
    ('\u{2066}', "LEFT-TO-RIGHT ISOLATE"),
    ('\u{2067}', "RIGHT-TO-LEFT ISOLATE"),
    ('\u{2068}', "FIRST STRONG ISOLATE"),
    ('\u{2069}', "POP DIRECTIONAL ISOLATE"),
    ('\u{061C}', "ARABIC LETTER MARK"),
    ('\u{200E}', "LEFT-TO-RIGHT MARK"),
    ('\u{200F}', "RIGHT-TO-LEFT MARK"),
];

/// Check a source file before either surface reads it.
///
/// One place, both surfaces, because the S-expression reader is the same front end and a rule that
/// holds on one notation and not the other is not a rule
/// ([`adr/0012`](../../../../../docs/adr/0012-the-front-end-counts-its-own-recursion.md) makes the
/// same argument about a different bound).
///
/// Zero-width *joiners* are deliberately not here. U+200D is how an emoji sequence is spelled, a
/// string is data, and with identifiers already restricted to ASCII there is no confusable
/// identifier for an invisible character to help build. A rule with no attack behind it is a rule
/// somebody will eventually be forced to work around.
pub fn scan(file: FileId, src: &str, diags: &mut Diagnostics) {
    for (offset, c) in src.char_indices() {
        // A byte-order mark is conventional at the start of a file and a zero-width no-break space
        // anywhere else, so the position is the whole difference.
        if c == '\u{FEFF}' && offset > 0 {
            diags.push(
                Diagnostic::error(
                    "B0102",
                    "a zero-width no-break space in the source",
                    Span::new(file, offset..offset + c.len_utf8()),
                )
                .with_primary_label("U+FEFF, which is invisible here")
                .with_note(
                    "a byte-order mark is only a byte-order mark at the very start of a file"
                        .to_string(),
                ),
            );
            continue;
        }
        let Some((_, name)) = BIDI.iter().find(|(b, _)| *b == c) else {
            continue;
        };
        diags.push(
            Diagnostic::error(
                "B0102",
                "a bidirectional control character in the source",
                Span::new(file, offset..offset + c.len_utf8()),
            )
            .with_primary_label(format!("U+{:04X} {name}, which is invisible", c as u32))
            .with_note(
                "these characters change how the text after them is displayed without changing \
                 what it means, so a reviewer and the compiler can read the same file differently \
                 (CVE-2021-42574). Beck adopts UTS #39's profile and refuses them; write \
                 `\\u{...}` if a string genuinely needs one"
                    .to_string(),
            ),
        );
    }
}

/// Whether a character would be an identifier character in some language but is not one in Beck.
///
/// Used only to turn "unrecognised character" into a diagnostic that says *why*: a Cyrillic `а` in
/// an identifier is not a typo, it is either a mistake worth naming or an attack, and either way
/// "not a Beck token" is the least useful thing to say about it.
pub fn is_non_ascii_letter(c: char) -> bool {
    !c.is_ascii() && (c.is_alphabetic() || c.is_numeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_diag::SourceMap;

    fn codes(src: &str) -> Vec<&'static str> {
        let mut map = SourceMap::new();
        let f = map.add("t.beck", src);
        let mut d = Diagnostics::new();
        scan(f, src, &mut d);
        d.iter().map(|x| x.code).collect()
    }

    /// The Trojan Source vector, in the shape the paper uses: a comment that ends, visually, before
    /// it actually does.
    #[test]
    fn a_comment_that_reorders_itself_is_refused() {
        let src = "def f() -> Int:\n    # \u{202E} return 0 #\n    return 1\n";
        assert_eq!(codes(src), vec!["B0102"]);
    }

    #[test]
    fn a_string_literal_is_not_a_way_round_it() {
        assert_eq!(
            codes("x = \"\u{2066}admin\u{2069}\"\n"),
            vec!["B0102", "B0102"]
        );
    }

    #[test]
    fn a_byte_order_mark_is_fine_at_the_start_and_not_anywhere_else() {
        assert!(codes("\u{FEFF}def f() -> Int:\n    return 1\n").is_empty());
        assert_eq!(
            codes("def f\u{FEFF}() -> Int:\n    return 1\n"),
            vec!["B0102"]
        );
    }

    /// The characters this check deliberately does not refuse, asserted so the omission is a
    /// decision rather than an oversight.
    #[test]
    fn an_emoji_sequence_in_a_string_still_reads() {
        assert!(codes("greeting = \"hello \u{1F469}\u{200D}\u{1F4BB}\"\n").is_empty());
    }
}
