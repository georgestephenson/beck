//! What an `import` makes visible, and what it must not depend on.
//!
//! §3.6's separate compilation says a module is compiled against its imports' *interfaces*, and
//! [`docs/10`](../../../../docs/10-decisions.md) D23 says where a name resolves **from** — the
//! root's directory, then the standard library. Neither gives `import` an **order**, and a
//! module's contract is derived from its body rather than from its position, so nothing a program
//! means may change when two `import` lines are swapped.
//!
//! It did. A trait is resolved by name, and the checker registered each imported module's traits
//! and impls together, one module at a time, in the order the lines were written. So
//! `import thing` before `import vocab` dropped `impl Labelled for Thing` — silently, with the
//! failure surfacing one module later as `B0387` asking for an impl that `beck iface thing.beck`
//! publishes in the very file the compiler read.
//!
//! # Why this drives the binary rather than the checker
//!
//! Because the property is about *files on disk in a written order*, and because the positive half
//! is that both orders reach **the same implementation** rather than merely typecheck. A gate that
//! compiled the two orders and compared diagnostics would pass for a fix that registered the impl
//! and dispatched it wrongly.

use std::process::Command;

fn beck() -> Command {
    Command::new(env!("CARGO_BIN_EXE_beck"))
}

/// Write a set of modules into a directory of their own, and hand back the root's path.
///
/// A directory per case, because `import` resolves against the root module's own directory and two
/// cases sharing one would resolve each other's modules.
fn project(case: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("beck-imports").join(case);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a writable temporary directory");
    for (name, src) in files {
        std::fs::write(dir.join(name), src).expect("the fixture is writable");
    }
    dir.join(files[0].0)
}

