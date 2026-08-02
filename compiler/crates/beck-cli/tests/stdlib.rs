//! The standard library: the half written in Beck, and the primitives it is written on.
//!
//! [`compiler/lib/README.md`](../../../../lib/README.md) states the division this harness enforces:
//! a host's table or grammar is a primitive, and composition is a file in `lib/` written in the
//! language. That claim is only worth making if the files in `lib/` actually run, so this is where
//! they do.
//!
//! Three things are asserted, in order of what they are worth:
//!
//! 1. **Every file in `lib/` passes its own tests**, through the binary, with no list of file names
//!    in this harness — a file added to that directory is gated by being there.
//! 2. **The primitives behave**, on the edge cases a library actually hits: an index past the end,
//!    an empty separator, a non-finite float, an instant before 1970.
//! 3. **The one wall is still a wall.** `money.beck` was meant to `impl Num`, and could not.
//!    `sicp/refusals/`'s pattern says a wall is a test that starts failing, so it is one.

use std::path::{Path, PathBuf};
use std::process::Command;

fn lib_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate lives two levels under the workspace root")
        .join("lib")
}

fn lib_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(lib_dir())
        .expect("the library directory is where the harness expects it")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
        .collect();
    out.sort();
    assert!(
        out.len() >= 3,
        "the library listing is wrong, not the repo: {out:?}"
    );
    out
}

fn beck(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .args(args)
        .output()
        .expect("the compiler is built");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// Every file in `lib/` runs its own tests and passes them.
///
/// No file names here on purpose: the gate is the directory, so adding a library and forgetting to
/// register it is not a thing that can happen.
#[test]
fn every_library_passes_its_own_tests() {
    for path in lib_files() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["test", &file]);
        assert!(ok, "`beck test {file}`:\n{text}");
        assert!(text.contains("0 failed"), "{file}:\n{text}");
    }
}

/// And each one is a **library** — no merge point, nothing to run.
///
/// Worth asserting separately, because "a library runs its own tests" and "a library is an
/// application" are different claims and [`27`](../../../../docs/27-walls-report.md) removed the
/// wall between the first and the second without merging them.
#[test]
fn each_one_is_a_library_and_beck_check_says_so() {
    for path in lib_files() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["check", &file]);
        assert!(ok, "`beck check {file}`:\n{text}");
        assert!(text.contains("a library"), "{file}:\n{text}");
    }
}

/// The library documents, which is what `##` is for and what a published one would be read from.
#[test]
fn every_library_documents() {
    for path in lib_files() {
        let file = path.to_string_lossy().to_string();
        let (ok, text) = beck(&["doc", "module", &file]);
        assert!(ok, "`beck doc {file}`:\n{text}");
    }
}

// ---------------------------------------------------------------------------------------------
// The primitives, on the edges a library hits
// ---------------------------------------------------------------------------------------------

/// Run a Beck expression and give back what it evaluated to, as the runner prints it.
fn expect_beck(body: &str, name: &str) {
    let src = format!("def check() -> Bool:\n    return true\n\ntest \"probe\":\n{body}\n");
    let file = std::env::temp_dir().join(format!("beck-stdlib-{name}.beck"));
    std::fs::write(&file, &src).expect("a scratch file");
    let (ok, text) = beck(&["test", file.to_string_lossy().as_ref()]);
    let _ = std::fs::remove_file(&file);
    assert!(ok && text.contains("0 failed"), "{src}\n{text}");
}

/// Indices are clamped rather than refused, and the module says why: a slice is not a parse, and
/// `raises` is for a program's own vocabulary rather than for the library's arithmetic.
#[test]
fn a_string_index_past_the_end_is_the_empty_string_and_not_a_failure() {
    expect_beck(
        "    expect str_slice(\"abc\", 10, 5) == \"\"\n\
         \x20   expect str_slice(\"abc\", 0, 100) == \"abc\"\n\
         \x20   expect str_slice(\"abc\", -5, 2) == \"ab\"\n\
         \x20   expect list_get([1, 2], 9) == None\n\
         \x20   expect list_take([1, 2], 99) == [1, 2]\n\
         \x20   expect list_drop([1, 2], 99) == []",
        "clamped",
    );
}

/// Characters, not bytes, and consistently: `str_len`, `str_slice` and `str_index_of` are one unit
/// or they are a trap.
#[test]
fn string_positions_are_characters_everywhere_or_nowhere() {
    expect_beck(
        "    expect str_len(\"héllo\") == 5\n\
         \x20   expect str_slice(\"héllo\", 1, 1) == \"é\"\n\
         \x20   expect str_index_of(\"héllo\", \"llo\") == Some(value=2)\n\
         \x20   expect list_len(str_chars(\"héllo\")) == 5",
        "characters",
    );
}

