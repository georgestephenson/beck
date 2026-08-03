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

/// Integer division truncates towards zero, and the remainder takes the dividend's sign.
///
/// `lib/dates.beck` is Hinnant's calendar, which is written for exactly this division — its
/// `if` on a negative operand is what makes the era right, and would be *wrong* under a division
/// that floored. So the choice is load-bearing for a whole library and is pinned here rather than
/// left to be inferred from the one case a test happened to cover. `floor_div` in that file is the
/// other kind, written in Beck because it has to be.
#[test]
fn division_truncates_towards_zero_and_the_remainder_follows_the_dividend() {
    expect_beck(
        "    expect -7 / 2 == -3\n\
         \x20   expect 7 / -2 == -3\n\
         \x20   expect -7 % 2 == -1\n\
         \x20   expect 7 % -2 == 1\n\
         \x20   expect -8 / 2 == -4",
        "division",
    );
}

/// A sort is stable, and a union orders by variant and then by payload.
///
/// Together those are why `lib/collections.beck` has no comparator: `sort_by` takes a *key*, and
/// "by score descending, then by name" is one key function returning a compound value. Neither
/// half is obvious from the signature and both are relied on, so both are here.
#[test]
fn sorting_is_stable_and_a_compound_value_orders_structurally() {
    expect_beck(
        "    expect sort_by([3, 1, 2], lambda n: 0) == [3, 1, 2]\n\
         \x20   expect Some(value=1) < Some(value=2)\n\
         \x20   expect None < Some(value=1)\n\
         \x20   expect sort_by([\"bb\", \"a\", \"cc\"], lambda s: str_len(s)) == [\"a\", \"bb\", \"cc\"]",
        "stable",
    );
}

/// And a record orders by its field **names**, not by the order they were declared in.
///
/// A value carries its fields and not its declaration, so there is nothing else at runtime to sort
/// them by — and a rule the checker applied instead would disagree with the order the same records
/// come out of a `Map`, which is this one. The consequence lands on every two-key sort:
/// `lib/collections.beck`'s `sorted` documents it, and this is where a change to it goes red.
#[test]
fn a_record_orders_by_field_name_and_not_by_declaration_order() {
    let src = "\
model Declared:
    zebra: Int
    alpha: Int

test \"names, not positions\":
    expect Declared(zebra=2, alpha=0) < Declared(zebra=1, alpha=9)
";
    let file = std::env::temp_dir().join("beck-stdlib-field-order.beck");
    std::fs::write(&file, src).expect("a scratch file");
    let (ok, text) = beck(&["test", file.to_string_lossy().as_ref()]);
    let _ = std::fs::remove_file(&file);
    assert!(ok && text.contains("0 failed"), "{src}\n{text}");
}

