//! The WebAssembly emitter, against the tree-walker, in a real WebAssembly engine.
//!
//! [`docs/05`](../../../../docs/05-tier-lowering.md) §5.1 asks for "the component's pure code
//! compiled to WASM", and [`adr/0022`](../../../../docs/adr/0022-mode-b-ships-the-backend-it-has.md)
//! records why Mode B ships an interpreter today. This is the first half of what would reverse
//! that decision: [`beck_wasmgen`] compiles the **scalar subset**, and the heap — which is what a
//! `view` is made of — is still not laid out on this target.
//!
//! # The programs are not this file's
//!
//! They are [`support::scalar`]'s, which `native.rs` and `cranelift.rs` already point at. That is
//! the whole reason they are shared: a fourth copy of "what the scalar subset is" would be a
//! fourth opinion, and what a differential is for is that there is only one.
//!
//! # What is compared
//!
//! The **whole outcome**: the value, or the failure *and its message*. A trap is a
//! [`beck_llvm::Trap`] code in an exported global here rather than a cell in an arena, and it is
//! decoded by [`beck_llvm::Trap::message`] — the same function the native host calls — so a
//! backend that failed for a different reason than the evaluator is a divergence rather than an
//! agreement.
//!
//! Reals cross as **bit patterns**, not as decimal: a differential that round-tripped a real
//! through JSON would be comparing two printers.
//!
//! # Skipping
//!
//! There is no WebAssembly engine in this workspace — [`docs/07`](../../../../docs/07-dependencies.md)
//! names Wasmtime for the *server* tier and nothing here takes it as a dependency to run a test.
//! What runs the module is a JavaScript engine, which is also what will run it in production, so
//! the suite looks for `node` (or `BECK_JS`) and **prints why it skipped** when there is none.
//! `BECK_REQUIRE_WASM_RUN=1` forbids the skip, which is what CI sets. `docs/19` §19.4 item 10 is
//! why the skip is loud.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use beck_core::backend::Backend;
use beck_core::{Program, Value};
use beck_llvm::heap::{self, Heap, Repr};
use beck_llvm::service::Asking;
use beck_llvm::{Question, Trap, Upcall};

mod support;
use support::hostfix::Stated;
use support::scalar::{
    float_pairs, floats, ints, pairs, render, singles, ARITHMETIC, CONTROL, REALS, RECURSION,
};
use support::{clofix, failfix, genfix, heapfix, hostfix, listfix, mapfix, textfix, viewfix};

fn require_run() -> bool {
    std::env::var("BECK_REQUIRE_WASM_RUN").is_ok_and(|v| v == "1")
}

/// A JavaScript engine that can load a module, or `None`.
fn engine() -> Option<PathBuf> {
    if let Ok(named) = std::env::var("BECK_JS") {
        let path = PathBuf::from(named);
        return Command::new(&path)
            .arg("--version")
            .output()
            .is_ok()
            .then_some(path);
    }
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from("node"))
}

macro_rules! engine {
    () => {
        match engine() {
            Some(js) => js,
            None => {
                assert!(
                    !require_run(),
                    "BECK_REQUIRE_WASM_RUN=1 and there is no JavaScript engine on the path"
                );
                println!(
                    "skipped: no JavaScript engine — no `node` on the path, and BECK_JS does not \
                     name one. Set BECK_REQUIRE_WASM_RUN=1 to make this a failure."
                );
                return;
            }
        }
    };
}

/// The driver: one process per definition, one JSON document in and one out.
///
/// Written by the test rather than checked in, so it cannot drift from the protocol the Rust half
/// encodes. It is the whole of the host this backend has — there is no worker and no pipe, because
/// a WebAssembly module is loaded by whoever is going to call it.
///
/// # The heap crosses as the memory itself
///
/// [`adr/0033`](../../../../docs/adr/0033-the-webassembly-heap-is-the-arena-in-linear-memory.md)
/// is what makes this eleven lines rather than a marshalling layer: the module's linear memory *is*
/// [`beck_llvm::heap`]'s arena, and byte `i` of what `Heap::encode_args` produces is offset `i`. So
/// the driver writes that blob at address zero, moves the bump pointer past it, calls, and hands
/// back the used prefix of the memory. Nothing here knows what a record or a list is.
const DRIVER: &str = r#"
const fs = require('fs');
const [, , wasmPath, callsPath] = process.argv;

// A synchronous line at a time, because an import has to *answer* before the call it interrupted
// can carry on — which is what a question is.
let pending = Buffer.alloc(0);
function line() {
  for (;;) {
    const nl = pending.indexOf(10);
    if (nl >= 0) {
      const got = pending.subarray(0, nl).toString();
      pending = pending.subarray(nl + 1);
      return got;
    }
    const chunk = Buffer.alloc(1 << 16);
    let n = 0;
    try {
      n = fs.readSync(0, chunk, 0, chunk.length, null);
    } catch (err) {
      if (err.code === 'EAGAIN') continue;
      throw err;
    }
    if (n <= 0) throw new Error('the host closed the pipe');
    pending = Buffer.concat([pending, chunk.subarray(0, n)]);
  }
}
const ask = (q) => {
  process.stdout.write(JSON.stringify({ q }) + '\n');
  return JSON.parse(line());
};

let e;
const grow = (bytes) => {
  if (!e.memory) return;
  const want = Math.ceil((bytes + 1) / 65536);
  const have = e.memory.buffer.byteLength / 65536;
  if (want > have) e.memory.grow(want - have);
};
// The four questions a computation cannot answer. What crosses is the frame the module built: the
// question, the shapes, and a word per argument — so this knows nothing about what `secret_env`
// takes or what `http_fetch` answers.
const imports = {
  beck: {
    upcall: (op, span, ret, raises, named, a0, s0, a1, s1) => {
      const used = Number(e.beck_heap.value);
      const answer = ask({
        op, span, ret, raises,
        used: String(used),
        args: [[s0, a0.toString()], [s1, a1.toString()]],
        arena: Buffer.from(new Uint8Array(e.memory.buffer, 0, used)).toString('base64'),
      });
      const tail = Buffer.from(answer.tail, 'base64');
      if (tail.length) {
        grow(used + tail.length);
        new Uint8Array(e.memory.buffer).set(tail, used);
      }
      e.beck_heap.value = BigInt(used + tail.length);
      if (answer.code !== 0) {
        e.beck_trap.value = answer.code;
        e.beck_trap_span.value = span;
        e.beck_trap_payload.value = BigInt(answer.payload);
        e.beck_trap_type.value = named;
        return 0n;
      }
      return BigInt(answer.value);
    },
  },
};

const inst = new WebAssembly.Instance(
  new WebAssembly.Module(fs.readFileSync(wasmPath)), imports);
e = inst.exports;
const view = new DataView(new ArrayBuffer(8));
const toF64 = (bits) => { view.setBigUint64(0, BigInt(bits)); return view.getFloat64(0); };
const fromF64 = (f) => { view.setFloat64(0, f); return view.getBigUint64(0).toString(); };
const calls = JSON.parse(fs.readFileSync(callsPath, 'utf8'));
const out = [];
for (let at = 0; at < calls.length; at += 1) {
  const c = calls[at];
  // The host is told a call is starting, so a stated answer that counts (a minted id) counts from
  // the same place it does for the tree-walker.
  process.stdout.write(JSON.stringify({ call: at }) + '\n');
  const blob = Buffer.from(c.blob, 'base64');
  if (e.memory) {
    grow(blob.length);
    if (blob.length) new Uint8Array(e.memory.buffer).set(blob, 0);
  }
  const args = c.args.map((a) =>
    a.k === 'i' ? BigInt(a.v) : a.k === 'f' ? toF64(a.v) : (a.v ? 1 : 0));
  e.beck_trap.value = 0;
  e.beck_trap_span.value = 0;
  e.beck_trap_payload.value = 0n;
  if (e.beck_trap_type) e.beck_trap_type.value = 0n;
  e.beck_heap.value = BigInt(c.hp);
  let r;
  try {
    r = e[c.fn](...args);
  } catch (err) {
    out.push({ crash: String(err) });
    continue;
  }
  const used = Number(e.beck_heap.value);
  const heap = e.memory
    ? Buffer.from(new Uint8Array(e.memory.buffer, 0, used)).toString('base64')
    : '';
  if (e.beck_trap.value !== 0) {
    // A `raise` carries a value, so the arena travels with the failure — which is the one place
    // the protocol treats a failure like an answer.
    out.push({
      trap: e.beck_trap.value,
      payload: e.beck_trap_payload.value.toString(),
      heap,
    });
    continue;
  }
  out.push(
    c.ret === 'i' ? { k: 'i', v: r.toString(), heap }
    : c.ret === 'f' ? { k: 'f', v: fromF64(r), heap }
    : { k: 'b', v: r !== 0, heap });
}
process.stdout.write(JSON.stringify({ out }) + '\n');
"#;

