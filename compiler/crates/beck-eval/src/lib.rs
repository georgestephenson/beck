//! Phase 1's `Core` backend: a tree-walking evaluator, behind [`beck_core::backend::Backend`].
//!
//! # Why this is its own crate
//!
//! The roadmap names Cranelift as Phase 1's server backend; what exists is this. That is stated in
//! [`docs/19-phase-1-report.md`] §19.6 and is a legitimate choice — [`docs/00-original-idea.md`]
//! names "engine-in-Rust with the language as its configuration" as one of the three routes that
//! work for a GC'd functional language on a Rust host.
//!
//! What was *not* legitimate was where it lived. A backend inside the crate that defines the IR,
//! called by name from the runtime, makes the native backend a refactor instead of an addition —
//! and makes §4.8's differential test *between backends* impossible to write, because there is no
//! interface for two of them to sit behind. `beck-core` now defines the seam; this crate is the
//! first thing to fill it, and a `beck-jit` would be the second, with the runtime unchanged.
//!
//! # What the evaluator guarantees
//!
//! Three properties that are not negotiable, and are tested:
//!
//! * **Replay purity.** Nothing here reads a clock, a random source, or performs I/O. `uuid()` is a
//!   primitive the *checker* refuses inside a fold (§3.7), and even outside one it comes from the
//!   host rather than the ambient environment, so a replay is reproducible.
//! * **Total order everywhere.** Maps are ordered and `sort_by` is stable, so two runs over the
//!   same log render identically — Phase 0 §18.5 item 4 learned this the hard way.
//! * **Errors are values, not panics.** A partial operation returns an error carrying its span,
//!   because a language server has to survive evaluating half-written code.

pub mod interp;

use std::sync::Arc;

use beck_core::backend::{Backend, Callable, ExecError};
use beck_core::{Core, Env, Program, Value};

pub use interp::{EvalError, Host, Interp};

/// The tree-walking backend.
///
/// Owns the program because a [`Callable`] outlives the call that produced it: the runtime prepares
/// `validate`, the fold and the view once at startup and calls them for the process's lifetime,
/// which a borrowed `Interp<'h>` cannot support.
pub struct Evaluator {
    program: Arc<Program>,
    /// The one impure capability the program may reach. Injected, so a test can make ids
    /// deterministic and a replay can refuse to mint them at all.
    uuid: Arc<dyn Fn() -> Arc<str> + Send + Sync>,
}

impl Evaluator {
    pub fn new(program: Arc<Program>) -> Evaluator {
        Evaluator {
            program,
            uuid: Arc::new(|| Arc::from(uuid_v7())),
        }
    }

    /// Replace the id source. The checker forbids `uuid()` inside a fold, so this can only affect
    /// code at the edge — but a test that wants reproducible ids needs to say so somewhere.
    pub fn with_uuid(mut self, f: impl Fn() -> Arc<str> + Send + Sync + 'static) -> Evaluator {
        self.uuid = Arc::new(f);
        self
    }
}

/// Bound to the program and the id source for one call.
struct Globals {
    program: Arc<Program>,
    uuid: Arc<dyn Fn() -> Arc<str> + Send + Sync>,
}

impl Host for Globals {
    fn global(&self, name: &str) -> Option<&Core> {
        self.program.defs.get(name).map(|d| &d.body)
    }
    fn new_uuid(&self) -> Arc<str> {
        (self.uuid)()
    }
}

impl Backend for Evaluator {
    fn name(&self) -> &'static str {
        "evaluator"
    }

    fn constant(&self, code: &Core) -> Result<Value, ExecError> {
        let host = Globals {
            program: self.program.clone(),
            uuid: self.uuid.clone(),
        };
        Interp::new(&host)
            .eval(code, &Env::new())
            .map_err(into_exec)
    }

    fn function(&self, code: &Core) -> Result<Callable, ExecError> {
        // A tree-walker's "compilation" is evaluating the lambda to a closure once; the work per
        // call is the walk. A compiling backend would do the opposite, which is the whole reason
        // this is two methods rather than one `call(code, args)`.
        let closure = self.constant(code)?;
        let program = self.program.clone();
        let uuid = self.uuid.clone();
        Ok(Arc::new(move |args: Vec<Value>| {
            let host = Globals {
                program: program.clone(),
                uuid: uuid.clone(),
            };
            Interp::new(&host)
                .apply(&closure, args, beck_diag::Span::NONE)
                .map_err(into_exec)
        }))
    }
}