fn run(subcommand: &str, root: &std::path::Path) -> String {
    let out = beck()
        .arg(subcommand)
        .arg(root)
        .output()
        .expect("beck runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const VOCAB: &str = "\
trait Labelled:
    def label(self) -> Str
";

/// Two types, two impls, so that reaching *an* implementation is not the same as reaching the
/// right one.
const THING: &str = "\
import vocab

model Thing:
    n: Int

model Other:
    s: Str

impl Labelled for Thing:
    def label(self):
        return \"thing \" + str(self.n)

impl Labelled for Other:
    def label(self):
        return \"other \" + self.s
";

fn root_calling(imports: &str) -> String {
    format!(
        "{imports}

def shown(t: Thing) -> Str:
    return t.label()

def shown_other(o: Other) -> Str:
    return o.label()

test \"each type reaches its own impl\":
    expect shown(Thing(n=7)) == \"thing 7\"
    expect shown_other(Other(s=\"x\")) == \"other x\"
"
    )
}

#[test]
fn an_imported_impl_does_not_depend_on_the_order_of_the_import_lines() {
    // `DEFECTS.md::an-imported-impl-is-visible-only-if-its-trait-was-imported-first`. Both orders,
    // and both have to *run*: a fix that made only the failing order compile would have moved the
    // bug, and one that registered the impl without dispatching it would pass a compile-only gate.
    for (case, imports) in [
        ("impl-then-trait", "import thing\nimport vocab"),
        ("trait-then-impl", "import vocab\nimport thing"),
    ] {
        let root = project(
            case,
            &[
                ("root.beck", &root_calling(imports)),
                ("thing.beck", THING),
                ("vocab.beck", VOCAB),
            ],
        );
        let out = run("test", &root);
        assert!(
            out.contains("1 passed, 0 failed"),
            "`{imports}` did not compile and run:\n{out}"
        );
    }
}

const PRICED: &str = "\
trait Priced:
    def price(self) -> Int
";

const GOODS: &str = "\
import priced

model Book:
    cost: Int

impl Priced for Book:
    def price(self):
        return self.cost

def total[T: Priced](xs: list[T]) -> Int:
    return list_sum(map_list(xs, lambda x: x.price()))
";

#[test]
fn a_bounded_import_keeps_its_dictionary_whichever_order_its_trait_arrives_in() {
    // The same defect one line down, and the one that would have been missed: a bounded `def`
    // publishes its bound, and the importer rebuilds the dictionary parameters the exporting
    // module lowered it with by resolving the trait **by name**. With the trait's module named
    // second, the rebuild found nothing and dropped them — so `total` was imported as a
    // one-argument function and the failure was not a diagnostic at all but
    // `expected 2 arguments, got 1`, at run time.
    for (case, imports) in [
        ("bounded-impl-then-trait", "import goods\nimport priced"),
        ("bounded-trait-then-impl", "import priced\nimport goods"),
    ] {
        let src = format!(
            "{imports}

def bill() -> Int:
    return total([Book(cost=3), Book(cost=4)])

test \"the dictionary survives the import\":
    expect bill() == 7
"
        );
        let root = project(
            case,
            &[
                ("root.beck", &src),
                ("goods.beck", GOODS),
                ("priced.beck", PRICED),
            ],
        );
        let out = run("test", &root);
        assert!(
            out.contains("1 passed, 0 failed"),
            "`{imports}` did not compile and run:\n{out}"
        );
    }
}

#[test]
fn an_impl_whose_trait_this_program_does_not_import_is_said_out_loud() {
    // The other half of the `continue` the fix above split. Once traits are registered across
    // every import, a trait still missing means the program does not import the module declaring
    // it — which is **not an error**: a name is visible where its module is imported directly, so
    // a module may perfectly well publish an impl for a trait the importer never names, and
    // refusing that would make importing a module for one plain function drag its whole trait
    // vocabulary in.
    //
    // It is not nothing either. The impl is dropped, and a later call fails as `B0350: no field or
    // function` — an absence, three modules from the fact that explains it. So it is a warning,
    // and the warning names the trait.
    let root = project(
        "trait-not-imported",
        &[
            (
                "root.beck",
                "import thing\n\ndef n_of(t: Thing) -> Int:\n    return t.n\n",
            ),
            ("thing.beck", THING),
            ("vocab.beck", VOCAB),
        ],
    );
    let out = run("check", &root);
    assert!(out.contains("B0388"), "{out}");
    assert!(out.contains("Labelled"), "the trait is named: {out}");
    assert!(
        out.contains("ok:"),
        "a program that never calls the method still compiles:\n{out}"
    );
}

// ---------------------------------------------------------------------------------------------
// A published interface, read back
// ---------------------------------------------------------------------------------------------

fn iface(root: &std::path::Path) -> String {
    let out = beck().arg("iface").arg(root).output().expect("beck runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_published_interface_naming_a_foreign_trait_can_be_read_back() {
    // `DEFECTS.md::a-published-interface-naming-a-trait-cannot-be-read-back`. `beck iface` wrote a
    // `.becki` that `beck check` then refused with `B0383`, because the reader checked it as a
    // module with **no imports** and the file carried no `import` line to resolve against. So any
    // module implementing a trait declared elsewhere, or publishing a bounded `def`, produced a
    // contract the compiler could not read — and checking it in stopped the program building.
    //
    // Both halves moved: the writer emits what the contract depends on, and the reader resolves
    // against the same interfaces the module itself was checked against.
    for (case, imports) in [
        ("iface-impl-then-trait", "import goods\nimport priced"),
        ("iface-trait-then-impl", "import priced\nimport goods"),
    ] {
        let src = format!(
            "{imports}

def bill() -> Int:
    return total([Book(cost=3), Book(cost=4)])

test \"the contract survives the round trip\":
    expect bill() == 7
"
        );
        let root = project(
            case,
            &[
                ("root.beck", &src),
                ("goods.beck", GOODS),
                ("priced.beck", PRICED),
            ],
        );
        let goods = root.with_file_name("goods.beck");
        let published = iface(&goods);
        assert!(
            published.contains("wrote"),
            "`beck iface` did not write one:\n{published}"
        );
        let text = std::fs::read_to_string(root.with_file_name("goods.becki")).expect("written");
        assert!(
            text.contains("import priced"),
            "the contract has to say where `Priced` comes from:\n{text}"
        );

        // And now the program builds *and runs* with that file checked in — dispatch reaches the
        // impl through the interface, which a gate that only checked for the absence of `B0383`
        // would not have shown.
        let out = run("test", &root);
        assert!(
            out.contains("1 passed, 0 failed"),
            "`{imports}` with the interface checked in:\n{out}"
        );
    }
}

#[test]
fn an_interface_naming_a_trait_nothing_declares_still_says_so() {
    // The half that would be forgotten: the fix must not turn `B0383` into silence. A hand-written
    // `.becki` whose import does not declare the trait it names is still refused, and the
    // diagnostic points into the `.becki` rather than at whoever imported it.
    let root = project(
        "iface-trait-nowhere",
        &[
            (
                "root.beck",
                "import goods\nimport priced\n\ndef bill() -> Int:\n    return total([Book(cost=3)])\n",
            ),
            (
                "goods.becki",
                "import priced\n\nmodel Book:\n    cost: Int\n\nimpl Nowhere for Book\n\n@on(any)\ndef total[T: Nowhere](xs: list[T]) -> Int\n",
            ),
            ("priced.beck", PRICED),
        ],
    );
    let out = run("check", &root);
    assert!(out.contains("B0383"), "{out}");
    assert!(out.contains("Nowhere"), "the trait is named: {out}");
    assert!(out.contains("goods.becki"), "and the file is: {out}");
}
