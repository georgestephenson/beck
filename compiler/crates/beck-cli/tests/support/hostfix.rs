//! The programs a differential over the **host effects** needs, and the host that answers them.
//!
//! Shared for the reason [`super::failfix`] is: two compiled backends and the tree-walker are held
//! to these, and a second copy of the programs would be a second opinion about what the subset is.
//!
//! # Why a differential over these needs a stated host at all
//!
//! Every other differential in this workspace compares two backends over a *function*: the same
//! arguments go in, the same answer must come out, and nothing outside the program is consulted.
//! These four primitives are the opposite — `uuid()` and `now()` are `nondet`, `secret_env` reads
//! the environment and `http_fetch` reaches a peer — so two backends asked the same question at
//! two instants would legitimately disagree, and a differential over the *process* clock would be
//! asserting that two calls happened in the same millisecond.
//!
//! So both backends are handed [`Stated`]: one clock reading, ids from a counter, a table of
//! secrets and canned replies. What the differential then compares is what it was always for —
//! that the two implementations do the same thing with the same answer.
//!
//! # What these are chosen to catch
//!
//! * **A question with no arguments and a scalar answer** (`now`), which is the whole protocol with
//!   nothing on the heap in either direction.
//! * **A question with no arguments and a `Str` answer** (`uuid`), which is the first time the host
//!   writes into the worker's arena rather than reading out of it.
//! * **A question whose argument is text** (`secret_env`), so the arena has to travel *to* the host
//!   — and whose answer is a `secret[Str]`, which is a one-field object no program declares.
//! * **Two questions in one call**, because the second one's offsets are measured from a mark the
//!   first one moved. A protocol that reset the mark would answer the first correctly and corrupt
//!   the second.
//! * **A question inside a loop**, which is where a per-call `alloca` for the question buffer would
//!   grow the stack, and where an arena that travels every time shows up as a cost.
//! * **A question whose answer is used to build something** — text concatenated, a record made —
//!   since an answer written at the wrong offset reads back as whatever was there before.
//! * **`http_fetch` succeeding and failing**, the second of which is a *raise* the compiled code
//!   has to carry: caught by a `try:` in one definition, uncaught and crossing the boundary in
//!   another.

#![allow(dead_code)] // each suite uses the half of this it needs

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use beck_core::net::{Failure, Reply, Request, Stop};
use beck_core::Value;

/// A host whose four answers are decided in advance.
///
/// Not a mock of an interface: it is a value for each effect, which is
/// [`docs/21`](../../../../../docs/21-tests-in-beck-and-proof.md) §21.3's rule and the shape
/// `beck test`'s own stubs take. The id counter is what makes `uuid()` comparable at all — two
/// backends minting a real v7 id would disagree correctly, and a differential cannot tell that
/// apart from disagreeing wrongly.
#[derive(Debug, Default)]
pub struct Stated {
    minted: AtomicUsize,
    asked: AtomicUsize,
}

impl Stated {
    pub fn new() -> Arc<Stated> {
        Arc::new(Stated::default())
    }

    /// How many outbound calls were made — the count that says *both* backends made the call
    /// rather than one of them being the evaluator twice.
    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }

    /// Forget how many ids have been minted.
    ///
    /// Called before each backend is driven over a case, which is what makes `uuid()` comparable:
    /// the two are asked the same question and the answers are a function of how many have been
    /// asked *within* the case rather than of which backend went first. Without it a differential
    /// over a minted id compares the first backend's `…000` against the second's `…001` and calls
    /// a correct implementation wrong.
    pub fn rewind(&self) {
        self.minted.store(0, Ordering::SeqCst);
    }

    /// The instant every `now()` reads. A stated number rather than a clock, because two backends
    /// called one after the other are not in the same millisecond.
    pub const INSTANT: i64 = 1_700_000_000_123;
}

impl beck_core::host::Atoms for Stated {
    fn now_millis(&self) -> i64 {
        Stated::INSTANT
    }

    /// Sequential rather than random, and rewound between backends ([`Stated::rewind`]): a real
    /// v7 id would make two backends disagree correctly, and a differential cannot tell that apart
    /// from disagreeing wrongly.
    fn new_uuid(&self) -> Arc<str> {
        let n = self.minted.fetch_add(1, Ordering::SeqCst);
        Arc::from(format!("00000000-0000-7000-8000-{n:012}"))
    }

    fn secret(&self, name: &str) -> Arc<str> {
        match name {
            "BECK_TEST_TOKEN" => Arc::from("s3cret"),
            // The evaluator's own default for a name the environment does not have, which is what
            // makes "no such variable" a comparable outcome rather than a failure.
            _ => Arc::from(""),
        }
    }

    /// `example.com` answers; anything else is unreachable. Both are *the same* answer however
    /// many times they are asked, because the differential asks twice.
    fn fetch(&self, request: &Request, _stop: &Stop) -> Result<Reply, Failure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if &*request.host != "example.com" {
            return Err(Failure::Unreachable(format!(
                "nothing is listening for {}",
                request.host
            )));
        }
        Ok(Reply {
            status: 200,
            headers: vec![(Arc::from("content-type"), Arc::from("text/plain"))],
            body: Arc::from(format!("{} {}", request.method, request.path)),
        })
    }
}