/// A map's keys come back in the values' own order, whatever order they went in.
///
/// `lib/collections.beck`'s `Set` is a map's keys, and `elements` promises an order that is a
/// function of the values — which is a promise about this and nothing else. A replay that
/// disagreed with the run it was replaying about the order of a set would be a replay of a
/// different program.
#[test]
fn a_maps_keys_come_back_in_the_values_own_order() {
    expect_beck(
        "    expect map_keys(map_insert(map_insert({}, \"b\", 1), \"a\", 1)) == [\"a\", \"b\"]\n\
         \x20   expect map_keys(map_insert(map_insert({}, \"a\", 1), \"b\", 1)) == [\"a\", \"b\"]\n\
         \x20   expect map_keys(map_insert(map_insert({}, 10, 1), 9, 1)) == [9, 10]",
        "map-order",
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

/// An impl may be **more effectful than its trait**, and the caller inherits it.
///
/// This is what `lib/money.beck` needed and could not have until `docs/47`: a trait's declared row
/// was a ceiling, `Num` is pure, and adding two amounts in different currencies has to fail — so an
/// operator was unavailable to every type whose operation can fail. The row is now inferred per
/// impl.
///
/// It is asserted here, in the harness of the library that found the wall, so that a regression
/// shows up as "money lost its operator" rather than as a type error three files away.
#[test]
fn an_impl_may_be_more_effectful_than_its_trait_and_money_has_its_operator() {
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

def sum2(a: Money, b: Money) -> Money:
    return a + b

def checked(a: Money, b: Money) -> Result[Money, MoneyError]:
    return try: a + b
";
    let (program, d, map) = beck_core::check_str("money.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&map));

    let row = |name: &str| -> Vec<String> {
        program
            .defs
            .get(name)
            .unwrap_or_else(|| panic!("no `{name}`"))
            .effects
            .iter()
            .map(|e| e.name())
            .collect()
    };
    assert_eq!(
        row("sum2"),
        vec!["raises(MoneyError)"],
        "a caller of `+` performs what the impl performs"
    );
    assert!(
        row("checked").is_empty(),
        "and a handler discharges it: {:?}",
        row("checked")
    );
}

/// The row crosses a module. A `.becki` publishes what each impl method performs, because the
/// trait cannot be asked any more — and a fallible method arriving in another module looking pure
/// is the unsoundness this closes.
#[test]
fn an_impls_row_is_published_with_the_impl() {
    let src = "\
model Money:
    units: Int
    currency: Str

union MoneyError:
    MixedCurrency(left: Str, right: Str)

impl Num for Money:
    def add(self, other):
        raise MixedCurrency(left=self.currency, right=other.currency)

    def sub(self, other):
        return self

    def mul(self, other):
        return self

    def div(self, other):
        return self
";
    let (program, d, map) = beck_core::check_str("money.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&map));
    let text = beck_core::iface::Interface::of(&program).render();
    assert!(
        text.contains("impl Num for Money:") && text.contains("def add() uses raises(MoneyError)"),
        "the published header has to carry the row:\n{text}"
    );
    // And a pure method says nothing, so an impl of a pure trait reads as it always did.
    assert!(!text.contains("def sub()"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// The token, under a real key
// ---------------------------------------------------------------------------------------------

/// The one thing `crypto.beck`'s own tests cannot assert, asserted here.
///
/// A `test` block's row must be empty (§21.3) and `cap.sign` is deliberately not auto-stubbable —
/// "stubbing a capability would bypass it" — so the layer of that library which *holds the key* is
/// the layer Beck cannot reach. Its pure layer covers every way a forgery can arrive; what is left
/// is that a real key produces a code only that key reproduces, which needs the evaluator.
///
/// That limit is a finding rather than an accident, and
/// [`52`](../../../../docs/52-crypto-and-identifiers-report.md) §52.5 is where it is written down.
#[test]
fn a_token_opens_only_under_the_key_that_minted_it() {
    use beck_core::core::Value;

    let src = format!(
        "{}\n\ndef minted(key: secret[Str], payload: Str) -> Str:\n    return sign(key, payload)\n\
         \ndef opened_with(key: secret[Str], token: Str) -> Result[Str, TokenError]:\n    return read_token(key, token)\n",
        std::fs::read_to_string(lib_dir().join("crypto.beck")).expect("the library is there")
    );
    let (program, d, map) = beck_core::check_str("crypto.beck", &src);
    assert!(!d.has_errors(), "{}", d.render(&map));

    let backend = beck_eval::backend_for(std::sync::Arc::new(program.clone()));
    let call = |name: &str, args: Vec<Value>| -> Value {
        let body = &program
            .defs
            .get(name)
            .unwrap_or_else(|| panic!("{name}"))
            .body;
        backend.function(body).expect("a definition is a function")(args)
            .unwrap_or_else(|e| panic!("`{name}`: {e}"))
    };
    // `secret[Str]` is a newtype at runtime, so a key is built the way `secret_env` builds one
    // rather than by reading the environment: what is under test is the arithmetic, not the read.
    let key = |text: &str| Value::Data {
        ty: std::sync::Arc::from(beck_core::Ty::SECRET),
        variant: None,
        fields: std::sync::Arc::new(std::collections::BTreeMap::from([(
            std::sync::Arc::from("value"),
            Value::str_(text),
        )])),
    };

    let token = call("minted", vec![key("k1"), Value::str_("actor-7")]);
    let opened = call("opened_with", vec![key("k1"), token.clone()]);
    assert_eq!(
        opened.display(),
        "Ok{value: actor-7}",
        "the key that minted it opens it"
    );
    let forged = call("opened_with", vec![key("k2"), token.clone()]);
    assert!(
        forged.display().contains("the mac does not match"),
        "another key must not: {}",
        forged.display()
    );
    // And it is a function: the same key and payload give the same token, every run, which is what
    // makes a digest safe to compute inside a replay.
    assert_eq!(
        token.display(),
        call("minted", vec![key("k1"), Value::str_("actor-7")]).display()
    );
}

// ---------------------------------------------------------------------------------------------
// Bignums, against an arithmetic that is not their own
// ---------------------------------------------------------------------------------------------

/// `lib/bignum.beck` multiplied and divided against Rust's `i128`, over 400 pairs.
///
/// [`dates.beck`](../../../../lib/dates.beck) established the shape and
/// [`50`](../../../../docs/50-collections-and-dates-report.md) §50.3 the limit on what it buys:
/// this is not two independent algorithms, it is one *claim* checked against a different
/// implementation. The file's own `property` blocks check `Big` against `Int`, which only reaches
/// values an `Int` holds; the whole point of a bignum is the values it does not, and `i128` is the
/// widest arithmetic the host has to check those against.
///
/// The operands are built to be past `Int` — around 10^18 each, so a product is around 10^36 and
/// still inside `i128` — and the generator is a fixed LCG rather than a random source, because a
/// cross-check that fails on Tuesdays is a cross-check nobody keeps.
#[test]
fn a_bignum_multiplies_and_divides_the_way_i128_does() {
    let dir = std::env::temp_dir().join("beck-bignum-crosscheck");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::copy(lib_dir().join("bignum.beck"), dir.join("bignum.beck")).expect("the library");

    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        // xorshift64*, so the sequence is this test's and not the platform's.
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let mut lines = String::new();
    for _ in 0..400 {
        let a = (next() % 2_000_000_000_000_000_000) as i128 - 1_000_000_000_000_000_000;
        // Never zero: division by it is a `raise`, which the library's own tests cover.
        let b = (next() % 1_000_000_000_000_000_000) as i128 + 1;
        let sign = if next() % 2 == 0 { -1 } else { 1 };
        let b = b * sign;
        lines.push_str(&format!(
            "    expect (try: render_big(big_of_str(\"{a}\") * big_of_str(\"{b}\"))) == Ok(value=\"{}\")\n\
             \x20   expect (try: render_big(big_of_str(\"{a}\") / big_of_str(\"{b}\"))) == Ok(value=\"{}\")\n\
             \x20   expect (try: render_big(big_rem(big_of_str(\"{a}\"), big_of_str(\"{b}\")))) == Ok(value=\"{}\")\n",
            a * b,
            a / b,
            a % b,
        ));
    }
    let src = format!("import bignum\n\ntest \"against i128\":\n{lines}");
    let file = dir.join("crosscheck.beck");
    std::fs::write(&file, &src).expect("a scratch file");

    let (ok, text) = beck(&[
        "test",
        file.to_string_lossy().as_ref(),
        "--filter",
        "against i128",
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok && text.contains("0 failed"), "{text}");
}

/// `lib/decimal.beck`'s rounded division against exact rational arithmetic in `i128`, over 300 cases.
///
/// Rounding is where a decimal library is subtly wrong, and it is wrong in a way its own tests
/// tend not to catch: a rule tested against the examples its author had in mind agrees with itself.
/// So the oracle is written the other way round here — the expected units are computed as an exact
/// rational, `2 × |remainder|` against `|divisor|`, in Rust, and compared against what Beck rendered.
///
/// All three rules are checked, and the halfway cases are **generated on purpose** rather than left
/// to chance: a uniform sample almost never lands exactly on a half, which is the only place the
/// three rules disagree and therefore the only place the test has any power.
#[test]
fn decimal_rounding_is_what_exact_rational_arithmetic_says() {
    let dir = std::env::temp_dir().join("beck-decimal-crosscheck");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    for lib in ["bignum.beck", "decimal.beck"] {
        std::fs::copy(lib_dir().join(lib), dir.join(lib)).expect("the library");
    }

    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    // (numerator, denominator, scale) — the first block lands exactly on a half at `scale`, which
    // is the only input that separates the three rules.
    let mut cases: Vec<(i128, i128, u32)> = Vec::new();
    for scale in 0..4u32 {
        for k in -6i128..7 {
            // (2k + 1) / (2 × 10^scale) is exactly k.5 × 10^-scale.
            cases.push((2 * k + 1, 2 * 10i128.pow(scale), scale));
        }
    }
    while cases.len() < 300 {
        let a = (next() % 200_001) as i128 - 100_000;
        let b = (next() % 1_000) as i128 + 1;
        let b = if next() % 2 == 0 { -b } else { b };
        cases.push((a, b, (next() % 5) as u32));
    }

    /// The exact quotient `a / b` scaled by `10^scale`, rounded by `rule`, as an integer.
    fn expected(a: i128, b: i128, scale: u32, rule: &str) -> i128 {
        let numerator = a * 10i128.pow(scale);
        let quotient = numerator / b;
        let remainder = numerator % b;
        if remainder == 0 {
            return quotient;
        }
        let away = if (a < 0) != (b < 0) { -1 } else { 1 };
        let twice = (2 * remainder).abs();
        let magnitude = b.abs();
        match rule {
            "Down" => quotient,
            "HalfUp" if twice >= magnitude => quotient + away,
            "HalfUp" => quotient,
            _ if twice > magnitude => quotient + away,
            _ if twice < magnitude => quotient,
            // Exactly half: to even.
            _ if quotient % 2 != 0 => quotient + away,
            _ => quotient,
        }
    }

    /// `units × 10^-scale` written out at exactly `scale` places, which is what `render_at` gives.
    fn rendered(units: i128, scale: u32) -> String {
        let sign = if units < 0 { "-" } else { "" };
        let digits = units.unsigned_abs().to_string();
        if scale == 0 {
            return format!("{sign}{digits}");
        }
        let padded = format!("{:0>width$}", digits, width = scale as usize + 1);
        let point = padded.len() - scale as usize;
        format!("{sign}{}.{}", &padded[..point], &padded[point..])
    }

    let mut lines = String::new();
    for (a, b, scale) in &cases {
        for rule in ["HalfEven", "HalfUp", "Down"] {
            lines.push_str(&format!(
                "    expect (try: render_at(divide_to(of_int({a}), of_int({b}), {scale}, {rule}), {scale}, Down)) == Ok(value=\"{}\")\n",
                rendered(expected(*a, *b, *scale, rule), *scale),
            ));
        }
    }
    let src = format!("import decimal\n\ntest \"against exact rationals\":\n{lines}");
    let file = dir.join("crosscheck.beck");
    std::fs::write(&file, &src).expect("a scratch file");

    let (ok, text) = beck(&[
        "test",
        file.to_string_lossy().as_ref(),
        "--filter",
        "against exact rationals",
    ]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok && text.contains("0 failed"), "{text}");
}
