//! What a `parallel:` scope buys, and what it costs when it buys nothing.
//!
//! Release-only by the convention every measurement suite here follows:
//!
//! ```text
//! cargo test --release --test measure_concurrency -- --nocapture
//! ```
//!
//! # The number `docs/80` §80.5 said nobody had
//!
//! > two children of a scope are worth running together exactly when each costs more than a thread,
//! > which is a number nobody here has.
//!
//! This is that number, measured from both sides rather than asserted:
//!
//! 1. **What a thread costs**, as the overhead a scope pays over running its children in order —
//!    measured on children that cost as close to nothing as a child can.
//! 2. **What a scope saves**, on children that wait. A `parallel:` over two outbound calls is the
//!    case the form exists for (`concurrency.rs::two_outbound_calls_are_what_the_form_is_for`), and
//!    a host that sleeps is the honest stand-in for a peer: the thing being measured is whether the
//!    waits overlap, and a real peer would add variance without adding meaning.
//!
//! Both at **two sizes**, because one measurement cannot tell a constant from a slope
//! (`AGENTS.md`), and the two sizes here are the two that decide the question: children that cost
//! far less than a thread, and children that cost far more.
//!
//! # What is asserted, and what is only printed
//!
//! The **rates are printed**. [`docs/13-testing.md`](../../../../docs/13-testing.md) §13.7: a
//! timing gate on a shared runner cannot be held honestly, and the crossover is a property of the
//! machine rather than of the language.
//!
//! What is asserted is the **direction on the side where it cannot be close**: two children that
//! each wait 200 ms must finish in well under the 400 ms an ordered join would take. That is not a
//! threshold anybody tuned — it is the difference between overlapping and not — and the
//! deadlock-or-pass proof that they overlap at all lives in `concurrency.rs`, with no clock in it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use beck_core::backend::Backend;
use beck_core::{Program, Value};

fn compile(src: &str) -> Arc<Program> {
    let (placed, diags, map) = beck_core::compile_or_library_str("scope.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    Arc::new(placed.expect("this program compiles").program)
}

/// The median of `runs` timings, because one wall-clock reading of anything is mostly noise.
fn median(runs: usize, mut f: impl FnMut()) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect();
    times.sort();
    times[times.len() / 2]
}

/// A host whose `fetch` sleeps, which is what a peer does.
///
/// Sleeping rather than calling one: the thing being measured is whether two waits overlap, and a
/// real peer would add variance without adding meaning. It is also the one honest way to state a
/// child's cost in the units the crossover is denominated in.
#[derive(Debug)]
struct Slow(Duration);

impl beck_core::host::Atoms for Slow {
    fn fetch(
        &self,
        request: &beck_core::net::Request,
    ) -> Result<beck_core::net::Reply, beck_core::net::Failure> {
        std::thread::sleep(self.0);
        Ok(beck_core::net::Reply {
            status: 200,
            headers: Vec::new(),
            body: Arc::from(&*request.path),
        })
    }
}

/// A scope over two children that wait, and the same two waits in order.
const WAITING: &str = r#"
def one(x: Str) -> Str uses net.out(a.example.com), raises(HttpError):
    return http_fetch("a.example.com", HttpRequest(method="GET", path=x, headers={}, body="", port=80, tls=False, secrets={})).body

def two(x: Str) -> Str uses net.out(b.example.com), raises(HttpError):
    return http_fetch("b.example.com", HttpRequest(method="GET", path=x, headers={}, body="", port=80, tls=False, secrets={})).body

def together(x: Str) -> Str uses net.out(a.example.com), net.out(b.example.com), spawn, raises(HttpError):
    return parallel:
        a = one(x)
        b = two(x)
        a + b

## The same work, written as a sequence — the control, and the thing a scope has to beat.
def in_order(x: Str) -> Str uses net.out(a.example.com), net.out(b.example.com), raises(HttpError):
    return one(x) + two(x)
"#;

/// A scope over two children that compute, and the same two computations in order.
///
/// Deliberately the *cheapest* thing a child can be: what is being measured is the scope's
/// overhead, and a child that did real work would hide it.
const COMPUTING: &str = r#"
## Iterative — a call in tail position does not nest (`docs/27` §27.2), so the size below is a
## measurement of work rather than of the depth ceiling.
def counting(n: Int, acc: Int) -> Int:
    if n <= 0:
        return acc
    return counting(n - 1, acc + 1)

