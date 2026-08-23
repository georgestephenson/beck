//! The columnar layout, and the four things a program must not be able to notice about it.
//!
//! [`docs/105-the-ecosystem-answer.md`](../../../../docs/105-the-ecosystem-answer.md) §105.10 asks
//! for a dense typed column and [`docs/08`](../../../../docs/08-roadmap.md) §8.5.4 schedules it
//! after the aggregates. [`beck_core::seq`] is it: a list of `Int` or of `Float` is held as a dense
//! buffer rather than as boxed `Value`s, which halves what it occupies and is the only thing in
//! this language a numeric kernel or an Arrow reader could be handed a pointer to.
//!
//! **A second representation is a correctness problem before it is a performance one.** Two lists
//! holding the same elements are one value, and four mechanisms have to agree about that: order,
//! equality, the state digest ([`docs/10`](../../../../docs/10-decisions.md) D3) and the wire
//! format. `seq.rs`'s own tests assert it on values a test constructed; these assert it on values a
//! **program** produced, folding every corpus program's log with the layout switched on and again
//! with it switched off and holding the two to the same digests and the same rendered pages.
//!
//! That switch is [`docs/08`](../../../../docs/08-roadmap.md) §8.3 item 8's, and running it here
//! does the two jobs that item asks for: it proves the switch works, and it makes "a caller cannot
//! tell" a measurement rather than a claim.

use std::sync::Arc;

use beck_core::gen::{arbitrary, Rng};
use beck_core::seq::{built, set_columns, Seq};
use beck_core::{Placed, Ty, Value};
use beck_rt::{Envelope, Instant, Runtime};

mod support;

const ACTORS: &[&str] = &["ana", "bo"];

/// `set_columns` is process-wide and `cargo test` runs this binary's tests on several threads, so
/// every test that flips it takes this first. The switch being global is the design
/// ([`beck_core::seq`]) — a list is built where no configuration is in scope — and this is what
/// that costs a test suite.
static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn switch() -> std::sync::MutexGuard<'static, ()> {
    SWITCH.lock().unwrap_or_else(|e| e.into_inner())
}

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

/// A deterministic log for a program, from its own `Event` union — `incremental_engine.rs`'s
/// generator, for its reason: the point is to run this against programs nobody wrote it for.
fn log_for(placed: &Placed, name: &str, n: usize) -> Vec<Value> {
    let mut rng = Rng::seeded(name, 1);
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

/// What a program's whole run comes to: the digest after every event, and every page it rendered.
///
/// The digest rather than the accumulator, because the digest is the replay-determinism oracle
/// (§4.8) and is therefore the thing that would actually be compared in production. The pages as
/// well, because a value can digest the same and render differently only if the digest is wrong —
/// so holding both says which of the two broke.
struct Run {
    digests: Vec<[u8; 32]>,
    pages: Vec<String>,
    /// How many columns the run built, which is the only way to see one that a page consumed.
    ///
    /// Walking the accumulator was the first instrument and it answered zero for every program in
    /// the corpus — truthfully, and about the wrong thing: `corpus/26-sensors.beck` builds a
    /// `list[Float]` *inside its view*, so the column exists for as long as it takes to render and
    /// is never in the state anybody could walk.
    built: u64,
}

fn run(placed: &Placed, log: &[Value]) -> Run {
    let before = built();
    let backend = beck_eval::backend(placed);
    let runtime = Runtime::new(placed.clone(), backend).expect("the program prepares");
    let mut state = runtime.initial_state().expect("an initial accumulator");
    let mut digests = Vec::with_capacity(log.len());
    let mut pages = Vec::with_capacity(log.len() * ACTORS.len());
    for (i, event) in log.iter().enumerate() {
        let seq = i as u64 + 1;
        let env = Envelope {
            seq,
            at: Instant(seq as i64),
            actor: ACTORS[i % ACTORS.len()].to_string(),
            body: event.clone(),
        };
        state = runtime
            .fold(&state, &env, event.clone())
            .expect("the fold succeeds");
        digests.push(beck_core::digest(&state));
        for actor in ACTORS {
            pages.push(match runtime.view(&state, actor) {
                Ok(html) => html.render(),
                Err(e) => format!("failed: {e}"),
            });
        }
    }
    Run {
        built: built() - before,
        digests,
        pages,
    }
}

/// Every corpus program, its generated log, and the two runs.
fn subjects() -> Vec<(String, Placed, Vec<Value>)> {
    let mut out = Vec::new();
    let sketch = support::todo_program();
    let log = log_for(&sketch, "examples/todo.beck", 30);
    out.push(("examples/todo.beck".to_string(), sketch, log));
    for path in corpus_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = std::fs::read_to_string(&path).expect("a readable corpus program");
        let placed = compile(&name, &src);
        let log = log_for(&placed, &name, 25);
        out.push((name, placed, log));
    }
    out
}

