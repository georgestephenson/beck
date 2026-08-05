//! §5.3's one shared dataflow, held to account.
//!
//! [`docs/05-tier-lowering.md`](../../../../docs/05-tier-lowering.md) §5.3:
//!
//! > a thousand connected users of `todos.map(filter_by(session.user))` must compile to *one*
//! > shared dataflow whose final per-session operators (filter, project, diff) run per subscriber
//!
//! [`docs/24-incremental-views-report.md`](../../../../docs/24-incremental-views-report.md) §24.7
//! recorded that as identified and not done, and named what blocked it: not the analysis, the
//! runtime. Subscribers render at different times, so a shared arrangement has to answer a question
//! a per-subscriber one never does — *what changed since **you** last looked* — and a subscriber
//! that fell behind needs either a version history or a rebuild.
//!
//! This file is where that answer is wrong if it is wrong. `incremental_engine.rs` already compares
//! the shared path with recompute for every corpus program at every event, with every subscriber
//! rendering at every version. What is here is everything that is only true of *sharing*:
//!
//! * a subscriber that skips versions, at every lag from 1 to past the history's end;
//! * a subscriber that joins after the log has already run;
//! * a subscriber that renders twice at the same version, and one that asks for a version older
//!   than the shared side has already reached;
//! * subscribers with different sessions over one dataflow, which is the case a shared arrangement
//!   could leak one subscriber's rows into another's page;
//! * that the sharing is real: advanced once for any number of subscribers, and the arrangements
//!   counted once rather than once each;
//! * and the *lifecycle* — how much of all that is held once nobody is reading it, which
//!   [`docs/26-arrangement-sharing-report.md`](../../../../docs/26-arrangement-sharing-report.md)
//!   §26.9 recorded as two loose ends. The second half of this file is that rule, and it asserts
//!   the page is unaffected before it asserts anything is dropped.

use std::sync::Arc;

use beck_core::engine::{Engine, Prepared, Retention, SharedDataflow};
use beck_core::gen::{arbitrary, Rng};
use beck_core::plan::Plan;
use beck_core::{Placed, Ty, Value};
use beck_rt::{Envelope, Instant, Runtime};

mod support;

const ACTORS: &[&str] = &["ana", "bo", "cy"];

fn corpus_files() -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut out: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("the corpus directory is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "beck"))
        .collect();
    out.sort();
    out
}

fn compile(name: &str, src: &str) -> Placed {
    let (placed, diags, map) = beck_core::compile_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    placed.unwrap_or_else(|| panic!("{name} did not slice"))
}

/// A program, a shared dataflow over it, and the states its log passes through.
struct Subject {
    name: String,
    runtime: Runtime,
    prepared: Arc<Prepared>,
    shared: Arc<SharedDataflow>,
    /// Every state the log produced, oldest first. `history[v]` is the state at version `v`.
    history: Vec<Value>,
}

impl Subject {
    fn new(name: &str, placed: Placed, events: usize) -> Subject {
        let log = log_for(&placed, name, events);
        assert!(!log.is_empty(), "{name}: no events could be generated");
        Subject::over(name, placed, log)
    }

    fn over(name: &str, placed: Placed, log: Vec<Value>) -> Subject {
        let backend = beck_eval::backend(&placed);
        let prepared =
            Arc::new(Prepared::compile(&placed, backend.as_ref()).expect("the plan prepares"));
        let shared = Arc::new(SharedDataflow::new(prepared.clone()));
        let runtime = Runtime::new(placed, backend).expect("the program prepares");

        let mut state = runtime.initial_state().expect("an initial accumulator");
        let mut history = vec![state.clone()];
        for (i, event) in log.into_iter().enumerate() {
            let seq = i as u64 + 1;
            let env = Envelope {
                seq,
                at: Instant(seq as i64),
                actor: ACTORS[seq as usize % ACTORS.len()].to_string(),
                body: event.clone(),
            };
            state = runtime
                .fold(&state, &env, event)
                .unwrap_or_else(|e| panic!("{name}: folding at seq {seq}: {e}"));
            history.push(state.clone());
        }
        Subject {
            name: name.to_string(),
            runtime,
            prepared,
            shared,
            history,
        }
    }