/// What one backend answered: a value, or the message it failed with.
type Outcome = Result<Value, String>;

fn outcome(r: Result<Value, beck_core::ExecError>) -> Outcome {
    r.map_err(|e| e.message)
}

/// Emit a module on the stack the front end declares.
///
/// `beck-cli` dispatches every command onto it (`beck_diag::depth::STACK_BYTES`), so this is the
/// same ground a real caller stands on. Without it a test thread's default stack is what decides
/// whether a program compiles, which is `docs/64` §64.4's defect exactly.
fn emit(program: &Program) -> beck_wasmgen::Module {
    beck_diag::depth::on_the_front_end_stack(|| beck_wasmgen::module(program))
}

fn compile(name: &str, src: &str) -> Arc<Program> {
    let (placed, diags, map) = beck_core::compile_or_library_str(name, src);
    assert!(!diags.has_errors(), "{name}:\n{}", diags.render(&map));
    Arc::new(
        placed
            .unwrap_or_else(|| panic!("{name} did not slice"))
            .program,
    )
}

/// Both backends over one program.
struct Both {
    program: Arc<Program>,
    module: beck_wasmgen::Module,
    evaluator: Arc<dyn Backend>,
    js: PathBuf,
    dir: PathBuf,
    /// The stated host, when this program has effects. Rewound before each backend is driven over
    /// a case, so that the *n*th question of a call gets the same answer whichever asked it.
    stated: Option<Arc<Stated>>,
}

impl Both {
    fn over(name: &str, src: &str, js: PathBuf) -> Both {
        Both::host(name, src, js, None)
    }

    /// The same, with both backends answering their host effects from one stated host.
    ///
    /// This is what makes a differential over `now()` mean anything: the two are asked the same
    /// question and told the same answer, so what is left to compare is what the *backends* did
    /// with it.
    fn answering(name: &str, src: &str, js: PathBuf, atoms: Arc<Stated>) -> Both {
        Both::host(name, src, js, Some(atoms))
    }

    fn host(name: &str, src: &str, js: PathBuf, stated: Option<Arc<Stated>>) -> Both {
        let program = compile(name, src);
        let module = emit(&program);
        // Unique per `Both` rather than per program: two tests over one fixture run in one
        // process, and a shared directory means one of them deletes the other's module on the way
        // out.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "beck-wasm-{}-{}-{}",
            name.replace(['/', '.'], "-"),
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("a working directory");
        std::fs::write(dir.join("module.wasm"), &module.wasm).expect("the module");
        std::fs::write(dir.join("module.wat"), &module.text).expect("the listing");
        std::fs::write(dir.join("driver.js"), DRIVER).expect("the driver");
        let evaluator: Arc<dyn Backend> = match &stated {
            Some(atoms) => {
                Arc::new(beck_eval::Evaluator::new(program.clone()).answering(atoms.clone()))
            }
            None => beck_eval::backend_for(program.clone()),
        };
        Both {
            evaluator,
            program,
            module,
            js,
            dir,
            stated,
        }
    }

    fn compiled(&self, name: &str) -> bool {
        self.module.signature(name).is_some()
    }

    fn refusal(&self, name: &str) -> Option<&str> {
        self.module
            .refusals
            .iter()
            .find(|r| &*r.name == name)
            .map(|r| r.reason.as_str())
    }

