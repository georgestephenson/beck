//! The macro sandbox: refused programs, and the enumeration that keeps the refusal honest.
//!
//! [`docs/02`](../../../../docs/02-syntax.md) §2.4 calls phase separation **non-negotiable**:
//! macro bodies run "with a *capability-restricted* environment: pure computation and reads of the
//! declared module graph, no ambient filesystem or network… build reproducibility and the 'compile
//! once, deploy many' model depend on it, and it closes a real supply-chain hole that Rust
//! `build.rs` and npm `postinstall` leave open."
//!
//! Until the interpreter existed that property was satisfied *by construction* — the expander was
//! a pure `Node -> Node` function over a template, so there was no name a macro body could use and
//! nothing to check. `security.rs` says so in those words, and
//! [`docs/12`](../../../../docs/12-standards-and-conformance.md) §12.7 is where this file was
//! demanded: compile-time evaluation is what turns the sandbox from a property nothing could
//! violate into a claim needing a gate. It is the G-class companion
//! [`docs/08`](../../../../docs/08-roadmap.md) §8.5.4 says lands *with* the interpreter rather than
//! after it.
//!
//! # The two halves
//!
//! A refusal per shape, and — because a refusal per shape is a list of the things somebody thought
//! of — an **enumeration**: every effectful primitive in the prelude, in a macro body, refused.
//! That is the half that goes red when a future primitive is added and the interpreter is not told
//! about it ([`docs/93`](../../../../docs/93-the-native-backends-report.md) §93.9: a refusal is a
//! claim and nothing was checking it).

use beck_core::prelude;
use beck_core::ty::{Effect, Ty};

/// The diagnostics a macro body raises, as codes.
///
/// The body is the *only* thing that varies: one parameter, one statement, one `return quote:`,
/// so a code in the output is about the statement and nothing else.
fn body_codes(stmt: &str) -> Vec<String> {
    let src = format!(
        "macro tries(x):\n    {stmt}\n    return quote:\n        $x\n\ndef f() -> Int:\n    return tries(1)\n"
    );
    let (_, diags, _) = beck_core::compile_or_library_str("sandbox.beck", &src);
    diags.iter().map(|d| d.code.to_string()).collect()
}

// ---------------------------------------------------------------------------------------------
// The shapes
// ---------------------------------------------------------------------------------------------

/// Reading the clock at compile time is refused, and told why.
///
/// The message matters here more than usual: `now()` is a name that *exists* in the language, so
/// "cannot find `now`" would be a lie about the reason. `B0207` names the atom.
#[test]
fn a_macro_body_cannot_read_the_clock() {
    let all = body_codes("y = now()");
    assert!(
        all.iter().any(|c| c == "B0207"),
        "reading the clock at compile time must be refused as a capability: {all:?}"
    );
}

/// Nor the environment, which is where a secret would come from.
#[test]
fn a_macro_body_cannot_read_the_environment() {
    let all = body_codes("y = secret_env(\"TOKEN\")");
    assert!(all.iter().any(|c| c == "B0207"), "{all:?}");
}

/// Nor the network — the supply-chain hole §2.4 names, in the one sentence that names `build.rs`.
#[test]
fn a_macro_body_cannot_reach_the_network() {
    let all = body_codes("y = http_fetch(\"https://example.com\", 1)");
    assert!(
        all.iter().any(|c| c == "B0207"),
        "an outbound call at compile time must be refused as a capability: {all:?}"
    );
    // And the diagnostic is about `http_fetch` rather than about its second argument: a callee is
    // checked before its arguments are evaluated, so the reason reported is the real one.
    assert!(
        !all.iter().any(|c| c == "B0208"),
        "the refusal should name the call, not a bad argument to it: {all:?}"
    );
}

/// A name for the host is not a name the environment has.
///
/// The other half of the sandbox, and the one that scales: the environment is a **whitelist**, so
/// a name nobody put on it resolves to nothing. `RESTRICTED` buys a better message for the names
/// the language does have; this is what happens to every name it does not.
#[test]
fn a_macro_body_has_no_name_for_the_host_at_all() {
    for reach in [
        "y = read_file(\"/etc/passwd\")",
        "y = write_file(\"/tmp/x\", \"\")",
        "y = getenv(\"HOME\")",
        "y = spawn(\"sh\")",
        "y = open(\"/dev/urandom\")",
    ] {
        let all = body_codes(reach);
        assert!(
            all.iter().any(|c| c == "B0208"),
            "`{reach}` should find nothing in the compile-time environment: {all:?}"
        );
    }
}