    /// The same subject under a different retention policy.
    ///
    /// Safe to swap the dataflow wholesale because nothing has rendered against it yet: a `Subject`
    /// folds its log with the runtime, and the dataflow is not touched until a subscriber attaches.
    fn retaining(mut self, retention: Retention) -> Subject {
        self.shared = Arc::new(SharedDataflow::with_retention(
            self.prepared.clone(),
            retention,
        ));
        self
    }

    fn head(&self) -> u64 {
        self.history.len() as u64 - 1
    }

    /// The recomputed page at a version, which is the oracle for everything in this file.
    fn expected(&self, version: u64, actor: &str) -> String {
        self.runtime
            .view(&self.history[version as usize], actor)
            .unwrap_or_else(|e| panic!("{}: recompute: {e}", self.name))
            .render()
    }

    /// Render one subscriber at a version through the shared dataflow, and check the page.
    ///
    /// Returns the version the shared side actually served, which is its own if it has already
    /// moved past the one asked for.
    fn render(&self, engine: &mut Engine, version: u64, actor: &str, at: &str) -> u64 {
        let session = self.runtime.session(actor);
        let (page, served) = self
            .shared
            .render(engine, &self.history[version as usize], version, &session)
            .unwrap_or_else(|e| panic!("{}: shared render: {e}", self.name));
        let Value::Html(page) = page else {
            panic!("{}: the engine produced {}", self.name, page.display())
        };
        assert_eq!(
            page.render(),
            self.expected(served, actor),
            "{} at {at}, subscriber `{actor}` asking for version {version}: the page served at \
             version {served} is not the recomputed one",
            self.name
        );
        served
    }
}

/// A deterministic log for a program, from its own `Event` union.
fn log_for(placed: &Placed, name: &str, n: usize) -> Vec<Value> {
    let mut rng = Rng::seeded(name, 7);
    let ty = Ty::con(
        placed
            .roles
            .event_ty
            .con_name()
            .expect("an event type with a name"),
    );
    (0..n)
        .filter_map(|_| arbitrary(&ty, &placed.program.types, &mut rng).ok())
        .collect()
}

/// `24-feed.beck` with `n` posts on it — a shared collection big enough to tell apart from the
/// handful of per-session operators above it.
///
/// The generated log cannot do this: `arbitrary` over the program's `Event` union produces a few
/// distinct ids however many events it draws, and a shared feed of three posts is smaller than the
/// page's own children. Everything about correctness is generated; the two measurements that need a
/// *size* say what size they need.
fn feed_with(posts: usize) -> Subject {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/24-feed.beck"),
    )
    .expect("24-feed.beck is in the corpus");
    let log = (0..posts)
        .map(|i| {
            Value::data(
                Arc::from("Event"),
                Some(Arc::from("Published")),
                std::collections::BTreeMap::from([
                    (Arc::from("id"), Value::str_(format!("p{i:05}"))),
                    (Arc::from("text"), Value::str_(format!("post {i}"))),
                ]),
            )
        })
        .collect();
    Subject::over("24-feed.beck", compile("24-feed.beck", &src), log)
}

fn subjects(events: usize) -> Vec<Subject> {
    let mut out = vec![Subject::new(
        "examples/todo.beck",
        support::todo_program(),
        events,
    )];
    for path in corpus_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("a readable corpus program");
        out.push(Subject::new(&name, compile(&name, &src), events));
    }
    out
}

#[test]
fn a_subscriber_that_skipped_versions_still_gets_the_recomputed_page() {
    // The failure this rules out is the whole reason the shared side keeps a history. A subscriber
    // is woken by a `watch`, which coalesces: three events can land between one render and the
    // next. An entry inserted at version 5 and removed at version 6 is *never mentioned again*, so
    // a subscriber that renders at 4 and then at 7 and is handed only version 7's changes keeps a
    // row the accumulator has forgotten — and the page it serves is one row wrong, forever.
    //
    // Every lag from 1 to 9 over every corpus program, plus 200, which is past the history's end
    // and therefore the rebuild path rather than the replay one.
    let mut checked = 0usize;
    for subject in subjects(24) {
        for lag in [1u64, 2, 3, 5, 9, 200] {
            let mut engines: Vec<Engine> =
                ACTORS.iter().map(|_| subject.shared.subscriber()).collect();
            let mut version = 0;
            while version <= subject.head() {
                for (i, actor) in ACTORS.iter().enumerate() {
                    subject.render(&mut engines[i], version, actor, &format!("lag {lag}"));
                    checked += 1;
                }
                version += lag;
            }
            // Whatever the lag, the last render is at the head, so a laggard and a keen subscriber
            // end at the same page.
            for (i, actor) in ACTORS.iter().enumerate() {
                subject.render(&mut engines[i], subject.head(), actor, "the head");
                checked += 1;
            }
        }
    }
    println!("{checked} pages compared across six lags");
    assert!(checked > 2_000, "only {checked} pages were compared");
}

