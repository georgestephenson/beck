//! Talking to the compiled program.
//!
//! # Why a process
//!
//! Calling machine code from Rust means turning a pointer into a function, and that is `unsafe` —
//! the one thing [`docs/43-threat-model.md`](../../../../../docs/43-threat-model.md) §43.2 claims
//! **structurally** about first-party code. So the compiled program is a *process*: it is started
//! once, it reads calls from its standard input and writes answers to its standard output, and the
//! compiler never executes a byte of it. What the host does with the artefact is spawn it and
//! write to a pipe, which needs no privilege `beck build` does not already have.
//!
//! The cost is one pipe round trip per call, and it is not hidden: `docs/93` §93.5 measures it at
//! two sizes, which is what separates the part of a call that is the round trip from the part that
//! is the computation.
//!
//! # There is no fuel in compiled code
//!
//! `beck-eval` bounds a run by steps ([`docs/53`](../../../../../docs/53-are-we-fast-yet-report.md)); machine
//! code has no step to count without paying for the counter on every one of them. What is here
//! instead is coarser and honest about being coarser: an optional **wall-clock limit**, after
//! which the worker is killed and the call is an error naming the limit. It bounds a run that will
//! not stop; it does not bound one that is merely slow, and it is not a quota — `docs/93` §93.14
//! says what a real one would take.
//!
//! # The protocol
//!
//! Fixed widths in the machine's own byte order, because both ends are the same machine and
//! neither should be parsing:
//!
//! | Direction | Bytes | Meaning |
//! |---|---|---|
//! | to the worker | 0..4 | function index |
//! | | 4..8 | argument count |
//! | | 8..16 | how many bytes of heap follow the arguments |
//! | | 16.. | one 8-byte cell per argument, then the heap |
//! | from the worker | 0..4 | trap code, `0` for a value |
//! | | 4..8 | which span trapped |
//! | | 8..16 | the trap's payload |
//! | | 16..24 | the result |
//! | | 24..32 | how many bytes of heap follow |
//! | | 32.. | the heap |
//!
//! The heap is a flat byte string in which an object refers to another by **offset**, so neither
//! end has to walk it to move it: the worker `memcpy`s the request's into its arena, and the reply
//! carries back however much of the arena the call used. [`crate::heap`] is the shape of what is
//! in there, and both directions are empty for a call whose arguments and answer are all scalars —
//! which is every call a program of arithmetic makes.
//!
//! # The second direction
//!
//! A call is not always one message each way. Four primitives are questions rather than
//! computations — `uuid()`, `now()`, `secret_env` and `http_fetch` — and a worker that reaches one
//! writes a **question** frame and blocks until the host answers, any number of times, before the
//! reply. [`crate::Upcall`] is that frame; [`Worker::call`] is the loop that services it, and it
//! services it by calling back into whatever the caller handed it, because this module knows the
//! wire and nothing about what a `Value` is.
//!
//! A question is told from a reply by its first word: [`crate::Upcall::MARKER`], which is no
//! [`crate::Trap`] code and not zero. A host that never asked for the second direction therefore
//! cannot see one — a module with none of those four primitives in it emits no question, and a
//! module with them emits one only where the program wrote the call.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the watchdog looks at the clock. Coarse on purpose: it is the resolution of a limit
/// measured in seconds, and a thread that wakes ten times a second costs nothing.
const TICK: Duration = Duration::from_millis(100);

/// One question the compiled program asked in the middle of a call.
///
/// The arena travels only for a question whose arguments can point into it
/// ([`crate::Upcall::carries_arena`]); for the other two `arena` is empty and `used` is still the
/// mark, because the answer's own offsets are measured from it.
#[derive(Clone, Debug)]
pub struct Question<'a> {
    pub op: crate::Upcall,
    /// Which span asked, as an index into [`crate::Module::spans`].
    pub span: u32,
    /// The worker's arena high-water mark: where an answer's bytes will be appended.
    pub used: u64,
    /// The [`crate::heap::Heap`] shape the answer is expected to have.
    pub ret: u32,
    /// The shape a **failure** would carry, for a question that can fail.
    ///
    /// Sent by the worker rather than found by the host, for [`crate::Upcall::raises`]'s reason:
    /// which type this primitive fails with is a fact about the program, and the host holding a
    /// second opinion about it is the drift this crate's wire exists to prevent. Meaningless — and
    /// zero — for a question that cannot fail.
    pub raises: u32,
    /// A shape and a word per argument, in the order the program wrote them.
    pub args: &'a [(u32, u64)],
    /// The live arena, or empty when this question's arguments cannot point into it.
    pub arena: &'a [u8],
}

