//! The host's half of the second direction: answering what a compiled program asked.
//!
//! # Why this is shared and the emitters are not
//!
//! `beck-clif` writes its own code generation on purpose — two independent readings of one
//! language are what the differential between them is worth. It does **not** write its own wire,
//! its own layout or its own host, because those are contracts with a third party (this process),
//! and a contract with two spellings drifts. [`crate::worker`] is the wire; [`crate::heap`] is the
//! layout; this module is the host, and both backends call it.
//!
//! # What answering a question is
//!
//! Three steps, and none of them knows which primitive it is serving:
//!
//! 1. **Decode.** Each argument arrives as a shape and a word; [`crate::heap::Heap::shape`] turns
//!    the shape into a [`crate::Repr`] and [`crate::heap::Heap::decode`] turns the word into a
//!    [`Value`]. The arena the words point into came with the question.
//! 2. **Ask.** `perform` is the only part that is per-primitive, and it is four match arms over
//!    [`beck_core::host::Atoms`] — the same trait the tree-walker's host extends, so a
//!    differential between the backends compares the *program* rather than two opinions about
//!    what the host said.
//! 3. **Encode.** The answer goes back at the shape the worker said it expects, appended to a blob
//!    that starts at the arena's high-water mark so that every offset it contains is one the
//!    worker will read at the right place. Only the appended **tail** travels.
//!
//! # A failure is an answer
//!
//! `http_fetch` fails by raising `HttpError`, and a raise is already a thing the compiled code
//! knows how to carry ([`docs/112`](../../../../../docs/112-a-raise-arrives-report.md)): the error
//! cell, a two-word pair of shape and value, and a `try:` that compares a type name. So a failed
//! fetch is answered with [`Trap::Raised`] and the pair, and the compiled program's own handler
//! catches it without knowing an upcall happened. Nothing was added to the raise mechanism to make
//! that work, which is the same shape §112.1 records for the mechanism itself.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use beck_core::host::Atoms;
use beck_core::Value;

use crate::heap::{Heap, Repr, RAISED_WORDS, WORD};
use crate::worker::{Answer, Question};
use crate::{Trap, Upcall};

/// What one call's questions did: how many there were, how much heap travelled, and the sentence a
/// [`Trap::HostFailed`] cannot carry.
///
/// **The counts are here because they are a gate.** How much arena an upcall copies is the one
/// design decision in this protocol that a reader would want checked rather than believed — a
/// question whose arguments cannot point into the heap sends none of it, and one whose arguments
/// can sends all of it — and both halves of that are countable with no clock in the measurement,
/// which is the kind of gate `AGENTS.md` asks for and the kind that does not flake.
///
/// **The excuse is here because a trap is a number.** "The compiled program asked for `secret_env`
/// with a word that is not text" is a sentence; [`crate::Artifact`] reads it back and reports it in
/// place of [`Trap::HostFailed`]'s own message, so a reader sees what went wrong rather than that
/// something did.
#[derive(Debug, Default)]
pub struct Asking {
    why: Mutex<Option<String>>,
    questions: AtomicU64,
    /// Bytes of arena sent *to* the host, summed over this call's questions.
    carried: AtomicU64,
}

impl Asking {
    pub fn new() -> Asking {
        Asking::default()
    }

    /// Forget the last call. A reason left over from a previous one would be attached to a failure
    /// it did not explain, and a count left over would be a count of two calls.
    pub fn clear(&self) {
        if let Ok(mut held) = self.why.lock() {
            *held = None;
        }
        self.questions.store(0, Ordering::Relaxed);
        self.carried.store(0, Ordering::Relaxed);
    }

    pub fn take(&self) -> Option<String> {
        self.why.lock().ok().and_then(|mut held| held.take())
    }

    /// How many questions the last call asked, and how many bytes of arena went with them.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.questions.load(Ordering::Relaxed),
            self.carried.load(Ordering::Relaxed),
        )
    }

    fn record(&self, why: String) -> Answer {
        if let Ok(mut held) = self.why.lock() {
            *held = Some(why);
        }
        Answer {
            code: Trap::HostFailed.code(),
            ..Answer::default()
        }
    }
}

