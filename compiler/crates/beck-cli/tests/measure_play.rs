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
//! Measured natively rather than in WebAssembly, exactly as `docs/94` §94.12's kernel numbers were:
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

    // Both modules, because a `@render(client)` program in the tab runs in Mode B's kernel and the
    // playground serves it beside the compiler (docs/103 §103.4). Relative to the workspace root
    // rather than to this crate: `cargo test` runs the binary from `crates/beck-cli`, and the
    // default paths are the ones `cargo build` writes to at the root.
    for (name, env, default, crate_name) in [
        (
            "beck-play.wasm",
            "BECK_PLAYGROUND",
            "target/wasm32-unknown-unknown/release/beck_play.wasm",
            "beck-play",
        ),
        (
            "beck-kernel.wasm",
            "BECK_KERNEL",
            "target/wasm32-unknown-unknown/release/beck_wasm.wasm",
            "beck-wasm",
        ),
    ] {
        let module = match std::env::var_os(env) {
            Some(named) => PathBuf::from(named),
            None => root().join(default),
        };
        match std::fs::read(&module) {
            Ok(bytes) => {
                let (packed, codec) = compressed(&bytes);
                println!("  {name:<22} {:>8}  ({packed} {codec})", bytes.len());
            }
            Err(_) => println!(
                "  {name:<22} — not built. \
                 `cargo build -p {crate_name} --release --target wasm32-unknown-unknown`"
            ),
        }
    }
    println!(
        "\n  the first module is the whole front end, the evaluator and the infrastructure\n  \
         derivation; the second is Mode B's kernel, which only a `@render(client)` program's\n  \
         iframe fetches. Both are the same download for every program, and a static host caches them."
    );
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
        tab.hello("s1", "ana", "/", None);
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

/// What the editor costs per keystroke, and what it does *not* cost.
///
/// Two operations with deliberately different prices, at two program sizes so the shape of each is
/// visible rather than asserted: highlighting is a lex and is what runs on **every** keystroke;
/// completion is a check and runs when somebody asks for one. If highlighting were as expensive as
/// completion, the page would have to debounce it, and a colour that arrives a quarter of a second
/// after the character is a colour that flickers.
#[test]
fn what_the_editor_costs_per_keystroke() {
    println!("\n-- the editor: two answers, two prices --");
    println!(
        "  {:<32} {:>7}  {:>12}  {:>14}  {:>8}",
        "program", "lines", "highlight µs", "completion µs", "tokens"
    );
    for path in [
        "crates/beck-play/examples/counter.beck",
        "examples/todo.beck",
        "corpus/25-thread.beck",
    ] {
        let src = source(path);
        let _ = beck_core::editor::Editor::of("playground.beck", &src);

        const RUNS: u32 = 50;
        let started = Instant::now();
        let mut tokens = 0;
        for _ in 0..RUNS {
            tokens = beck_core::editor::tokens(&src).len();
        }
        let highlight = started.elapsed() / RUNS;

        let caret = src.len() as u32;
        let started = Instant::now();
        const ASKS: u32 = 10;
        for _ in 0..ASKS {
            let editor = beck_core::editor::Editor::of("playground.beck", &src);
            let _ = editor.completions(caret);
        }
        let completion = started.elapsed() / ASKS;

        println!(
            "  {:<32} {:>7}  {:>12}  {:>14}  {:>8}",
            path.rsplit('/').next().unwrap_or(path),
            src.lines().count(),
            highlight.as_micros(),
            completion.as_micros(),
            tokens
        );
    }
    println!(
        "\n  highlighting is a lex and needs no program, which is why it is not debounced and why it\n  \
         still works while the file is broken. Completion is a check — the same one the analysis\n  \
         pays for — and is asked for rather than continuous (docs/103)."
    );
}

/// What a log costs to keep, and a link to carry.
///
/// The two artefacts this phase added that leave the tab: the records a page stores in IndexedDB,
/// and the fragment a share link is. Both are per-thing costs rather than per-program ones, so both
/// are measured against a growing log and a real program.
#[test]
fn what_a_kept_log_and_a_share_link_weigh() {
    println!("\n-- the log a page keeps --");
    println!(
        "  {:>8}  {:>12}  {:>16}  {:>16}  {:>12}",
        "events", "record bytes", "bytes per event", "restore µs", "of which load"
    );
    for events in [100u64, 1000] {
        let placed = compiled("crates/beck-play/examples/counter.beck");
        let mut tab = Tab::load(placed.clone()).expect("loads");
        tab.hello("s1", "ana", "/", None);
        // `by: 0`, because this program refuses a count over 100 and what is being measured is
        // the record rather than the arithmetic.
        for i in 0..events {
            tab.command("s1", &format!("k{i}"), &json!({"c": "Bump", "by": 0}));
        }
        let records = tab.records(0).expect("the records");
        assert_eq!(records.len() as u64, events);
        let bytes: usize = records.iter().map(|r| r.len()).sum();

        const RUNS: u32 = 5;
        let started = Instant::now();
        for _ in 0..RUNS {
            let mut back = Tab::load(placed.clone()).expect("loads");
            assert_eq!(back.restore(&records).expect("restores"), events);
        }
        let restore = started.elapsed() / RUNS;

        // Preparing the program is in that number and is not part of the fold, so it is measured
        // rather than left to be assumed away: without it the two rows would look sublinear in a
        // way a fold cannot be.
        let started = Instant::now();
        for _ in 0..RUNS {
            let _ = Tab::load(placed.clone()).expect("loads");
        }
        let load = started.elapsed() / RUNS;

        println!(
            "  {:>8}  {:>12}  {:>16.1}  {:>16}  {:>12}",
            events,
            bytes,
            bytes as f64 / events as f64,
            restore.as_micros(),
            load.as_micros()
        );
    }
    println!(
        "\n  a record is what a durable store writes (`beck_host::Envelope::encode`), so this is the\n  \
         same encoding Postgres and redb hold — and a restore is preparing the program once and then\n  \
         folding the whole log, which is the same shape as a scrub and grows the same way."
    );

    println!("\n-- a share link --");
    println!(
        "  {:<32} {:>8}  {:>12}  {:>8}",
        "program", "source", "link chars", "ratio"
    );
    for path in [
        "crates/beck-play/examples/counter.beck",
        "examples/todo.beck",
        "examples/board.beck",
    ] {
        let src = source(path);
        let fragment = beck_play::share::pack(&src);
        println!(
            "  {:<32} {:>8}  {:>12}  {:>7.2}×",
            path.rsplit('/').next().unwrap_or(path),
            src.len(),
            fragment.len(),
            fragment.len() as f64 / src.len() as f64
        );
    }
    println!(
        "\n  a link carries the program, so it is proportional to it: §17.4's short, resolvable link\n  \
         needs a registry, and this is the half that works with no server at all (docs/103)."
    );
}