/// **A program cannot tell which layout its lists got** — the whole obligation, over the whole
/// corpus.
///
/// Each program's log is folded twice, with [`set_columns`] on and off, and the two runs are held
/// to the same digest after every event and the same rendered page for every subscriber. A derived
/// `Ord` on the layout enum, a digest that hashed the buffer instead of the elements, or a `Float`
/// column that lost `Value::float`'s canonicalisation would each turn this red.
///
/// It is written against the *shape of the gap* ([`docs/82`](../../../../docs/82-the-edge-report.md)
/// §82.10): the failure it exists to catch is a value that behaves differently because of how it
/// happened to be built, so it compares two builds of the same value rather than one build against
/// a constant.
#[test]
fn a_program_cannot_tell_which_layout_its_lists_got() {
    let _held = switch();
    let all = subjects();
    assert!(
        all.len() >= 24,
        "only {} programs were exercised; the corpus is the measurement",
        all.len()
    );
    let mut with_columns = 0;
    let mut columns_seen = 0;
    for (name, placed, log) in &all {
        set_columns(true);
        let on = run(placed, log);
        set_columns(false);
        let off = run(placed, log);
        set_columns(true);

        assert_eq!(
            off.built, 0,
            "{name}: a column was still built with the switch off, so the switch is not one"
        );
        if on.built > 0 {
            with_columns += 1;
            columns_seen += on.built;
        }
        assert_eq!(
            on.digests, off.digests,
            "{name}: the state digest depends on how a list was stored, which breaks replay"
        );
        assert_eq!(
            on.pages, off.pages,
            "{name}: the rendered page depends on how a list was stored"
        );
    }
    println!(
        "{with_columns} of {} programs build a column while folding and rendering; \
         {columns_seen} columns in all",
        all.len()
    );
    // The lesson `docs/99` §99.9 item 3 records, applied to a representation: an operator — or a
    // layout — with no program is a hole in the differential. If nothing in the tree produces one,
    // everything above is comparing two identical runs and proves nothing.
    assert!(
        with_columns > 0,
        "no program in the corpus builds a column, so this gate compares one layout with itself"
    );
}