/// The tree-walking backend for a placed program — what a process picks when it has not been told
/// to pick something else.
///
/// Clones the program into an `Arc` because a [`Callable`] must outlive the `Placed` that produced
/// it. That is one `O(program size)` copy per process, at startup, against a borrow that would
/// otherwise infect every role signature with a lifetime.
pub fn backend(placed: &beck_core::Placed) -> Arc<dyn Backend> {
    Arc::new(Evaluator::new(Arc::new(placed.program.clone())))
}

fn into_exec(e: EvalError) -> ExecError {
    ExecError::new(e.message, e.span)
}

/// A time-ordered id, without pulling a uuid crate into this crate's dependency tree for one call.
///
/// UUIDv7 layout: 48 bits of Unix milliseconds, then version and variant bits, then randomness.
/// The randomness here comes from the system, via `getrandom` through the standard library's hash
/// seed — good enough for an id at the edge, and never reached inside a fold, which is where
/// determinism actually matters (§3.7).
fn uuid_v7() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
        & 0x0000_FFFF_FFFF_FFFF;
    let rand = || RandomState::new().build_hasher().finish();
    let (a, b) = (rand(), rand());

    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    bytes[6..12].copy_from_slice(&a.to_be_bytes()[..6]);
    bytes[12..].copy_from_slice(&b.to_be_bytes()[..4]);
    bytes[6] = (bytes[6] & 0x0F) | 0x70; // version 7
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // variant 10

    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use beck_core::backend::Backend;

    fn program(src: &str) -> Arc<Program> {
        let (placed, d, map) = beck_core::compile_str("t.beck", src);
        assert!(!d.has_errors(), "{}", d.render(&map));
        Arc::new(placed.expect("compiles").program)
    }

    /// A whole program, because the front end refuses one without a merge point — the compiler
    /// will not hand out a `Placed` for a fragment, which is the right call and means a backend
    /// test cannot be written against two loose functions.
    const SRC: &str = r#"
union Command:
    Ping

union Event:
    Pinged

union Rejection:
    No

model State:
    n: Int

def double(n: Int) -> Int:
    return n * 2

def start() -> Int:
    return 21

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(n=(s.n + 1))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Pinged])

def view(s: State, session: Session) -> Html:
    return ui:
        main: str(s.n)

@on(server)
proposals: Stream[Proposal] = merge_clients()

@on(server)
events: Stream[Event] = decide(proposals, st, validate)

@on(data)
st: Signal[State] = durable(fold(apply_event, State(n=0), events))

@on(client)
page: Signal[Html] = per_session(st, view)
"#;

    #[test]
    fn the_backend_prepares_a_function_once_and_calls_it_many_times() {
        let p = program(SRC);
        let backend = Evaluator::new(p.clone());
        let double = backend.function(&p.defs["double"].body).expect("prepares");
        // The property the seam depends on: the callable outlives the call that made it, so a
        // compiling backend can do its expensive work in `function` rather than per event.
        for n in 0..5 {
            assert_eq!(double(vec![Value::Int(n)]).unwrap(), Value::Int(n * 2));
        }
        assert_eq!(backend.name(), "evaluator");
    }

    #[test]
    fn a_constant_reduces_and_an_error_carries_its_span() {
        let p = program(SRC);
        let backend = Evaluator::new(p.clone());
        let start = backend.function(&p.defs["start"].body).expect("prepares");
        assert_eq!(start(vec![]).unwrap(), Value::Int(21));

        // Wrong arity is the runtime's most likely mistake against a compiled program, and it must
        // be an error rather than a panic.
        let double = backend.function(&p.defs["double"].body).unwrap();
        let err = double(vec![]).expect_err("arity is checked");
        assert!(err.to_string().contains("argument"), "{err}");
    }

    #[test]
    fn minted_ids_are_v7_shaped_and_distinct() {
        let a = uuid_v7();
        let b = uuid_v7();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'7', "version nibble: {a}");
        assert!(
            matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
            "variant: {a}"
        );
    }
}