/// What the host answered.
///
/// `code` is `0` for a value and a [`crate::Trap`] code otherwise — including
/// [`crate::Trap::Raised`], which is how `http_fetch`'s failure becomes a raise the compiled
/// program's own `try:` can catch.
#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub code: u32,
    pub payload: i64,
    pub value: u64,
    /// Bytes to append at [`Question::used`] — never a whole arena. The host may add to what the
    /// worker allocated and may not rewrite it, which is what makes an answer safe to `memcpy`
    /// into a live heap.
    pub tail: Vec<u8>,
}

/// What one call answered.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub code: u32,
    pub span: u32,
    pub payload: i64,
    pub value: u64,
    /// The arena as it stood when the call answered, when the answer is on it. Empty otherwise —
    /// including for a call that trapped, since a trap's answer is its message.
    pub heap: Vec<u8>,
}

/// A running compiled program.
///
/// One process per [`crate::Artifact`], with the pipe behind a `Mutex` because a
/// [`beck_core::backend::Callable`] is `Send + Sync` and the runtime calls the fold from one task
/// and a view from another. The lock is the round trip, not a computation queue: two threads
/// calling at once serialise, which is what one pipe means and what §93.14 names as the first thing
/// a second version would change.
pub struct Worker {
    pipe: Mutex<Pipe>,
    /// The child, behind its own lock **on purpose**: the watchdog has to be able to kill a worker
    /// while the calling thread is blocked reading from it, and one lock over both would mean
    /// waiting for the very call it is trying to end.
    child: Arc<Mutex<Child>>,
    guard: Option<Guard>,
}

struct Pipe {
    stdin: ChildStdin,
    stdout: ChildStdout,
}

/// The watchdog, and the two words the caller and it share.
struct Guard {
    /// Milliseconds since [`Guard::started`] by which the call in flight must have answered, or
    /// `0` for "no call in flight".
    deadline: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    fired: Arc<AtomicBool>,
    started: Instant,
    limit: Duration,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    /// Start the compiled program, with no limit on how long a call may take.
    pub fn start(exe: &std::path::Path) -> Result<Worker, String> {
        Worker::start_with(exe, None)
    }

