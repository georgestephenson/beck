//! The Phase 2 exit criterion, measured.
//!
//! [`docs/08-roadmap.md`](../../../../docs/08-roadmap.md), Phase 2:
//!
//! > **Exit**: on a corpus of 20+ programs, placement is inferred with no annotations for the
//! > common cases; every §3.5 property is a passing test; a 3-module project rebuilds incrementally
//! > without recompiling dependencies whose signatures didn't change.
//!
//! The second clause is [`security.rs`](security.rs) and the third is `beck-db`'s
//! `a_body_edit_upstream_does_not_recheck_anything_downstream`. This file is the first, and it is
//! written to be falsifiable: it counts the programs, asserts that they carry no annotations, and
//! then checks *where each one was placed* — because "twenty programs compiled" would be true of a
//! compiler that placed everything on the server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beck_core::{Sources, Tier};

/// Where the corpus lives, relative to this crate.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .canonicalize()
        .expect("the corpus is checked in")
}

/// Every single-file program, in name order.
fn single_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(corpus())
        .expect("the corpus is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    out.sort();
    out
}

/// The multi-module project, as a loader.
struct Dir(PathBuf);

impl beck_core::project::Loader for Dir {
    fn load(&self, name: &str) -> Option<Sources> {
        let path = self.0.join(format!("{name}.beck"));
        Some(Sources {
            module: std::fs::read_to_string(&path).ok(),
            interface: std::fs::read_to_string(self.0.join(format!("{name}.becki"))).ok(),
            path: Some(path.display().to_string()),
        })
    }
}

fn project_files() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(corpus().join("project"))
        .expect("the project is checked in")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    out.sort();
    out
}

fn name_of(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().to_string()
}

/// The solved tier of every definition and signal in a single-file program.
fn placement(path: &Path) -> BTreeMap<String, Tier> {
    let src = std::fs::read_to_string(path).expect("readable");
    let (program, d, map) = beck_core::check_str(&name_of(path), &src);
    assert!(!d.has_errors(), "{}:\n{}", name_of(path), d.render(&map));
    beck_core::place::solve(&program, None)
        .tiers
        .into_iter()
        .map(|(k, t)| (k.to_string(), t))
        .collect()
}

#[test]
fn the_corpus_is_at_least_twenty_programs() {
    let n = single_files().len() + project_files().len();
    assert!(
        n >= 20,
        "the exit criterion says 20+; the corpus has {n}: {:?}",
        single_files()
            .iter()
            .map(|p| name_of(p))
            .collect::<Vec<_>>()
    );
}

#[test]
fn no_program_in_the_corpus_carries_a_placement_annotation() {
    // "…placement is inferred **with no annotations** for the common cases." One file is the
    // exception and says so in its own first line — it exists to check that an annotation wins.
    const DELIBERATE: &[&str] = &["18-pinned.beck"];
    for path in single_files().into_iter().chain(project_files()) {
        let name = name_of(&path);
        let src = std::fs::read_to_string(&path).expect("readable");
        if DELIBERATE.contains(&name.as_str()) {
            assert!(
                src.contains("@on("),
                "{name} is listed as a deliberate exception but has no annotation"
            );
            assert!(
                src.contains("**This file has annotations on purpose**"),
                "{name} must say in its own text why it is the exception"
            );
            continue;
        }
        assert!(
            !src.contains("@on("),
            "{name} carries a placement annotation, so it does not test inference"
        );
    }
}

#[test]
fn every_program_in_the_corpus_compiles() {
    for path in single_files() {
        let src = std::fs::read_to_string(&path).expect("readable");
        let (placed, d, map) = beck_core::compile_str(&name_of(&path), &src);
        assert!(
            !d.has_errors(),
            "{} does not compile:\n{}",
            name_of(&path),
            d.render(&map)
        );
        assert!(placed.is_some(), "{} did not slice", name_of(&path));
    }
}

