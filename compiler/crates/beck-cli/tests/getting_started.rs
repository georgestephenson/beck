//! `docs/86-getting-started.md`, compiled.
//!
//! [`08`](../../../../docs/08-roadmap.md) §8.5.4 names documentation as the one remaining reason
//! Phase 3's exit criterion cannot be attempted. A guide is the answer to that, and **a guide whose
//! examples do not compile is worse than no guide** — a newcomer who types what it says and gets a
//! diagnostic learns that the documentation lies, which is a slower and more expensive lesson than
//! finding nothing at all.
//!
//! So the guide is gated the way `docs/reference/` is: this file reads the markdown, extracts every
//! ` ```beck ` block and every `beck <command>` shown, and checks them. The discipline is
//! `docs/34`'s — a document derived from or checked against the compiler cannot drift from it in
//! silence.
//!
//! Two conventions the guide has to keep, both asserted below:
//!
//! * a ` ```beck ` block is a **complete module**, checked with the real front end. A fragment goes
//!   in a ` ```text ` block, which nothing here reads;
//! * a command shown in a ` ```text ` block starting `$ beck ` names a real subcommand.
//!
//! A block whose first line is `# <name>.beck` publishes under that name, and the guide's other
//! blocks may import it — which is how the guide shows a program split across files without a
//! second copy of it living in `compiler/`. Everything else resolves against the standard library,
//! so `import http` in the guide is the same `import http` a reader gets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beck_core::project::Loader;
use beck_core::row::Effect;
use beck_core::Sources;

fn guide() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../docs/86-getting-started.md")
        .canonicalize()
        .expect("the guide is checked in")
}

/// Every fenced block with the given language tag, in document order.
fn blocks(src: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in src.lines() {
        match &mut current {
            Some(buf) => {
                if line.trim_start().starts_with("```") {
                    out.push(std::mem::take(buf));
                    current = None;
                } else {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
            None => {
                if line.trim_end() == format!("```{tag}") {
                    current = Some(String::new());
                }
            }
        }
    }
    out
}

/// One program in the guide, under the module name it publishes.
struct Program {
    name: String,
    src: String,
}

/// The module name a block declares in its first line — `# club.beck` — or `shelf`, which is what
/// the guide's shell transcripts call the single-file programs.
fn declared_name(src: &str) -> Option<String> {
    let first = src.lines().next()?.trim();
    let name = first.strip_prefix("# ")?.trim().strip_suffix(".beck")?;
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .then(|| name.to_string())
}

fn programs(src: &str) -> Vec<Program> {
    blocks(src, "beck")
        .into_iter()
        .map(|src| Program {
            name: declared_name(&src).unwrap_or_else(|| "shelf".to_string()),
            src,
        })
        .collect()
}

/// The guide's own named modules, as something `import` resolves against.
///
/// A name this does not hold falls through to the standard library, which is where `import http`
/// and `import json` come from — [`beck_core::project`] tries the loader first and the library
/// second, exactly as it does for a program in a directory.
struct Guide(BTreeMap<String, String>);

impl Loader for Guide {
    fn load(&self, name: &str) -> Option<Sources> {
        self.0.get(name).map(|src| Sources {
            module: Some(src.clone()),
            interface: None,
            path: Some(format!("{name}.beck")),
        })
    }
}

impl Guide {
    /// Every named block, plus the one being compiled — so an unnamed program with imports is a
    /// root the loader can serve too.
    fn around(all: &[Program], root: &Program) -> Guide {
        let mut modules: BTreeMap<String, String> = BTreeMap::new();
        for p in all {
            if declared_name(&p.src).is_some() {
                assert!(
                    modules.insert(p.name.clone(), p.src.clone()).is_none(),
                    "two blocks in docs/86 both call themselves `{}.beck`, so one of them is \
                     compiled and the other is not",
                    p.name
                );
            }
        }
        modules.insert(root.name.clone(), root.src.clone());
        Guide(modules)
    }
}

/// Compile one block the way `beck test` compiles the file a reader would have saved it in.
///
/// A program with no imports takes the single-file path; one with imports goes through the project
/// pipeline, which is the same split `beck-cli`'s own `compile` makes and for the same reason —
/// resolving imports for a program that has none would be a directory walk nobody asked for.
fn compile(
    all: &[Program],
    p: &Program,
) -> (
    Option<beck_core::Placed>,
    beck_diag::Diagnostics,
    beck_diag::SourceMap,
) {
    let display = format!("{}.beck", p.name);
    let mut map = beck_diag::SourceMap::new();
    let id = map.add(display.clone(), p.src.clone());
    if beck_core::project::imports_of(id, &display, &p.src).is_empty() {
        return beck_core::compile_or_library_str(&display, &p.src);
    }
    let mut diags = beck_diag::Diagnostics::new();
    let placed = beck_core::project::check_project(
        &p.name,
        &Guide::around(all, p),
        None,
        &mut map,
        &mut diags,
    )
    .and_then(|project| beck_core::project::slice_or_library(project, &mut diags));
    (placed, diags, map)
}

/// Every Beck program in the guide compiles, with the real front end.
#[test]
fn every_program_in_the_guide_compiles() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let programs = programs(&src);
    assert!(
        programs.len() >= 6,
        "the guide should build up more than {} programs",
        programs.len()
    );

    for (i, p) in programs.iter().enumerate() {
        let (_, d, map) = beck_diag::depth::on_the_front_end_stack(|| compile(&programs, p));
        assert!(
            !d.has_errors(),
            "program {} of {} in docs/86 does not compile:\n{}\n---\n{}",
            i + 1,
            programs.len(),
            d.render(&map),
            p.src
        );
    }
}

/// …and the tests each program declares pass, which is a stronger claim than "it compiles".
///
/// The guide shows their output. A reader who runs `beck test` and sees a failure where the guide
/// printed `ok` has been told something untrue, and the shape of that lie — a passing compile and a
/// failing assertion — is exactly what a compile-only gate would miss.
#[test]
fn every_program_in_the_guide_passes_its_own_tests() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let programs = programs(&src);
    for (i, p) in programs.iter().enumerate() {
        let (placed, d, map) = compile(&programs, p);
        assert!(!d.has_errors(), "{}", d.render(&map));
        let placed = placed.expect("it compiles");
        let backend = beck_eval::backend(&placed);
        let report = beck_rt::testing::run(&placed, backend, &beck_rt::testing::Options::default());
        for case in &report.cases {
            assert!(
                case.outcome.is_pass(),
                "in program {} of docs/86, `{}` fails: {:?}",
                i + 1,
                case.name,
                case.outcome
            );
        }
    }
}

