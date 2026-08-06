//! The security controls this project has **not** built, asserted as absent.
//!
//! [`docs/42-security-assurance.md`](../../../../docs/42-security-assurance.md) §42.6 lists four
//! findings whose status in [`docs/14`](../../../../docs/14-review-findings.md) is a word and whose
//! status in the code is silence: the actor is whatever the client says it is (identity), there is
//! no per-actor quota (F3), no subscription or connection quota (F15), no bounded deploy buffer
//! (F12) and no macro fuel (F17). §42.11 asks for exactly this file — "`sicp/refusals/`'s pattern
//! applied to security debt" — and the gate is the same one that directory had: **the day somebody
//! builds one of these, its test goes red**, and the person who built it has to come here and to
//! the documents and say so.
//!
//! Three rules, because a suite of this shape is easy to get wrong:
//!
//! * **Each test asserts the gap, and its failure message says what to do.** A red test here is
//!   good news; it must not read like a regression.
//! * **It asserts behaviour where behaviour is observable, and the source where it is not.** "No
//!   quota exists" is not a request anybody can make; it is the absence of a mechanism, and the
//!   honest way to test for an absence is to look.
//! * **Nothing here is a proposal.** What each control should *be* is in `docs/14` and `docs/42`;
//!   this file only records that it is not there yet.

use std::path::{Path, PathBuf};

mod support;

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under crates/")
        .to_path_buf()
}

/// Every `.rs` file in the workspace except this one, which names all of these words in prose.
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&crates_dir(), &mut out);
    out.retain(|p| !p.ends_with("tests/pending_security.rs"));
    out.sort();
    assert!(out.len() > 20, "the source listing is wrong, not the repo");
    out
}