#[test]
fn every_program_is_placed_the_way_its_effects_say_it_should_be() {
    // The claim that matters. Every one of these follows from an effect row and a cost, and not one
    // of them is written down in the program.
    for path in single_files() {
        let name = name_of(&path);
        if name == "18-pinned.beck" {
            continue; // annotated on purpose; checked separately
        }
        let t = placement(&path);

        // `ingress` is discharged by exactly one tier.
        let ingress: Vec<&String> = t
            .keys()
            .filter(|k| k.starts_with("signal/"))
            .filter(|k| t[*k] == Tier::Server)
            .collect();
        assert!(
            !ingress.is_empty(),
            "{name}: something must be on the server — the merge point is: {t:?}"
        );

        // Exactly one signal is the browser's — the page. Everything else in the graph stays
        // behind the boundary, which is "the log and the rules never ship to clients" (§3.5) as a
        // property of the *solution* rather than of an annotation.
        let on_client: Vec<&String> = t
            .iter()
            .filter(|(k, v)| k.starts_with("signal/") && **v == Tier::Client)
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            on_client,
            vec!["signal/page"],
            "{name}: only the page belongs in a browser: {t:?}"
        );

        // The page is the browser's subscription.
        assert_eq!(
            t.get("signal/page"),
            Some(&Tier::Client),
            "{name}: the page is what the browser subscribes to: {t:?}"
        );

        // And the fold function is unplaced, so it compiles to both sides — the property that lets
        // a client apply an event optimistically with the same code the server folds with.
        assert_eq!(
            t.get("def/apply_event"),
            Some(&Tier::Any),
            "{name}: a replay-pure fold must be unplaced: {t:?}"
        );
    }
}

/// The signal holding the `durable` fold, by name, per program.
const FOLDS: &[(&str, &str)] = &[
    ("01-counter.beck", "signal/count"),
    ("02-chat.beck", "signal/room"),
    ("03-billing.beck", "signal/ledger"),
    ("04-kanban.beck", "signal/board"),
    ("05-poll.beck", "signal/poll"),
    ("06-inventory.beck", "signal/stock"),
    ("07-leaderboard.beck", "signal/board"),
    ("08-audit.beck", "signal/log_state"),
    ("09-signup.beck", "signal/directory"),
    ("10-cart.beck", "signal/carts"),
    ("11-flags.beck", "signal/flags"),
    ("12-webhooks.beck", "signal/hooks"),
    ("13-reservations.beck", "signal/diary"),
    ("14-wiki.beck", "signal/wiki"),
    ("15-presence.beck", "signal/room"),
    ("16-money.beck", "signal/ledger"),
    ("17-derived.beck", "signal/counts"),
    ("19-clock.beck", "signal/series"),
    ("20-moderation.beck", "signal/queue"),
    // The general slicer's three. `21` and `23` each declare two folds; the entry names the one
    // the chokepoint reads, which is the one this table is about.
    ("21-two-folds.beck", "signal/roster"),
    ("22-shared.beck", "signal/ballot"),
    ("23-slices.beck", "signal/ledger"),
];

#[test]
fn a_big_accumulator_goes_to_the_data_tier_and_a_small_one_does_not_care() {
    // The cost model charges a fold that is *not* at the data tier an edge to the log, sized from
    // its accumulator. So a `Map` of records goes to the data tier by a wide margin, and an `Int`
    // genuinely does not care — the crossing is eight bytes.
    //
    // That is not the model failing to decide; it is the model declining to invent a difference.
    // In Phase 2's single-process deployment the data tier and the server tier *are* the same
    // process, so for a scalar accumulator the two answers cost the same thing because they are
    // the same thing. It becomes a real decision in Phase 3, when the incremental view engine and
    // the read models make the data tier its own address.
    let mut at_data = 0;
    for (file, signal) in FOLDS {
        let t = placement(&corpus().join(file));
        let where_ = t
            .get(*signal)
            .unwrap_or_else(|| panic!("{file}: no `{signal}` — {t:?}"));
        assert_ne!(
            *where_,
            Tier::Client,
            "{file}: the log is not the browser's"
        );
        if *where_ == Tier::Data {
            at_data += 1;
        }
    }
    // The map-shaped ones are the majority of the corpus and all of them land at the data tier.
    assert!(
        at_data >= 12,
        "only {at_data} of {} folds went to the data tier",
        FOLDS.len()
    );

    // And the sharp pair, named: the same program shape with a `Map` accumulator and with an `Int`.
    assert_eq!(
        placement(&corpus().join("02-chat.beck")).get("signal/room"),
        Some(&Tier::Data),
        "a map of records is worth keeping next to the log"
    );
    assert_eq!(
        placement(&corpus().join("01-counter.beck")).get("signal/count"),
        Some(&Tier::Server),
        "…and an `Int` is not, which the model says rather than pretending otherwise"
    );
}

