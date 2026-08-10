//! What the playground costs — release-only, and printed rather than asserted.
//!
//! ```text
//! cargo test --release --test measure_play -- --nocapture
//! ```
//!
//! Three questions, and they are separate because they are answered by different things:
//!
//! 1. **What does a visitor download?** Rung A "costs a CDN" (`docs/17` §17.1), and what it costs
//!    *them* is a module plus eight small files. The module is the whole compiler, so this is the
//!    honest price of the claim.
//! 2. **What does an answer cost?** An analysis is every section §17.1 names, derived from source.
//!    Measured at two program sizes, because one size cannot tell a cost that is a function of the
//!    program from one that is a constant.
//! 3. **What does an interaction cost, and does it grow with the log?** A tab holds its whole
//!    history, and the question a person will ask after ten minutes of clicking is whether it slows
//!    down. Measured at two log lengths, for the same reason.
//!
//! Measured natively rather than in WebAssembly, exactly as `docs/94` §94.14's kernel numbers were:
//! the crate is an `rlib` as well as a `cdylib`. The *ratios* and the shapes carry across; the
//! absolute microseconds do not, and a browser will be slower by some factor this does not
//! establish.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use beck_play::Tab;
use serde_json::json;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("the compiler directory")
}

fn source(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).expect("the example")
}

fn compiled(path: &str) -> beck_core::Placed {
    let src = source(path);
    let (placed, diags, map) = beck_core::compile_str(path, &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("an application")
}

/// Compressed size, in the codec a CDN would serve it in, or the best available.
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
        // The write happens on its own thread, and it has to: a compressor emits as it reads, so
        // writing megabytes into its stdin while nothing drains its stdout fills the output pipe,
        // the child blocks on the write it cannot finish, and this blocks on the write *it* cannot
        // finish. `gzip -9` on a 2.6 MB module deadlocks that way every time; `brotli -q 11` does
        // not, because it buffers a whole window before emitting anything — which is why this went
        // unnoticed on a machine that had brotli.
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin");
        let input = bytes.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let out = child.wait_with_output().expect("compresses");
        let _ = writer.join();
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
fn what_the_playground_costs_to_load() {
    println!("\n-- the download --");
    let mut page = 0usize;
    for asset in beck_play::serve::bundle() {
        page += asset.body.len();
        println!("  {:<22} {:>8}", asset.path, asset.body.len());
    }
    let (packed, codec) = compressed(
        &beck_play::serve::bundle()
            .iter()
            .flat_map(|a| a.body.bytes())
            .collect::<Vec<u8>>(),
    );
    println!(
        "  {:<22} {page:>8}  ({packed} {codec})",
        "the page, together"
    );

    // Relative to the workspace root rather than to this crate: `cargo test` runs the binary from
    // `crates/beck-cli`, and the default path is the one `cargo build` writes to at the root.
    let module = match std::env::var_os("BECK_PLAYGROUND") {
        Some(named) => PathBuf::from(named),
        None => root().join("target/wasm32-unknown-unknown/release/beck_play.wasm"),
    };
    match std::fs::read(&module) {
        Ok(bytes) => {
            let (packed, codec) = compressed(&bytes);
            println!(
                "  {:<22} {:>8}  ({packed} {codec})",
                "beck-play.wasm",
                bytes.len()
            );
            println!(
                "\n  the module is the whole front end, the evaluator and the infrastructure\n  \
                 derivation. It is the same download for every program, and a static host caches it."
            );
        }
        Err(_) => println!(
            "  beck-play.wasm         — not built. \
             `cargo build -p beck-play --release --target wasm32-unknown-unknown`"
        ),
    }
}

#[test]
fn what_an_answer_costs() {
    println!("\n-- rung A: one analysis, every section --");
    println!("  {:<32} {:>7}  {:>7}", "program", "lines", "µs");
    for path in [
        "crates/beck-play/examples/counter.beck",
        "examples/todo.beck",
        "corpus/25-thread.beck",
    ] {
        let src = source(path);
        // Warm: the first analysis of the process pays for the prelude's schemes.
        let _ = beck_play::analyse(&src);
        let started = Instant::now();
        const RUNS: u32 = 20;
        for _ in 0..RUNS {
            let out = beck_play::analyse(&src);
            assert_eq!(out.errors, 0);
        }
        let each = started.elapsed() / RUNS;
        println!(
            "  {:<32} {:>7}  {:>7}",
            path.rsplit('/').next().unwrap_or(path),
            src.lines().count(),
            each.as_micros()
        );
    }
    println!(
        "\n  a keystroke is debounced by 250 ms in the page, so this is the budget it has to fit in."
    );
}

#[test]
fn what_an_interaction_costs_and_whether_history_makes_it_worse() {
    println!("\n-- rung B: one command, at two log lengths --");
    println!(
        "  {:>8}  {:>10}  {:>14}  {:>16}",
        "events", "command µs", "scrub to head µs", "scrub per event ns"
    );
    for events in [100u64, 1000] {
        let mut tab = Tab::load(compiled("crates/beck-play/examples/counter.beck")).expect("loads");
        tab.hello("s1", "ana", None);
        for i in 0..events {
            tab.command("s1", &format!("k{i}"), &json!({"c": "Bump", "by": 0}));
        }
        assert_eq!(tab.head(), events);

        let started = Instant::now();
        const RUNS: u32 = 20;
        for i in 0..RUNS {
            tab.command("s1", &format!("m{i}"), &json!({"c": "Bump", "by": 1}));
        }
        let command = started.elapsed() / RUNS;

        // To *head*, which is the whole log — the position the scrubber is at when it is
        // released, and the most expensive one there is.
        let head = tab.head();
        let started = Instant::now();
        for _ in 0..5 {
            tab.page_at(head, "ana").expect("the scrubber");
        }
        let scrub = started.elapsed() / 5;

        println!(
            "  {:>8}  {:>10}  {:>14}  {:>16.0}",
            events,
            command.as_micros(),
            scrub.as_micros(),
            scrub.as_nanos() as f64 / tab.head() as f64
        );
    }
    println!(
        "\n  a command is a fold and a render of the *state*: this program's state is two integers, so\n  \
         what is measured here is the constant. A scrub is a fold *of* the log, so it grows with\n  \
         the history — linearly, which is what the last column is for. Both are what those two operations are, and the\n  \
         second is what makes the scrubber a replay rather than an undo stack (docs/98 §98.6)."
    );
}
