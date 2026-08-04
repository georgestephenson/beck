//! The `beck test` runner — `docs/21-tests-in-beck-and-proof.md` §21.2 and §21.3, executed.
//!
//! # Why the runner lives in the runtime crate
//!
//! §21.2: "the test runs the same `Roles` the runtime drives, with the tiers co-located. What it
//! proves is what the boundary *means*." That is not a figure of speech here — a test's `when`
//! goes through the same `validate` [`crate::Runtime`] calls on a websocket frame, its `given` goes
//! through the same fold the sequencer drives, and `expect page(session("bo"))` renders through the
//! same view the server diffs. There is no second execution path to keep in agreement, which is the
//! only reason the cross-boundary test in §21.2 is three lines instead of a docker-compose file.
//!
//! # What makes a test here unable to flake
//!
//! Three things, none of them a convention:
//!
//! * **The log is the state**, so there is nothing to arrange and nothing to tear down.
//! * **Time and identity are data.** The envelope's `at` is the sequence position and its `actor`
//!   is written in the test, so two runs produce the same state bit for bit.
//! * **Effects are stubbed**, and the *complete list* of what was stubbed is the effect row, which
//!   the compiler already computed. §21.3 rule 1: "'any value' is the default, so it needs no
//!   expression" — and rule 1's price is that the default must say what it did, which
//!   [`Report`] does.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use beck_core::backend::{Backend, Callable, Interceptor};
use beck_core::core::{Core, CoreKind, VarId};
use beck_core::testing::{Clause, Count, Expectation, TestDef};
use beck_core::{digest, Effect, Placed, Tier, Ty, Value};

use crate::log::{Envelope, Instant};
use crate::program::Runtime;

/// How many inputs a `property` block is run with when nothing says otherwise.
pub const DEFAULT_RUNS: u64 = 100;

/// The actor a test speaks as when it does not name one.
pub const DEFAULT_ACTOR: &str = "test";

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Only run tests whose name contains this.
    pub filter: Option<String>,
    /// Inputs per `property` block.
    pub runs: u64,
    /// Where a `expect wire_compatible_with "…"` path is resolved from, and where `snapshots/`
    /// lives for `expect page matches snapshot`.
    pub base_dir: std::path::PathBuf,
    /// `beck test --update`: write what the page renders to instead of comparing against it.
    ///
    /// Off by default and never inferred. A snapshot that updates itself when it disagrees is not
    /// an assertion — §21.2's stated risk is snapshot rot and its stated mitigation is reviewing
    /// the diff, which only exists if writing is something a person asked for.
    pub update_snapshots: bool,
}

impl Options {
    pub fn runs(&self) -> u64 {
        if self.runs == 0 {
            DEFAULT_RUNS
        } else {
            self.runs
        }
    }
}

#[derive(Clone, Debug)]
pub struct Report {
    pub cases: Vec<Case>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.cases.iter().filter(|c| c.outcome.is_pass()).count()
    }
    pub fn failed(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| matches!(c.outcome, Outcome::Failed { .. }))
            .count()
    }
    pub fn skipped(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| matches!(c.outcome, Outcome::Skipped(_)))
            .count()
    }
    pub fn ok(&self) -> bool {
        self.failed() == 0
    }
}

#[derive(Clone, Debug)]
pub struct Case {
    pub name: Arc<str>,
    pub outcome: Outcome,
    /// What §21.3 rule 1's hidden default did, so it is not hidden: every definition that was
    /// stubbed, the atom that caused it, the value it returned and how often it was called.
    pub stubbed: Vec<Stubbed>,
    /// Inputs actually run, for a `property`.
    pub runs: u64,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Passed,
    Failed {
        why: String,
    },
    /// Not run, and why — never silently counted as a pass.
    Skipped(String),
}

impl Outcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, Outcome::Passed)
    }
}

#[derive(Clone, Debug)]
pub struct Stubbed {
    pub def: Arc<str>,
    pub atom: String,
    /// What it answered. `None` for a stub that answers from the call (§21.3 rule 3) and was never
    /// called — there is no single value to name, and inventing one for the report would be the
    /// same mistake the stub itself exists to avoid.
    pub returned: Option<Value>,
    pub calls: usize,
    /// True when the test named it, false when §21.3 rule 1 supplied it.
    pub explicit: bool,
    /// True when the stub is a body over the call's arguments rather than a fixed value.
    pub from_the_call: bool,
}

// ---------------------------------------------------------------------------------------------
// The stub table
// ---------------------------------------------------------------------------------------------