#[test]
fn an_effect_that_only_one_tier_discharges_pins_its_definition() {
    // Spot checks with names, so a regression says which rule broke rather than "something moved".
    let cases: &[(&str, &str, Tier)] = &[
        // `net.out(a named host)` and `env` are the server's.
        ("03-billing.beck", "def/charge", Tier::Server),
        ("03-billing.beck", "def/credentials", Tier::Server),
        ("12-webhooks.beck", "def/notify", Tier::Server),
        // `cap.*` is held where sessions are minted.
        ("02-chat.beck", "def/mine_only", Tier::Server),
        ("20-moderation.beck", "def/lift", Tier::Server),
        ("06-inventory.beck", "def/restock", Tier::Server),
        ("13-reservations.beck", "def/release", Tier::Server),
        // `external.read` is an escape hatch, and the fold engine refuses it.
        ("11-flags.beck", "def/legacy_flag", Tier::Server),
        // `nondet` is refused by the data tier and discharged by the other two.
        ("19-clock.beck", "def/stamp", Tier::Server),
        // Ambient effects force nothing: §3.2's `log` and `metrics` leave a definition unplaced.
        ("08-audit.beck", "def/audited", Tier::Any),
        // …and a pure query is unplaced however much it computes.
        ("04-kanban.beck", "def/in_column", Tier::Any),
        ("07-leaderboard.beck", "def/ranked", Tier::Any),
        ("16-money.beck", "def/balance", Tier::Any),
    ];
    for (file, key, want) in cases {
        let t = placement(&corpus().join(file));
        assert_eq!(
            t.get(*key),
            Some(want),
            "{file}: `{key}` should be on `{}`\n{t:#?}",
            want.name()
        );
    }
}

#[test]
fn the_own_origin_is_reachable_from_a_browser_and_a_named_host_is_not() {
    // The one placement in the corpus that is a genuine *choice* rather than a forced move: both
    // the client and the server can discharge `net.out(origin)`, so only the cost model decides.
    let t = placement(&corpus().join("12-webhooks.beck"));
    assert_eq!(t.get("def/notify"), Some(&Tier::Server));
    assert!(
        matches!(
            t.get("def/ping_self"),
            Some(Tier::Client) | Some(Tier::Server)
        ),
        "own-origin is dischargeable on both, so either answer is legal: {t:?}"
    );
}

#[test]
fn an_annotation_wins_over_the_solver() {
    // §3.3: "with explicit `@on(...)` always available and always winning." The solver would put
    // this fold at the data tier; the program says server; the program wins.
    let t = placement(&corpus().join("18-pinned.beck"));
    assert_eq!(t.get("signal/count"), Some(&Tier::Server));
    assert_eq!(t.get("signal/page"), Some(&Tier::Client));
}

#[test]
fn placement_is_deterministic_across_the_whole_corpus() {
    // §3.4's first guardrail: "determinism (same input, same solution)". Over every program, four
    // times each — because a solver that is deterministic on one graph and not on another is not
    // deterministic.
    for path in single_files() {
        let first = placement(&path);
        for _ in 0..3 {
            assert_eq!(placement(&path), first, "{} moved", name_of(&path));
        }
    }
}

