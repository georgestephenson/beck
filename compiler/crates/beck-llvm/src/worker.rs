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
//! `beck-eval` bounds a run by steps ([`docs/62`](../../../../../docs/62-fuel-report.md)); machine
//! code has no step to count without paying for the counter on every one of them. What is here
//! instead is coarser and honest about being coarser: an optional **wall-clock limit**, after
//! which the worker is killed and the call is an error naming the limit. It bounds a run that will
//! not stop; it does not bound one that is merely slow, and it is not a quota — `docs/93` §93.7
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

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the watchdog looks at the clock. Coarse on purpose: it is the resolution of a limit
/// measured in seconds, and a thread that wakes ten times a second costs nothing.
const TICK: Duration = Duration::from_millis(100);

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
/// calling at once serialise, which is what one pipe means and what §93.7 names as the first thing
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
    pub fn call(&self, index: u32, args: &[u64], heap: &[u8]) -> Result<Reply, String> {
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
        let answer = self.exchange(&mut pipe, index, args, heap);
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
    /// exist (`docs/93` §93.7).
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
        let mut carried_heap = vec![0u8; carried as usize];
        pipe.stdout
            .read_exact(&mut carried_heap)
            .map_err(|e| format!("the worker stopped answering: {e}"))?;
        Ok(Reply {
            code: u32::from_ne_bytes(reply[0..4].try_into().expect("four bytes")),
            span: u32::from_ne_bytes(reply[4..8].try_into().expect("four bytes")),
            payload: i64::from_ne_bytes(reply[8..16].try_into().expect("eight bytes")),
            value: u64::from_ne_bytes(reply[16..24].try_into().expect("eight bytes")),
            heap: carried_heap,
        })
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