def count(n: Int) -> Int:
    return counting(n, 0)

def together(n: Int) -> Int uses spawn:
    return parallel:
        a = count(n)
        b = count(n)
        a + b

def in_order(n: Int) -> Int:
    return count(n) + count(n)
"#;

fn call(program: &Arc<Program>, atoms: Arc<dyn beck_core::host::Atoms>, name: &str, arg: Value) {
    let backend = beck_eval::Evaluator::new(program.clone()).answering(atoms);
    beck_eval::on_the_evaluator_stack(|| {
        let f = backend
            .function(&program.defs[name].body)
            .expect("prepares");
        f(vec![arg]).expect("answers");
    });
}

/// What a scope saves on children that wait — the side the form exists for.
#[test]
fn what_a_scope_saves_when_its_children_wait() {
    let program = compile(WAITING);
    println!(
        "\n{:<12} {:>12} {:>12} {:>10}",
        "each waits", "in order", "together", "saved"
    );
    let mut ratios = Vec::new();
    for wait in [Duration::from_millis(20), Duration::from_millis(200)] {
        let atoms: Arc<dyn beck_core::host::Atoms> = Arc::new(Slow(wait));
        let ordered = median(5, || {
            call(&program, atoms.clone(), "in_order", Value::str_("/x"));
        });
        let scoped = median(5, || {
            call(&program, atoms.clone(), "together", Value::str_("/x"));
        });
        let ratio = ordered.as_secs_f64() / scoped.as_secs_f64();
        ratios.push((wait, ordered, scoped, ratio));
        println!(
            "{:<12} {:>12} {:>12} {:>9.2}×",
            format!("{wait:?}"),
            format!("{ordered:?}"),
            format!("{scoped:?}"),
            ratio
        );
    }

    // The one assertion, and it is on the side where it cannot be close: two children that each
    // wait 200 ms overlap or they do not, and an ordered join takes 400 ms. Anything under 300 is
    // unambiguous and no threshold anybody tuned.
    let (_, _, scoped, _) = ratios.last().expect("two sizes");
    assert!(
        *scoped < Duration::from_millis(300),
        "two children that each wait 200ms took {scoped:?} together, which is an ordered join"
    );
    println!(
        "\nThe ceiling is 2× for two children and it is approached from below, because the scope \
         still\npays for the threads. What that costs is the row below."
    );
}

/// Where the crossover is: what a child has to cost before a thread is worth it.
///
/// The other half of §80.5's question, and the half a reader needs in order to decide *not* to
/// write a scope. Children that compute are not a lost cause — two of them on two cores is the same
/// 2× two waits get — so what this measures is not "compute loses" but **the size at which it stops
/// losing**, which is the thread's own cost.
///
/// Nothing is asserted: the crossover is a property of the machine and of how many cores it has.
/// The number worth carrying away is the shape — the overhead is a **constant per child**, so it
/// dominates a child cheaper than itself and disappears into one that is dearer.
#[test]
fn what_a_child_must_cost_before_a_thread_is_worth_it() {
    let program = compile(COMPUTING);
    let atoms: Arc<dyn beck_core::host::Atoms> = Arc::new(beck_core::host::ProcessAtoms);
    println!(
        "\n{:<12} {:>14} {:>14} {:>14} {:>10}",
        "each counts", "one child", "in order", "together", "ratio"
    );
    let mut overheads = Vec::new();
    for n in [8i64, 1_000, 20_000, 100_000] {
        let alone = median(9, || {
            call(&program, atoms.clone(), "count", Value::Int(n));
        });
        let ordered = median(9, || {
            call(&program, atoms.clone(), "in_order", Value::Int(n));
        });
        let scoped = median(9, || {
            call(&program, atoms.clone(), "together", Value::Int(n));
        });
        let ratio = ordered.as_secs_f64() / scoped.as_secs_f64();
        overheads.push((n, alone, ratio));
        println!(
            "{:<12} {:>14} {:>14} {:>14} {:>9.2}×",
            n,
            format!("{alone:?}"),
            format!("{ordered:?}"),
            format!("{scoped:?}"),
            ratio
        );
    }
    println!(
        "\nThe `ratio` column crosses 1.00 where a child costs about what a thread does. Below it \
         a\nscope is a loss and above it the ceiling is the core count — the same 2× two waits get, \
         for\nthe same reason. A thread here is its own stack reservation and its own globals \
         cache, and\n`docs/117` §117.5 is what each of those is worth."
    );
}