#[test]
fn every_program_round_trips_through_the_formatter() {
    // A corpus of twenty programs is also a parser corpus, and it is free to use it as one:
    // `parse(print(parse(src))) == parse(src)`, and the printed form still compiles.
    for path in single_files() {
        let name = name_of(&path);
        let src = std::fs::read_to_string(&path).expect("readable");
        let mut map = beck_diag::SourceMap::new();
        let file = map.add(name.clone(), src.clone());
        let mut d = beck_diag::Diagnostics::new();
        let parsed = beck_syntax::parse_file(file, &name, &src, &mut d);
        assert!(!d.has_errors(), "{name}: {}", d.render(&map));

        let printed = beck_syntax::print::to_python(&parsed);
        let (again, d2, map2) = beck_core::compile_str(&name, &printed);
        assert!(
            !d2.has_errors(),
            "{name} does not compile after formatting:\n{}",
            d2.render(&map2)
        );
        assert!(again.is_some());

        // …and the canonical surface reads the same program. The `.sx` suffix is what selects the
        // S-expression reader — one binary, two surfaces, one language (§2.2).
        let sexpr = beck_syntax::print::to_sexpr_pretty(&parsed);
        let sx_name = name.replace(".beck", ".sx");
        let (from_sexpr, d3, map3) = beck_core::compile_str(&sx_name, &sexpr);
        assert!(
            !d3.has_errors(),
            "{name} does not compile from the S-expression surface:\n{}",
            d3.render(&map3)
        );
        assert!(from_sexpr.is_some());
    }
}

#[test]
fn every_programs_interface_round_trips_and_is_stable_under_a_body_edit() {
    // §3.6's firewall over the whole corpus rather than one file: publish, re-read, and the
    // contract is the same one.
    for path in single_files() {
        let name = name_of(&path);
        let src = std::fs::read_to_string(&path).expect("readable");
        let (placed, _, _) = beck_core::compile_str(&name, &src);
        let iface = beck_core::Interface::of(&placed.expect("it compiles").program);

        let text = iface.render();
        let mut d = beck_diag::Diagnostics::new();
        let reread = beck_core::Interface::parse(&name, &text, &mut d);
        let mut map = beck_diag::SourceMap::new();
        map.add(format!("{name}i"), text.clone());
        assert!(!d.has_errors(), "{name}:\n{}\n{text}", d.render(&map));
        assert_eq!(
            iface.digest(),
            reread.digest(),
            "{name}: a published contract must read back as itself\n{text}"
        );
    }
}

#[test]
fn the_three_module_project_checks_links_and_places() {
    // The exit criterion's third clause, at the level a person would run it: point the compiler at
    // the app and it resolves two imports, checks each module against the others' *interfaces*,
    // links the bodies, and slices the result.
    let dir = corpus().join("project");
    let mut map = beck_diag::SourceMap::new();
    let mut diags = beck_diag::Diagnostics::new();
    let placed = beck_core::compile_project("app", &Dir(dir), None, &mut map, &mut diags);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the project links");

    // Definitions from all three modules are in the linked program.
    for name in ["apply_event", "toggled", "validate", "owned", "view"] {
        assert!(
            placed.program.defs.contains_key(name),
            "`{name}` did not survive the link"
        );
    }
    // `cap.session` lives in `policy`, the wiring lives in `app`, and the capability is discharged
    // — a check that only has an answer once the modules are linked.
    assert_eq!(placed.program.defs["owned"].tier, Tier::Server);
    assert_eq!(
        placed
            .program
            .signals
            .iter()
            .find(|s| s.name.as_ref() == "todos")
            .map(|s| s.tier),
        Some(Tier::Data)
    );
}