    /// Every tuple through the emitted module, in one engine process.
    fn in_wasm(&self, name: &str, tuples: &[Vec<Value>]) -> Vec<Outcome> {
        let sig = self
            .module
            .signature(name)
            .unwrap_or_else(|| panic!("`{name}` did not compile"));
        let ret = kind(sig.ret);
        let heap = &self.module.heap;
        // Where the arena starts, which is past the pool the module's own data segment holds.
        let pool = heap::FIRST + heap.pool_bytes();
        let mut encoded = Vec::with_capacity(tuples.len());
        let mut calls: Vec<serde_json::Value> = Vec::new();
        for args in tuples {
            let (cells, blob) = heap
                .encode_args(args, &sig.params)
                .unwrap_or_else(|e| panic!("`{name}{}` does not encode: {e}", render(args)));
            calls.push(serde_json::json!({
                "fn": name,
                "ret": ret,
                "hp": (blob.len() as u64).max(pool).to_string(),
                "blob": base64(&blob),
                "args": cells
                    .iter()
                    .zip(&sig.params)
                    .map(|(w, r)| cell(*w, *r))
                    .collect::<Vec<_>>(),
            }));
            encoded.push(blob);
        }
        let calls_path = self.dir.join(format!("{name}.json"));
        std::fs::write(
            &calls_path,
            serde_json::to_string(&calls).expect("the calls encode"),
        )
        .expect("the calls file");

        let mut child = Command::new(&self.js)
            .arg(self.dir.join("driver.js"))
            .arg(self.dir.join("module.wasm"))
            .arg(&calls_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the engine runs");
        let mut ask = child.stdin.take().expect("the engine takes questions");
        let said = BufReader::new(child.stdout.take().expect("the engine answers"));
        // Drained on a thread of its own: a child that filled its error pipe while this loop was
        // waiting on its output would deadlock, and what fills it is exactly the case worth
        // reading — a module the engine refused.
        let mut errors = child.stderr.take().expect("the engine complains");
        let complaints = std::thread::spawn(move || {
            let mut held = String::new();
            let _ = errors.read_to_string(&mut held);
            held
        });

        let asking = Asking::new();
        let mut answers: Vec<serde_json::Value> = Vec::new();
        for line in said.lines() {
            let line = line.expect("a line from the engine");
            let message: serde_json::Value =
                serde_json::from_str(&line).unwrap_or_else(|e| panic!("`{line}` is not JSON: {e}"));
            if let Some(q) = message.get("q") {
                let answer = self.serve(q, &asking);
                writeln!(ask, "{answer}").expect("the engine is listening");
                ask.flush().expect("the engine is listening");
            } else if message.get("call").is_some() {
                if let Some(atoms) = &self.stated {
                    atoms.rewind();
                }
            } else if let Some(out) = message.get("out") {
                answers = out.as_array().expect("a list of answers").clone();
            }
        }
        drop(ask);
        let status = child.wait().expect("the engine exits");
        let complaints = complaints.join().unwrap_or_default();
        assert!(
            status.success(),
            "the engine refused the module:\n{complaints}"
        );
        assert_eq!(
            answers.len(),
            tuples.len(),
            "the engine answered {} of {} calls:\n{complaints}",
            answers.len(),
            tuples.len()
        );
        answers.iter().map(|a| decode(a, sig.ret, heap)).collect()
    }

    /// One question, answered by [`beck_llvm::service`] — the host half the native backends
    /// already have, unchanged.
    ///
    /// That is the point of the frame's shapes: this function knows what `Upcall::arity` is and
    /// nothing else about the four primitives, and the encoding and decoding are `Heap`'s.
    fn serve(&self, q: &serde_json::Value, asking: &Asking) -> String {
        let atoms = self
            .stated
            .as_ref()
            .expect("only a program with effects asks anything");
        let code = q["op"].as_u64().expect("a question names itself") as u32;
        let op = Upcall::from_code(code).unwrap_or_else(|| panic!("`{code}` is not a question"));
        let used: u64 = q["used"]
            .as_str()
            .expect("the mark")
            .parse()
            .expect("a decimal");
        let arena = unbase64(q["arena"].as_str().unwrap_or(""));
        let args: Vec<(u32, u64)> = q["args"]
            .as_array()
            .expect("a shape and a word per argument")
            .iter()
            .take(op.arity())
            .map(|a| {
                (
                    a[0].as_u64().expect("a shape") as u32,
                    a[1].as_str()
                        .expect("a word")
                        .parse::<i64>()
                        .expect("an i64") as u64,
                )
            })
            .collect();
        let answered = beck_llvm::service::answer(
            &self.module.heap,
            atoms.as_ref(),
            asking,
            Question {
                op,
                span: q["span"].as_u64().unwrap_or(0) as u32,
                used,
                ret: q["ret"].as_u64().expect("the answer's shape") as u32,
                raises: q["raises"].as_u64().unwrap_or(0) as u32,
                args: &args,
                arena: &arena,
            },
        );
        serde_json::json!({
            "code": answered.code,
            "payload": answered.payload.to_string(),
            "value": answered.value.to_string(),
            "tail": base64(&answered.tail),
        })
        .to_string()
    }

    /// Assert the two agree on every tuple, and answer how many were compared.
    fn agree(&self, name: &str, tuples: &[Vec<Value>]) -> usize {
        assert!(
            self.compiled(name),
            "`{name}` did not compile to WebAssembly, so this compares the evaluator with itself"
        );
        let theirs = self.in_wasm(name, tuples);
        for (args, in_wasm) in tuples.iter().zip(&theirs) {
            let evaluated = self.evaluated(name, args);
            assert_eq!(
                &evaluated,
                in_wasm,
                "`{name}{}`: the evaluator and WebAssembly disagree",
                render(args)
            );
        }
        tuples.len()
    }

    fn refusals(&self) -> Vec<String> {
        self.module
            .refusals
            .iter()
            .map(|r| format!("{}: {}", r.name, r.reason))
            .collect()
    }

    /// One call on both, for a case that wants to look at the outcome rather than only compare it.
    fn call(&self, name: &str, args: &[Value]) -> (Outcome, Outcome) {
        let theirs = self.in_wasm(name, std::slice::from_ref(&args.to_vec()));
        (
            self.evaluated(name, args),
            theirs.into_iter().next().expect("one answer per call"),
        )
    }

    /// What the *evaluator* answers, for a case that needs a value only it can build.
    fn evaluated(&self, name: &str, args: &[Value]) -> Outcome {
        let def = &self.program.defs[name];
        if let Some(atoms) = &self.stated {
            atoms.rewind();
        }
        beck_eval::on_the_evaluator_stack(|| {
            let f = self.evaluator.function(&def.body).expect("prepares");
            outcome(f(args.to_vec()))
        })
    }
}

impl Drop for Both {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn kind(r: Repr) -> &'static str {
    match r.machine() {
        beck_llvm::Scalar::Int => "i",
        beck_llvm::Scalar::Float => "f",
        beck_llvm::Scalar::Bool => "b",
    }
}

/// One argument cell, as the JSON the driver turns into a WebAssembly value.
///
/// The cell is `Heap::encode_args`' — an `i64` for a number or an offset, the *bit pattern* of a
/// real, and `0`/`1` for a `Bool` — so nothing goes through a decimal printer on the way in.
fn cell(word: u64, r: Repr) -> serde_json::Value {
    match r.machine() {
        beck_llvm::Scalar::Float => serde_json::json!({ "k": "f", "v": word.to_string() }),
        beck_llvm::Scalar::Bool => serde_json::json!({ "k": "b", "v": word != 0 }),
        beck_llvm::Scalar::Int => serde_json::json!({ "k": "i", "v": (word as i64).to_string() }),
    }
}

fn decode(answer: &serde_json::Value, ret: Repr, heap: &Heap) -> Outcome {
    if let Some(crash) = answer.get("crash").and_then(|c| c.as_str()) {
        return Err(format!("the engine threw: {crash}"));
    }
    if let Some(code) = answer.get("trap").and_then(serde_json::Value::as_u64) {
        let payload: i64 = answer
            .get("payload")
            .and_then(|p| p.as_str())
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let trap = Trap::from_code(code as u32)
            .unwrap_or_else(|| panic!("`{code}` is not a trap either native backend stores"));
        // A raise is the one failure that is not a fault, so the message says *what* was raised —
        // decoded by `Heap::raised`, which is the same function the native host calls.
        if trap == Trap::Raised {
            let blob = unbase64(answer.get("heap").and_then(|h| h.as_str()).unwrap_or(""));
            return Err(heap
                .raised(payload as u64, &blob)
                .map_or_else(|why| why, |v| format!("raised `{}`", v.display())));
        }
        return Err(trap.message(payload));
    }
    let v = answer.get("v").expect("an answer carries a value");
    let word = match answer.get("k").and_then(|k| k.as_str()) {
        Some("i") => v
            .as_str()
            .expect("a decimal")
            .parse::<i64>()
            .expect("an i64") as u64,
        Some("b") => u64::from(v.as_bool().expect("a Bool")),
        Some("f") => v.as_str().expect("a bit pattern").parse().expect("bits"),
        other => panic!("`{other:?}` is not a kind this protocol has"),
    };
    let blob = unbase64(answer.get("heap").and_then(|h| h.as_str()).unwrap_or(""));
    // `Heap::decode` is the host half the two native backends already use, unchanged: what the
    // module left in its memory is what a worker would have sent down the pipe.
    heap.decode(word, ret, &blob).map_err(|e| e.to_string())
}

/// Base64, both ways, in twenty lines — the driver speaks it and `serde_json` does not.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn unbase64(text: &str) -> Vec<u8> {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = ABC.iter().position(|a| *a == c) else {
            continue;
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The differential
// ---------------------------------------------------------------------------------------------

#[test]
fn the_evaluator_and_webassembly_agree_on_integer_arithmetic() {
    let js = engine!();
    let both = Both::over("arith.beck", ARITHMETIC, js);
    let xs = ints(0x5EED, 26);
    let two = pairs(&xs);
    let one = singles(&xs);
    let mut n = 0;
    for f in ["plus", "minus", "times", "over", "modulo", "chained"] {
        n += both.agree(f, &two);
    }
    for f in ["compares", "orders", "logic"] {
        n += both.agree(f, &two);
    }
    for f in ["negated", "absolute"] {
        n += both.agree(f, &one);
    }
    println!("{n} calls agreed on integer arithmetic, in a WebAssembly engine");
}

#[test]
fn the_evaluator_and_webassembly_agree_on_reals() {
    let js = engine!();
    let both = Both::over("reals.beck", REALS, js);
    let xs = floats(0xF10A7, 22);
    let two = float_pairs(&xs);
    let one: Vec<Vec<Value>> = xs.iter().map(|x| vec![Value::float(*x)]).collect();
    let mut n = 0;
    for f in [
        "rplus",
        "rminus",
        "rtimes",
        "rover",
        "rless",
        "requal",
        "rorder",
        "reciprocal_of_product",
        "product_is_zero",
        "product_order",
        "zero_through_sqrt",
        "signed_zero",
    ] {
        n += both.agree(f, &two);
    }
    for f in ["rnegated", "rabs", "rsqrt", "truncated"] {
        n += both.agree(f, &one);
    }
    n += both.agree("widened", &singles(&ints(0xB17, 20)));
    println!("{n} calls agreed on reals, in a WebAssembly engine");
}

/// The three places a real is normalised, each with a program that makes the difference
/// observable.
///
/// The same three [`93`](../../../../docs/93-the-native-backends-report.md) §93.3 found the hard way, on a
/// third target that had to make the same decisions — and where the *obvious* instruction is wrong
/// twice over: `f64.lt` orders the two zeros and a NaN differently from the language, and
/// `i64.trunc_f64_s` traps where the language saturates.
#[test]
fn a_signed_zero_and_a_nan_are_the_languages_and_not_the_engines() {
    let js = engine!();
    let both = Both::over("reals.beck", REALS, js);
    let inf = f64::INFINITY;
    both.agree(
        "product_order",
        &[vec![Value::float(0.0), Value::float(inf)]],
    );
    both.agree(
        "product_is_zero",
        &[vec![Value::float(0.0), Value::float(-1.0)]],
    );
    both.agree(
        "reciprocal_of_product",
        &[vec![Value::float(0.0), Value::float(-1.0)]],
    );
    both.agree(
        "zero_through_sqrt",
        &[vec![Value::float(2.0), Value::float(-3.0)]],
    );
    both.agree(
        "signed_zero",
        &[vec![Value::float(0.0), Value::float(-1.0)]],
    );
}

#[test]
fn the_evaluator_and_webassembly_agree_on_control_flow() {
    let js = engine!();
    let both = Both::over("control.beck", CONTROL, js);
    let xs = ints(0xC0FFEE, 24);
    let one = singles(&xs);
    let two = pairs(&xs);
    let mut n = 0;
    for f in ["classify", "shadowing", "guard_falls_through"] {
        n += both.agree(f, &one);
    }
    n += both.agree("nested", &two);
    n += both.agree(
        "truthy",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    println!("{n} calls agreed on control flow, in a WebAssembly engine");
}

#[test]
fn the_evaluator_and_webassembly_agree_on_recursion() {
    let js = engine!();
    let both = Both::over("recursion.beck", RECURSION, js);
    let small: Vec<Vec<Value>> = (0..12).map(|n| vec![Value::Int(n)]).collect();
    let mut n = 0;
    n += both.agree("fib", &small);
    n += both.agree("even", &small);
    n += both.agree("odd", &small);
    n += both.agree(
        "gcd",
        &pairs(&[0, 1, 2, 12, 18, 270, 192, -12, i64::MIN + 1]),
    );
    let accumulating: Vec<Vec<Value>> = (0..12)
        .map(|n| vec![Value::Int(n), Value::Int(0)])
        .collect();
    n += both.agree("sum_to", &accumulating);
    n += both.agree("drain", &accumulating);
    n += both.agree("ackermann", &pairs(&[0, 1, 2]));
    println!("{n} calls agreed on recursion, in a WebAssembly engine");
}

/// A tail call is a **jump**, and the proof is a recursion deeper than any stack.
///
/// §93.4 makes this a guarantee rather than an optimisation, and WebAssembly spells it
/// `return_call`. A million frames is not a number chosen for drama: it is far past what an engine
/// gives a wasm stack, so this fails by throwing rather than by being slow if the emitter ever
/// stops emitting the tail form.
#[test]
fn a_tail_recursion_a_million_deep_does_not_grow_a_stack() {
    let js = engine!();
    let both = Both::over("recursion.beck", RECURSION, js);
    let deep = vec![vec![Value::Int(1_000_000), Value::Int(0)]];
    let answered = both.in_wasm("sum_to", &deep);
    assert_eq!(
        answered[0],
        Ok(Value::Int(500_000_500_000)),
        "a million tail calls should be a million jumps"
    );
    // …and the same through a tail call to a definition of a *different* arity, which is the case
    // a C calling convention cannot express and `return_call` can.
    let answered = both.in_wasm("drain", &deep);
    assert_eq!(answered[0], Ok(Value::Int(2_000_000)));
}

// ---------------------------------------------------------------------------------------------
// What it refuses, and the control beside it
// ---------------------------------------------------------------------------------------------

/// What the heap is, held to the tree-walker: records and unions, their comparisons, and the
/// layout rules two of these fixtures exist to catch.
///
/// The programs are [`support::heapfix`]'s, which `native.rs` and `cranelift.rs` already point at.
/// A variant's tag is its rank **by name** and a record's fields are compared in name order, so
/// `Ranked` is declared out of alphabetical order and `Key`'s fields are too: a backend that made
/// either one a declaration index answers those two definitions backwards and nothing else.
#[test]
fn the_evaluator_and_webassembly_agree_on_records_and_unions() {
    let js = engine!();
    let both = Both::over("records.beck", heapfix::RECORDS, js);
    let ps = heapfix::records();
    let mut n = 0;
    n += both.agree("origin", &[vec![]]);
    n += both.agree("make", &pairs(&ints(0x5eed_0011, 12)));
    n += both.agree("sum_of", &heapfix::singles(&ps));
    n += both.agree("swapped", &heapfix::singles(&ps));
    for name in ["same_point", "point_order"] {
        n += both.agree(name, &heapfix::pairs(&ps));
    }
    n += both.agree("key_order", &heapfix::pairs(&heapfix::keys()));
    for name in ["heavier", "same_weight"] {
        n += both.agree(name, &heapfix::pairs(&heapfix::weighted()));
    }
    for name in ["negated", "negated_is_zero"] {
        n += both.agree(name, &heapfix::singles(&heapfix::weighted()));
    }
    for name in ["span_of", "segment_order"] {
        n += both.agree(name, &heapfix::pairs(&ps));
    }
    let with_dx: Vec<Vec<Value>> = ps
        .iter()
        .flat_map(|p| [-1i64, 0, 1, i64::MAX].map(|d| vec![p.clone(), Value::Int(d)]))
        .collect();
    n += both.agree("moved", &with_dx);
    n += both.agree("scaled", &with_dx);
    // The set has to *contain* a failure, or this passed by never reaching one: a field expression
    // that overflows means the record is never built, and the message is the evaluator's.
    let (walked, compiled) = both.call("scaled", &[heapfix::point(i64::MAX, 0), Value::Int(2)]);
    assert_eq!(walked, compiled);
    assert!(walked.is_err(), "an overflow in a field has to fail");
    println!("{n} record calls agreed, in a WebAssembly engine");
}

/// A variant's tag is its rank **by name** and a record's fields are compared in name order, so
/// `Ranked` is declared out of alphabetical order and `Key`'s fields are too: a backend that made
/// either one a declaration index answers those two definitions backwards and nothing else.
#[test]
fn the_evaluator_and_webassembly_agree_on_unions() {
    let js = engine!();
    let both = Both::over("unions.beck", heapfix::UNIONS, js);
    let rs = heapfix::ranked();
    let ts = heapfix::trees();
    let mut n = 0;
    for name in ["rank", "guarded", "either", "whole", "n_or_zero"] {
        n += both.agree(name, &heapfix::singles(&rs));
    }
    for name in ["ranked_order", "same_ranked"] {
        n += both.agree(name, &heapfix::pairs(&rs));
    }
    n += both.agree("bigger", &singles(&ints(0x5eed_0012, 10)));
    for name in ["total", "left_leaf", "first_number"] {
        n += both.agree(name, &heapfix::singles(&ts));
    }
    n += both.agree("tree_order", &heapfix::pairs(&ts));
    n += both.agree("spine", &singles(&(0..12).collect::<Vec<_>>()));
    n += both.agree(
        "chain",
        &(0..12)
            .map(|k| vec![Value::Int(k), heapfix::leaf(0)])
            .collect::<Vec<_>>(),
    );
    n += both.agree("wrap", &singles(&ints(0x5eed_0013, 8)));
    n += both.agree(
        "unwrap",
        &heapfix::singles(&[heapfix::id(0), heapfix::id(7)]),
    );
    n += both.agree("maybe", &singles(&ints(0x5eed_0014, 8)));
    n += both.agree(
        "or_else",
        &heapfix::options()
            .iter()
            .map(|o| vec![o.clone(), Value::Int(-1)])
            .collect::<Vec<_>>(),
    );
    println!("{n} union calls agreed, in a WebAssembly engine");
}

/// Text, compared against the tree-walker over every string in [`textfix`] and every clamp.
///
/// The point of the sweep is the *pairs*: a three-way comparison can be right for `<` and wrong for
/// `<=`, and a byte comparison over the shorter length answers `0` for `"ab"` against `"abc"` — so
/// every operator is asked about every ordered pair rather than about a sample.
#[test]
fn the_evaluator_and_webassembly_agree_on_text() {
    let js = engine!();
    let both = Both::over("text.beck", textfix::TEXT, js);
    let ss = textfix::strings();
    let mut n = 0;
    for name in [
        "size", "empty", "first", "rest", "greeting", "is_yes", "echoed", "which", "tag",
    ] {
        n += both.agree(name, &textfix::singles(&ss));
    }
    for name in [
        "joined",
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
        "inside",
        "opens",
        "closes",
        "at",
    ] {
        n += both.agree(name, &textfix::pairs(&ss));
    }
    n += both.agree("thrice", &textfix::singles(&ss));
    n += both.agree("cut", &textfix::slices(&ss));
    n += both.agree("count_of", &textfix::with_char(&ss));
    n += both.agree(
        "at_or",
        &textfix::pairs(&ss)
            .into_iter()
            .map(|mut t| {
                t.push(Value::Int(-1));
                t
            })
            .collect::<Vec<_>>(),
    );
    n += both.agree("repeat", &textfix::repeats(&ss));
    n += both.agree("shown", &textfix::integers());
    n += both.agree(
        "shown_bool",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    n += both.agree("shown_str", &textfix::singles(&ss));
    n += both.agree(
        "repeated",
        &textfix::repeats(&ss)
            .into_iter()
            .map(|mut t| {
                t.truncate(2);
                t
            })
            .collect::<Vec<_>>(),
    );
    n += both.agree("glued", &textfix::joins(&ss));
    for name in ["or_else", "present"] {
        n += both.agree(
            name,
            &textfix::options()
                .into_iter()
                .map(|mut t| {
                    if name == "present" {
                        t.truncate(1);
                    }
                    t
                })
                .collect::<Vec<_>>(),
        );
    }
    n += both.agree(
        "sliced_or",
        &textfix::with_char(&ss)
            .into_iter()
            .map(|mut t| {
                t.remove(1);
                t
            })
            .collect::<Vec<_>>(),
    );

    // Text inside a record and inside a union, so a `Str` in a field is compared, rebuilt by
    // `with`, read back out and ordered against another record's.
    let named: Vec<Value> = ss
        .iter()
        .map(|s| heapfix::record("Named", &[("label", s.clone()), ("rank", Value::Int(1))]))
        .collect();
    n += both.agree("label_of", &textfix::singles(&named));
    for name in ["named_below", "named_same"] {
        n += both.agree(name, &textfix::pairs(&named));
    }
    n += both.agree(
        "relabel",
        &named
            .iter()
            .flat_map(|x| ss.iter().map(move |s| vec![x.clone(), s.clone()]))
            .collect::<Vec<_>>(),
    );
    let tagged: Vec<Value> = ss
        .iter()
        .map(|s| heapfix::variant("Tagged", "Word", &[("text", s.clone())]))
        .chain([heapfix::variant(
            "Tagged",
            "Number",
            &[("n", Value::Int(3))],
        )])
        .collect();
    n += both.agree("untag", &textfix::singles(&tagged));

    // The trim, over the strings written for it, and then every code point Rust calls whitespace —
    // the list is `char::is_whitespace` itself, so this asks about a new one the day Rust learns
    // about it.
    let sp = textfix::spaced();
    for name in ["trimmed", "trimmed_len", "blank"] {
        n += both.agree(name, &textfix::singles(&sp));
    }
    n += both.agree("trimmed_up", &textfix::repeats(&sp));
    let ws = textfix::every_whitespace();
    for name in ["trimmed", "trimmed_len", "blank"] {
        n += both.agree(name, &ws);
    }

    // Splitting, which answers with a **list** — so the differential reads the length *and* an
    // element, because a backend that counted the pieces correctly and allocated them wrongly
    // passes the first and fails the second.
    let cuts = textfix::separators(&ss);
    for name in ["parts", "split_len", "rejoined"] {
        n += both.agree(name, &cuts);
    }
    n += both.agree("split_at", &textfix::indexed(&cuts));
    for name in ["letters", "letter_count"] {
        n += both.agree(name, &textfix::singles(&ss));
    }
    n += both.agree("letter_at", &textfix::indexed(&textfix::singles(&ss)));
    println!("{n} text calls agreed, in a WebAssembly engine");
}

#[test]
fn the_evaluator_and_webassembly_agree_on_lists() {
    let js = engine!();
    let both = Both::over("lists.beck", listfix::LISTS, js);
    let xs = listfix::lists();
    let mut n = 0;
    for name in ["size", "empty", "flipped", "held"] {
        n += both.agree(name, &listfix::singles(&xs));
    }
    for name in [
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
    ] {
        n += both.agree(name, &listfix::pairs(&xs));
    }
    for name in ["nth", "nth_or"] {
        n += both.agree(name, &listfix::indexed(&xs));
    }
    for name in ["has", "at_of"] {
        n += both.agree(name, &listfix::searched(&xs));
    }
    n += both.agree("middle", &listfix::ranges(&xs));
    for name in ["front", "back"] {
        n += both.agree(name, &listfix::counted(&xs));
    }
    n += both.agree("three", &[vec![]]);
    n += both.agree("none_at_all", &[vec![]]);
    // Growing one: the operation, the fork onto a shared block, and the accumulator.
    for name in ["appended", "forked"] {
        n += both.agree(name, &listfix::searched(&xs));
    }
    for name in ["doubled_up", "sum_of"] {
        n += both.agree(name, &listfix::singles(&xs));
    }
    n += both.agree(
        "named",
        &listfix::texts()
            .iter()
            .flat_map(|v| {
                ["", "z", "aa"]
                    .iter()
                    .map(move |s| vec![v.clone(), Value::str_(s)])
            })
            .collect::<Vec<_>>(),
    );
    n += both.agree("doubled", &singles(&ints(0x5eed_0031, 12)));
    n += both.agree(
        "total",
        &xs.iter()
            .map(|v| vec![v.clone(), Value::Int(0), Value::Int(0)])
            .collect::<Vec<_>>(),
    );
    n += both.agree(
        "walked",
        &xs.iter()
            .map(|v| vec![v.clone(), Value::Int(0), Value::list(Vec::new())])
            .collect::<Vec<_>>(),
    );

    // An element that is itself an offset, one kind each.
    let ts = listfix::texts();
    for name in ["texts_below", "texts_same"] {
        n += both.agree(name, &listfix::pairs(&ts));
    }
    let ns = listfix::nested();
    for name in ["nested_below", "nested_same"] {
        n += both.agree(name, &listfix::pairs(&ns));
    }
    n += both.agree("nested_first", &listfix::singles(&ns));

    // A list inside a record and inside a union.
    let bags: Vec<Value> = xs
        .iter()
        .map(|v| heapfix::record("Bag", &[("items", v.clone()), ("rank", Value::Int(1))]))
        .collect();
    n += both.agree("bag_items", &listfix::singles(&bags));
    for name in ["bag_below", "bag_same"] {
        n += both.agree(name, &listfix::pairs(&bags));
    }
    n += both.agree(
        "rebagged",
        &bags
            .iter()
            .flat_map(|bag| xs.iter().map(move |v| vec![bag.clone(), v.clone()]))
            .collect::<Vec<_>>(),
    );
    n += both.agree(
        "bagged",
        &xs.iter()
            .map(|v| vec![v.clone(), Value::Int(3)])
            .collect::<Vec<_>>(),
    );
    let holdings: Vec<Value> = xs
        .iter()
        .map(|v| heapfix::variant("Holding", "Some_", &[("xs", v.clone())]))
        .chain([heapfix::variant("Holding", "None_", &[])])
        .collect();
    n += both.agree("held_size", &listfix::singles(&holdings));
    println!("{n} list calls agreed, in a WebAssembly engine");
}

/// Maps, whose search ends four ways — on the key, below every key, above every key, and
/// **between** two, which is the one a window that shrinks wrongly never leaves.
#[test]
fn the_evaluator_and_webassembly_agree_on_maps() {
    let js = engine!();
    let both = Both::over("maps.beck", mapfix::MAPS, js);
    let ms = mapfix::maps();
    let mut n = 0;
    for name in ["size", "names", "totals", "is_nothing", "held"] {
        n += both.agree(name, &mapfix::singles(&ms));
    }
    for name in [
        "below",
        "above",
        "same",
        "differ",
        "not_after",
        "not_before",
    ] {
        n += both.agree(name, &mapfix::pairs(&ms));
    }
    for name in ["lookup", "lookup_or", "holds"] {
        n += both.agree(name, &mapfix::keyed(&ms));
    }
    for name in ["put", "branched"] {
        n += both.agree(
            name,
            &mapfix::keyed(&ms)
                .iter()
                .map(|args| {
                    let mut args = args.clone();
                    args.push(Value::Int(7));
                    args
                })
                .collect::<Vec<_>>(),
        );
    }
    n += both.agree("dropped", &mapfix::keyed(&ms));
    n += both.agree("joined", &mapfix::pairs(&ms));
    for name in ["grown", "descending"] {
        n += both.agree(
            name,
            &[0i64, 1, 2, 3, 7, 16, 33]
                .iter()
                .map(|k| vec![Value::Int(*k)])
                .collect::<Vec<_>>(),
        );
    }
    n += both.agree("nothing", &[vec![]]);
    n += both.agree(
        "total",
        &ms.iter()
            .map(|m| vec![m.clone(), Value::Int(0), Value::Int(0)])
            .collect::<Vec<_>>(),
    );

    let ns = mapfix::nested();
    for name in ["nested_below", "nested_same"] {
        n += both.agree(name, &mapfix::pairs(&ns));
    }
    n += both.agree("nested_at", &mapfix::keyed(&ns));

    let cs: Vec<Value> = ms
        .iter()
        .map(|m| {
            heapfix::record(
                "Counts",
                &[("tally", m.clone()), ("label", Value::str_("x"))],
            )
        })
        .collect();
    n += both.agree("counts_tally", &mapfix::singles(&cs));
    n += both.agree("counts_below", &mapfix::pairs(&cs));
    n += both.agree(
        "recounted",
        &cs.iter()
            .flat_map(|c| ms.iter().map(move |m| vec![c.clone(), m.clone()]))
            .collect::<Vec<_>>(),
    );
    n += both.agree(
        "counted",
        &ms.iter()
            .map(|m| vec![m.clone(), Value::str_("k")])
            .collect::<Vec<_>>(),
    );
    let hs: Vec<Value> = ms
        .iter()
        .map(|m| heapfix::variant("Holding", "Held", &[("m", m.clone())]))
        .chain([heapfix::variant("Holding", "Empty", &[])])
        .collect();
    n += both.agree("held_size", &mapfix::singles(&hs));
    println!("{n} map calls agreed, in a WebAssembly engine");
}

/// Closures, applied through the family's table.
///
/// The one place this target is not a transcription of the native backends, so the differential
/// matters more here than anywhere else in this file: they switch on a closure's rank into a direct
/// call, and this does a `call_indirect` into a table whose gaps are `Trap::NoSuchLambda`.
#[test]
fn the_evaluator_and_webassembly_agree_on_closures() {
    let js = engine!();
    let both = Both::over("closures.beck", clofix::CLOSURES, js);
    let ns: Vec<i64> = vec![0, 1, -1, 2, 7, -7, i64::MAX, i64::MIN];
    let mut n = 0;
    for name in ["twice", "again", "through", "double"] {
        n += both.agree(name, &clofix::each_of(&ns));
    }
    for name in ["add_on", "nested"] {
        n += both.agree(name, &clofix::pairs_of(&ns));
    }
    n += both.agree("between", &clofix::triples_of(&ns));
    n += both.agree("either", &clofix::flagged(&ns));

    let xs = clofix::lists();
    let bys: Vec<i64> = vec![0, 1, -1, 3, i64::MAX];
    for name in ["doubled", "summed", "flags", "tally", "risky"] {
        n += both.agree(name, &clofix::singles(&xs));
    }
    for name in [
        "scaled",
        "kept",
        "biggest",
        "all_above",
        "any_above",
        "twice_over",
    ] {
        n += both.agree(name, &clofix::with(&xs, &bys));
    }
    let ts = clofix::texts();
    for name in ["lengths", "shouted", "long_ones", "joined"] {
        n += both.agree(name, &clofix::singles(&ts));
    }
    let rs = clofix::reals();
    for name in ["halved", "negated", "added"] {
        n += both.agree(name, &clofix::singles(&rs));
    }
    n += both.agree("spread", &clofix::singles(&xs));
    // `by_rank` is the stability case: every key in one of those lists is the same, so an unstable
    // sort is free to answer anything and a stable one answers the input order.
    for name in ["ascending", "descending", "by_sign"] {
        n += both.agree(name, &clofix::singles(&xs));
    }
    for name in ["by_length", "by_text"] {
        n += both.agree(name, &clofix::singles(&ts));
    }
    n += both.agree("by_real", &clofix::singles(&rs));
    n += both.agree("by_rank", &clofix::singles(&clofix::notes()));

    // Comparing two closures, which is `Closure`'s own order: the parameters, then where the body
    // starts — and *not* the captured frame, which `captures_ignored` is about.
    for name in ["same_lambda", "two_lambdas", "ordered"] {
        n += both.agree(name, &[vec![]]);
    }
    n += both.agree(
        "captures_ignored",
        &ns.iter()
            .flat_map(|a| ns.iter().map(move |b| vec![Value::Int(*a), Value::Int(*b)]))
            .collect::<Vec<_>>(),
    );
    println!("{n} closure calls agreed, in a WebAssembly engine");
}

/// Generic definitions, which is **monomorphisation** differentially — the pass is
/// [`beck_llvm::mono`]'s and this is a third caller of it.
#[test]
fn the_evaluator_and_webassembly_agree_on_generics() {
    let js = engine!();
    let both = Both::over("generic.beck", genfix::GENERIC, js);
    let mut n = 0;
    n += both.agree("of_ints", &genfix::ints());
    n += both.agree("of_bools", &genfix::bools());
    n += both.agree("of_texts", &genfix::texts());
    n += both.agree("of_records", &genfix::records());
    n += both.agree("of_unions", &genfix::unions());
    n += both.agree("of_lists", &genfix::lists());
    n += both.agree("of_lists_of_lists", &genfix::nested());
    n += both.agree("second_int", &genfix::ints());
    n += both.agree("second_text", &genfix::texts());
    n += both.agree("count_ints", &genfix::ints());
    n += both.agree("count_texts", &genfix::texts());
    n += both.agree("int_then_text", &genfix::int_and_text());
    n += both.agree("text_then_int", &genfix::text_and_int());
    n += both.agree("ints_agree", &genfix::int_pairs());
    n += both.agree("texts_agree", &genfix::text_pairs());
    n += both.agree("no_ints", &[vec![]]);
    n += both.agree("no_texts", &[vec![]]);
    n += both.agree("three_ints", &genfix::scalars());
    n += both.agree("three_texts", &genfix::singles());
    n += both.agree("bound", &genfix::scalars());
    // The control: a run that built `firstly` once and called it three times would answer
    // correctly and be wrong, so the instantiations are asserted by name.
    for wanted in [
        "firstly@Int",
        "firstly@Bool",
        "firstly@Str",
        "firstly@list[Int]",
        "paired@Int,Str",
    ] {
        assert!(
            both.compiled(wanted),
            "`{wanted}` is not among what compiled: {:?}",
            both.module
                .functions
                .iter()
                .map(|f| f.name.to_string())
                .collect::<Vec<_>>()
        );
    }
    println!("{n} generic calls agreed, in a WebAssembly engine");
}

/// A page, which is what Mode B is for — and the direction nothing in a program needs: a baked
/// tree back *in*, which the host has to write as a recipe whose leaves are text.
#[test]
fn the_evaluator_and_webassembly_agree_on_views() {
    let js = engine!();
    let both = Both::over("views.beck", viewfix::VIEWS, js);
    let cards = viewfix::cards();
    let lists = viewfix::lists();
    let mut n = 0;
    n += both.agree("just_text", &viewfix::singles(&textfix::strings()));
    n += both.agree("a_number", &singles(&[0, 1, -1, i64::MAX, i64::MIN]));
    n += both.agree(
        "a_flag",
        &[vec![Value::Bool(true)], vec![Value::Bool(false)]],
    );
    n += both.agree(
        "a_real",
        &[0.0, -0.0, 1.5, f64::INFINITY, f64::NAN]
            .iter()
            .map(|f| vec![Value::float(*f)])
            .collect::<Vec<_>>(),
    );
    n += both.agree("a_record", &viewfix::singles(&cards));
    n += both.agree("a_list", &viewfix::singles(&lists));
    for name in [
        "titled",
        "maybe_done",
        "ordered",
        "keyed",
        "keyed_number",
        "handled",
        "handled_nullary",
        "wrapped",
        "nested",
        "one_attr",
        "one_key",
        "one_handler",
        "panelled",
    ] {
        n += both.agree(name, &viewfix::singles(&cards));
    }
    n += both.agree("blank", &[vec![]]);
    for name in ["rows", "attrs_from"] {
        n += both.agree(name, &viewfix::singles(&lists));
    }
    n += both.agree("whole", &viewfix::with(&cards, &lists));

    let mut trees: Vec<Value> = Vec::new();
    for name in ["titled", "keyed", "handled", "nested"] {
        for c in &cards {
            trees.push(
                both.evaluated(name, std::slice::from_ref(c))
                    .expect("the evaluator builds it"),
            );
        }
    }
    trees.push(
        both.evaluated("just_text", &[Value::str_("hello")])
            .expect("the evaluator builds it"),
    );
    n += both.agree("again", &viewfix::singles(&trees));
    n += both.agree("beside", &viewfix::with(&trees, &cards));
    println!("{n} view calls agreed, in a WebAssembly engine");
    assert!(n >= 200, "only {n} calls compared");
}

/// The same page written with `ui:`, which is what a program actually contains.
#[test]
fn a_ui_block_compiles_and_agrees() {
    let js = engine!();
    let both = Both::over("page.beck", viewfix::PAGE, js);
    let lefts = [0i64, 1, 7];
    let tuples: Vec<Vec<Value>> = viewfix::todos()
        .iter()
        .flat_map(|ts| lefts.iter().map(move |k| vec![ts.clone(), Value::Int(*k)]))
        .collect();
    let n = both.agree("page", &tuples);
    println!("{n} `ui:` renders agreed, in a WebAssembly engine");
}

#[test]
fn the_evaluator_and_webassembly_agree_on_list_patterns() {
    let js = engine!();
    let both = Both::over("patterns.beck", listfix::PATTERNS, js);
    let xs = listfix::lists();
    let mut n = 0;
    for name in [
        "described",
        "tail",
        "after_two",
        "leading_one",
        "exactly_two",
    ] {
        n += both.agree(name, &listfix::singles(&xs));
    }
    n += both.agree("inner_first", &listfix::singles(&listfix::nested()));
    n += both.agree("joined", &listfix::singles(&listfix::texts()));
    // The set has to *contain* the boundaries, or this passed without reaching one.
    for (name, arg, want) in [
        ("described", vec![], "none"),
        ("described", vec![9], "one:9"),
        ("described", vec![1, 2, 3], "many:1:2"),
    ] {
        let list = Value::list(arg.into_iter().map(Value::Int).collect());
        let (walked, compiled) = both.call(name, &[list]);
        assert_eq!(walked, compiled);
        assert_eq!(walked.expect("answers"), Value::str_(want));
    }
    println!("{n} list-pattern calls agreed, in a WebAssembly engine");
}

/// Failure, which is `raise` and `try:` — the one control-flow shape a `block` is for here.
///
/// The native backends unwind through an error cell that was already an unwinder; here the trap
/// globals are that cell and a handler is a `block` a failure branches out of, so what the
/// differential is asking is whether a lexical handler written as structured control flow catches
/// exactly what a label-and-branch one catches.
#[test]
fn the_evaluator_and_webassembly_agree_on_failure() {
    let js = engine!();
    let both = Both::over("failure.beck", failfix::FAILURE, js);
    let ns = failfix::ints(&failfix::numbers());
    let mut n = 0;
    for name in [
        "checked",
        "uncaught",
        "caught",
        "described",
        "overflows",
        "wrong_type",
        "nested",
    ] {
        n += both.agree(name, &ns);
    }
    n += both.agree("named", &failfix::texts());
    n += both.agree("several", &failfix::lists());
    n += both.agree("all_checked", &failfix::lists());
    println!("{n} fallible calls agreed, in a WebAssembly engine");
    assert!(n >= 80, "only {n} calls compared");
}

/// The message a raise crosses the boundary with is the evaluator's, value and all.
///
/// Asserted directly as well as differentially, because the differential compares two backends
/// against each other and this says what the string *is*: a regression to "the compiled program
/// failed" would still agree with itself.
#[test]
fn an_uncaught_raise_names_the_value_it_carried() {
    let js = engine!();
    let both = Both::over("failure.beck", failfix::FAILURE, js);
    for (k, want) in [(101, "raised `TooBig{n: 101}`"), (0, "raised `Blank`")] {
        let (walked, compiled) = both.call("uncaught", &[Value::Int(k)]);
        assert_eq!(compiled, Err(want.to_string()), "`uncaught({k})`");
        assert_eq!(walked, compiled);
    }
    // The control: the same definition, on an argument that does not raise.
    let (walked, compiled) = both.call("uncaught", &[Value::Int(2)]);
    assert_eq!(compiled, Ok(Value::Int(5)));
    assert_eq!(walked, compiled);
}

/// The four questions a computation cannot answer, asked of a **stated** host.
///
/// The native backends write a question frame into the arena and block on a pipe; here the loader
/// is in the same tab, holds the memory, and can be called — so the frame's fields are the
/// import's arguments and the answer is its result. What is *not* different is who answers:
/// [`beck_llvm::service::answer`] services this suite's questions and the native worker's, so a
/// divergence here is the emitter's rather than two hosts disagreeing.
#[test]
fn the_evaluator_and_webassembly_agree_on_the_host_effects() {
    let js = engine!();
    let atoms = Stated::new();
    let both = Both::answering("effects.beck", hostfix::EFFECTS, js, atoms.clone());
    let mut n = 0;
    for (name, args) in hostfix::calls() {
        n += both.agree(name, std::slice::from_ref(&args));
    }
    // Both backends made every outbound call, rather than one of them being the evaluator twice:
    // five of the cases reach `http_fetch` once each, and the count is per backend.
    assert_eq!(
        atoms.asked(),
        10,
        "every `http_fetch` case has to have been asked by both backends"
    );
    // The set has to *contain* the failure, or this passed by never carrying a raise across the
    // boundary.
    let (walked, compiled) = both.call("unreachable", &[]);
    assert_eq!(walked, compiled);
    assert!(
        walked
            .expect_err("nowhere.invalid is unreachable")
            .contains("HttpUnreachable"),
        "an uncaught raise carries the value, not the fact of one"
    );
    println!("{n} host-effect calls agreed, in a WebAssembly engine");
}

/// A module that asks nothing declares nothing.
///
/// The property is what keeps the import a cost only the programs that need one pay — a browser
/// that had to supply a function for every module would be a browser that knows what a Beck
/// program is — and it is decided from the definitions the fixed point *kept*, so a module whose
/// only user of `now()` was refused for another reason still declares nothing.
#[test]
fn only_a_module_that_asks_declares_an_import() {
    let quiet = compile(
        "quiet.beck",
        "def twice(n: Int) -> Int:\n    return n + n\n",
    );
    assert!(!emit(&quiet).text.contains("(import"));
    let asking = compile("asking.beck", "def stamped() -> Int:\n    return now()\n");
    let module = emit(&asking);
    assert!(
        module.signature("stamped").is_some(),
        "{:?}",
        module.refusals
    );
    assert!(
        module.text.contains(r#"(import "beck" "upcall""#),
        "{}",
        module.text
    );
}

/// Running out of heap is a **message**, not a fault.
///
/// The memory grows under the bump pointer up to `heap::ARENA_BYTES`, which is the bound the two
/// native backends carry — so the program that exceeds it gets `Trap::HeapExhausted` and the
/// sentence that names the same number, rather than an aborted instance.
#[test]
fn running_out_of_heap_is_a_message_and_not_a_fault() {
    let js = engine!();
    let both = Both::over(
        "grow.beck",
        "def wide(n: Int, acc: list[Int]) -> Int:\n\
         \x20   if n == 0:\n\
         \x20       return list_len(acc)\n\
         \x20   return wide(n - 1, list_append(acc, n))\n",
        js,
    );
    assert!(both.compiled("wide"), "{:?}", both.refusals());
    // Small enough to answer, so the failure below is the arena and not the program.
    let small = both.in_wasm("wide", &[vec![Value::Int(1_000), Value::list(Vec::new())]]);
    assert_eq!(small[0], Ok(Value::Int(1_000)));
    let huge = both.in_wasm(
        "wide",
        &[vec![Value::Int(40_000_000), Value::list(Vec::new())]],
    );
    assert_eq!(
        huge[0],
        Err(Trap::HeapExhausted.message(0)),
        "an arena that runs out is a message with the same number the native backends give"
    );
}

/// `sin` and `cos` are refused, and the reason is the link line rather than effort.
///
/// The two native backends do not compile them either: nothing pins the digits of a sine, so both
/// **call** `beck-prim`, which computes one (`beck_prim::math`). A WebAssembly module reaches that
/// library only as an import the bundle does not carry, and emitting the algorithm here instead
/// would be a second implementation of the one thing whose whole value is that there is one.
/// `sqrt` is not on this list because IEEE-754 pins it to one correctly-rounded answer, which is
/// what makes the distinction a rule rather than a mood.
#[test]
fn the_transcendentals_are_refused_because_the_module_cannot_link_the_one_answer() {
    let js = engine!();
    let both = Both::over("reals.beck", REALS, js);
    for name in ["rsin", "rcos"] {
        let why = both
            .refusal(name)
            .unwrap_or_else(|| panic!("`{name}` should be refused"));
        assert!(why.contains("runtime library"), "{why}");
    }
    assert!(
        both.compiled("rsqrt"),
        "`sqrt` is IEEE-pinned and must not be refused with them"
    );
}

/// The module a browser would load is the module the engine loaded.
///
/// A listing that disagreed with the bytes would be the second account of an artefact
/// `docs/92` §92.2 exists to refuse, so the text is rendered from the same instruction list the
/// encoder walks — and this asserts the property that makes that worth having: the artefact is
/// readable, and it names the definitions it holds.
#[test]
fn the_artefact_is_readable_and_names_what_it_holds() {
    let js = engine!();
    let both = Both::over("arith.beck", ARITHMETIC, js);
    assert!(both.module.text.starts_with("(module"));
    for f in &both.module.functions {
        assert!(
            both.module.text.contains(&format!("(func ${}", f.name)),
            "the listing should name `{}`",
            f.name
        );
    }
    assert_eq!(&both.module.wasm[..4], b"\0asm");
}

/// Every module the tree produces is one a real engine accepts.
///
/// [`native.rs`]'s corpus walk **assembles** what it emits rather than only emitting it — "what is
/// being checked is that LLVM accepts the IR" — and this is that gate one target over. It is worth
/// having for the reason §106.7's last two defects are: a block with the wrong type and a double
/// `i32.wrap_i64` are refusals by the engine rather than wrong answers, so validation catches a
/// whole class of emitter defect over shapes no fixture has, and catches it on every program in
/// the tree rather than on the ones somebody wrote a case for.
///
/// It validates rather than instantiates, so a module that declares the host import needs no host.
#[test]
fn every_module_the_tree_produces_is_one_the_engine_accepts() {
    let js = engine!();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::temp_dir().join(format!("beck-wasm-validate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a working directory");
    let mut names = Vec::new();
    for source in ["corpus", "awfy", "sicp", "examples"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(root.join(source))
            .expect("the directory is there")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("beck"))
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("x.beck")
                .to_string();
            let src = std::fs::read_to_string(&path).expect("a program");
            let module = beck_diag::depth::on_the_front_end_stack(|| {
                let (placed, diags, _) = beck_core::compile_or_library_str(&name, &src);
                // A library that imports another module does not compile on its own, and this is
                // not the suite that checks that.
                placed
                    .filter(|_| !diags.has_errors())
                    .map(|p| beck_wasmgen::module(&p.program))
            });
            let Some(module) = module else { continue };
            let at = format!("{source}-{name}.wasm");
            std::fs::write(dir.join(&at), &module.wasm).expect("the module");
            names.push(serde_json::json!({ "at": at, "of": format!("{source}/{name}") }));
        }
    }
    assert!(names.len() > 40, "only {} programs emitted", names.len());
    let driver = "\
const fs = require('fs');\n\
const [, , dir, listed] = process.argv;\n\
const bad = [];\n\
for (const m of JSON.parse(fs.readFileSync(listed, 'utf8'))) {\n\
  try { new WebAssembly.Module(fs.readFileSync(dir + '/' + m.at)); }\n\
  catch (e) { bad.push(m.of + ': ' + e); }\n\
}\n\
process.stdout.write(JSON.stringify(bad));\n";
    std::fs::write(dir.join("validate.js"), driver).expect("the driver");
    let listed = dir.join("modules.json");
    std::fs::write(
        &listed,
        serde_json::to_string(&names).expect("the list encodes"),
    )
    .expect("the list");
    let out = Command::new(&js)
        .arg(dir.join("validate.js"))
        .arg(&dir)
        .arg(&listed)
        .output()
        .expect("the engine runs");
    assert!(
        out.status.success(),
        "the engine failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let refused: Vec<String> =
        serde_json::from_slice(&out.stdout).expect("the driver's answer is JSON");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        refused.is_empty(),
        "{} of {} modules were refused by the engine:\n  {}",
        refused.len(),
        names.len(),
        refused.join("\n  ")
    );
    println!("{} modules validated in a WebAssembly engine", names.len());
}

/// What the tree compiles to WebAssembly, printed rather than gated.
///
/// [`docs/93`](../../../../docs/93-the-native-backends-report.md) §93.6 keeps the same tally for
/// the native backends, and the reason it is printed is that the number is a *statement of where
/// the emitter is* rather than a threshold anybody chose.
///
/// It is also the honest measure of what this emitter buys Mode B today, which is **nothing**: the
/// corpus is applications, an application is records and lists and a page, and none of that is
/// scalar. The benchmarks are where scalar arithmetic lives, so the two directories are counted
/// separately rather than added — a single number over both would hide exactly the fact worth
/// reporting.
#[test]
fn what_the_tree_compiles_and_what_it_refuses() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut total = 0usize;
    // `clbg/` is not here: its programs import the standard library, so compiling one on its
    // own is a different thing from what the Benchmarks Game harness compiles, and a tally over
    // programs that do not compile would be a tally of nothing.
    for dir in ["corpus", "awfy"] {
        let (mut compiled, mut refused) = (0usize, 0usize);
        let mut reasons: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(root.join(dir)).expect("the directory is there") {
            let path = entry.expect("a directory entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("beck") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("a program");
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("x.beck");
            // Checking *and* emitting on the declared stack: the benchmarks are the largest
            // programs here, and a test thread's default stack is not the ground `beck` stands on.
            let module = beck_diag::depth::on_the_front_end_stack(|| {
                let (placed, diags, map) = beck_core::compile_or_library_str(name, &src);
                assert!(!diags.has_errors(), "{name}: {}", diags.render(&map));
                beck_wasmgen::module(&placed.expect("a program compiles").program)
            });
            compiled += module.functions.len();
            refused += module.refusals.len();
            for r in &module.refusals {
                assert!(
                    !r.reason.is_empty(),
                    "`{}` was refused with no reason",
                    r.name
                );
                // The first clause of the reason, which is the *class* rather than the instance.
                let class = r.reason.split(',').next().unwrap_or(&r.reason).to_string();
                *reasons.entry(class).or_default() += 1;
            }
        }
        total += compiled;
        println!("{dir}: {compiled} definitions compiled to WebAssembly, {refused} refused");
        for (reason, n) in &reasons {
            println!("  {n:>4}  {reason}");
        }
    }
    assert!(
        total > 0,
        "nothing in the tree compiled, so the refusals above are the only thing this measures"
    );
}