/// The forms that belong to the program are refused as forms, not as missing names.
#[test]
fn the_programs_own_forms_are_not_the_expanders() {
    for (stmt, what) in [
        ("y = raise 1", "raise"),
        ("match x:\n        case _:\n            y = 1", "match"),
        ("y = ui:\n        p: \"hello\"", "ui"),
    ] {
        let all = body_codes(stmt);
        assert!(
            all.iter().any(|c| c == "B0205"),
            "`{what}` in a macro body should be refused as a form: {all:?}"
        );
    }
}

/// A macro body is refused when it computes nonsense, rather than expanding into nonsense.
///
/// The interpreter runs before the checker, so there is no type to have refused this: the answer
/// has to come from evaluation, and it has to come with a span in the macro body rather than in
/// the program the macro produced.
#[test]
fn a_compile_time_type_error_is_reported_in_the_macro_body() {
    let all = body_codes("y = 1 + \"one\"");
    assert!(all.iter().any(|c| c == "B0209"), "{all:?}");

    let all = body_codes("y = [1, 2][9]");
    assert!(
        all.iter().any(|c| c == "B0209"),
        "an index past the end is refused rather than answered: {all:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The enumeration
// ---------------------------------------------------------------------------------------------

/// Every effectful primitive in the prelude is named in `beck_macro::RESTRICTED`.
///
/// The drift gate. `beck-macro` sits *below* `beck-core` and cannot read the prelude, so the list
/// it carries is a copy — and a copy of a table is a thing that goes out of date silently. This is
/// what makes it fail loudly instead: add a primitive that performs an atom, and the list has to
/// learn about it in the same change.
///
/// `Raises` is not an atom for this purpose. A primitive that can fail — `json_parse`,
/// `time_parse` — is pure computation with a failure case, and refusing it at compile time would
/// be refusing arithmetic because it can divide by zero.
#[test]
fn every_effectful_primitive_is_named_in_the_restricted_list() {
    let restricted: Vec<&str> = beck_macro::RESTRICTED.iter().map(|(n, _)| *n).collect();
    let mut missing: Vec<String> = Vec::new();
    for (name, _, scheme) in prelude::prims() {
        let Ty::Fun(_, _, row) = &scheme.ty else {
            continue;
        };
        let performs: Vec<&Effect> = row
            .atoms
            .iter()
            .filter(|a| !matches!(a, Effect::Raises(_)))
            .collect();
        if performs.is_empty() || restricted.contains(&name) {
            continue;
        }
        missing.push(format!("{name} performs {performs:?}"));
    }
    assert!(
        missing.is_empty(),
        "these primitives perform an effect and are not in `beck_macro::RESTRICTED`, so a macro \
         body calling one would be told the name does not exist rather than that it may not be \
         called: {missing:#?}"
    );
}

/// …and no primitive that performs an effect is a compile-time builtin.
///
/// The list above buys a message; this is the control that says the message is not the only thing
/// standing between a macro and the host. It reads `BUILTINS` — the whitelist itself — rather than
/// trying calls, because the property is about the *environment* and not about one call site.
#[test]
fn no_effectful_primitive_is_a_compile_time_builtin() {
    for (name, _, scheme) in prelude::prims() {
        let Ty::Fun(_, _, row) = &scheme.ty else {
            continue;
        };
        if row.atoms.iter().all(|a| matches!(a, Effect::Raises(_))) {
            continue;
        }
        assert!(
            !beck_macro::BUILTINS.contains(&name),
            "`{name}` performs an effect and is on the compile-time whitelist"
        );
    }
}

/// Every name in `RESTRICTED` is refused when a macro body calls it.
///
/// The list is a claim about behaviour, so the behaviour is what is asserted — one compile per
/// name, each one a macro body whose only statement is that call.
#[test]
fn every_restricted_name_is_refused_when_a_macro_body_calls_it() {
    for (name, atom) in beck_macro::RESTRICTED {
        let all = body_codes(&format!("y = {name}()"));
        assert!(
            all.iter().any(|c| c == "B0207"),
            "`{name}` performs `{atom}` and a macro body called it: {all:?}"
        );
    }
}

/// The whole tree still expands, which is the other direction every gate here needs.
///
/// A sandbox that refused everything would pass all of the above. `macro_interp.rs` runs the
/// macro-heavy programs; this runs the corpus, whose 32 programs are the ones with no annotations
/// and therefore the ones nobody tuned to get through a check.
#[test]
fn the_corpus_still_compiles_with_the_interpreter_in_the_path() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("the corpus is there") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("beck") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("a corpus program");
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("x.beck");
        let (_, diags, map) = beck_core::compile_or_library_str(name, &src);
        assert!(!diags.has_errors(), "{name}: {}", diags.render(&map));
        seen += 1;
    }
    assert!(seen >= 30, "only {seen} corpus programs were read");
}