fn mentions(needles: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for path in sources() {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if needles.iter().any(|w| code.contains(w)) {
                out.push(format!("{}:{}", path.display(), n + 1));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Identity — Phase 3's own bullet, and the one that makes every ownership check conditional
// ---------------------------------------------------------------------------------------------

/// The **default** is still to believe the client, and D6's OIDC relying party is still unbuilt.
///
/// This is narrower than it was. [`docs/48`](../../../../docs/48-identity-report.md) made identity
/// a seam with a verifying implementation, so "the actor is whatever the client says" is now a
/// property of `DevIdentity` — a choice an operator makes — rather than of the protocol. What
/// remains absent, and what this asserts, is that the default is that choice, and that nothing
/// here speaks OIDC.
#[test]
fn identity_defaults_to_believing_the_client() {
    let config = beck_rt::AppConfig::default();
    assert!(
        !config.identity.verifies(),
        "the default provider verifies something now — say so in docs/43 §43.4 and §42.1's table, \
         and rewrite this test to assert whatever the new default is"
    );
    assert_eq!(config.identity.kind(), "dev");
}

/// No OIDC relying party: no JWKS, no issuer or audience validation, no asymmetric signature.
///
/// `SignedIdentity` is symmetric — everything that can verify a credential can also mint one —
/// which suits a gateway in front of a Beck process and does not suit a public identity provider.
/// D6 asks for the second.
#[test]
fn nothing_here_speaks_oidc() {
    let sites = mentions(&["jwks", "Jwks", "id_token", "issuer", "RS256"]);
    assert!(
        sites.is_empty(),
        "an OIDC relying party appears to exist now ({sites:?}) — delete this test, and correct \
         docs/48 §48.5, docs/43 §43.4 and the roadmap's identity bullet"
    );
}

/// And the claims a verified identity carries do not reach the program yet.
///
/// D6 asks for "claims → `Session` capability mapping". The verification half is built; the
/// mapping half is not, because the actor travels through the view path as a `String`
/// ([`48`](../../../../docs/48-identity-report.md) §48.5).
#[test]
fn a_verified_identitys_claims_do_not_reach_the_program() {
    let session = beck_core::prelude::types()
        .get("Session")
        .cloned()
        .expect("the prelude declares a Session");
    let fields = match session {
        beck_core::ty::TyDecl::Model { fields, .. } => fields,
        _ => panic!("Session is a model"),
    };
    assert!(
        !fields.iter().any(|(n, _)| n.as_ref() == "claims"),
        "`Session` carries claims now — delete this test and correct docs/48 §48.5"
    );
}

// ---------------------------------------------------------------------------------------------
// The outbound call — built, and plaintext
// ---------------------------------------------------------------------------------------------

/// An outbound request is not encrypted, and nothing in the tree pretends otherwise.
///
/// `http_fetch` speaks HTTP/1.1 over TCP. [`docs/07`](../../../../docs/07-dependencies.md) chooses
/// rustls for the other half, and taking a TLS stack is a dependency decision rather than a line
/// in `beck-rt/src/outbound.rs` — so until it is taken, a credential sent with
/// `with_secret_header` is confidential exactly as far as the network under it is. Whoever adds
/// TLS deletes this test and corrects docs/43 §43.4 and docs/49 §49.6.
#[test]
fn an_outbound_call_has_no_transport_security() {
    let sites = mentions(&["rustls", "TlsConnector", "tokio_rustls", "webpki"]);
    assert!(
        sites.is_empty(),
        "a TLS stack appears to exist now ({sites:?}) — delete this test, and correct docs/43 \
         §43.4, docs/49 §49.6 and the `net.rs` seam's own module comment"
    );
    // The port a request defaults to is the plaintext one, which is the same fact stated where a
    // reader of the library will meet it.
    let lib = std::fs::read_to_string(
        crates_dir()
            .parent()
            .expect("compiler/")
            .join("lib/http.beck"),
    )
    .expect("lib/http.beck is readable");
    assert!(
        lib.contains("port=80"),
        "`lib/http.beck` no longer defaults to the plaintext port; this test is out of date"
    );
}

// ---------------------------------------------------------------------------------------------
// F3 — per-actor quotas, `APPROVED` and "on by default with generous limits"
// ---------------------------------------------------------------------------------------------

#[test]
fn no_quota_limits_what_one_actor_can_write_to_the_log() {
    let sites = mentions(&["rate_limit", "per_actor_quota", "QuotaConfig"]);
    assert!(
        sites.is_empty(),
        "F3's per-actor quotas appear to exist now ({sites:?}) — good. Delete this test, and \
         correct docs/14 F3 (whose status is `APPROVED`, not built), §42.1's table and §42.6"
    );
}

/// The observable half of the same gap: one actor may append as much as it likes, and nothing in
/// the runtime counts.
#[tokio::test]
async fn one_actor_may_fill_the_log_unchecked() {
    use std::sync::Arc;

    let placed = support::todo_program();
    let backend = beck_eval::backend(&placed);
    let runtime = beck_rt::Runtime::new(placed, backend).expect("the example prepares");
    let store: Arc<dyn beck_rt::LogStore> = Arc::new(beck_rt::MemoryLog::new());
    let app = beck_rt::App::start(runtime, store.clone(), beck_rt::AppConfig::default())
        .await
        .expect("the app starts");

    // A small number, because the point is that nothing refuses — not how fast it does not.
    for i in 0..200 {
        app.propose(
            format!("c{i}"),
            "one-noisy-actor".into(),
            support::command("Add", &[("id", &format!("t{i}")), ("text", "spam")]),
        )
        .await
        .unwrap_or_else(|e| {
            panic!(
                "proposal {i} was refused: {e} — if that is a quota, F3 is \
             built and this test should be deleted along with §42.6's fourth bullet"
            )
        });
    }
    assert_eq!(
        store.head().await.expect("the log has a head"),
        200,
        "every proposal from one actor became a permanent event, which is F3's channel (b)"
    );
}

// ---------------------------------------------------------------------------------------------
// F15 — subscription and connection quotas
// ---------------------------------------------------------------------------------------------

#[test]
fn no_quota_limits_how_many_subscriptions_a_connection_opens() {
    let sites = mentions(&[
        "max_subscriptions",
        "subscription_quota",
        "connection_limit",
    ]);
    assert!(
        sites.is_empty(),
        "F15's quotas appear to exist now ({sites:?}) — delete this test and correct docs/14 F15, \
         whose status is `DESIGNED`"
    );
}

// ---------------------------------------------------------------------------------------------
// F12 — the deploy choreography's bounded buffer
// ---------------------------------------------------------------------------------------------

#[test]
fn the_quiesce_buffer_has_no_declared_budget() {
    let sites = mentions(&["quiesce", "Retry-After", "drain_budget"]);
    assert!(
        sites.is_empty(),
        "the deploy choreography appears to exist now ({sites:?}). F12 says its command buffer \
         must be bounded with a declared budget and reject with `Retry-After` — check that it is, \
         then delete this test"
    );
}

// ---------------------------------------------------------------------------------------------
// The runtime edge — §42.6's second and third bullets
// ---------------------------------------------------------------------------------------------

// §42.6's second and third bullets were here — the websocket's limits were tungstenite's and
// nothing inspected `Origin`. Both are built (`docs/83`), so both tests are gone, which is what
// this file's own rule asks for: "the day somebody builds one of these, its test goes red, and the
// person who built it has to come here and to the documents and say so." What replaces them is
// `beck-cli/tests/runtime_edge.rs`, which asserts the behaviour over a real socket, and the unit
// tests beside `beck_rt::http::same_origin`, which assert the rule.

// ---------------------------------------------------------------------------------------------
// F17 — macro fuel
// ---------------------------------------------------------------------------------------------

/// Macro expansion is bounded in **depth** and not in **work**.
///
/// `adr/0012` separated the expander's structural walk from its re-expansion count, and both are
/// depth counters. Neither bounds the total size of what is produced, which is what F17 asks for:
/// a macro that doubles its output at each of a few levels is shallow, terminating, and enormous.
/// The test keeps the doubling small on purpose — it is asserting that nothing refuses, and a test
/// that proves the point by exhausting memory is not a test.
#[test]
fn macro_expansion_is_bounded_in_depth_but_not_in_work() {
    let sites = mentions(&["macro_fuel", "expansion_budget", "Fuel"]);
    assert!(
        sites.is_empty(),
        "macro fuel appears to exist now ({sites:?}) — delete this test and correct docs/14 F17"
    );

    // Eight doublings: 256 copies of the leaf, produced by a program of six lines. The shape is
    // what matters — the multiplier is under the author's control and the compiler does not count.
    let mut src = String::from("macro pair(x):\n    return quote:\n        f(x, x)\n\n");
    src.push_str("def f(a: Int, b: Int) -> Int:\n    return a + b\n\n");
    src.push_str("def go() -> Int:\n    return ");
    let mut expr = String::from("1");
    for _ in 0..8 {
        expr = format!("pair({expr})");
    }
    src.push_str(&expr);
    src.push('\n');

    let (_, d, map) = beck_core::compile_str("bomb.beck", &src);
    assert!(
        !d.iter().any(|x| x.code == "B0201" || x.code == "B0213"),
        "expansion was refused, which would mean something now bounds it:\n{}",
        d.render(&map)
    );
}