/// What a stub answers with.
///
/// §21.3 rule 2 is a value: "no parameter list, because parameters are not how the stub is
/// selected". Rule 3 is a *body* over the stubbed definition's parameters, so that "matching by
/// value uses the language's own `match`, and there is no mock DSL". The second is prepared through
/// [`Backend::function`] like any other code, and by the *base* backend — a stub is test code, and
/// stubbing a stub would be a loop.
enum Answer {
    Value(Value),
    FromTheCall(Callable),
}

/// One entry per definition a stub stands in for. The unit is a *definition* because that is what
/// gets called; the *identity* is the effect atom, which is what the test names.
struct Entry {
    atom: Effect,
    answer: Answer,
    explicit: bool,
    /// Set when the generator refused to invent a return value. §21.3 rule 5: "it can refuse, with
    /// a diagnostic, for a type with no inhabitant it can construct". The refusal has to reach the
    /// person, so it is carried to the point of use and reported as a failure of the test that
    /// reached it — not swallowed, and not turned into a real call.
    refused: Option<String>,
}

/// One recorded performance of an effect: what was called, with what, and what it answered.
struct Call {
    def: Arc<str>,
    args: Vec<Value>,
    returned: Value,
}

#[derive(Default)]
struct Recorder {
    entries: BTreeMap<Arc<str>, Entry>,
    calls: Mutex<Vec<Call>>,
    /// Anything that went wrong *inside* the stub machinery, reported as a failure of the test that
    /// reached it rather than swallowed.
    problems: Mutex<Vec<String>>,
}

impl Interceptor for Recorder {
    fn intercept(&self, name: &str, args: &[Value]) -> Option<Value> {
        let e = self.entries.get(name)?;
        if let Some(why) = &e.refused {
            let atom = e.atom.name();
            self.problems.lock().expect("stub log").push(format!(
                "the generator cannot invent a return value for `{name}` ({atom}): {why}\n  \
                 write the stub out: `stub {atom}: <value>`"
            ));
        }
        let returned = match &e.answer {
            Answer::Value(v) => v.clone(),
            Answer::FromTheCall(f) => match f(args.to_vec()) {
                Ok(v) => v,
                Err(err) => {
                    self.problems
                        .lock()
                        .expect("stub log")
                        .push(format!("the stub for `{}` failed: {err}", e.atom.name()));
                    Value::Unit
                }
            },
        };
        self.calls.lock().expect("stub log").push(Call {
            def: Arc::from(name),
            args: args.to_vec(),
            returned: returned.clone(),
        });
        Some(returned)
    }
}

impl Recorder {
    fn count(&self, atom: &Effect) -> usize {
        self.calls
            .lock()
            .expect("stub log")
            .iter()
            .filter(|c| self.entries.get(&c.def).map(|e| &e.atom) == Some(atom))
            .count()
    }

    fn called_with(&self, atom: &Effect, wanted: &Value) -> bool {
        let want = digest(wanted);
        self.calls
            .lock()
            .expect("stub log")
            .iter()
            .filter(|c| self.entries.get(&c.def).map(|e| &e.atom) == Some(atom))
            .any(|c| c.args.iter().any(|a| digest(a) == want))
    }