/// Where a child's fixed cost goes.
///
/// The row above says a scope costs about a tenth of a millisecond per child. `AGENTS.md` says a
/// number like that is a design question rather than a fact to write down, so this asks where it
/// goes — and the answer is not the one the question expects. Most of it is the **thread**, which
/// on this machine is expensive before anybody has reserved anything; the 256 MiB stack
/// [`beck_eval::STACK_BYTES`] asks for adds the rest.
///
/// Neither half is a knob. The reservation is what the evaluator's depth ceiling needs in order to
/// be a diagnostic instead of a `SIGSEGV` (`docs/adr/0007`), and a `parallel:` child may recurse
/// exactly as deep as anything else; the thread is what running two things at once *is*.
#[test]
fn where_a_childs_fixed_cost_goes() {
    let spawn = |bytes: Option<usize>| {
        median(99, || {
            let mut b = std::thread::Builder::new();
            if let Some(bytes) = bytes {
                b = b.stack_size(bytes);
            }
            b.spawn(|| {}).expect("a thread").join().expect("joins");
        })
    };
    let bare = spawn(None);
    let evaluators = spawn(Some(beck_eval::STACK_BYTES));
    println!(
        "\n{:<34} {:>14}\n{:<34} {:>14}\n{:<34} {:>14}",
        "a thread with the default stack",
        format!("{bare:?}"),
        format!("a thread with {} MiB", beck_eval::STACK_BYTES >> 20),
        format!("{evaluators:?}"),
        "the difference",
        format!("{:?}", evaluators.saturating_sub(bare))
    );
    println!(
        "\nThe reservation is address space rather than memory — pages are committed as they are\n\
         touched — and it is the *smaller* half: on this machine a bare thread already costs most\n\
         of what a child costs, and the stack adds the rest. Neither is a knob. The reservation is\n\
         what the depth ceiling needs, and the thread is what running two things at once is."
    );
}

/// What the cancellation check costs a program that never writes `parallel:`.
///
/// Cancellation rides the **step counter** — `Interp::burn`, the one path every evaluation step
/// passes through, and the path [`docs/70`](../../../../docs/70-the-evaluator-gets-fast-report.md)
/// spent a chapter on. `AGENTS.md` says a cost is part of a change's correctness rather than a
/// follow-up to it, so this is that cost, on the program that pays it for nothing: no scope, no
/// child, `cancel` is `None`, and the check is a branch on a discriminant.
///
/// Nothing is asserted — a timing gate on a shared runner cannot be held honestly
/// ([`docs/13`](../../../../docs/13-testing.md) §13.7) — and the shape claim is the one worth
/// reading: the branch is **loop-invariant**, so what would show here is a constant per step and
/// not a slope. Two sizes are what say which it is.
#[test]
fn what_the_cancellation_check_costs_a_program_without_a_scope() {
    let program = compile(COMPUTING);
    let atoms: Arc<dyn beck_core::host::Atoms> = Arc::new(beck_core::host::ProcessAtoms);
    println!(
        "\n{:<14} {:>14} {:>18}",
        "iterations", "in order", "per iteration"
    );
    for n in [20_000i64, 200_000] {
        let took = median(9, || {
            call(&program, atoms.clone(), "in_order", Value::Int(n));
        });
        // Two children of `n` each. Per *iteration* and not per evaluation step: an iteration
        // is an `if`, a comparison, two calls and an add, and this suite does not count nodes.
        let per = took.as_secs_f64() / (2.0 * n as f64) * 1e9;
        println!("{:<14} {:>14} {:>15.1} ns", 2 * n, format!("{took:?}"), per);
    }
    println!(
        "\nA cost per iteration that grew with the number of iterations would be a check that is\n\
         not loop-invariant. `docs/118` §118.4 reads these against the same two numbers taken with\n\
         the check removed."
    );
}
