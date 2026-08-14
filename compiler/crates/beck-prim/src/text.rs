//! The four text primitives that are somebody else's table or somebody else's parser.
//!
//! Thin — every one of these is a `str::` method — and that is the point rather than an apology.
//! `str_upper` is Unicode's full case mapping, where `ß` uppercases to two characters and `İ`
//! lowercases to two; `str_to_int` is the parser that decides whether `+7`, ` 7` and `007` are
//! numbers. A code generator that emitted an ASCII fold or a digit loop would disagree with the
//! evaluator on the first input somebody's language actually uses, which is exactly what both
//! backends' refusals said. So the answer is not to emit them: it is to call the same function,
//! from one place that the evaluator calls too.

/// Unicode's full uppercase mapping.
pub fn upper(s: &str) -> String {
    s.to_uppercase()
}

/// Unicode's full lowercase mapping.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// Rust's own `i64` parser, and therefore the answer for every input that is not a number.
pub fn to_int(s: &str) -> Option<i64> {
    s.parse::<i64>().ok()
}

/// Every occurrence of `from` in `s`, replaced by `to`.
///
/// An empty needle answers the subject unchanged. `str::replace` would splice `to` between every
/// character instead, and the evaluator has always had this guard — it is a decision about what
/// the primitive means rather than an implementation detail, so it lives with the implementation.
pub fn replace(s: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return s.to_string();
    }
    s.replace(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_mapping_is_the_table_and_not_the_ascii_range() {
        // Each of these is a case a fold over bytes gets wrong: one that grows, one that is not
        // Latin at all, and one whose lowercase is two characters.
        assert_eq!(upper("straße"), "STRASSE");
        assert_eq!(upper("ǳ"), "Ǳ");
        assert_eq!(lower("İ"), "i\u{307}");
        assert_eq!(upper("ΣΟΦΌΣ").len(), "ΣΟΦΌΣ".len());
        // And the final-sigma rule, which is a property of the *position* rather than the letter.
        assert_eq!(lower("ΟΔΟΣ"), "οδος");
    }

    #[test]
    fn the_parser_is_rusts_and_what_it_refuses_is_the_answer() {
        assert_eq!(to_int("7"), Some(7));
        assert_eq!(to_int("+7"), Some(7), "a leading plus is a number");
        assert_eq!(to_int("007"), Some(7));
        assert_eq!(to_int("-9223372036854775808"), Some(i64::MIN));
        for not in [
            "",
            " 7",
            "7 ",
            "7.0",
            "9223372036854775808",
            "0x10",
            "seven",
            "_7",
        ] {
            assert_eq!(to_int(not), None, "{not:?}");
        }
    }

    #[test]
    fn an_empty_needle_leaves_the_subject_alone() {
        assert_eq!(replace("abc", "", "-"), "abc");
        assert_eq!(replace("abc", "b", "-"), "a-c");
        assert_eq!(replace("aaa", "aa", "b"), "ba", "leftmost, non-overlapping");
        assert_eq!(replace("", "a", "b"), "");
    }
}