/// Answer one question, or record why it could not be answered.
///
/// The `Ok`/`Err` split inside is between *the program failing the way its type says it can* — a
/// raise, which is an answer — and *this compiler being unable to serve the question at all*,
/// which is a bug in this backend and is reported as one.
pub fn answer(heap: &Heap, atoms: &dyn Atoms, asking: &Asking, q: Question) -> Answer {
    asking.questions.fetch_add(1, Ordering::Relaxed);
    asking
        .carried
        .fetch_add(q.arena.len() as u64, Ordering::Relaxed);
    match serve(heap, atoms, &q) {
        Ok(answer) => answer,
        Err(why) => asking.record(format!("`{}` could not be answered: {why}", q.op.name())),
    }
}

fn serve(heap: &Heap, atoms: &dyn Atoms, q: &Question) -> Result<Answer, String> {
    if !q.used.is_multiple_of(WORD) {
        return Err(format!(
            "the compiled program's heap is {} bytes used, which is not a whole number of words",
            q.used
        ));
    }
    let mut args = Vec::with_capacity(q.args.len());
    for (shape, cell) in q.args {
        let repr = shape_of(heap, *shape)?;
        args.push(heap.decode(*cell, repr, q.arena)?);
    }

    // The blob starts *at* the mark rather than empty, so that an offset `Heap::encode` computes
    // from its length is the offset the worker will read the object at. The bytes below the mark
    // are never sent: for a question whose arguments point into the arena they are the arena, and
    // for one whose arguments do not they are padding this process invented and nothing reads.
    let mut blob = if q.arena.is_empty() {
        vec![0u8; q.used as usize]
    } else {
        q.arena.to_vec()
    };
    if blob.len() as u64 != q.used {
        return Err(format!(
            "the compiled program sent {} bytes of heap and said it had used {}",
            blob.len(),
            q.used
        ));
    }

    match perform(atoms, q.op, &args)? {
        Ok(value) => {
            let repr = shape_of(heap, q.ret)?;
            let cell = heap.encode(&value, repr, &mut blob)?;
            Ok(Answer {
                code: 0,
                payload: 0,
                value: cell,
                tail: blob.split_off(q.used as usize),
            })
        }
        // A raise: the value, then the two-word pair the error cell points at — the shape and the
        // word, which is exactly what a compiled `raise` builds.
        Err(failure) => {
            let repr = shape_of(heap, q.raises)?;
            let cell = heap.encode(&failure, repr, &mut blob)?;
            let pair = blob.len() as u64;
            debug_assert_eq!(RAISED_WORDS, 2);
            blob.extend_from_slice(&u64::from(q.raises).to_ne_bytes());
            blob.extend_from_slice(&cell.to_ne_bytes());
            Ok(Answer {
                code: Trap::Raised.code(),
                payload: pair as i64,
                value: 0,
                tail: blob.split_off(q.used as usize),
            })
        }
    }
}

/// The four questions, and the only per-primitive code in this module.
///
/// The outer `Result` is "could this compiler serve the question"; the inner one is "did the
/// program's own call succeed", and only `http_fetch` has a way to say no.
fn perform(atoms: &dyn Atoms, op: Upcall, args: &[Value]) -> Result<Result<Value, Value>, String> {
    match op {
        Upcall::Now => Ok(Ok(Value::Int(atoms.now_millis()))),
        Upcall::NewUuid => Ok(Ok(Value::str_(atoms.new_uuid()))),
        Upcall::SecretEnv => {
            let name = text(args.first(), "secret_env")?;
            // A `secret[Str]` is a one-field object at runtime, and the layout the worker asked
            // for is that object's — so this builds the same `Value` the evaluator's arm builds
            // and lets `Heap::encode` place it.
            Ok(Ok(Value::data(
                std::sync::Arc::from(beck_core::Ty::SECRET),
                None,
                beck_core::core::Fields::from_iter([(
                    std::sync::Arc::from("value"),
                    Value::str_(atoms.secret(&name)),
                )]),
            )))
        }
        Upcall::HttpFetch => {
            let host = text(args.first(), "http_fetch")?;
            let request = args
                .get(1)
                .ok_or("`http_fetch` was asked with no request")?
                .clone();
            let request = beck_core::host::request_of(&host, &request)?;
            Ok(match atoms.fetch(&request) {
                Ok(reply) => Ok(beck_core::host::reply_value(&reply)),
                Err(f) => Err(beck_core::host::failure_value(&host, &f)),
            })
        }
    }
}

fn text(v: Option<&Value>, who: &str) -> Result<String, String> {
    v.and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("`{who}` was asked with a word that is not text"))
}

fn shape_of(heap: &Heap, at: u32) -> Result<Repr, String> {
    heap.shape(at).ok_or_else(|| {
        format!("the compiled program named shape {at}, and this module has no such shape")
    })
}
