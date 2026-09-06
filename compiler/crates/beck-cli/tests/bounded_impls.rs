//! An impl whose type parameter carries a bound, and the suggestion that told you to write one.
//!
//! [`docs/27`](../../../../docs/27-the-walls-come-down-report.md) §27.5 built traits and §27.6
//! built bounds, and the two met at `impl[T: Ord] Ranked for list[T]` — a bounded *impl*, which
//! neither report wrote a program for. It was refused, and the refusal was
//! `B0310: cannot find type `T`` pointing at the `list[T]` in the impl's own header.
//!
//! What makes it worth a gate of its own rather than a line in §27.10 is that **the compiler
//! suggests it**. An unbounded impl whose method calls a trait method on `T` reports `B0386` with
//! `help: bound it: `[T: Ord]``, and taking the advice produced an error about a type parameter
//! written three characters to the left. A suggestion that does not compile reads as the compiler
//! contradicting itself, and whoever follows it has no way to tell which message is the true one.
//!
//! # Both halves, and the second is the one that would be forgotten
//!
//! The positive half is that the suggested program compiles **and runs**: dispatch has to reach
//! the bound's own method at a concrete element type, not merely typecheck as though it might.
//!
//! The negative half is that the same program with the bound *removed* still reports `B0386` and
//! still suggests the bound. A "fix" that made an unbounded parameter satisfy every trait would
//! pass the first half and delete the check — which is
//! [`docs/82`](../../../../docs/82-the-edge-report.md) §82.10's pattern, a gate that tests the
//! shape of the fix rather than the shape of the gap.

use std::process::Command;

fn beck() -> Command {
    Command::new(env!("CARGO_BIN_EXE_beck"))
}

fn write(name: &str, src: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("beck-bounded-impls");
    std::fs::create_dir_all(&dir).expect("a writable temporary directory");
    let path = dir.join(name);
    std::fs::write(&path, src).expect("the fixture is writable");
    path
}

/// Compile and run a program's own tests, returning what `beck test` printed.
fn run_tests(name: &str, src: &str) -> String {
    let path = write(name, src);
    let out = beck().arg("test").arg(&path).output().expect("beck runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Check a program and return every diagnostic it produced.
fn check(name: &str, src: &str) -> String {
    let path = write(name, src);
    let out = beck().arg("check").arg(&path).output().expect("beck runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The program the suggestion asks for, with `BOUND` standing in for the impl's parameter list.
///
/// One source with one hole, so that the two halves of the gate cannot drift apart: the negative
/// half has to be *the same program*, or it is testing a different one.
fn ranked(bound: &str) -> String {
    format!(
        "\
trait Ord:
    def less(self, other: Self) -> Bool

trait Ranked:
    def rising(self) -> Bool

impl Ord for Int:
    def less(self, other):
        return self < other

impl[{bound}] Ranked for list[T]:
    def rising(self):
        match list_get(self, 0):
            case None:
                return true
            case Some(a):
                match list_get(self, 1):
                    case None:
                        return true
                    case Some(b):
                        return less(a, b)

def up() -> Bool:
    return rising([1, 2])

def down() -> Bool:
    return [2, 1].rising()

test \"a bounded impl dispatches at a concrete element type\":
    expect up() == true
    expect down() == false
"
    )
}

/// **The positive half**: the impl the compiler suggests compiles, and its method reaches the
/// bound's own method at a concrete element type.
///
/// Run rather than checked. A bounded definition is lowered by *passing a dictionary*
/// (`check/traits.rs`'s `expand_bounds`), so a bounded impl that typechecked and then called the
/// wrong thing — or nothing — would pass a `beck check` and fail here, which is the failure this
/// is written against: an impl that compiles and whose calls cannot is worse than the refusal it
/// replaced.
#[test]
fn a_bounded_impl_compiles_and_its_method_reaches_the_bound() {
    let out = run_tests("bounded.beck", &ranked("T: Ord"));
    assert!(
        out.contains("1 passed, 0 failed"),
        "the impl the compiler suggests has to compile and run:\n{out}"
    );
}

/// **The negative half**: with the bound removed, the same program is still refused, and still
/// told to write the bound.
///
/// This is the half that a fix deletes by accident. Making the header accept `[T: Ord]` is a
/// parsing question; making `T` *satisfy* `Ord` because it was named in a bound is the whole
/// point, and a checker that dropped the bound on the floor — treating every parameter as
/// implementing everything — would pass the test above and silently accept a program with no
/// implementation behind it.
#[test]
fn without_the_bound_the_same_program_is_still_refused_and_still_told_to_bound_it() {
    let out = check("unbounded.beck", &ranked("T"));
    assert!(
        out.contains("B0386"),
        "an unbounded parameter still may not call a trait method:\n{out}"
    );
    assert!(
        out.contains("bound it: `[T: Ord]`"),
        "and the suggestion has to still be the one this defect is about:\n{out}"
    );
}