/// An empty separator splits into characters, which is the only total answer available and what
/// every caller who writes it means.
#[test]
fn an_empty_separator_splits_into_characters() {
    expect_beck(
        "    expect str_split(\"abc\", \"\") == [\"a\", \"b\", \"c\"]\n\
         \x20   expect str_replace(\"abc\", \"\", \"-\") == \"abc\"",
        "empty-sep",
    );
}

/// `str_repeat` is bounded. Fuel is the general answer to "a program asked for too much"; this is
/// the specific one, and it is here rather than nowhere.
#[test]
fn a_repeat_is_bounded_rather_than_fatal() {
    expect_beck(
        "    expect str_len(str_repeat(\"x\", 2000000)) == 1000000\n\
         \x20   expect str_repeat(\"x\", -1) == \"\"",
        "repeat",
    );
}

/// The predicates short-circuit, which is observable when the predicate has effects — so it is a
/// promise and not an optimisation.
#[test]
fn the_predicates_short_circuit() {
    expect_beck(
        "    expect list_any([1, 2, 3], lambda x: x == 1)\n\
         \x20   expect not list_all([1, 2, 3], lambda x: x == 1)\n\
         \x20   expect list_all([], lambda x: false)\n\
         \x20   expect not list_any([], lambda x: true)",
        "shortcircuit",
    );
}

/// Time before the epoch is the second it is *in*, not the one after it. Floor division, which is
/// the bug every hand-rolled formatter has.
#[test]
fn an_instant_before_the_epoch_formats_as_the_second_it_is_in() {
    expect_beck(
        "    expect time_format(0) == \"1970-01-01T00:00:00.000Z\"\n\
         \x20   expect time_format(-1) == \"1969-12-31T23:59:59.999Z\"\n\
         \x20   expect time_format(-86400000) == \"1969-12-31T00:00:00.000Z\"",
        "epoch",
    );
}

/// A leap day, and the century that is *not* a leap year — the two cases the calendar arithmetic
/// gets wrong and a round-trip test would not notice, because a wrong formatter and a wrong parser
/// agree with each other. 2100 is the interesting one: divisible by four, not a leap year.
#[test]
fn the_calendar_gets_the_leap_years_right() {
    expect_beck(
        "    expect str_slice(time_format(951782400000), 0, 10) == \"2000-02-29\"\n\
         \x20   expect str_slice(time_format(4107456000000), 0, 10) == \"2100-02-28\"\n\
         \x20   expect str_slice(time_format(1709164800000), 0, 10) == \"2024-02-29\"",
        "leap",
    );
}

/// An offset is refused rather than silently shifted: two spellings of one instant would be two
/// values, and a log is not where you want to discover that.
#[test]
fn a_time_with_an_offset_is_refused_rather_than_shifted() {
    expect_beck(
        "    expect (try: time_parse(\"2024-01-01T00:00:00+01:00\")) == \
         Err(error=BadTime(why=\"`2024-01-01T00:00:00+01:00` is not an RFC 3339 instant in UTC\"))",
        "offset",
    );
}

// ---------------------------------------------------------------------------------------------
// The wall
// ---------------------------------------------------------------------------------------------

/// A trait's declared row is a bound on every impl, so a fallible operation cannot be a method of a
/// pure trait.
///
/// This is why `lib/money.beck` has `plus` and `minus` rather than `impl Num for Money`, and it is
/// the first thing writing a *library* found that four phases of writing a compiler had not. The
/// fix is a trait whose method signatures carry a row variable — which
/// [`33`](../../../../docs/33-effect-polymorphism-and-list-patterns-report.md) did for a user's
/// higher-order definitions and nothing has done for traits.
///
/// Asserted in `sicp/refusals/`'s pattern: **the day it lands, this test goes red**, and whoever
/// lands it deletes it, deletes four lines of `money.beck`, and corrects
/// [`46`](../../../../docs/46-standard-library-report.md) §46.5 in the same change.
#[test]
fn a_trait_method_may_not_be_more_effectful_than_its_trait() {
    let src = "\
model Money:
    units: Int
    currency: Str

union MoneyError:
    MixedCurrency(left: Str, right: Str)

impl Num for Money:
    def add(self, other):
        if self.currency != other.currency:
            raise MixedCurrency(left=self.currency, right=other.currency)
        return Money(units=self.units + other.units, currency=self.currency)

    def sub(self, other):
        return self

    def mul(self, other):
        return self

    def div(self, other):
        return self
";
    let (_, d, map) = beck_core::check_str("wall.beck", src);
    assert!(
        d.iter().any(|x| x.code == "B0370"),
        "a raising trait method is accepted now — the wall is down, so delete this test, give \
         `lib/money.beck` its `impl Num`, and say so in docs/46 §46.5:\n{}",
        d.render(&map)
    );
}
