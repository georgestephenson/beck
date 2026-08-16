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

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use beck_core::backend::Backend;
use beck_core::{Program, Value};
use beck_llvm::{Repr, Trap};

mod support;
use support::scalar::{
    float_pairs, floats, ints, pairs, render, singles, ARITHMETIC, CONTROL, REALS, RECURSION,
};

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
const DRIVER: &str = r#"
const fs = require('fs');
const [, , wasmPath, callsPath] = process.argv;
const inst = new WebAssembly.Instance(
  new WebAssembly.Module(fs.readFileSync(wasmPath)), {});
const e = inst.exports;
const view = new DataView(new ArrayBuffer(8));
const toF64 = (bits) => { view.setBigUint64(0, BigInt(bits)); return view.getFloat64(0); };
const fromF64 = (f) => { view.setFloat64(0, f); return view.getBigUint64(0).toString(); };
const calls = JSON.parse(fs.readFileSync(callsPath, 'utf8'));
const out = [];
for (const c of calls) {
  const args = c.args.map((a) =>
    a.k === 'i' ? BigInt(a.v) : a.k === 'f' ? toF64(a.v) : (a.v ? 1 : 0));
  e.beck_trap.value = 0;
  e.beck_trap_payload.value = 0n;
  let r;
  try {
    r = e[c.fn](...args);
  } catch (err) {
    out.push({ crash: String(err) });
    continue;
  }
  if (e.beck_trap.value !== 0) {
    out.push({ trap: e.beck_trap.value, payload: e.beck_trap_payload.value.toString() });
    continue;
  }
  out.push(
    c.ret === 'i' ? { k: 'i', v: r.toString() }
    : c.ret === 'f' ? { k: 'f', v: fromF64(r) }
    : { k: 'b', v: r !== 0 });
}
process.stdout.write(JSON.stringify(out));
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
}

impl Both {
    fn over(name: &str, src: &str, js: PathBuf) -> Both {
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
        Both {
            evaluator: beck_eval::backend_for(program.clone()),
            program,
            module,
            js,
            dir,
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
        let calls: Vec<serde_json::Value> = tuples
            .iter()
            .map(|args| {
                serde_json::json!({
                    "fn": name,
                    "ret": ret,
                    "args": args.iter().map(encode).collect::<Vec<_>>(),
                })
            })
            .collect();
        let calls_path = self.dir.join(format!("{name}.json"));
        std::fs::write(
            &calls_path,
            serde_json::to_string(&calls).expect("the calls encode"),
        )
        .expect("the calls file");

        let out = Command::new(&self.js)
            .arg(self.dir.join("driver.js"))
            .arg(self.dir.join("module.wasm"))
            .arg(&calls_path)
            .output()
            .expect("the engine runs");
        assert!(
            out.status.success(),
            "the engine refused the module:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let answers: Vec<serde_json::Value> =
            serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
                panic!(
                    "the driver's answer is not JSON ({e}): {}",
                    String::from_utf8_lossy(&out.stdout)
                )
            });
        answers.iter().map(decode).collect()
    }

    /// Assert the two agree on every tuple, and answer how many were compared.
    fn agree(&self, name: &str, tuples: &[Vec<Value>]) -> usize {
        assert!(
            self.compiled(name),
            "`{name}` did not compile to WebAssembly, so this compares the evaluator with itself"
        );
        let def = &self.program.defs[name];
        let theirs = self.in_wasm(name, tuples);
        for (args, in_wasm) in tuples.iter().zip(&theirs) {
            let evaluated = beck_eval::on_the_evaluator_stack(|| {
                let f = self.evaluator.function(&def.body).expect("prepares");
                outcome(f(args.to_vec()))
            });
            assert_eq!(
                &evaluated,
                in_wasm,
                "`{name}{}`: the evaluator and WebAssembly disagree",
                render(args)
            );
        }
        tuples.len()
    }
}

impl Drop for Both {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn kind(r: Repr) -> &'static str {
    match r {
        Repr::Int => "i",
        Repr::Float => "f",
        Repr::Bool => "b",
        other => unreachable!("{other:?} is not a scalar and cannot have compiled"),
    }
}

fn encode(v: &Value) -> serde_json::Value {
    match v {
        Value::Int(n) => serde_json::json!({ "k": "i", "v": n.to_string() }),
        Value::Bool(b) => serde_json::json!({ "k": "b", "v": b }),
        // The *bit pattern* of the canonicalised real, so nothing goes through a decimal printer
        // on the way to the engine.
        Value::Float(_) => serde_json::json!({
            "k": "f",
            "v": v.as_f64().expect("a real").to_bits().to_string(),
        }),
        other => panic!("{other:?} is not a scalar argument"),
    }
}

fn decode(answer: &serde_json::Value) -> Outcome {
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
        return Err(trap.message(payload));
    }
    let v = answer.get("v").expect("an answer carries a value");
    Ok(match answer.get("k").and_then(|k| k.as_str()) {
        Some("i") => Value::Int(v.as_str().expect("a decimal").parse().expect("an Int")),
        Some("b") => Value::Bool(v.as_bool().expect("a Bool")),
        // Through `Value::float`, which is what a host does on the way in: §93.2's "a real is
        // normalised on the way into a field", and the reason `signed_zero` is a case at all.
        Some("f") => Value::float(f64::from_bits(
            v.as_str().expect("a bit pattern").parse().expect("bits"),
        )),
        other => panic!("`{other:?}` is not a kind this protocol has"),
    })
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

/// The heap is refused **by name, with the reason** — which is the honest statement of where this
/// emitter is, and the sentence [`adr/0022`](../../../../docs/adr/0022-mode-b-ships-the-backend-it-has.md)
/// said would still be true: "the heap is still the whole of the remaining work".
#[test]
fn the_heap_is_refused_by_name_and_the_scalars_are_not() {
    let js = engine!();
    let both = Both::over("reals.beck", REALS, js);
    // The control first: a list of refusals with nothing on the other side of it would pass
    // against an emitter that refused everything.
    assert!(both.compiled("rplus"), "the scalar subset compiles");

    let heap = "\
def joins(a: Str, b: Str) -> Str:
    return a + b

def counts(xs: list[Int]) -> Int:
    return list_len(xs)
";
    let both = Both::over("heap.beck", heap, engine().expect("checked above"));
    for name in ["joins", "counts"] {
        let why = both
            .refusal(name)
            .unwrap_or_else(|| panic!("`{name}` should be refused"));
        assert!(
            why.contains("heap"),
            "a refusal says what it refused and why: {why}"
        );
    }
}

/// `sin` and `cos` are refused, and the reason is F9 rather than effort.
///
/// The two native backends compile them by calling the host's libm.
/// [`docs/35`](../../../../docs/35-standards-landscape.md) §35.5 item 1 is the open question — a
/// deterministic libm — and a WebAssembly engine's transcendentals are a *third* implementation
/// with no obligation to agree with either. `sqrt` is not on this list because IEEE-754 pins it to
/// one correctly-rounded answer, which is what makes the distinction a rule rather than a mood.
#[test]
fn the_transcendentals_are_refused_because_nothing_pins_their_digits() {
    let js = engine!();
    let both = Both::over("reals.beck", REALS, js);
    for name in ["rsin", "rcos"] {
        let why = both
            .refusal(name)
            .unwrap_or_else(|| panic!("`{name}` should be refused"));
        assert!(why.contains("F9"), "{why}");
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
