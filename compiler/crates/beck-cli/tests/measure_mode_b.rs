//! What Mode B costs — the numbers `docs/94` §94.6 quotes.
//!
//! Release-only by the convention every measurement suite here follows:
//!
//! ```text
//! cargo test --release --test measure_mode_b -- --nocapture
//! ```
//!
//! Three questions, and they are separate on purpose:
//!
//! 1. **What does a component cost?** The bundle — the per-component payload, the thing §5.1's
//!    "< 150 KB brotli per component bundle" budget is about.
//! 2. **What does the kernel cost?** Once per application, shared by every component of it, and
//!    unchanged by which program is running. Reported apart from the bundle because a budget that
//!    adds them together answers neither question.
//! 3. **What does an event cost on the wire?** Mode A sends the difference between two pages,
//!    Mode B the difference between two states. Both are measured over the same event on the same
//!    program, at two sizes — because one size cannot tell "the change" from "the collection".
//!
//! `brotli` is what §5.1's budget is denominated in and it is not a Rust dependency here: the
//! command is shelled out to, and a machine without it gets gzip and a line saying so.

use beck_core::{Bundle, Placed, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod support;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

fn example(name: &str) -> Placed {
    let path = root().join("examples").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("examples/{name}"));
    let (placed, diags, map) = beck_core::compile_str(path.to_str().expect("utf-8"), &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// Compressed size, in the codec §5.1's budget is written in, or the best available.
fn compressed(bytes: &[u8]) -> (usize, &'static str) {
    for (program, args) in [("brotli", ["-q", "11", "-c"]), ("gzip", ["-9", "-c", ""])] {
        let mut child = match Command::new(program)
            .args(args.iter().filter(|a| !a.is_empty()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => continue,
        };
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(bytes)
            .expect("writes");
        let out = child.wait_with_output().expect("compresses");
        if out.status.success() {
            return (
                out.stdout.len(),
                if program == "brotli" {
                    "brotli"
                } else {
                    "gzip"
                },
            );
        }
    }
    (
        bytes.len(),
        "uncompressed — no brotli or gzip on this machine",
    )
}

#[test]
fn what_a_component_costs() {
    let placed = example("board.beck");
    let bundle = Bundle::of(&placed);
    let bytes = bundle.to_bytes();
    let (packed, codec) = compressed(&bytes);

    println!("\n-- the bundle: one component's slice --");
    println!("  component        {}", bundle.component);
    println!("  definitions      {}", bundle.defs.len());
    println!("  Core nodes       {}", bundle.nodes());
    println!("  bytes            {}", bytes.len());
    println!("  {codec:<16} {packed}");
    println!(
        "  §5.1's budget is 150 KB brotli: this is {:.1}% of it",
        packed as f64 / (150.0 * 1024.0) * 100.0
    );
}

#[test]
fn what_the_kernel_costs() {
    let module = root().join("target/wasm32-unknown-unknown/release/beck_wasm.wasm");
    let Ok(bytes) = std::fs::read(&module) else {
        println!(
            "\nskipped: no kernel at {}. Build it with \
             `cargo build -p beck-wasm --release --target wasm32-unknown-unknown`.",
            module.display()
        );
        return;
    };
    let (packed, codec) = compressed(&bytes);
    println!("\n-- the kernel: one per application, whatever the program --");
    println!("  bytes            {}", bytes.len());
    println!("  {codec:<16} {packed}");
    println!(
        "  §5.1's budget is 150 KB brotli: this is {:.1}% of it",
        packed as f64 / (150.0 * 1024.0) * 100.0
    );
    println!("  (no `wasm-opt -Oz`, which §5.1's release path calls for and this does not run)");
}

/// What one event puts on the wire, in each mode, at two sizes.
///
/// Two sizes because one cannot tell a cost that is a function of the change from a cost that is a
/// function of the collection — which is the whole claim being made about the data patch.
#[test]
fn what_an_event_costs_on_the_wire() {
    println!("\n-- one event on the wire, both modes --");
    println!(
        "  {:>7}  {:>14}  {:>14}",
        "cards", "Mode A (bytes)", "Mode B (bytes)"
    );
    for n in [100usize, 1000] {
        let placed = example("board.beck");
        let rt =
            beck_rt::Runtime::new(placed.clone(), beck_eval::backend(&placed)).expect("prepares");

        let mut state = rt.initial_state().expect("init");
        for i in 0..n {
            state = fold_one(
                &rt,
                &state,
                serde_json::json!({"c":"Add","id":format!("c{i:05}"),"text":format!("card {i}")}),
                i as u64 + 1,
            );
        }
        // The event under measurement: one card moved, on a board of `n`.
        let after = fold_one(
            &rt,
            &state,
            serde_json::json!({"c":"Move","id":format!("c{:05}", n / 2),"column":1}),
            n as u64 + 1,
        );

        let dom = beck_core::diff::diff(
            &rt.view(&state, "ana").expect("renders"),
            &rt.view(&after, "ana").expect("renders"),
        );
        let mode_a = serde_json::to_vec(&beck_rt::PatchFrame::new(1, dom).to_json())
            .expect("encodes")
            .len();
        let mode_b = serde_json::to_vec(
            &beck_rt::patch::DataFrame::Ops {
                seq: 1,
                ops: beck_core::delta::diff(&state, &after),
            }
            .to_json(),
        )
        .expect("encodes")
        .len();
        println!("  {n:>7}  {mode_a:>14}  {mode_b:>14}");
        if std::env::var("BECK_MODE_B_OPS").is_ok() {
            let dom2 = beck_core::diff::diff(
                &rt.view(&state, "ana").expect("renders"),
                &rt.view(&after, "ana").expect("renders"),
            );
            for op in dom2.iter().take(6) {
                println!(
                    "    op: {}",
                    &format!("{op:?}")[..120.min(format!("{op:?}").len())]
                );
            }
            println!("    ops: {}", dom2.len());
        }
    }
    println!("  A cost that does not grow with the board is a cost that is a function of the");
    println!("  change. A cost that does is a function of the collection.");
}

fn fold_one(rt: &beck_rt::Runtime, state: &Value, command: serde_json::Value, seq: u64) -> Value {
    let command = rt.decode_command(&command).expect("decodes");
    let proposal = rt.proposal("ana", command);
    let Ok(events) = rt.validate(state, &proposal) else {
        return state.clone();
    };
    let mut out = state.clone();
    for event in events {
        let env = beck_rt::Envelope {
            seq,
            at: beck_rt::Instant(1_700_000_000_000),
            actor: "ana".into(),
            body: event.clone(),
        };
        out = rt.fold(&out, &env, event).expect("folds");
    }
    out
}