    fn report(&self) -> Vec<Stubbed> {
        let calls = self.calls.lock().expect("stub log");
        let mut out: Vec<Stubbed> = self
            .entries
            .iter()
            .map(|(def, e)| {
                let mine: Vec<&Call> = calls.iter().filter(|c| c.def == *def).collect();
                Stubbed {
                    def: def.clone(),
                    atom: e.atom.name(),
                    returned: match &e.answer {
                        Answer::Value(v) => Some(v.clone()),
                        // The last answer it gave, which is the only honest single value a stub
                        // that varies with the call has.
                        Answer::FromTheCall(_) => mine.last().map(|c| c.returned.clone()),
                    },
                    calls: mine.len(),
                    explicit: e.explicit,
                    from_the_call: matches!(e.answer, Answer::FromTheCall(_)),
                }
            })
            .collect();
        out.sort_by(|a, b| a.def.cmp(&b.def));
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------------

/// Run every `test` and `property` block in a compiled program.
///
/// On a thread with as much host stack as the backend says it needs
/// ([`Backend::stack_bytes`]), because a test is the most likely place for a program to recurse
/// further than its author expected and the answer to that has to be a failing case rather than a
/// dead process. A backend that needs nothing gets no thread.
pub fn run(placed: &Placed, backend: Arc<dyn Backend>, opts: &Options) -> Report {
    match backend.stack_bytes() {
        0 => run_here(placed, backend, opts),
        bytes => std::thread::scope(|scope| {
            std::thread::Builder::new()
                .stack_size(bytes)
                .name("beck-test".into())
                .spawn_scoped(scope, || run_here(placed, backend, opts))
                .expect("a thread for the tests")
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        }),
    }
}

fn run_here(placed: &Placed, backend: Arc<dyn Backend>, opts: &Options) -> Report {
    let mut cases = Vec::new();
    for t in &placed.program.tests {
        if let Some(f) = &opts.filter {
            if !t.name.contains(f.as_str()) {
                continue;
            }
        }
        cases.push(run_one(placed, backend.clone(), t, opts));
    }
    Report { cases }
}

/// Which of a test's clauses need an *application*, and are therefore not available to a library.
///
/// A library has no merge point, so it has no log to fold, no `validate` to propose through and no
/// page to render. Its `Placed` carries placeholder roles ([`beck_core::split::Placed::library`]),
/// and running one of those would report a pass for a test that asserted nothing — which is worse
/// than the refusal it replaced. So the clause is named and refused.
///
/// Everything else works: `expect <Bool>` over the module's own definitions, `property` blocks and
/// their generated inputs, `stub`, and the static expectations. That is the whole of a unit test for
/// a domain module, and it is what docs/22 §22.6 said was missing "for exactly the modules that most
/// want unit tests".
fn needs_an_application(t: &TestDef) -> Option<&'static str> {
    for clause in &t.clauses {
        match clause {
            Clause::Given { .. } => return Some("`given` folds a log, and a library has none"),
            Clause::When { .. } => {
                return Some("`when` proposes a command through `validate`, and a library has none")
            }
            Clause::Expect { what, .. } => match what {
                Expectation::PageContains { .. } | Expectation::PageMatchesSnapshot { .. } => {
                    return Some("`page` is the view of an application, and a library has none")
                }
                Expectation::FoldEquals { .. } => {
                    return Some("`fold_of` folds a log, and a library has none")
                }
                _ => {}
            },
            Clause::Stub { .. } => {}
        }
    }
    None
}

fn run_one(placed: &Placed, backend: Arc<dyn Backend>, t: &TestDef, opts: &Options) -> Case {
    if !placed.is_application() {
        if let Some(why) = needs_an_application(t) {
            return Case {
                name: t.name.clone(),
                outcome: Outcome::Failed {
                    why: format!(
                        "{why}. This module has no merge point, so it is a library: add \
                         `proposals: Stream[Proposal] = merge_clients()` and a `durable` fold to \
                         make it an application, or write this test over the module's own \
                         definitions"
                    ),
                },
                stubbed: Vec::new(),
                runs: 0,
            };
        }
    }
    let stubs = match build_stubs(placed, &backend, t) {
        Ok(s) => s,
        Err(why) => {
            return Case {
                name: t.name.clone(),
                outcome: Outcome::Failed { why },
                stubbed: Vec::new(),
                runs: 0,
            }
        }
    };

    // A test that only asks compile-time questions needs no execution at all — §21.2: "`beck test`
    // answers them without running anything".
    if t.is_static_only() {
        let outcome = match static_only(placed, t, opts) {
            Ok(()) => Outcome::Passed,
            Err(why) => Outcome::Failed { why },
        };
        return Case {
            name: t.name.clone(),
            outcome,
            stubbed: Vec::new(),
            runs: 0,
        };
    }

    let recorder = Arc::new(stubs);
    let needs_stubs = !recorder.entries.is_empty();
    let exec: Arc<dyn Backend> = if needs_stubs {
        match backend.intercepting(recorder.clone()) {
            Some(b) => b,
            None => {
                return Case {
                    name: t.name.clone(),
                    outcome: Outcome::Skipped(format!(
                        "the `{}` backend cannot install stubs, and this test's subject performs \
                         effects that must not run for real",
                        backend.name()
                    )),
                    stubbed: Vec::new(),
                    runs: 0,
                }
            }
        }
    } else {
        backend.clone()
    };

    let runtime = match Runtime::new(placed.clone(), exec) {
        Ok(r) => r,
        Err(e) => {
            return Case {
                name: t.name.clone(),
                outcome: Outcome::Failed {
                    why: format!("preparing the program: {e}"),
                },
                stubbed: Vec::new(),
                runs: 0,
            }
        }
    };

    let runs = if t.is_property() { opts.runs() } else { 1 };
    let mut ran = 0;
    for run in 0..runs {
        ran += 1;
        let inputs = match generate(placed, t, run) {
            Ok(v) => v,
            Err(why) => {
                return Case {
                    name: t.name.clone(),
                    outcome: Outcome::Failed { why },
                    stubbed: recorder.report(),
                    runs: ran,
                }
            }
        };
        if let Err(why) = execute(placed, &runtime, &recorder, t, &inputs, opts) {
            // §21.3 rule 5's shrinking: report the smallest input that still fails, because the
            // point of a generated counterexample is that a person can read it.
            let (inputs, why) = shrink_failure(placed, &runtime, &recorder, t, inputs, why, opts);
            let shown = describe_inputs(t, &inputs);
            return Case {
                name: t.name.clone(),
                outcome: Outcome::Failed {
                    why: if shown.is_empty() {
                        why
                    } else {
                        format!("{why}\n  with {shown}")
                    },
                },
                stubbed: recorder.report(),
                runs: ran,
            };
        }
    }

    let problems = recorder.problems.lock().expect("stub log").clone();
    if !problems.is_empty() {
        return Case {
            name: t.name.clone(),
            outcome: Outcome::Failed {
                why: problems.join("\n"),
            },
            stubbed: recorder.report(),
            runs: ran,
        };
    }

    Case {
        name: t.name.clone(),
        outcome: Outcome::Passed,
        stubbed: recorder.report(),
        runs: ran,
    }
}

/// §21.3 rules 1 and 2: everything is stubbed by default, and naming an effect overrides it.
fn build_stubs(
    placed: &Placed,
    backend: &Arc<dyn Backend>,
    t: &TestDef,
) -> Result<Recorder, String> {
    let program = &placed.program;
    // The stubs the test named. A plain value is evaluated once, here; a body over the call's
    // arguments (§21.3 rule 3) is prepared as a function and called per interception.
    let mut explicit: BTreeMap<Effect, Answer> = BTreeMap::new();
    for c in &t.clauses {
        if let Clause::Stub {
            atom,
            params,
            value,
            ..
        } = c
        {
            let answer = if params.is_empty() {
                Answer::Value(
                    backend
                        .constant(value)
                        .map_err(|e| format!("evaluating the stub for `{}`: {e}", atom.name()))?,
                )
            } else {
                let lam = Core {
                    kind: CoreKind::Lam {
                        params: params.clone(),
                        body: Box::new(value.clone()),
                    },
                    ty: Ty::fun(Vec::new(), value.ty.clone()),
                    tier: Tier::Any,
                    span: value.span,
                };
                Answer::FromTheCall(
                    backend
                        .function(&lam)
                        .map_err(|e| format!("preparing the stub for `{}`: {e}", atom.name()))?,
                )
            };
            explicit.insert(atom.clone(), answer);
        }
    }

    let mut entries = BTreeMap::new();
    for (name, def) in &program.defs {
        // The first atom this definition *performs* — not the first its row mentions. A row
        // propagates to callers, so matching on the row would stub `validate` itself and the test
        // would exercise nothing. See `beck_core::testing::performs_itself`.
        //
        // A definition performing two atoms is stubbed by whichever the test named, and by the
        // first otherwise, which is why the report prints the atom beside the definition rather
        // than leaving it to be guessed.
        let atom = def
            .effects
            .iter()
            .filter(|e| beck_core::testing::performs_itself(def, e))
            .find(|e| explicit.contains_key(e))
            .or_else(|| {
                def.effects
                    .iter()
                    .filter(|e| beck_core::testing::performs_itself(def, e))
                    .find(|e| beck_core::testing::is_auto_stubbable(e))
            });
        let Some(atom) = atom.cloned() else { continue };
        let (answer, refused, is_explicit) = match explicit.get(&atom) {
            Some(Answer::Value(v)) => (Answer::Value(v.clone()), None, true),
            Some(Answer::FromTheCall(f)) => (Answer::FromTheCall(f.clone()), None, true),
            None => match beck_core::gen::canonical(&def.ret, &program.types) {
                Ok(v) => (Answer::Value(v), None, false),
                Err(e) => (Answer::Value(Value::Unit), Some(e.to_string()), false),
            },
        };
        entries.insert(
            name.clone(),
            Entry {
                atom,
                answer,
                explicit: is_explicit,
                refused,
            },
        );
    }

    // An explicit stub for an atom nothing performs is a compile error (B0704), so anything left
    // here is a real entry.
    Ok(Recorder {
        entries,
        ..Default::default()
    })
}

/// A `property`'s inputs for one run. A `test` has none, and the vector is empty.
fn generate(placed: &Placed, t: &TestDef, run: u64) -> Result<Vec<Value>, String> {
    let mut rng = beck_core::gen::Rng::seeded(&t.name, run);
    let mut out = Vec::with_capacity(t.params.len());
    for (_, name, ty) in &t.params {
        let v = beck_core::gen::arbitrary(ty, &placed.program.types, &mut rng)
            .map_err(|e| format!("generating `{name}`: {e}"))?;
        out.push(v);
    }
    Ok(out)
}

fn describe_inputs(t: &TestDef, inputs: &[Value]) -> String {
    t.params
        .iter()
        .zip(inputs)
        .map(|((_, n, _), v)| format!("{n} = {}", v.display()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Shrink a failing `property` input to the smallest one that still fails.
///
/// Greedy and terminating: every candidate is strictly smaller by [`beck_core::gen::size`], so the
/// loop makes progress and stops. A `test` block has no inputs and this returns immediately.
fn shrink_failure(
    placed: &Placed,
    runtime: &Runtime,
    recorder: &Arc<Recorder>,
    t: &TestDef,
    inputs: Vec<Value>,
    why: String,
    opts: &Options,
) -> (Vec<Value>, String) {
    let mut best = inputs;
    let mut why = why;
    let mut improved = true;
    while improved {
        improved = false;
        for i in 0..best.len() {
            for candidate in beck_core::gen::shrink(&best[i]) {
                let mut next = best.clone();
                next[i] = candidate;
                if let Err(w) = execute(placed, runtime, recorder, t, &next, opts) {
                    best = next;
                    why = w;
                    improved = true;
                    break;
                }
            }
        }
    }
    (best, why)
}

/// Compare a rendered page against its checked-in snapshot, or write one.
///
/// `docs/21` §21.2 asked for this and named both the risk and the mitigation: "the risk is snapshot
/// rot, and the mitigation is the same one — review the diff." Three things follow from taking that
/// seriously rather than quoting it.
///
/// **A missing snapshot is a failure, not a silent write.** The first run of a new assertion has to
/// tell somebody it recorded nothing, or a test that has never compared anything reads as a test
/// that passes. It fails, and says which flag writes it.
///
/// **Writing is only ever `--update`.** A snapshot that rewrites itself when it disagrees asserts
/// nothing at all.
///
/// **The diff is in the failure.** A message that says two pages differ, without saying where, sends
/// the reader to a file comparison the harness could have done — so the first differing line is
/// named, with both sides.
fn snapshot(
    opts: &Options,
    test: &str,
    name: Option<&str>,
    actor: &str,
    rendered: &str,
) -> Result<(), String> {
    let dir = opts.base_dir.join("snapshots");
    let path = dir.join(format!("{}.html", snapshot_key(test, name, actor)));

    if opts.update_snapshots {
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        std::fs::write(&path, rendered).map_err(|e| format!("writing {}: {e}", path.display()))?;
        return Ok(());
    }

    let Ok(want) = std::fs::read_to_string(&path) else {
        return Err(format!(
            "no snapshot recorded at {}\n  \
             run `beck test --update` to write it, then review the file like any other diff",
            path.display()
        ));
    };
    if want == rendered {
        return Ok(());
    }
    Err(format!(
        "the page `{actor}` sees does not match {}\n{}",
        path.display(),
        first_difference(&want, rendered)
    ))
}

/// Sixty characters either side of `at`, with `…` where the line was cut.
fn window(line: &str, at: usize) -> String {
    const EITHER_SIDE: usize = 60;
    let floor = |i: usize| {
        (0..=i)
            .rev()
            .find(|&i| line.is_char_boundary(i))
            .unwrap_or(0)
    };
    let ceil = |i: usize| {
        (i..=line.len())
            .find(|&i| line.is_char_boundary(i))
            .unwrap_or(line.len())
    };
    let start = floor(at.saturating_sub(EITHER_SIDE));
    let end = ceil((at + EITHER_SIDE).min(line.len()));
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        &line[start..end],
        if end < line.len() { "…" } else { "" }
    )
}

/// A file name that is stable, readable, and cannot collide by accident.
///
/// The actor is part of the key because one test may assert two people's pages, and the two are
/// different snapshots of the same test. Everything outside `[A-Za-z0-9_-]` becomes `-`, so a test
/// called `"a user's page"` is a file somebody can open on any platform.
fn snapshot_key(test: &str, name: Option<&str>, actor: &str) -> String {
    let slug = |s: &str| -> String {
        let mut out = String::new();
        let mut dash = false;
        for c in s.chars() {
            if c.is_ascii_alphanumeric() || c == '_' {
                out.push(c);
                dash = false;
            } else if !dash && !out.is_empty() {
                out.push('-');
                dash = true;
            }
        }
        out.trim_end_matches('-').to_string()
    };
    format!("{}@{}", slug(name.unwrap_or(test)), slug(actor))
}

/// The first line that differs, with both sides — so a failure is readable without a second tool.
///
/// A rendered page is frequently **one very long line**, so eliding both sides from the start shows
/// two identical prefixes and hides the difference — which is the failure mode this whole message
/// exists to avoid (§4.5: an error message is a product surface). The window is therefore centred
/// on the first differing *character* rather than on the start of the line.
fn first_difference(want: &str, got: &str) -> String {
    for (i, (a, b)) in want.lines().zip(got.lines()).enumerate() {
        if a != b {
            let at = a
                .char_indices()
                .zip(b.char_indices())
                .find(|((_, x), (_, y))| x != y)
                .map(|((i, _), _)| i)
                .unwrap_or_else(|| a.len().min(b.len()));
            return format!(
                "  line {}, column {}:\n    snapshot: {}\n    rendered: {}",
                i + 1,
                a[..at].chars().count() + 1,
                window(a, at),
                window(b, at)
            );
        }
    }
    let (w, g) = (want.lines().count(), got.lines().count());
    if w == g {
        // Equal line-by-line and unequal overall: a trailing newline, which is exactly the
        // difference a reader would stare past.
        return "  the lines are identical and the files are not — a trailing newline differs"
            .to_string();
    }
    format!("  the snapshot has {w} lines and the page rendered {g}")
}

/// One pass through a test's clauses.
fn execute(
    placed: &Placed,
    runtime: &Runtime,
    recorder: &Arc<Recorder>,
    t: &TestDef,
    inputs: &[Value],
    opts: &Options,
) -> Result<(), String> {
    let program = &placed.program;
    let mut state = runtime
        .initial_state()
        .map_err(|e| format!("evaluating the initial state: {e}"))?;
    let mut events: Vec<Value> = Vec::new();
    let mut result = Value::ok(Value::List(Arc::new(Vec::new())));
    let mut seq: u64 = 0;
    let mut actor: Arc<str> = Arc::from(DEFAULT_ACTOR);

    for clause in &t.clauses {
        match clause {
            Clause::Given {
                events: code,
                actor: who,
                ..
            } => {
                let who = who.clone().unwrap_or_else(|| actor.clone());
                let log = eval(runtime, t, code, &state, &events, &result, inputs)?;
                let log = log
                    .as_list()
                    .cloned()
                    .ok_or_else(|| "`given` did not produce a list of events".to_string())?;
                for e in log {
                    seq += 1;
                    state = fold(runtime, &state, seq, &who, e)?;
                }
            }
            Clause::When {
                actor: who,
                commands,
                ..
            } => {
                if let Some(w) = who {
                    actor = w.clone();
                }
                for cmd in commands {
                    let c = eval(runtime, t, cmd, &state, &events, &result, inputs)?;
                    let proposal = runtime.proposal(&actor, c);
                    let out = runtime
                        .decide(&state, &proposal)
                        .map_err(|e| format!("`validate` failed: {e}"))?;
                    result = out.clone();
                    // Only an `Ok` reaches the log — the chokepoint is the chokepoint.
                    if out.variant() == Some("Ok") {
                        let produced = out
                            .field("value")
                            .and_then(|v| v.as_list())
                            .cloned()
                            .unwrap_or_default();
                        for e in produced {
                            events.push(e.clone());
                            seq += 1;
                            state = fold(runtime, &state, seq, &actor, e)?;
                        }
                    }
                }
            }
            Clause::Stub { .. } => {}
            Clause::Expect { what, .. } => match what {
                Expectation::Holds(code) => {
                    let v = eval(runtime, t, code, &state, &events, &result, inputs)?;
                    if v.as_bool() != Some(true) {
                        // §4.5: an error message is a product surface. "expected true, got false"
                        // is what a boolean assertion can say and no more, so a comparison — which
                        // is what almost every expectation is — reports both sides instead.
                        if let CoreKind::Prim {
                            op: beck_core::Prim::Eq,
                            args,
                        } = &code.kind
                        {
                            if args.len() == 2 {
                                let l =
                                    eval(runtime, t, &args[0], &state, &events, &result, inputs)?;
                                let r =
                                    eval(runtime, t, &args[1], &state, &events, &result, inputs)?;
                                return Err(format!(
                                    "these are not equal\n     is: {}\n  wanted: {}",
                                    elide(&l.display()),
                                    elide(&r.display())
                                ));
                            }
                        }
                        return Err(format!("expected true, got {}", v.display()));
                    }
                }
                Expectation::PageContains { needle, actor: who } => {
                    let n = eval(runtime, t, needle, &state, &events, &result, inputs)?;
                    let n = n.as_str().unwrap_or_default().to_string();
                    let who = who.clone().unwrap_or_else(|| actor.clone());
                    let page = runtime
                        .view(&state, &who)
                        .map_err(|e| format!("rendering the page for `{who}`: {e}"))?;
                    let rendered = page.render();
                    if !rendered.contains(&n) {
                        return Err(format!(
                            "the page `{who}` sees does not contain {n:?}\n  page: {}",
                            elide(&rendered)
                        ));
                    }
                }
                Expectation::PageMatchesSnapshot { name, actor: who } => {
                    let who = who.clone().unwrap_or_else(|| actor.clone());
                    let page = runtime
                        .view(&state, &who)
                        .map_err(|e| format!("rendering the page for `{who}`: {e}"))?;
                    snapshot(opts, &t.name, name.as_deref(), &who, &page.render())?;
                }
                Expectation::FoldEquals {
                    events: code,
                    actor: who,
                } => {
                    let log = eval(runtime, t, code, &state, &events, &result, inputs)?;
                    let log = log
                        .as_list()
                        .cloned()
                        .ok_or_else(|| "`fold_of` did not get a list of events".to_string())?;
                    let who = who.clone().unwrap_or_else(|| Arc::from(DEFAULT_ACTOR));
                    let mut other = runtime
                        .initial_state()
                        .map_err(|e| format!("evaluating the initial state: {e}"))?;
                    for (i, e) in log.into_iter().enumerate() {
                        other = fold(runtime, &other, i as u64 + 1, &who, e)?;
                    }
                    if digest(&state) != digest(&other) {
                        return Err(format!(
                            "the state is not the fold of that log\n     is: {}\n  wanted: {}",
                            elide(&state.display()),
                            elide(&other.display())
                        ));
                    }
                }
                Expectation::Performed { atom, how } => match how {
                    Count::Never => {
                        let n = recorder.count(atom);
                        if n != 0 {
                            return Err(format!(
                                "`{}` was performed {n} time(s), and the test says it is not",
                                atom.name()
                            ));
                        }
                    }
                    Count::Times(k) => {
                        let n = recorder.count(atom) as i64;
                        if n != *k {
                            return Err(format!(
                                "`{}` was performed {n} time(s), not {k}",
                                atom.name()
                            ));
                        }
                    }
                    Count::With(code) => {
                        let want = eval(runtime, t, code, &state, &events, &result, inputs)?;
                        if !recorder.called_with(atom, &want) {
                            return Err(format!(
                                "`{}` was never performed with {}",
                                atom.name(),
                                want.display()
                            ));
                        }
                    }
                },
                Expectation::Place { .. }
                | Expectation::Flow { .. }
                | Expectation::WireCompatible { .. } => {
                    check_static(placed, what, opts)?;
                }
            },
        }
    }
    let _ = program;
    Ok(())
}

fn static_only(placed: &Placed, t: &TestDef, opts: &Options) -> Result<(), String> {
    for c in &t.clauses {
        if let Clause::Expect { what, .. } = c {
            check_static(placed, what, opts)?;
        }
    }
    Ok(())
}

/// The assertions answered from the compiler's own data.
fn check_static(placed: &Placed, what: &Expectation, opts: &Options) -> Result<(), String> {
    match what {
        Expectation::Place { what: name, tier } => {
            let actual = placed
                .program
                .defs
                .get(name.as_ref())
                .map(|d| d.tier)
                .or_else(|| {
                    placed
                        .program
                        .signals
                        .iter()
                        .find(|s| s.name == *name)
                        .map(|s| s.tier)
                })
                .ok_or_else(|| format!("`{name}` is not a definition or a signal"))?;
            if actual != *tier {
                return Err(format!(
                    "`{name}` is placed on `{}`, not `{}`",
                    actual.name(),
                    tier.name()
                ));
            }
            Ok(())
        }
        Expectation::Flow { ty, tier } => {
            // An unplaced-pure definition is compiled to whichever tier calls it (§3.3), so it
            // reaches *every* tier. Counting it as reaching only `any` would make this assertion
            // pass for the most dangerous case there is.
            let reached: Vec<String> = beck_core::secure::flow(&placed.program, ty)
                .into_iter()
                .filter(|r| r.tier == *tier || r.tier == Tier::Any)
                .map(|r| format!("{} ({})", r.what, r.tier.name()))
                .collect();
            if !reached.is_empty() {
                return Err(format!(
                    "`{ty}` reaches {} on `{}`",
                    reached.join(", "),
                    tier.name()
                ));
            }
            Ok(())
        }
        Expectation::WireCompatible { path } => {
            let file = opts.base_dir.join(path.as_ref());
            let src = std::fs::read_to_string(&file)
                .map_err(|e| format!("reading `{}`: {e}", file.display()))?;
            let mut diags = beck_diag::Diagnostics::new();
            let mut map = beck_diag::SourceMap::new();
            let previous =
                beck_core::Interface::parse(&placed.program.name, &src, &mut map, &mut diags);
            if diags.has_errors() {
                return Err(format!("`{}` is not a readable interface", file.display()));
            }
            let current = beck_core::Interface::of(&placed.program);
            let changes = beck_core::compare(&previous, &current);
            if beck_core::is_breaking(&changes) {
                let why: Vec<String> = changes
                    .iter()
                    .filter(|c| c.severity == beck_core::compat::Severity::Breaking)
                    .map(|c| format!("{}: {}", c.what, c.because))
                    .collect();
                return Err(format!(
                    "not wire-compatible with `{path}`\n  {}",
                    why.join("\n  ")
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn fold(
    runtime: &Runtime,
    state: &Value,
    seq: u64,
    actor: &str,
    event: Value,
) -> Result<Value, String> {
    // `at` is the sequence position, not a clock. §3.7 makes time data on the envelope, so a test
    // that reads `env.at` reads something reproducible instead of something that moves.
    let env = Envelope {
        seq,
        at: Instant(seq as i64),
        actor: actor.to_string(),
        body: event.clone(),
    };
    runtime
        .fold(state, &env, event)
        .map_err(|e| format!("folding at seq {seq}: {e}"))
}

/// Evaluate a clause expression with `state`, `events`, `result` and a property's parameters bound.
///
/// The expression is wrapped as a lambda and prepared through [`Backend::function`], so this goes
/// through the same seam the runtime's roles do rather than reaching into a particular evaluator.
fn eval(
    runtime: &Runtime,
    t: &TestDef,
    code: &Core,
    state: &Value,
    events: &[Value],
    result: &Value,
    inputs: &[Value],
) -> Result<Value, String> {
    let mut params: Vec<VarId> = vec![t.bindings.state, t.bindings.events, t.bindings.result];
    params.extend(t.params.iter().map(|(id, _, _)| *id));
    let lam = Core {
        kind: CoreKind::Lam {
            params,
            body: Box::new(code.clone()),
        },
        ty: Ty::fun(Vec::new(), code.ty.clone()),
        tier: Tier::Any,
        span: code.span,
    };
    let f = runtime
        .prepare(&lam)
        .map_err(|e| format!("preparing an expectation: {e}"))?;
    let mut args = vec![
        state.clone(),
        Value::List(Arc::new(events.to_vec())),
        result.clone(),
    ];
    args.extend(inputs.iter().cloned());
    f(args).map_err(|e| e.to_string())
}

fn elide(s: &str) -> String {
    if s.chars().count() <= 200 {
        s.to_string()
    } else {
        let head: String = s.chars().take(200).collect();
        format!("{head}…")
    }
}

/// The console form, so `beck test` and a harness print the same thing.
pub fn render(report: &Report, verbose: bool) -> String {
    let mut out = String::new();
    for c in &report.cases {
        let mark = match &c.outcome {
            Outcome::Passed => "ok".to_string(),
            Outcome::Failed { .. } => "FAILED".to_string(),
            Outcome::Skipped(_) => "skipped".to_string(),
        };
        let runs = if c.runs > 1 {
            format!(" ({} inputs)", c.runs)
        } else {
            String::new()
        };
        out.push_str(&format!("test {:?} … {mark}{runs}\n", c.name));
        match &c.outcome {
            Outcome::Failed { why } => {
                for line in why.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
            }
            Outcome::Skipped(why) => out.push_str(&format!("  {why}\n")),
            Outcome::Passed => {}
        }
        // §21.3: "which is the thing a hidden default must always do: say what it did."
        let shown: Vec<&Stubbed> = c
            .stubbed
            .iter()
            .filter(|s| verbose || s.calls > 0)
            .collect();
        if !shown.is_empty() && (verbose || matches!(c.outcome, Outcome::Failed { .. })) {
            out.push_str("  stubbed:\n");
            for s in shown {
                let how = match (s.explicit, s.from_the_call) {
                    (true, true) => "named, from the call",
                    (true, false) => "named",
                    (false, _) => "automatically",
                };
                let answered = match &s.returned {
                    Some(v) => v.display(),
                    None => "—".into(),
                };
                out.push_str(&format!(
                    "    {:<24} by `{}`  → {answered}   called {}× ({how})\n",
                    s.atom, s.def, s.calls
                ));
            }
        }
    }
    out.push_str(&format!(
        "\n{} passed, {} failed, {} skipped\n",
        report.passed(),
        report.failed(),
        report.skipped()
    ));
    out
}
