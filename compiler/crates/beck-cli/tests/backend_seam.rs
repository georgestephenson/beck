//! The `Backend` seam, exercised by something that is not the evaluator.
//!
//! A trait with one implementation is a claim, not a fact. `beck-rt` does not depend on any backend
//! crate — that is checked by the build, since a missing dependency does not compile — but nothing
//! stops the *interface* from being shaped so that only a tree-walker could satisfy it.
//!
//! So this harness drives the whole runtime through a backend the runtime has never heard of. When
//! `docs/04-compiler-architecture.md` §4.8's differential test between a native backend and this
//! one gets written, it is this file's shape: two `Runtime`s over one `Placed`, and an assertion
//! that they cannot be told apart.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use beck_core::backend::{Backend, Callable, ExecError};
use beck_core::{Core, Value};
use beck_rt::{App, AppConfig, MemoryLog, Runtime};

mod support;
use support::{command, todo_program};

/// Wraps another backend and counts what the runtime asks of it.
///
/// Deliberately *not* an evaluator: the point is that the runtime's contract with a backend is
/// exactly [`Backend`], and anything satisfying it can be substituted without the runtime knowing.
struct Counting {
    inner: Arc<dyn Backend>,
    prepared: AtomicUsize,
    calls: Arc<AtomicUsize>,
}

impl Backend for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }

    fn constant(&self, code: &Core) -> Result<Value, ExecError> {
        self.inner.constant(code)
    }

    /// Forwarded, and that is the point of the test below: what a wrapper forgets to pass on, the
    /// runtime never learns. A backend that needs host stack and a wrapper that swallows the
    /// number is an abort waiting for a deep program (`docs/27` §27.2).
    fn stack_bytes(&self) -> usize {
        self.inner.stack_bytes()
    }

    fn function(&self, code: &Core) -> Result<Callable, ExecError> {
        self.prepared.fetch_add(1, Ordering::Relaxed);
        let f = self.inner.function(code)?;
        // Captured into the callable, which is where a compiling backend would capture its own
        // compiled artefact — the reason `function` returns a closure rather than a handle.
        let calls = self.calls.clone();
        Ok(Arc::new(move |args| {
            calls.fetch_add(1, Ordering::Relaxed);
            f(args)
        }))
    }
}