#[test]
fn a_subscriber_that_joins_late_sees_the_same_page_as_one_that_was_always_there() {
    // A shared dataflow is warm by the time a new subscriber attaches, and the new subscriber's own
    // operators are cold. That mismatch — a cold consumer over a warm producer — is not a case a
    // per-subscriber engine ever had, because there the two were always the same age.
    for subject in subjects(20) {
        let mut early = subject.shared.subscriber();
        for version in 0..=subject.head() {
            subject.render(&mut early, version, ACTORS[0], "the early subscriber");
        }
        for actor in ACTORS {
            let mut late = subject.shared.subscriber();
            subject.render(&mut late, subject.head(), actor, "the late subscriber");
            // And it keeps working afterwards, which a subscriber whose first render was a special
            // case would not.
            subject.render(
                &mut late,
                subject.head(),
                actor,
                "the late subscriber again",
            );
        }
    }
}

#[test]
fn rendering_twice_at_one_version_changes_nothing() {
    // The engine's `changed` flags are per *tick*, and with a shared upstream they are per *window*
    // — the versions since this subscriber last looked. A second render at the same version has an
    // empty window, so every upstream node reads as unchanged, and every pointwise operator below
    // is entitled to skip. If any of them were skipping wrongly, this is where a page would come
    // back empty.
    for subject in subjects(16) {
        let mut engine = subject.shared.subscriber();
        for version in 0..=subject.head() {
            subject.render(&mut engine, version, ACTORS[0], "first");
            subject.render(&mut engine, version, ACTORS[0], "again");
            subject.render(&mut engine, version, ACTORS[0], "and again");
        }
    }
}

#[test]
fn a_subscriber_asking_for_a_version_the_shared_side_has_passed_is_served_the_newer_one() {
    // Two subscribers race: one reads the state at version v, another advances the shared side to
    // v + 3 before the first gets its render. The first is served v + 3.
    //
    // That is a deliberate choice and not an accident, so it is asserted rather than tolerated.
    // Unwinding an arrangement to an older version would need a version history of *values* rather
    // than of changes; serving the newer page is correct because the subscriber is about to be
    // woken for v + 3 anyway. What makes it safe is that the served version comes back to the
    // caller: `beck-rt` labels the patch frame with it, so a resuming client asks for the
    // difference from the state its page actually shows.
    let subject = Subject::new("examples/todo.beck", support::todo_program(), 12);
    let mut ahead = subject.shared.subscriber();
    let mut behind = subject.shared.subscriber();

    subject.render(&mut ahead, 0, ACTORS[0], "both at the start");
    subject.render(&mut behind, 0, ACTORS[1], "both at the start");

    // One subscriber runs the whole log; the other is still asking for version 1.
    for version in 1..=subject.head() {
        subject.render(&mut ahead, version, ACTORS[0], "the fast subscriber");
    }
    let served = subject.render(&mut behind, 1, ACTORS[1], "the slow subscriber");
    assert_eq!(
        served,
        subject.head(),
        "a subscriber asking for version 1 of an already-advanced dataflow was told it got \
         version {served}"
    );
}

#[test]
fn one_dataflow_serves_every_session_without_mixing_them() {
    // The failure a shared arrangement makes newly possible: an operator below the session reading
    // an arrangement another subscriber's render left in a state that suited *it*. The sketch's
    // `mine` filters by `session.actor` immediately below the accumulator, so if a shared
    // arrangement were being mutated per subscriber this is the program where one actor would see
    // another's todos.
    //
    // Interleaved deliberately — every subscriber renders at every version, in a different order
    // each time — because a bug of this shape is invisible when subscribers take turns neatly.
    for subject in subjects(18) {
        let mut engines: Vec<Engine> = ACTORS.iter().map(|_| subject.shared.subscriber()).collect();
        for version in 0..=subject.head() {
            let rotate = version as usize % ACTORS.len();
            for k in 0..ACTORS.len() {
                let i = (k + rotate) % ACTORS.len();
                subject.render(&mut engines[i], version, ACTORS[i], "interleaved");
            }
        }
    }
}