    /// The same, killed if a call takes longer than `limit`.
    pub fn start_with(exe: &std::path::Path, limit: Option<Duration>) -> Result<Worker, String> {
        let mut child = Command::new(exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("starting {}: {e}", exe.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or("the worker has no standard input")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("the worker has no standard output")?;

        let worker = Worker {
            pipe: Mutex::new(Pipe { stdin, stdout }),
            child: Arc::new(Mutex::new(child)),
            guard: None,
        };
        let Some(limit) = limit else {
            return Ok(worker);
        };
        Ok(worker.watched(limit))
    }

    fn watched(mut self, limit: Duration) -> Worker {
        let deadline = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let fired = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        self.guard = Some(Guard {
            deadline: deadline.clone(),
            stop: stop.clone(),
            fired: fired.clone(),
            started,
            limit,
            thread: None,
        });
        // The watchdog cannot hold a `&Worker` — it outlives no borrow — so it is handed the same
        // `Arc`s and its own way to reach the child.
        let child = self.child.clone();
        let thread = std::thread::Builder::new()
            .name("beck-native-watchdog".into())
            .spawn(move || loop {
                std::thread::sleep(TICK);
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let at = deadline.load(Ordering::Relaxed);
                if at == 0 || (started.elapsed().as_millis() as u64) < at {
                    continue;
                }
                fired.store(true, Ordering::SeqCst);
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
                return;
            })
            .expect("a watchdog thread");
        if let Some(guard) = &mut self.guard {
            guard.thread = Some(thread);
        }
        self
    }

    /// Call function `index` with `args`, already widened to eight bytes each, and `heap` — the
    /// object graph those cells point into, or empty when none of them do.
    ///
    /// A module with none of the four host primitives in it never asks anything, and `answer` is
    /// never called; [`Worker::call`] with a closure that panics would be a correct way to run
    /// one. [`crate::Artifact::call`] passes the real thing either way, because whether a
    /// *particular call* reaches such a primitive is not a property of the module.
    pub fn call(
        &self,
        index: u32,
        args: &[u64],
        heap: &[u8],
        answer: &dyn Fn(Question) -> Answer,
    ) -> Result<Reply, String> {
        let mut pipe = self
            .pipe
            .lock()
            .map_err(|_| "the worker's pipe was poisoned by a panic".to_string())?;
        if let Some(guard) = &self.guard {
            let at = guard.started.elapsed() + guard.limit;
            guard
                .deadline
                .store(at.as_millis() as u64, Ordering::Relaxed);
        }
        let answer = self.exchange(&mut pipe, index, args, heap, answer);
        if let Some(guard) = &self.guard {
            guard.deadline.store(0, Ordering::Relaxed);
            if guard.fired.load(Ordering::SeqCst) {
                return Err(format!(
                    "the compiled program did not answer within {:?} and was stopped",
                    guard.limit
                ));
            }
        }
        match answer {
            Ok(reply) => Ok(reply),
            Err(e) => Err(self.explain(e)),
        }
    }

    /// Why the pipe went quiet.
    ///
    /// "failed to fill whole buffer" is what the standard library says and it is no use to anybody.
    /// The overwhelmingly likely cause is that the worker died, and the overwhelmingly likely
    /// reason for *that* is a recursion that is not in tail position exhausting the stack — which
    /// is `docs/adr/0007`'s abort, on the one backend where the ceiling that replaced it does not
    /// exist (`docs/93` §93.14).
    fn explain(&self, io: String) -> String {
        let Ok(mut child) = self.child.lock() else {
            return io;
        };
        // `wait` and not `try_wait`: the pipe reaching end of file means the worker's standard
        // output was closed, and the only thing that closes it is the worker exiting — so there is
        // a status coming, and `try_wait` merely races it.
        match child.wait().map(Some) {
            Ok(Some(status)) => format!(
                "the compiled program stopped without answering ({status}). A recursion that is \
                 not in tail position spends host stack, and compiled code has no ceiling on it — \
                 the evaluator's is `beck_eval::DEFAULT_MAX_DEPTH`"
            ),
            _ => io,
        }
    }

    fn exchange(
        &self,
        pipe: &mut Pipe,
        index: u32,
        args: &[u64],
        heap: &[u8],
        answer: &dyn Fn(Question) -> Answer,
    ) -> Result<Reply, String> {
        let mut request = Vec::with_capacity(16 + args.len() * 8 + heap.len());
        request.extend_from_slice(&index.to_ne_bytes());
        request.extend_from_slice(&(args.len() as u32).to_ne_bytes());
        request.extend_from_slice(&(heap.len() as u64).to_ne_bytes());
        for a in args {
            request.extend_from_slice(&a.to_ne_bytes());
        }
        request.extend_from_slice(heap);
        pipe.stdin
            .write_all(&request)
            .map_err(|e| format!("the worker stopped reading: {e}"))?;
        pipe.stdin
            .flush()
            .map_err(|e| format!("the worker stopped reading: {e}"))?;

        loop {
            let mut reply = [0u8; 32];
            pipe.stdout
                .read_exact(&mut reply)
                .map_err(|e| format!("the worker stopped answering: {e}"))?;
            let carried = u64::from_ne_bytes(reply[24..32].try_into().expect("eight bytes"));
            // A length the worker could not have meant is the pipe out of step rather than a big
            // answer, and reading it would be this process allocating whatever the bytes said.
            if carried > crate::heap::ARENA_BYTES {
                return Err(format!(
                    "the compiled program said its answer carries {carried} bytes of heap, and its                  arena is {} bytes",
                    crate::heap::ARENA_BYTES
                ));
            }
            let code = u32::from_ne_bytes(reply[0..4].try_into().expect("four bytes"));
            if code == crate::Upcall::MARKER {
                self.serve(pipe, &reply, carried, answer)?;
                continue;
            }
            let mut carried_heap = vec![0u8; carried as usize];
            pipe.stdout
                .read_exact(&mut carried_heap)
                .map_err(|e| format!("the worker stopped answering: {e}"))?;
            return Ok(Reply {
                code,
                span: u32::from_ne_bytes(reply[4..8].try_into().expect("four bytes")),
                payload: i64::from_ne_bytes(reply[8..16].try_into().expect("eight bytes")),
                value: u64::from_ne_bytes(reply[16..24].try_into().expect("eight bytes")),
                heap: carried_heap,
            });
        }
    }

    /// Read the rest of a question, answer it, and write the answer back.
    ///
    /// **The watchdog is stood down while the host works.** [`Worker::start_with`]'s limit bounds
    /// how long *compiled code* may run without answering; an `http_fetch` that waits thirty
    /// seconds for a peer is not a compiled loop that will not stop, and killing the worker for it
    /// would make the limit a network timeout it was never measured as. The deadline is restored
    /// before the worker is let go again, so the bound still covers the compiled half of the call.
    fn serve(
        &self,
        pipe: &mut Pipe,
        header: &[u8; 32],
        carried: u64,
        answer: &dyn Fn(Question) -> Answer,
    ) -> Result<(), String> {
        let span = u32::from_ne_bytes(header[4..8].try_into().expect("four bytes"));
        let code = i64::from_ne_bytes(header[8..16].try_into().expect("eight bytes"));
        let used = u64::from_ne_bytes(header[16..24].try_into().expect("eight bytes"));
        let op = u32::try_from(code)
            .ok()
            .and_then(crate::Upcall::from_code)
            .ok_or_else(|| format!("the compiled program asked for host effect {code}, which this compiler does not have"))?;

        let mut words = vec![0u8; (2 + 2 * op.arity()) * 8];
        pipe.stdout
            .read_exact(&mut words)
            .map_err(|e| format!("the worker stopped answering: {e}"))?;
        let word =
            |i: usize| u64::from_ne_bytes(words[i * 8..i * 8 + 8].try_into().expect("eight"));
        let (ret, raises) = (word(0) as u32, word(1) as u32);
        let args: Vec<(u32, u64)> = (0..op.arity())
            .map(|i| (word(2 + 2 * i) as u32, word(3 + 2 * i)))
            .collect();

        let mut arena = vec![0u8; carried as usize];
        pipe.stdout
            .read_exact(&mut arena)
            .map_err(|e| format!("the worker stopped answering: {e}"))?;

        let held = self.guard.as_ref().map(|guard| {
            let was = guard.deadline.swap(0, Ordering::Relaxed);
            (guard, was)
        });
        let out = answer(Question {
            op,
            span,
            used,
            ret,
            raises,
            args: &args,
            arena: &arena,
        });
        if let Some((guard, was)) = held {
            // Restored as a *fresh* deadline rather than the old one: the compiled half of this
            // call is starting again from here, and charging it for the time the host spent is
            // exactly the conflation the paragraph above refuses.
            let at = if was == 0 {
                0
            } else {
                (guard.started.elapsed() + guard.limit).as_millis() as u64
            };
            guard.deadline.store(at, Ordering::Relaxed);
        }

        let mut frame = Vec::with_capacity(32 + out.tail.len());
        frame.extend_from_slice(&out.code.to_ne_bytes());
        frame.extend_from_slice(&0u32.to_ne_bytes());
        frame.extend_from_slice(&out.payload.to_ne_bytes());
        frame.extend_from_slice(&out.value.to_ne_bytes());
        frame.extend_from_slice(&(out.tail.len() as u64).to_ne_bytes());
        frame.extend_from_slice(&out.tail);
        pipe.stdin
            .write_all(&frame)
            .map_err(|e| format!("the worker stopped reading: {e}"))?;
        pipe.stdin
            .flush()
            .map_err(|e| format!("the worker stopped reading: {e}"))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(guard) = &mut self.guard {
            guard.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = guard.thread.take() {
                let _ = thread.join();
            }
        }
        // Killed rather than asked: a worker looping inside compiled code will never notice its
        // pipe closing, and a `wait` on it would never return.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