pub const EFFECTS: &str = r#"
model Stamped:
    at: Int
    id: Str

# The whole protocol with nothing on the heap in either direction.
def instant() -> Int uses nondet:
    return now()

# The first answer the host writes into the worker's arena.
def minted() -> Str uses nondet:
    return uuid()

# Two questions in one call: the second one's answer is written past where the first one's went.
def stamp() -> Stamped uses nondet:
    return Stamped(at=now(), id=uuid())

# An answer built on rather than returned, so an answer at the wrong offset reads back wrong.
def labelled(prefix: Str) -> Str uses nondet:
    return prefix + ":" + uuid()

# A question inside a loop. The buffer is one per function and the mark moves every time.
def several(n: Int) -> list[Str] uses nondet:
    return map_list(range_list(n), lambda i: uuid())

def range_list(n: Int) -> list[Int]:
    if n <= 0:
        return []
    return list_append(range_list(n - 1), n)

# The arena travelling *to* the host: the name is text the program built.
def token(name: Str) -> secret[Str] uses env:
    return secret_env(name)

# A secret is opaque — it can be carried but not read — so what a test can compare is a request
# built from one, which is the shape `lib/http.beck` writes.
def authorized(name: Str) -> HttpRequest uses env:
    return HttpRequest(
        method="GET",
        path="/me",
        headers={},
        body="",
        port=443,
        tls=True,
        # `map_insert` on `{}` rather than a map literal with an entry in it, which this backend
        # still refuses for a reason that has nothing to do with secrets: a literal's keys are
        # expressions and would have to be sorted at run time.
        secrets=map_insert({}, "authorization", secret_env(name)),
    )

def plain(path: Str) -> HttpRequest:
    return HttpRequest(method="GET", path=path, headers={}, body="", port=80, tls=False, secrets={})

# Grow the arena, then ask — the pair that says what a question copies is a decision.
#
# The two are the same program but for which question they end on: `now()` takes no argument and
# cannot point into the heap, so nothing of the arena travels however much is live; `secret_env`
# takes text the program built, so the host cannot read it without the bytes. Both are counted
# rather than timed, at two sizes.
def clock_after(n: Int) -> Int uses nondet:
    xs = range_list(n)
    return now() + list_len(xs)

def secret_after(n: Int) -> HttpRequest uses env:
    xs = range_list(n)
    return HttpRequest(
        method=str(list_len(xs)),
        path="/",
        headers={},
        body="",
        port=80,
        tls=False,
        secrets=map_insert({}, "authorization", secret_env("BECK_TEST_TOKEN")),
    )

# The outbound call, answering.
def reached(path: Str) -> Int uses net.out("example.com"), raises(HttpError):
    return http_fetch("example.com", plain(path)).status

def body_of(path: Str) -> Str uses net.out("example.com"), raises(HttpError):
    return http_fetch("example.com", plain(path)).body

# The outbound call, failing, with nothing catching: the raise crosses the boundary and the host
# builds the message out of the value.
def unreachable() -> Int uses net.out("nowhere.invalid"), raises(HttpError):
    return http_fetch("nowhere.invalid", plain("/")).status

# The same failure, caught. A `try:` over an upcall is the case where the answer *is* the failure.
def attempted() -> Str uses net.out("nowhere.invalid"):
    r = try:
        http_fetch("nowhere.invalid", plain("/")).body
    match r:
        case Ok(value):
            return value
        case Err(error):
            match error:
                case HttpUnreachable(host, why):
                    return "unreachable " + host
                case HttpTimedOut(host, millis):
                    return "timed out"
                case HttpBadResponse(why):
                    return "bad"
                case HttpStatus(status, body):
                    return "status"

# Caught on the path that succeeds, which is the arm that would still pass if the handler were
# never reached.
def either(path: Str) -> Str uses net.out("example.com"):
    r = try:
        http_fetch("example.com", plain(path)).body
    match r:
        case Ok(value):
            return value
        case Err(error):
            return "no"
"#;

/// The calls, in the order the differential makes them.
///
/// `several` is bounded at four rather than swept: every element is a round trip to this process,
/// and a differential that minted ten thousand ids would be measuring the pipe.
pub fn calls() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        ("instant", vec![]),
        ("minted", vec![]),
        ("stamp", vec![]),
        ("labelled", vec![Value::str_("run")]),
        ("labelled", vec![Value::str_("")]),
        ("several", vec![Value::Int(0)]),
        ("several", vec![Value::Int(4)]),
        ("token", vec![Value::str_("BECK_TEST_TOKEN")]),
        ("token", vec![Value::str_("NOT_IN_THE_ENVIRONMENT")]),
        ("authorized", vec![Value::str_("BECK_TEST_TOKEN")]),
        ("plain", vec![Value::str_("/things")]),
        ("reached", vec![Value::str_("/things")]),
        ("body_of", vec![Value::str_("/things")]),
        ("unreachable", vec![]),
        ("attempted", vec![]),
        ("either", vec![Value::str_("/ok")]),
    ]
}
