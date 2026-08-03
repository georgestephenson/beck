//! UTS #39's conformance vectors, against the profile Beck adopts.
//!
//! [`docs/35-standards-landscape.md`](../../../../docs/35-standards-landscape.md) §35.5 item 2 asks
//! for two things — a pinned Unicode version and "UTS #39's security profile with conformance
//! vectors" — and [`docs/08`](../../../../docs/08-roadmap.md) §8.5.2 dates the cost: expensive "the
//! moment identifiers exist in published packages".
//!
//! The vectors are grouped by the attack each defeats, because that is the only way to tell a
//! conformance suite from a list of strings. Two of the three groups pass **by construction** —
//! Beck's identifiers are ASCII, which is UTS #39's strictest restriction level — and §12.7's
//! vocabulary for that is "unrepresentable by construction, with the test proving it". The third
//! group needed a check written, because no restriction on identifiers touches it.

/// The diagnostic codes a source string produces, through the whole front end.
fn codes(src: &str) -> Vec<&'static str> {
    let (_, d, _) = beck_core::compile_str("vector.beck", src);
    d.iter().map(|x| x.code).collect()
}

/// A fragment is checked for *these* codes, not for zero diagnostics: a bare `def` with no
/// application around it has no signal graph, and B0500 is the slicer saying so rather than
/// anything to do with what the file contains.
fn within_the_profile(src: &str) {
    let found = codes(src);
    assert!(
        !found.contains(&"B0102") && !found.contains(&"B0103"),
        "the profile refused a program it should not have: {found:?} for:\n{src}"
    );
}

fn refused_with(src: &str, code: &str) {
    let found = codes(src);
    assert!(
        found.contains(&code),
        "expected {code}, got {found:?} for:\n{src}"
    );
}

// ---------------------------------------------------------------------------------------------
// 1. Confusables (UTS #39 §4) — two identifiers that look identical and are not
// ---------------------------------------------------------------------------------------------

/// The canonical pair: Latin `a` and Cyrillic `а` (U+0430). In a language with non-ASCII
/// identifiers these are two names that render the same; in Beck the second is not a name at all.
#[test]
fn a_cyrillic_lookalike_is_not_a_second_name() {
    refused_with(
        "def f() -> Int:\n    dat\u{0430} = 1\n    return data\n",
        "B0103",
    );
}

/// Greek omicron for Latin o, and the mathematical alphanumerics — the two other families the
/// confusables data is mostly made of.
#[test]
fn the_other_confusable_families_are_refused_the_same_way() {
    for name in ["c\u{03BF}unt", "\u{1D482}lpha", "\u{FF41}scii"] {
        refused_with(
            &format!("def f() -> Int:\n    {name} = 1\n    return 1\n"),
            "B0103",
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Mixed script (UTS #39 §5) — one identifier drawn from two writing systems
// ---------------------------------------------------------------------------------------------

#[test]
fn a_single_script_identifier_in_another_script_is_refused_too() {
    // Not mixed at all — wholly Cyrillic. Beck's profile is ASCII-Only, so "single-script" is not
    // a defence here, and stating that is the point of the vector.
    refused_with(
        "def f() -> Int:\n    \u{0438}\u{043C}\u{044F} = 1\n    return 1\n",
        "B0103",
    );
}

#[test]
fn an_identifier_mixing_scripts_is_refused() {
    refused_with(
        "def f() -> Int:\n    admin\u{0456}d = 1\n    return 1\n",
        "B0103",
    );
}

// ---------------------------------------------------------------------------------------------
// 3. Bidirectional confusion (UTS #39 §4.1; CVE-2021-42574) — the group that needed a check
// ---------------------------------------------------------------------------------------------

/// The Trojan Source paper's "stretched string" shape: an override inside a literal makes the
/// text after it render in the wrong order, so the comparison a reviewer reads is not the one the
/// program makes.
#[test]
fn a_string_that_renders_backwards_is_refused() {
    refused_with(
        "def f() -> Str:\n    return \"\u{202E}nimda\u{202C}\"\n",
        "B0102",
    );
}

/// The "commenting-out" shape: a comment that appears to end before it does.
#[test]
fn a_comment_that_appears_to_end_early_is_refused() {
    refused_with(
        "def f() -> Int:\n    # \u{2066}return 0;\u{2069}\n    return 1\n",
        "B0102",
    );
}

/// Both surfaces, because the S-expression reader is the same front end. `docs/02` §2.2 calls it
/// canonical, which makes a rule that holds on one notation and not the other worse than no rule.
#[test]
fn the_canonical_surface_is_checked_too() {
    let src = "(def f (typarams) (params) (returns Str) (uses) (do (return \"\u{202E}x\")))";
    let (_, d, _) = beck_core::compile_str("vector.sx", src);
    let found: Vec<&str> = d.iter().map(|x| x.code).collect();
    assert!(found.contains(&"B0102"), "got {found:?}");
}

/// The escape the refusal implies. A program may legitimately want one of these characters as a
/// **value**; spelling it out is the difference between a value and a disguise.
#[test]
fn a_program_that_needs_the_character_can_still_write_it() {
    within_the_profile("def f() -> Str:\n    return \"\\u{202E}\"\n");
}

// ---------------------------------------------------------------------------------------------
// The pin, and the things that must keep working
// ---------------------------------------------------------------------------------------------

/// The version is a statement rather than a dependency today (`beck_syntax::security`'s note), and
/// this asserts it is *stated* — a pin nobody can find is not a pin.
#[test]
fn the_unicode_version_is_pinned_where_a_reader_can_see_it() {
    assert_eq!(beck_syntax::security::UNICODE, "17.0");
}

/// Text is data. Restricting identifiers must not restrict what a program can *say*, and the
/// distinction is the whole reason the profile is about identifiers.
#[test]
fn a_string_may_contain_any_script_at_all() {
    within_the_profile(
        "def greeting() -> Str:\n    return \"\u{3053}\u{3093}\u{306B}\u{3061}\u{306F} \u{1F44B}\"\n",
    );
}

/// Emoji sequences use U+200D, which `security::scan` deliberately does not refuse. Asserted so
/// that the omission stays a decision.
#[test]
fn an_emoji_sequence_is_not_collateral_damage() {
    within_the_profile("def who() -> Str:\n    return \"\u{1F469}\u{200D}\u{1F4BB}\"\n");
}

/// And the corpus, which is the only way to know the check does not refuse real programs.
#[test]
fn every_corpus_program_is_within_the_profile() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus");
    for entry in std::fs::read_dir(dir).expect("the corpus is where the harnesses expect it") {
        let path = entry.expect("a corpus entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("beck") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("a corpus program reads");
        let (_, d, map) = beck_core::compile_str(path.to_string_lossy().as_ref(), &src);
        assert!(
            !d.iter().any(|x| x.code == "B0102" || x.code == "B0103"),
            "{}:\n{}",
            path.display(),
            d.render(&map)
        );
    }
}