/// The guide covers more than one **shape** of program.
///
/// The guide's closing section — §86.12, "what this guide does not do" — carried this as its own
/// largest gap for as long as there was one worked example: "one fold, one view, one command union
/// — nothing here shows two folds, a module boundary, a trait, an outbound call, a macro or a
/// `parallel:` scope, all of which exist and are documented in the reports rather than here." That
/// bullet is gone, and this is what holds it gone. Each is asserted from the **compiled** program
/// rather than from the
/// text of the block, because the text is the thing that would be easy to keep and easy to make
/// meaningless: a `parallel:` whose scope the checker collapsed performs no `spawn`, and an
/// `import` nothing calls links nothing.
#[test]
fn the_guide_reaches_the_second_shape_of_program() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let programs = programs(&src);
    let compiled: Vec<(&Program, beck_core::Placed)> = programs
        .iter()
        .map(|p| {
            let (placed, d, map) = compile(&programs, p);
            assert!(!d.has_errors(), "{}", d.render(&map));
            (p, placed.expect("it compiles"))
        })
        .collect();

    let rows = |placed: &beck_core::Placed| -> Vec<Effect> {
        placed
            .program
            .defs
            .values()
            .flat_map(|d| d.effects.clone())
            .chain(
                placed
                    .program
                    .signals
                    .iter()
                    .flat_map(|s| s.effects.clone()),
            )
            .collect()
    };
    let any = |what: &str, f: &dyn Fn(&Program, &beck_core::Placed) -> bool| {
        assert!(
            compiled.iter().any(|(p, placed)| f(p, placed)),
            "no program in docs/86 shows {what}, which its §86.12 named as a gap"
        );
    };

    any("two durable folds", &|_, placed| {
        placed
            .program
            .signals
            .iter()
            .filter(|s| s.effects.contains(&Effect::Durable))
            .count()
            >= 2
    });
    // A module boundary of the guide's own: some block imports a module another block declares,
    // and a definition written over there is in the program that comes out over here. `imports` on
    // the linked `Program` is the first module's rather than the root's, so the witness is the
    // definition itself — which is the thing a boundary is for.
    let defines: fn(&str) -> Vec<String> = |src| {
        src.lines()
            .filter_map(|l| l.strip_prefix("def "))
            .filter_map(|l| l.split_once('(').map(|(n, _)| n.trim().to_string()))
            .collect()
    };
    any("a module boundary of its own", &|p, placed| {
        programs.iter().any(|q| {
            declared_name(&q.src).is_some()
                && q.name != p.name
                && p.src.contains(&format!("import {}", q.name))
                && defines(&q.src)
                    .iter()
                    .any(|n| placed.program.defs.contains_key(n.as_str()))
        })
    });
    // The standard library, which resolves through the same door. Compiling is the assertion: an
    // `import` naming a module nothing can find is `B0603`, and every program here compiles.
    any("an import of the standard library", &|p, _| {
        beck_core::stdlib::names().any(|m| p.src.contains(&format!("import {m}")))
    });
    any("an outbound call", &|_, placed| {
        rows(placed).iter().any(|e| matches!(e, Effect::NetOut(_)))
    });
    any("a `parallel:` scope", &|_, placed| {
        rows(placed).contains(&Effect::Spawn)
    });

    // A trait, and a macro, read off the definitions an `impl` lowers to — `ToJson::to_json@Book`
    // is a definition, and the `impls` list is per module rather than per link. Which of the two an
    // impl is is decided by whether the block's own source spells its header out: the one the
    // reader wrote is there, and the one `derive_json:` wrote is not. Textual *absence* is the
    // whole of the macro assertion, so it is paired with the target being a type this same block
    // declares — otherwise every program that imports the standard library would satisfy it with
    // `impl ToJson for Int`.
    let impls_of = |placed: &beck_core::Placed| -> Vec<(String, String)> {
        placed
            .program
            .defs
            .keys()
            .filter_map(|n| n.split_once("::"))
            .filter_map(|(t, rest)| {
                rest.split_once('@')
                    .map(|(_, x)| (t.to_string(), x.to_string()))
            })
            .collect()
    };
    let declares = |p: &Program, x: &str| {
        p.src.contains(&format!("model {x}:")) || p.src.contains(&format!("union {x}:"))
    };
    any("a trait implemented by hand", &|p, placed| {
        impls_of(placed)
            .iter()
            .any(|(t, x)| p.src.contains(&format!("impl {t} for {x}")))
    });
    any("a macro writing a declaration", &|p, placed| {
        impls_of(placed)
            .iter()
            .any(|(t, x)| declares(p, x) && !p.src.contains(&format!("impl {t} for {x}")))
    });
}