#[test]
fn a_polymorphic_definition_crosses_a_becki_boundary_and_is_fresh_at_every_call() {
    // docs/27 §27.7 named the shape of this gap for recursive types: "no *project* test imports a
    // module whose published type is recursive, so separate compilation over recursive types is
    // compiled-and-believed rather than measured". User-written polymorphism (docs/29) would have
    // had exactly the same gap, so the project carries it instead of the SICP suite.
    //
    // `domain.beck` defines `only[T]` and `count_where[T]`; `app.beck` imports them and uses them
    // at `Todo` and at `Str`. If the published scheme were monomorphic — `Scheme::mono` rather than
    // `Scheme::generic` — the first use would fix the second's element type and this would fail to
    // link, while every single-module test in the suite went on passing.
    let dir = corpus().join("project");
    let mut map = beck_diag::SourceMap::new();
    let mut diags = beck_diag::Diagnostics::new();
    let placed = beck_core::compile_project("app", &Dir(dir.clone()), None, &mut map, &mut diags);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("the project links");
    for name in ["only", "count_where", "remaining", "named_owners"] {
        assert!(
            placed.program.defs.contains_key(name),
            "`{name}` did not survive the link"
        );
    }
    assert_eq!(
        placed.program.defs["only"].typarams.len(),
        1,
        "and it is still polymorphic after the link"
    );

    // The published contract has to *say* so, or an importer compiled against the file rather than
    // the source would get a monomorphic one.
    let mut map = beck_diag::SourceMap::new();
    let mut diags = beck_diag::Diagnostics::new();
    let project =
        beck_core::project::check_project("domain", &Dir(dir), None, &mut map, &mut diags);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let rendered = project.expect("a library checks").interface.render();
    assert!(
        rendered.contains("def only[T](xs: list[T], keep: (T) -> Bool) -> list[T]"),
        "the `.becki` has to carry the type parameters:\n{rendered}"
    );
}

#[test]
fn each_library_in_the_project_publishes_a_contract_without_being_an_application() {
    // A domain module has no merge point and no page. It is not a broken application; it is not an
    // application. Both halves are asserted, because the first without the second would pass on a
    // compiler that simply ignored the difference.
    let dir = corpus().join("project");
    for (name, expect_items) in [("domain", 2), ("policy", 2)] {
        let mut map = beck_diag::SourceMap::new();
        let mut diags = beck_diag::Diagnostics::new();
        let project =
            beck_core::project::check_project(name, &Dir(dir.clone()), None, &mut map, &mut diags);
        assert!(!diags.has_errors(), "{name}: {}", diags.render(&map));
        let project = project.expect("a library checks");
        assert!(
            project.interface.items.len() >= expect_items,
            "{name} publishes {} items",
            project.interface.items.len()
        );

        // …and slicing it fails for exactly the reason "this is not an application".
        let mut slicing = beck_diag::Diagnostics::new();
        assert!(beck_core::project::slice(project, &mut slicing).is_none());
        assert!(
            slicing
                .iter()
                .all(|d| beck_core::project::NOT_AN_APPLICATION.contains(&d.code)),
            "{name} failed to slice for a reason other than being a library: {:?}",
            slicing.iter().map(|d| d.code).collect::<Vec<_>>()
        );
    }
}

#[test]
fn every_test_written_in_a_corpus_program_passes() {
    // §21.2's construct, measured the way the corpus measures placement: on programs a reader can
    // open, not on strings inside this file. A corpus program that carries `test` blocks is a
    // program whose *behaviour* is asserted in Beck, by the same command an outside developer runs.
    let mut programs = 0;
    let mut cases = 0;
    for path in single_files() {
        let src = std::fs::read_to_string(&path).expect("readable");
        let (placed, d, map) = beck_core::compile_str(&name_of(&path), &src);
        assert!(!d.has_errors(), "{}:\n{}", name_of(&path), d.render(&map));
        let placed = placed.expect("it slices");
        if placed.program.tests.is_empty() {
            continue;
        }
        programs += 1;
        let backend = beck_eval::backend(&placed);
        let report = beck_rt::testing::run(&placed, backend, &beck_rt::testing::Options::default());
        cases += report.cases.len();
        assert!(
            report.ok() && report.skipped() == 0,
            "{}:\n{}",
            name_of(&path),
            beck_rt::testing::render(&report, true)
        );
    }
    assert!(
        programs >= 2 && cases >= 6,
        "the corpus carries {cases} tests across {programs} programs, which is not evidence of \
         anything"
    );
}