#[tokio::test]
async fn the_runtime_drives_a_backend_it_has_never_heard_of() {
    let placed = todo_program();
    let counting = Arc::new(Counting {
        inner: beck_eval::backend(&placed),
        prepared: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let runtime = Runtime::new(placed, counting.clone()).expect("prepares");

    // The runtime reports whichever backend it was given — the hook a differential report needs to
    // say *which* of two backends produced the answer that differed.
    assert_eq!(runtime.backend(), "counting");
    // Everything the runtime will ever call is prepared once, at startup: the three roles, plus
    // every operator of the view's dataflow plan (§5.3). A backend that compiles depends on this,
    // and so does the fanout — preparing the plan per *subscription* cost about 90 KB each until
    // `Prepared` was split out of `Engine` (docs/24 §23.8).
    let plan = runtime.plan();
    let operators = plan
        .nodes
        .iter()
        .filter(|n| {
            matches!(
                n.op,
                beck_core::plan::Op::Pointwise { .. }
                    | beck_core::plan::Op::MapList { .. }
                    | beck_core::plan::Op::FilterList { .. }
                    | beck_core::plan::Op::SortBy { .. }
                    | beck_core::plan::Op::FlatMap { .. }
            )
        })
        .count();
    let at_startup = counting.prepared.load(Ordering::Relaxed);
    assert_eq!(
        at_startup,
        3 + operators,
        "validate, the fold, the view, and {operators} plan operators"
    );

    // A hundred subscriptions prepare nothing: they share the prepared plan and hold only their
    // own arrangements.
    let engines: Vec<_> = (0..100)
        .map(|_| runtime.view_engine().expect("an engine"))
        .collect();
    assert_eq!(
        counting.prepared.load(Ordering::Relaxed),
        at_startup,
        "a subscription prepared code of its own"
    );
    drop(engines);

    let app = App::start(runtime, Arc::new(MemoryLog::new()), AppConfig::default())
        .await
        .expect("app starts");

    for i in 0..5 {
        app.propose(
            format!("k{i}"),
            "alice",
            command(
                "Add",
                &[("id", &format!("t{i}")), ("text", &format!("todo {i}"))],
            ),
        )
        .await
        .expect("accepted");
    }

    // …and the whole stack works over it: validate ran, the fold ran, the view rendered.
    assert_eq!(app.head(), 5);
    assert_eq!(
        counting.prepared.load(Ordering::Relaxed),
        at_startup,
        "an event prepared code"
    );
    let html = app.render("alice").await.expect("render").render();
    assert!(html.contains("todo 4"), "{html}");

    // Every role went through the seam. Five validates, five folds, and at least one view — if the
    // runtime had kept a private path to the evaluator, this count would be short.
    assert!(
        counting.calls.load(Ordering::Relaxed) >= 11,
        "only {} calls reached the backend",
        counting.calls.load(Ordering::Relaxed)
    );
}

#[tokio::test]
async fn two_backends_over_one_program_agree() {
    // The shape §4.8 asks for. Today both sides are the same evaluator wearing different names, so
    // this asserts the *harness* is sound rather than that two implementations agree — which is
    // worth having in place before there is a second implementation to point it at, and is stated
    // here rather than left for a reader to discover.
    let a = {
        let placed = todo_program();
        let backend = beck_eval::backend(&placed);
        Runtime::new(placed, backend).expect("prepares")
    };
    let b = {
        let placed = todo_program();
        let backend: Arc<dyn Backend> = Arc::new(Counting {
            inner: beck_eval::backend(&placed),
            prepared: AtomicUsize::new(0),
            calls: Arc::new(AtomicUsize::new(0)),
        });
        Runtime::new(placed, backend).expect("prepares")
    };
    assert_ne!(
        a.backend(),
        b.backend(),
        "the harness must compare two backends"
    );

    let mut state_a = a.initial_state().expect("init");
    let mut state_b = b.initial_state().expect("init");
    assert_eq!(beck_core::digest(&state_a), beck_core::digest(&state_b));

    for i in 0..8 {
        let cmd = command(
            "Add",
            &[("id", &format!("t{i}")), ("text", &format!("todo {i}"))],
        );
        let (pa, pb) = (a.proposal("alice", cmd.clone()), b.proposal("alice", cmd));

        let events_a = a.validate(&state_a, &pa).expect("a accepts");
        let events_b = b.validate(&state_b, &pb).expect("b accepts");
        assert_eq!(events_a.len(), events_b.len(), "at {i}");

        for (n, (ea, eb)) in events_a.into_iter().zip(events_b).enumerate() {
            assert_eq!(
                beck_core::digest(&ea),
                beck_core::digest(&eb),
                "event {n} at {i}"
            );
            let env = envelope(i as u64 + 1);
            state_a = a.fold(&state_a, &env, ea).expect("a folds");
            state_b = b.fold(&state_b, &env, eb).expect("b folds");
        }

        // Digests, not equality: the digest is what §4.8's replay harness compares, so a backend
        // that produced an equal-but-differently-shaped value would still be caught.
        assert_eq!(
            beck_core::digest(&state_a),
            beck_core::digest(&state_b),
            "the two backends diverged after {} events",
            i + 1
        );
        assert_eq!(
            a.view(&state_a, "alice").expect("a renders").render(),
            b.view(&state_b, "alice").expect("b renders").render(),
        );
    }
}

fn envelope(seq: u64) -> beck_rt::Envelope {
    beck_rt::Envelope {
        seq,
        at: beck_rt::Instant(0),
        actor: "alice".into(),
        body: beck_core::Value::Unit,
    }
}

/// The newest thing on the seam, and the one a second backend is most likely to get wrong.
///
/// [`Backend::stack_bytes`] exists because the runtime spawns threads and may not name a backend
/// crate: it has to *ask*. Three claims, and none of them is about the evaluator's number being
/// right — only about the seam carrying it.
#[test]
fn the_seam_carries_how_much_host_stack_a_backend_needs() {
    let placed = todo_program();
    let evaluator = beck_eval::backend(&placed);

    // A tree-walker nests host frames on the program's recursion and says so.
    assert_eq!(
        evaluator.stack_bytes(),
        beck_eval::STACK_BYTES,
        "the evaluator declares the stack its depth ceiling needs"
    );

    // A backend that does not is answering "whatever the caller has", which is the right answer
    // for anything that compiles to a loop — and it must not be forced to invent a number.
    struct Compiled;
    impl Backend for Compiled {
        fn name(&self) -> &'static str {
            "compiled"
        }
        fn constant(&self, _: &Core) -> Result<Value, ExecError> {
            unreachable!("nothing here executes")
        }
        fn function(&self, _: &Core) -> Result<Callable, ExecError> {
            unreachable!("nothing here executes")
        }
    }
    assert_eq!(
        Compiled.stack_bytes(),
        0,
        "the default is zero, so the seam costs a compiling backend nothing"
    );

    // And a wrapper has to pass it through, or the runtime under-provisions a backend that needs
    // the stack for a reason the wrapper knows nothing about.
    let wrapped = Counting {
        inner: evaluator,
        prepared: AtomicUsize::new(0),
        calls: Arc::new(AtomicUsize::new(0)),
    };
    assert_eq!(
        wrapped.stack_bytes(),
        beck_eval::STACK_BYTES,
        "a backend that wraps another inherits its requirement"
    );

    // The interceptor `beck test` installs makes a *new* backend out of the old one, which is the
    // same trap one layer down.
    let intercepting = beck_eval::backend(&placed)
        .intercepting(Arc::new(NeverIntercepts))
        .expect("the evaluator can intercept");
    assert_eq!(
        intercepting.stack_bytes(),
        beck_eval::STACK_BYTES,
        "and so does the one `beck test` swaps in"
    );
}

struct NeverIntercepts;
impl beck_core::backend::Interceptor for NeverIntercepts {
    fn intercept(&self, _: &str, _: &[Value]) -> Option<Value> {
        None
    }
}