#[test]
fn the_shared_prefix_is_advanced_once_however_many_subscribers_render() {
    // §5.3's sentence, as a count. A thousand connected users must compile to *one* shared
    // dataflow, and "one" is a number: the shared side advances per version, not per subscriber.
    let subject = Subject::new("examples/todo.beck", support::todo_program(), 10);
    let mut engines: Vec<Engine> = (0..64).map(|_| subject.shared.subscriber()).collect();
    for version in 0..=subject.head() {
        for (i, engine) in engines.iter_mut().enumerate() {
            subject.render(engine, version, ACTORS[i % ACTORS.len()], "the fanout");
        }
    }
    assert_eq!(
        subject.shared.advances(),
        subject.history.len() as u64,
        "64 subscribers over {} versions advanced the shared prefix {} times",
        subject.history.len(),
        subject.shared.advances()
    );
}

#[test]
fn what_a_subscriber_holds_is_only_what_reads_the_session() {
    // The memory half of the same claim. A subscriber attached to a shared dataflow must hold *no*
    // arrangement that does not read the session — otherwise the sharing is a lock and a history
    // that bought nothing.
    for subject in subjects(20) {
        let plan = Plan::compile(subject.runtime.placed());
        let mut engine = subject.shared.subscriber();
        let mut standalone = Engine::new(subject.prepared.clone());
        let head = subject.head();
        subject.render(&mut engine, head, ACTORS[0], "the shared subscriber");
        standalone
            .render(
                &subject.history[head as usize],
                &subject.runtime.session(ACTORS[0]),
            )
            .expect("a standalone render");

        assert_eq!(
            engine.arranged_shared(),
            0,
            "{}: a subscriber over a shared dataflow is still holding {} entries in operators \
             that do not read the session",
            subject.name,
            engine.arranged_shared()
        );
        // And the entries did not vanish — they moved to the one dataflow above.
        assert_eq!(
            engine.arranged() + subject.shared.arranged(),
            standalone.arranged(),
            "{}: the shared and per-subscriber halves do not add up to what one engine held \
             ({} shared nodes of {})",
            subject.name,
            plan.shared().len(),
            plan.nodes.len(),
        );
    }
}

#[test]
fn a_fanout_costs_the_shared_prefix_once_rather_than_once_each() {
    // The trade §24.7 measured and could not yet improve: a maintained subscription cost about 4×
    // the page it already held, all of it per subscriber. What sharing changes is the *slope*, and
    // the slope is what a fanout estimate multiplies.
    //
    // Asserted on `24-feed.beck` because it is the corpus program written for this case — a sorted
    // public feed with a per-session greeting.
    // Both sides are measured with `fanout_footprint`, which walks the accumulator, the shared side
    // and every subscriber with **one** exclusion set. Summing per-engine footprints would charge
    // every subscriber for the page subtrees they now hold by `Arc` between them, which is exactly
    // the saving under measurement.
    const FANOUT: usize = 32;
    let subject = feed_with(200);
    let head = subject.head();
    let state = &subject.history[head as usize];

    let mut alone: Vec<Engine> = (0..FANOUT)
        .map(|_| Engine::new(subject.prepared.clone()))
        .collect();
    for (i, engine) in alone.iter_mut().enumerate() {
        engine
            .render(state, &subject.runtime.session(ACTORS[i % ACTORS.len()]))
            .expect("a standalone render");
    }
    let unshared =
        beck_core::engine::fanout_footprint(state, None, &alone.iter().collect::<Vec<_>>()).bytes;

    let mut engines: Vec<Engine> = (0..FANOUT).map(|_| subject.shared.subscriber()).collect();
    for (i, engine) in engines.iter_mut().enumerate() {
        subject.render(engine, head, ACTORS[i % ACTORS.len()], "the fanout");
    }
    let shared = beck_core::engine::fanout_footprint(
        state,
        Some(&subject.shared),
        &engines.iter().collect::<Vec<_>>(),
    )
    .bytes;

    println!(
        "24-feed.beck over 200 posts, {FANOUT} subscribers: {} KB shared against {} KB unshared \
         ({:.1}× less)",
        shared / 1024,
        unshared / 1024,
        unshared as f64 / shared.max(1) as f64,
    );
    assert!(
        shared * 2 < unshared,
        "sharing a public feed between {FANOUT} subscribers saved less than half: {shared} \
         against {unshared}"
    );
}

