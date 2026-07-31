//! The `beck test` runner — [`docs/21-tests-in-beck-and-proof.md`] §21.2 and §21.3, executed.
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

use beck_core::backend::{Backend, Interceptor};
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
    /// Where a `expect wire_compatible_with "…"` path is resolved from.
    pub base_dir: std::path::PathBuf,
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
    pub returned: Value,
    pub calls: usize,
    /// True when the test named it, false when §21.3 rule 1 supplied it.
    pub explicit: bool,
}

// ---------------------------------------------------------------------------------------------
// The stub table
// ---------------------------------------------------------------------------------------------

/// One entry per definition a stub stands in for. The unit is a *definition* because that is what
/// gets called; the *identity* is the effect atom, which is what the test names.
#[derive(Clone, Debug)]
struct Entry {
    atom: Effect,
    value: Value,
    explicit: bool,
    /// Set when the generator refused to invent a return value. §21.3 rule 5: "it can refuse, with
    /// a diagnostic, for a type with no inhabitant it can construct". The refusal has to reach the
    /// person, so it is carried to the point of use and reported as a failure of the test that
    /// reached it — not swallowed, and not turned into a real call.
    refused: Option<String>,
}

#[derive(Debug, Default)]
struct Recorder {
    entries: BTreeMap<Arc<str>, Entry>,
    calls: Mutex<Vec<(Arc<str>, Vec<Value>)>>,
    demanded: Mutex<Vec<String>>,
}

impl Interceptor for Recorder {
    fn intercept(&self, name: &str, args: &[Value]) -> Option<Value> {
        let e = self.entries.get(name)?;
        if let Some(why) = &e.refused {
            self.demanded
                .lock()
                .expect("stub log")
                .push(format!("`{name}` ({}): {why}", e.atom.name()));
        }
        self.calls
            .lock()
            .expect("stub log")
            .push((Arc::from(name), args.to_vec()));
        Some(e.value.clone())
    }
}

impl Recorder {
    fn count(&self, atom: &Effect) -> usize {
        self.calls
            .lock()
            .expect("stub log")
            .iter()
            .filter(|(n, _)| self.entries.get(n).map(|e| &e.atom) == Some(atom))
            .count()
    }

    fn called_with(&self, atom: &Effect, wanted: &Value) -> bool {
        let want = digest(wanted);
        self.calls
            .lock()
            .expect("stub log")
            .iter()
            .filter(|(n, _)| self.entries.get(n).map(|e| &e.atom) == Some(atom))
            .any(|(_, args)| args.iter().any(|a| digest(a) == want))
    }

    fn report(&self) -> Vec<Stubbed> {
        let calls = self.calls.lock().expect("stub log");
        let mut out: Vec<Stubbed> = self
            .entries
            .iter()
            .map(|(def, e)| Stubbed {
                def: def.clone(),
                atom: e.atom.name(),
                returned: e.value.clone(),
                calls: calls.iter().filter(|(n, _)| n == def).count(),
                explicit: e.explicit,
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
pub fn run(placed: &Placed, backend: Arc<dyn Backend>, opts: &Options) -> Report {
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

fn run_one(placed: &Placed, backend: Arc<dyn Backend>, t: &TestDef, opts: &Options) -> Case {
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

    let demanded = recorder.demanded.lock().expect("stub log").clone();
    if !demanded.is_empty() {
        return Case {
            name: t.name.clone(),
            outcome: Outcome::Failed {
                why: format!(
                    "the generator cannot invent a return value for {}\n  write the stub out: \
                     `stub <atom>: <value>`",
                    demanded.join("; ")
                ),
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
    let mut explicit: BTreeMap<Effect, Value> = BTreeMap::new();
    for c in &t.clauses {
        if let Clause::Stub { atom, value, .. } = c {
            let v = backend
                .constant(value)
                .map_err(|e| format!("evaluating the stub for `{}`: {e}", atom.name()))?;
            explicit.insert(atom.clone(), v);
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
        let (value, refused, is_explicit) = match explicit.get(&atom) {
            Some(v) => (v.clone(), None, true),
            None => match beck_core::gen::canonical(&def.ret, &program.types) {
                Ok(v) => (v, None, false),
                Err(e) => (Value::Unit, Some(e.to_string()), false),
            },
        };
        entries.insert(
            name.clone(),
            Entry {
                atom,
                value,
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
            let previous = beck_core::Interface::parse(&placed.program.name, &src, &mut diags);
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
                let how = if s.explicit { "named" } else { "automatically" };
                out.push_str(&format!(
                    "    {:<24} by `{}`  → {}   called {}× ({how})\n",
                    s.atom,
                    s.def,
                    s.returned.display(),
                    s.calls
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