/// **The layout is a fact about the elements, and the accumulator idiom finds it.**
///
/// `go(i + 1, list_append(done, x))` is how `lib/`, the corpus and both SICP chapters build a list
/// ([`docs/70`](../../../../docs/70-the-evaluator-gets-fast-report.md) §70.6), and it starts from
/// `[]` — so a layout chosen only at `pack` time would never reach it. `Seq::push` promotes an
/// empty list on its first element, which is what makes this work with no program changing a line.
#[test]
fn a_list_a_program_accumulates_is_a_column() {
    let _held = switch();
    let src = r#"
def build(n: Int) -> list[Int]:
    return go(0, n, [])

def go(i: Int, n: Int, done: list[Int]) -> list[Int]:
    if i >= n:
        return done
    return go(i + 1, n, list_append(done, i * 2))

def halves(n: Int) -> list[Float]:
    return map_list(build(n), lambda x: float(x) / 2.0)
"#;
    let (placed, diags, map) = beck_core::compile_or_library_str("columns.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    let placed = placed.expect("this library compiles");
    let backend = beck_eval::backend(&placed);

    let call = |name: &str| -> Value {
        let def = placed
            .program
            .defs
            .get(name)
            .unwrap_or_else(|| panic!("no `{name}`"));
        let f = backend.function(&def.body).expect("prepares");
        f(vec![Value::Int(64)]).expect("runs")
    };

    let ints = call("build");
    let seq = ints.as_list().expect("a list");
    assert!(
        seq.is_column(),
        "an accumulated list of Int is not a column, so the promotion rule does not reach the \
         idiom every program in this tree builds lists with"
    );
    assert_eq!(seq.ints().map(<[i64]>::len), Some(64));

    // And a `map_list` over it lands in the other column, which is the one with a consumer beyond
    // memory: a `&[f64]` is what a kernel and an Arrow `Float64Array` both take.
    let floats = call("halves");
    let seq = floats.as_list().expect("a list");
    assert!(seq.is_column(), "a mapped list of Float is not a column");
    let dense = seq.floats().expect("a dense f64 run");
    assert_eq!(dense.len(), 64);
    assert_eq!(dense[3], 3.0);

    // With the switch off the same program answers the same list, and holds it the old way.
    set_columns(false);
    let off = call("build");
    set_columns(true);
    assert_eq!(off, ints);
    assert!(!off.as_list().expect("a list").is_column());
}

/// **What the layout is worth, measured at two sizes**, which is the only claim it makes.
///
/// A shape rather than a rate and with no clock in it: what is asserted is bytes, which are the
/// same number on every machine. The boxed layout is `size_of::<Value>()` an element and the dense
/// one is eight, so the ratio is the same at both sizes — and asserting it at two says the saving
/// is proportional rather than a fixed overhead somewhere.
#[test]
fn a_column_halves_what_a_list_of_numbers_occupies() {
    let _held = switch();
    let measure = |n: usize| -> (usize, usize) {
        set_columns(true);
        let on = Seq::pack((0..n as i64).map(Value::Int).collect());
        set_columns(false);
        let off = Seq::pack((0..n as i64).map(Value::Int).collect());
        set_columns(true);
        assert!(on.is_column() && !off.is_column());
        assert_eq!(on, off, "the two layouts are not the same list");
        (on.heap_bytes(), off.heap_bytes())
    };
    let small = measure(1_000);
    let large = measure(8_000);
    println!(
        "1,000 ints: {} bytes as a column against {}; 8,000: {} against {}",
        small.0, small.1, large.0, large.1
    );
    assert_eq!(small.0 * 2, small.1);
    assert_eq!(large.0 * 2, large.1);
    // Eight times the elements for eight times the bytes, both ways: nothing here is a fixed cost
    // being amortised, which is what a single size could not have said.
    assert_eq!(large.0, small.0 * 8);
    assert_eq!(large.1, small.1 * 8);
}

/// The wire format is the fourth mechanism, and it is the one a client would notice.
///
/// [`beck_core::repr`] is what a patch carries, so a column that serialised differently would send
/// a browser a different DOM for the same state.
#[test]
fn the_wire_bytes_are_the_same_for_both_layouts() {
    let _held = switch();
    let values: Vec<Value> = (0..16).map(Value::Int).collect();
    set_columns(true);
    let on = Value::list(values.clone());
    set_columns(false);
    let off = Value::list(values);
    set_columns(true);
    assert!(on.as_list().expect("a list").is_column());
    assert!(!off.as_list().expect("a list").is_column());

    let bytes = |v: &Value| {
        let repr = beck_core::repr::Repr::of(v).expect("storable");
        serde_json::to_vec(&repr).expect("encodes")
    };
    assert_eq!(bytes(&on), bytes(&off));
    assert_eq!(beck_core::digest(&on), beck_core::digest(&off));
    assert_eq!(on.to_json(), off.to_json());
    assert_eq!(on.display(), off.display());

    // And the round trip comes back as a list, whichever way it went out.
    let there_and_back = beck_core::repr::Repr::of(&on).expect("storable").to_value();
    assert_eq!(there_and_back, off);
}

/// The runtime's switch is the one an operator reaches, and it has to actually reach the layout.
/// A plain `#[test]` driving its own runtime rather than a `#[tokio::test]`, because the switch is
/// process-wide: the guard that serialises it must not be held across an `await`, and here it is
/// held across a `block_on` that cannot yield to another test instead.
#[test]
fn the_apps_configuration_turns_the_layout_off() {
    use beck_rt::{App, AppConfig, MemoryLog};
    let _held = switch();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    rt.block_on(async {
        let config = AppConfig {
            columns: false,
            ..Default::default()
        };
        let app = App::start(support::todo_runtime(), Arc::new(MemoryLog::new()), config)
            .await
            .expect("starts");
        assert!(
            !beck_core::seq::columns(),
            "`AppConfig::columns` did not reach the layout"
        );
        drop(app);

        let app = App::start(
            support::todo_runtime(),
            Arc::new(MemoryLog::new()),
            AppConfig::default(),
        )
        .await
        .expect("starts");
        assert!(beck_core::seq::columns(), "the default is not on");
        drop(app);
    });
}