#[test]
fn a_shared_arrangement_is_listed_once_between_the_subscribers_that_read_it() {
    // The `O(n)` §24.6 named is assembling an arrangement into a `list` for a pointwise consumer,
    // and it was paid per subscriber because the cache lived in the subscriber's cell. It now lives
    // beside the arrangement, so the second subscriber to need the list gets the first one's.
    //
    // On `24-feed.beck` it is more than the list: the whole `ul`, its 200 `li` children and the
    // `html_el` that assembles them read the state and not the session, so the *page's* `O(n)` half
    // is above the cut. Eight subscribers pay it once between them and hold a per-session constant
    // each — which is §5.3's sentence about the part of the render §24.6 said was where the
    // remaining `O(n)` lives.
    //
    // Measured at two sizes, because "did not grow with the collection" is the claim and one size
    // cannot say it.
    let mut per_subscriber = Vec::new();
    for posts in [50usize, 400] {
        let subject = feed_with(posts);
        let head = subject.head();
        let mut engines: Vec<Engine> = (0..8).map(|_| subject.shared.subscriber()).collect();
        let mut each = Vec::new();
        for (i, engine) in engines.iter_mut().enumerate() {
            subject.render(engine, head, ACTORS[i % ACTORS.len()], "the fanout");
            each.push(engine.work().materialised);
        }
        let total: u64 = each.iter().sum();
        let once = subject.shared.work().materialised;
        println!(
            "{posts:>4} posts: the shared side materialised {once:>5} entries once; \
             8 subscribers materialised {total:>3} between them ({each:?})"
        );
        assert!(
            once >= posts as u64,
            "the shared side materialised {once} entries over {posts} posts, so the feed is not \
             what is being shared"
        );
        per_subscriber.push(total);
    }
    assert_eq!(
        per_subscriber[0], per_subscriber[1],
        "eight subscribers paid {} entries over 50 posts and {} over 400; the shared page is \
         being assembled per subscriber",
        per_subscriber[0], per_subscriber[1]
    );
}

// ---------------------------------------------------------------------------------------------
// The lifecycle: what the dataflow holds when nobody is reading it, and for how long
//
// `docs/26-arrangement-sharing-report.md` §26.9 recorded two loose ends — the arrangements are
// never released, and the change history is a constant rather than a policy. Both are the same
// missing rule, and everything below is where it is wrong if it is wrong. The rule is a *reader
// set*: a subscriber engine is counted while it lives and publishes how far it has rendered, so
// the history can be compacted to the oldest frontier and the arrangements dropped when the set
// empties.
//
// What these have to establish, and in this order: that the release and the compaction cannot
// change a page (they are memory, not semantics); and only then that they actually happen.
// ---------------------------------------------------------------------------------------------

#[test]
fn what_is_retained_never_changes_a_page() {
    // The one that matters. Every other test in this section asserts something is *dropped*, and a
    // dropped arrangement that was still needed is a wrong page rather than a crash — so the
    // correctness claim is made first and over the whole corpus.
    //
    // Three subscribers, arriving and leaving at different points, against the recompute oracle at
    // every version. The middle one is dropped and replaced halfway through, which is the case that
    // exercises both halves at once: its departure compacts the history the survivors are using,
    // and its replacement attaches to a dataflow that has moved on.
    for subject in subjects(20) {
        let head = subject.head();
        let mut steady = subject.shared.subscriber();
        let mut leaver = Some(subject.shared.subscriber());
        for version in 0..=head {
            subject.render(&mut steady, version, ACTORS[0], "the steady subscriber");
            match leaver.as_mut() {
                Some(engine) => {
                    subject.render(engine, version, ACTORS[1], "the leaver");
                    if version == head / 2 {
                        // Detaches, compacting the history back to what `steady` still needs.
                        leaver = None;
                    }
                }
                None => {
                    let mut fresh = subject.shared.subscriber();
                    subject.render(&mut fresh, version, ACTORS[2], "the replacement");
                }
            }
        }
    }
}