/// Every `beck <command>` the guide shows is a subcommand the binary has.
///
/// The cheapest kind of documentation rot: a guide that names a command somebody renamed. Checked
/// against the CLI's own help rather than against a list here, because a second list is a second
/// thing to keep true.
#[test]
fn every_command_the_guide_shows_exists() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let shown: Vec<String> = blocks(&src, "text")
        .iter()
        .flat_map(|b| b.lines().map(|l| l.to_string()).collect::<Vec<_>>())
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("$ beck ")?;
            let word = rest.split_whitespace().next()?;
            // `--version` and friends are flags, not subcommands.
            (!word.starts_with('-')).then(|| word.to_string())
        })
        .collect();
    assert!(
        shown.len() >= 4,
        "the guide should show more commands than {shown:?}"
    );

    let help = std::process::Command::new(env!("CARGO_BIN_EXE_beck"))
        .arg("--help")
        .output()
        .expect("the binary this test run built");
    let help = String::from_utf8_lossy(&help.stdout).to_string();
    for command in shown {
        assert!(
            help.lines()
                .any(|l| l.trim_start().starts_with(&format!("{command} "))),
            "docs/86 shows `beck {command}`, which the binary's own help does not list"
        );
    }
}

/// The placement table §86.5 prints is the one the compiler produces.
///
/// That table is the guide's central claim — "you do not choose a tier, you write what a function
/// does, and the placement follows" — and it is shown as a **transcript**. Every other thing the
/// guide asserts is held by running it: the programs compile, their tests pass, the commands exist.
/// A transcript is the one shape that rots silently, because it is prose that looks like evidence,
/// and the reader has no way to tell it apart from the real thing.
///
/// It is checked against `place::report` rather than against a list here, for the reason
/// `every_command_the_guide_shows_exists` is: a second list is a second thing to keep true.
#[test]
fn the_placement_table_in_the_guide_is_the_one_the_compiler_prints() {
    let src = std::fs::read_to_string(guide()).expect("readable");
    let programs = programs(&src);

    // The transcript: the rows under a `$ beck explain place` line, to the end of its block.
    let shown: Vec<Vec<String>> = blocks(&src, "text")
        .iter()
        .filter_map(|b| {
            let mut lines = b.lines().map(str::trim_end);
            lines.find(|l| l.trim_start().starts_with("$ beck explain place"))?;
            let rows: Vec<String> = lines
                .take_while(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().to_string())
                .collect();
            (!rows.is_empty()).then_some(rows)
        })
        .collect();
    assert!(
        !shown.is_empty(),
        "docs/86 no longer shows a placement table, and this test is the reason it was trustworthy"
    );

    for table in &shown {
        // What the compiler says, for whichever program in the guide this table is of. Matching by
        // *content* rather than by position: the guide's blocks build one program up over several
        // sections and they are all called `shelf.beck`, so naming one would be guessing.
        let printed: Vec<Vec<String>> = programs
            .iter()
            .filter_map(|p| {
                let (placed, d, _) = compile(&programs, p);
                let placed = placed.filter(|_| !d.has_errors())?;
                let report = beck_core::place::report(&placed.placement, None).ok()?;
                Some(
                    report
                        .lines()
                        .take_while(|l| !l.trim().is_empty())
                        .map(|l| l.trim_end().to_string())
                        .collect(),
                )
            })
            .collect();
        assert!(
            printed.iter().any(|p| p == table),
            "the placement table in docs/86 §86.5 is not what any program in the guide places to.\n\
             shown:\n{}\n\nthe guide's programs place to:\n{}",
            table.join("\n"),
            printed
                .iter()
                .map(|p| p.join("\n"))
                .collect::<Vec<_>>()
                .join("\n\n---\n\n")
        );
    }
}
