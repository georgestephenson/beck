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

// `nothing_here_speaks_oidc` and `a_verified_identitys_claims_do_not_reach_the_program` were here,
// and both are **built** (`docs/95`): `beck_rt::oidc` is an asymmetric relying party, and `Session`
// carries the claims it verified. Both are gone, which is what this file's own rule asks for, and
// what replaces them is `beck-cli/tests/oidc.rs` — where the tokens are signed by a key pair
// generated for the test and the relying party is given nothing but the public half.
//
// The first of the two is worth a note beside `docs/84` §84.5's list. It was a **name grep**, for
// `jwks`, `id_token`, `issuer` and `RS256`, and it would have fired on this change — all four
// appear. But it would equally have fired on a module that fetched a JWKS and checked nothing, and
// on a comment. A grep proves a subject was touched; it cannot prove a control works, and it is
// used here only where there is no behaviour to look at.
//
// `an_outbound_call_has_no_transport_security` is gone too (`docs/adr/0022`). It grepped for
// `rustls`, `TlsConnector`, `tokio_rustls` and `webpki`, and it also asserted that `lib/http.beck`
// defaults to `port=80` — which is *still true*, and is now a statement about a default rather than
// about a missing capability, since `over_tls` is the call that changes it. That half moved into
// `lib/http.beck`'s own tests, where a reader of the library meets it.

// `no_identity_provider_is_provisioned_into_the_object_graph` was here, and it is **built**
// (`docs/95` §95.10): `identity = managed()` derives a provider, its Service, its volume, its
// credentials and a realm wired to this application's own route, and the application's egress to it
// is a `Peer` rather than a DNS name. What replaces it is `oidc.rs`, in both directions — the
// declaration with it and the same program without.
//
// That test is also the one this file's own rules were hardest on. Its first version searched the
// rendered YAML for `keycloak`, `ory` and `Kratos`, and went red on **`revisionHistoryLimit`**; the
// second counted `Workload` nodes. Both were asserting an absence by looking at the shape of the
// fix rather than the shape of the gap, which is `docs/84` §84.5's pattern from the other side.
//
// **Nothing about identity is asserted as absent here now.** Presence — D6's "who is connected now,
// as a first-class non-durable Signal" — is the last unbuilt row of that bullet, and it is a
// language feature rather than a control, so it belongs in `docs/08`'s list and not in this file.

// ---------------------------------------------------------------------------------------------
// F3 — per-actor quotas: BUILT (`docs/84`), and the two tests that were here did not notice
// ---------------------------------------------------------------------------------------------
//
// They are gone, and how they failed is worth more than they were.
//
// `no_quota_limits_what_one_actor_can_write_to_the_log` grepped the workspace for `rate_limit`,
// `per_actor_quota` and `QuotaConfig`. The quota was built as `RateLimit`, `Quota` and
// `quota::admit`, so it matched nothing and the test stayed green through the change it existed to
// detect. **A name grep is a proxy for a control, and a proxy can be defeated by naming** — not
// deliberately, just by somebody choosing different words a year later.
//
// `one_actor_may_fill_the_log_unchecked` was the behavioural half, and it sent 200 proposals. The
// limit that was eventually chosen is 600 a minute, so it passed under it. **A behavioural test
// for an absence has to be calibrated against a limit that does not exist yet**, which is a thing
// nobody can do — so the honest form is a *ratio*: send more than any plausible limit, or assert
// the shape (unbounded) rather than a number.
//
// What replaces them is in `runtime_edge.rs`, where the assertions are on the log's head rather
// than on identifiers: five events allowed out of fifty proposed, and the head stops at five.
//
// The rest of this file still uses the grep, because for a control that does not exist there is
// often nothing else to look at. `docs/84` §84.5 is the caveat that now belongs to every one of
// them.

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

/// The read-model port has no authentication, and the loopback bound is what stands in for one.
///
/// Behaviour rather than a grep, which is the second rule above and `docs/84` §84.5's lesson: a
/// password is a thing a client either has to send or does not, so the honest test connects without
/// one. Two halves, and they fail on different days — the first when authentication is built, the
/// second if the bound is ever lifted without one.
#[tokio::test]
async fn the_read_model_port_authenticates_nobody_and_answers_only_to_localhost() {
    let app = beck_rt::App::start(
        support::todo_runtime(),
        std::sync::Arc::new(beck_rt::MemoryLog::new()),
        beck_rt::AppConfig::default(),
    )
    .await
    .expect("the app starts");
    let listener = beck_rt::pgwire::bind("127.0.0.1:0".parse().expect("an address"))
        .await
        .expect("binds on loopback");
    let port = listener.local_addr().expect("a bound port").port();
    tokio::spawn(async move {
        let _ = beck_rt::pgwire::serve_on(listener, app).await;
    });

    let connected = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user=whoever dbname=whatever"),
        tokio_postgres::NoTls,
    )
    .await;
    assert!(
        connected.is_ok(),
        "the read-model port asked for a credential. If authentication has been built, this test \
         is the record that it had not been: delete it, correct docs/43 §43.4, and revisit \
         docs/adr/0020 — the loopback bound exists *because* there is no authentication"
    );

    assert!(
        beck_rt::pgwire::bind("0.0.0.0:0".parse().expect("an address"))
            .await
            .is_err(),
        "the read-model port bound to a non-loopback address. With no authentication that is an \
         unauthenticated read of the whole application state from any host on the network; \
         docs/adr/0020 is the record that would have to change first"
    );
}

// ---------------------------------------------------------------------------------------------
// The release artefacts — a checksum, and no signature
// ---------------------------------------------------------------------------------------------

/// A reader who downloads a compiler binary reasonably assumes it is signed. It is not.
///
/// `docs/104-the-release-and-the-installer-report.md` §104.6 is the argument: `beck sign`'s subject
/// is an OCI manifest digest, so the signing machinery `docs/99` built does not reach a tarball,
/// and what a release publishes is a `SHA256SUMS` — which proves a download was not corrupted and
/// nothing whatever about the page it came from.
///
/// This asserts the gap from both ends, because either one could close first: the pipeline signs
/// nothing, and the installer checks no signature.
#[test]
fn a_release_artefact_carries_a_checksum_and_no_signature() {
    let repo = crates_dir()
        .ancestors()
        .nth(2)
        .expect("the repository is two levels above crates/")
        .to_path_buf();

    for (file, needles) in [
        (
            ".github/workflows/release.yml",
            ["cosign", "beck sign", "attest", "--certificate"],
        ),
        (
            "install.sh",
            ["cosign", "beck verify", "gpg", "--certificate"],
        ),
    ] {
        let text = std::fs::read_to_string(repo.join(file)).expect("checked in");
        // Comments are where this absence is *explained*, so they are not evidence that it closed.
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let found: Vec<&str> = needles
            .iter()
            .copied()
            .filter(|n| code.contains(n))
            .collect();
        assert!(
            found.is_empty(),
            "{file} appears to sign or verify a release artefact now ({found:?}). Good — delete \
             this test, correct docs/43 §43.4 and docs/104 §104.6, and say in the same change what \
             the signature's subject is and who can check it"
        );
    }
}