#[test]
fn a_page_survives_the_arrangements_being_released_underneath_it() {
    // The release path's own correctness. A dataflow that has been emptied must be indistinguishable
    // from one that was never started — not nearly, exactly — because the next subscriber's page is
    // compared against a recompute that knows nothing about any of this.
    //
    // The subtle half is the *second* render after the release: a dataflow that reset its
    // arrangements but kept its version would advance from a version it can no longer describe, and
    // hand its next subscriber deltas against arrangements that are not there.
    for subject in subjects(16) {
        let head = subject.head();
        {
            let mut first = subject.shared.subscriber();
            for version in 0..=head {
                subject.render(&mut first, version, ACTORS[0], "before the release");
            }
        }
        assert_eq!(
            subject.shared.readers(),
            0,
            "{}: the subscriber was dropped and the reader set still has entries",
            subject.name
        );
        let mut second = subject.shared.subscriber();
        subject.render(&mut second, head, ACTORS[1], "after the release");
        subject.render(&mut second, head, ACTORS[1], "and again");
    }
}

#[test]
fn the_history_is_bounded_by_the_laggiest_subscriber_and_not_by_the_ceiling() {
    // §26.9: "The history is a constant, not a policy. 64 versions, chosen because a subscriber
    // further behind than that is not the bottleneck, and not because anything measured where the
    // knee is."
    //
    // It is now a ceiling with a floor underneath it, and the floor is a fact rather than a number
    // somebody picked: a step every reader has already rendered past is retained for nobody.
    let subject = Subject::new("examples/todo.beck", support::todo_program(), 40);
    let head = subject.head();
    assert!(
        head > 8,
        "the log is too short to say anything: {head} versions"
    );

    let mut keen = subject.shared.subscriber();
    let mut laggard = subject.shared.subscriber();
    subject.render(&mut laggard, 3, ACTORS[1], "the laggard's only render");
    for version in 0..=head {
        subject.render(&mut keen, version, ACTORS[0], "the keen subscriber");
    }

    // Everything from the laggard's frontier forward, and nothing before it.
    assert_eq!(
        subject.shared.retained(),
        (head - 3) as usize,
        "with one subscriber stuck at version 3 and the head at {head}, the dataflow kept {} \
         versions of history",
        subject.shared.retained()
    );

    // And the laggard leaving is what releases it — the history was being kept *for it*.
    drop(laggard);
    assert_eq!(
        subject.shared.retained(),
        0,
        "the laggard left and the dataflow is still keeping {} versions for nobody",
        subject.shared.retained()
    );
    // The survivor is unaffected: it was never behind.
    subject.render(&mut keen, head, ACTORS[0], "after the laggard left");
}

#[test]
fn a_fanout_that_keeps_up_keeps_one_version_of_history() {
    // The common case, and the one the constant was most wrong about. Thirty-two subscribers all
    // rendering at every version were costing 64 versions of retained change; they cost one.
    const FANOUT: usize = 32;
    let subject = Subject::new("examples/todo.beck", support::todo_program(), 40);
    let head = subject.head();
    let mut engines: Vec<Engine> = (0..FANOUT).map(|_| subject.shared.subscriber()).collect();
    for version in 0..=head {
        for (i, engine) in engines.iter_mut().enumerate() {
            subject.render(engine, version, ACTORS[i % ACTORS.len()], "the fanout");
        }
    }
    println!(
        "{FANOUT} subscribers over {head} versions retained {} version(s) of change history \
         (ceiling {})",
        subject.shared.retained(),
        subject.shared.retention().depth
    );
    assert_eq!(
        subject.shared.retained(),
        1,
        "a fanout that renders at every version kept {} versions of history",
        subject.shared.retained()
    );
}

#[test]
fn a_subscriber_that_has_not_rendered_pins_nothing() {
    // A reader's frontier is `UNRENDERED` until its first render, and the alternative — treating it
    // as version 0 — would pin the whole history for the one subscriber that cannot use a single
    // step of it, because an engine with no arrangements rebuilds whatever it is offered.
    //
    // The failure this rules out is a connection that opens and never renders holding the ceiling's
    // worth of change for the lifetime of the process.
    let subject = Subject::new("examples/todo.beck", support::todo_program(), 40);
    let head = subject.head();
    let _idle = subject.shared.subscriber();
    let mut keen = subject.shared.subscriber();
    for version in 0..=head {
        subject.render(&mut keen, version, ACTORS[0], "beside an idle subscriber");
    }
    assert_eq!(
        subject.shared.retained(),
        1,
        "a subscriber that never rendered pinned {} versions of history",
        subject.shared.retained()
    );
}

