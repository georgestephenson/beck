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

/// The actor is self-asserted: it arrives in the client's own `hello` frame and is believed.
///
/// [`docs/42`](../../../../docs/42-security-assurance.md) §42.5 is the sentence this test exists to
/// stop anybody from misquoting: "a capability required outside the chokepoint has no holder" is
/// true and proven by `security.rs`; "only the owner may toggle their todo" is enforced against a
/// string the caller chose. The two read alike in a slide deck and are different guarantees.
#[test]
fn the_actor_is_whatever_the_client_says_it_is() {
    let msg = beck_rt::protocol::ClientMsg::parse(
        r#"{"t":"hello","sub":"s1","seq":0,"actor":"the-auditor"}"#,
    )
    .expect("the frame parses");
    let claimed = match msg {
        beck_rt::protocol::ClientMsg::Hello { actor, .. } => actor,
        _ => panic!("that is a hello"),
    };
    assert_eq!(
        claimed, "the-auditor",
        "identity is dev-mode: the runtime takes the actor from the frame. When D6's OIDC relying \
         party lands, this test goes red — delete it, and correct §42.1's table, §42.6's first \
         bullet and the roadmap's identity bullet in the same change"
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

/// The websocket's limits are tungstenite's defaults — 64 MiB a message — because the upgrade
/// passes `None` for the configuration. Bounded, then, but by somebody else's judgement.
#[test]
fn the_websocket_takes_whatever_limits_its_library_defaults_to() {
    let src = std::fs::read_to_string(crates_dir().join("beck-rt/src/http.rs"))
        .expect("the http module is readable");
    assert!(
        src.contains("Role::Server, None"),
        "the websocket now configures itself — pick the numbers this project can defend, then \
         delete this test and correct §42.6's second bullet"
    );
}

/// Nothing inspects `Origin` on the upgrade, so a page on any host may open a socket.
///
/// Scoped to the module that performs the upgrade rather than to the workspace: "origin" is an
/// ordinary English word, and a scan wide enough to catch a header is wide enough to catch a
/// comment about where a type came from.
#[test]
fn the_upgrade_does_not_look_at_the_origin() {
    let src = std::fs::read_to_string(crates_dir().join("beck-rt/src/http.rs"))
        .expect("the http module is readable");
    let code: String = src
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.to_lowercase().contains("origin"),
        "an origin check appears to exist now — delete this test and correct §42.6's third bullet"
    );
}

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