#[test]
fn the_arrangements_go_when_the_last_subscriber_does() {
    // §26.9: "The shared dataflow is never released. It holds its arrangements whether or not
    // anybody is subscribed. A process that had a fanout and now has none keeps the accumulator's
    // arrangements warm for a reconnection that may not come. Nothing measures how much that is and
    // nothing drops it."
    //
    // Both halves, on the program written for the case where the shared side is most of the plan.
    let subject = feed_with(200);
    let head = subject.head();
    let state = &subject.history[head as usize];

    let mut engines: Vec<Engine> = (0..8).map(|_| subject.shared.subscriber()).collect();
    for (i, engine) in engines.iter_mut().enumerate() {
        subject.render(engine, head, ACTORS[i % ACTORS.len()], "the fanout");
    }
    let held = subject.shared.arranged();
    let bytes = subject.shared.footprint(state).bytes;
    assert!(held > 200, "the shared side is holding only {held} entries");
    assert_eq!(subject.shared.releases(), 0, "released while subscribed");

    // Seven of the eight go. Nothing is released: somebody is still reading.
    engines.truncate(1);
    assert_eq!(
        subject.shared.arranged(),
        held,
        "the arrangements were dropped with a subscriber still attached"
    );

    drop(engines);
    assert_eq!(subject.shared.readers(), 0);
    assert_eq!(
        subject.shared.arranged(),
        0,
        "the last subscriber left and the dataflow is still holding {} entries",
        subject.shared.arranged()
    );
    assert_eq!(
        subject.shared.releases(),
        1,
        "the arrangements went without being counted as released"
    );
    let after = subject.shared.footprint(state).bytes;
    println!(
        "24-feed.beck, 200 posts, 8 subscribers: the shared side held {held} entries \
         ({bytes} bytes beyond the accumulator); with nobody subscribed it holds {} ({after} bytes)",
        subject.shared.arranged()
    );
    assert!(
        after < bytes / 4,
        "releasing gave back {bytes} - {after} bytes, which is not most of what was held"
    );
}

#[test]
fn a_dataflow_told_to_stay_warm_stays_warm() {
    // The policy half. Releasing costs the next subscriber a cold start, and a deployment whose
    // clients reconnect constantly would rather pay the memory — which is a deployment's judgement
    // and not this file's, so it is a field rather than a `const`.
    let subject = feed_with(200).retaining(Retention {
        release_when_idle: false,
        ..Retention::default()
    });
    let head = subject.head();
    {
        let mut engine = subject.shared.subscriber();
        subject.render(&mut engine, head, ACTORS[0], "the only subscriber");
    }
    assert_eq!(subject.shared.readers(), 0);
    assert!(
        subject.shared.arranged() > 200,
        "a dataflow told not to release when idle dropped its arrangements anyway"
    );
    assert_eq!(subject.shared.releases(), 0);

    // And the reconnection it was kept warm for is served without a rebuild of the shared side.
    let advances = subject.shared.advances();
    let mut engine = subject.shared.subscriber();
    subject.render(&mut engine, head, ACTORS[1], "the reconnection");
    assert_eq!(
        subject.shared.advances(),
        advances,
        "a warm dataflow at the version being asked for advanced again"
    );
}

#[test]
fn a_release_costs_the_next_subscriber_a_cold_start() {
    // What the default trades away, asserted rather than left to be inferred. The reconnection after
    // a release is served correctly — `a_page_survives_the_arrangements_being_released_underneath_it`
    // is the corpus-wide version of that — and it is served by rebuilding the shared prefix.
    let subject = feed_with(200);
    let head = subject.head();
    {
        let mut engine = subject.shared.subscriber();
        subject.render(&mut engine, head, ACTORS[0], "the first subscriber");
    }
    let advances = subject.shared.advances();
    let mut engine = subject.shared.subscriber();
    subject.render(&mut engine, head, ACTORS[1], "the reconnection");
    assert_eq!(
        subject.shared.advances(),
        advances + 1,
        "the shared prefix was not rebuilt after being released"
    );
    assert!(
        subject.shared.arranged() > 200,
        "the rebuilt dataflow is holding only {} entries",
        subject.shared.arranged()
    );
}
