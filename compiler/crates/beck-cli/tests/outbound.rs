//! The outbound call: `http_fetch`, the atom it performs, and the egress rule that follows.
//!
//! [`docs/46`](../../../../docs/46-standard-library-report.md) §46.16 called the HTTP client "the
//! one item on the list with an effect row nobody has designed — `net.out(host)` per call site,
//! with the host in the type". This harness is the design, asserted:
//!
//! * the row is inferred from the **host written at the call site**, with no `uses` clause;
//! * the derived NetworkPolicy's egress host is that same string, so a call nobody wrote is a
//!   host the cluster refuses;
//! * a host that is *computed* is a compile error rather than a call the deployment cannot be
//!   told about;
//! * the call is stubbed by the existing §21.3 machinery, because the atom is an ordinary atom;
//! * and a credential reaches the peer without ever being a `Str` the program could read.

mod support;

use std::sync::Arc;

use beck_core::net::{Canned, Reply};
use beck_core::{Effect, Placed, Value};

/// A program that reads a rate from somewhere else, and says nothing about effects.
///
/// Note what is *not* in it: no `uses` clause anywhere, and no `@on`. Everything below is
/// inferred from the two words `"rates.example.com"` on the `http_fetch` line.
const RATES: &str = r#"
model State:
    rate: Int

union Command:
    Refresh

union Event:
    Refreshed(rate: Int)

union Rejection:
    Unavailable

def fresh_rate() -> Int:
    return unwrap_or(str_to_int(http_fetch("rates.example.com",
        HttpRequest(method="GET", path="/usd", headers={}, body="", port=80, tls=False, secrets={})).body), 0)

def rate_or_error() -> Result[Int, HttpError]:
    return try: fresh_rate()

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Refreshed(rate):
            return s.with(rate=rate)

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Refresh:
            match rate_or_error():
                case Ok(value):
                    return Ok(value=[Refreshed(rate=value)])
                case Err(error):
                    return Err(error=Unavailable)

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            p: str(s.rate)

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, book, validate)
book: Signal[State] = durable(fold(apply_event, State(rate=0), events))
page: Signal[Html] = per_session(book, view)
"#;

fn compile(src: &str) -> Placed {
    let (placed, d, m) = beck_core::compile_str("t.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    placed.expect("this program compiles")
}

fn codes(src: &str) -> Vec<String> {
    let (_, d, _) = beck_core::compile_str("t.beck", src);
    d.iter().map(|x| x.code.to_string()).collect()
}

fn run(placed: &Placed) -> beck_rt::testing::Report {
    let backend = beck_eval::backend(placed);
    beck_rt::testing::run(placed, backend, &beck_rt::testing::Options::default())
}

// ---------------------------------------------------------------------------------------------
// 1. The row is the call site
// ---------------------------------------------------------------------------------------------

#[test]
fn the_host_written_at_the_call_site_is_the_atom_that_is_performed() {
    let placed = compile(RATES);
    let def = &placed.program.defs["fresh_rate"];
    assert!(
        def.declared_effects.is_empty(),
        "nothing in the program declares an effect"
    );
    assert!(
        def.effects
            .contains(&Effect::NetOut("rates.example.com".into())),
        "{:?}",
        def.effects
    );
    assert!(
        def.effects.contains(&Effect::Raises("HttpError".into())),
        "a call can fail, and the row says so without anybody writing it: {:?}",
        def.effects
    );
    // …and the atom is what places it: a browser cannot discharge `net.out(a named host)`.
    assert_eq!(def.tier, beck_core::Tier::Server);
    assert!(!def.tier_is_annotated, "nobody wrote `@on`");
}

#[test]
fn the_clusters_egress_rule_is_that_same_string() {
    // §3.5's "least-privilege infra, computed", starting one step further back than
    // `security.rs` starts it: that harness hands `beck_infra::derive` an atom, and this one
    // derives the atom from a program that only ever wrote a hostname inside a call.
    let g = beck_infra::graph(&compile(RATES));
    let hosts: Vec<String> = g
        .nodes
        .iter()
        .filter_map(|n| match &n.node {
            beck_infra::Node::Policy {
                allow_egress_hosts, ..
            } => Some(allow_egress_hosts.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(hosts, vec!["rates.example.com".to_string()]);
}

#[test]
fn a_computed_host_is_refused_because_nothing_downstream_could_name_it() {
    let computed = RATES.replace(
        "def fresh_rate() -> Int:",
        "def fresh_rate_at(where: Str) -> Int:\n    return unwrap_or(str_to_int(http_fetch(where,\n        HttpRequest(method=\"GET\", path=\"/usd\", headers={}, body=\"\", port=80, tls=False, secrets={})).body), 0)\n\ndef fresh_rate() -> Int:",
    );
    assert!(
        codes(&computed).contains(&"B0395".to_string()),
        "{:?}",
        codes(&computed)
    );
}

// ---------------------------------------------------------------------------------------------
// 2. It is an ordinary atom, so §21.3's machinery already covers it
// ---------------------------------------------------------------------------------------------

#[test]
fn the_peer_is_stubbed_by_naming_it_and_by_naming_nothing() {
    let placed = compile(&format!(
        "{RATES}\ntest \"the stub is the peer\":\n\
         \x20   stub net.out(rates.example.com): 42\n\
         \x20   when session(\"ana\") sends Refresh\n\
         \x20   expect events == [Refreshed(rate=42)]\n\
         \n\
         test \"and nobody has to mention it\":\n\
         \x20   when session(\"ana\") sends Refresh\n\
         \x20   expect events == [Refreshed(rate=0)]\n"
    ));
    let report = run(&placed);
    assert_eq!(
        report.failed(),
        0,
        "{}",
        beck_rt::testing::render(&report, true)
    );
    // The auto-stub replaced the definition that *performs* the atom, not the one that inherits
    // it — `validate` still ran, which is what makes the second test an assertion about the
    // program rather than about the harness.
    let hidden = report
        .cases
        .iter()
        .find(|c| c.name.contains("nobody has to mention"))
        .expect("the second test ran");
    assert_eq!(hidden.stubbed.len(), 1);
    assert_eq!(hidden.stubbed[0].atom, "net.out(rates.example.com)");
    assert_eq!(hidden.stubbed[0].def.as_ref(), "fresh_rate");
    assert!(!hidden.stubbed[0].explicit);
}

// ---------------------------------------------------------------------------------------------
// 3. A credential, which is what an outbound call is usually for
// ---------------------------------------------------------------------------------------------

/// A program cannot turn a `secret[Str]` into the `Str` a header needs — and must not be able to.
#[test]
fn a_secret_cannot_become_a_header_value_the_program_wrote() {
    // §3.5: there is no `reveal` for a `secret[T]`. That is the claim that keeps one out of a
    // browser, and it is also why `HttpRequest` has a `secrets` field: the alternative to a
    // separate field is not "`"Bearer " + key`", it is "no authenticated request at all".
    let src = "\
def header(token: secret[Str]) -> Str:
    return \"Bearer \" + reveal(token)
";
    assert!(
        codes(src).contains(&"B0320".to_string()),
        "{:?}",
        codes(src)
    );
}

#[test]
fn a_secret_header_reaches_the_peer_without_becoming_a_value() {
    // The other half: the credential does go out. This runs the real primitive against a canned
    // client — the seam's second implementation — and reads what was sent.
    let client = Arc::new(Canned::new(vec![Ok(Reply {
        status: 200,
        headers: vec![(Arc::from("content-type"), Arc::from("text/plain"))],
        body: Arc::from("41"),
    })]));
    assert!(
        beck_core::net::set_process_outbound(client.clone()),
        "this test binary installs the process client exactly once"
    );

    let src = "\
def rate() -> Int:
    return unwrap_or(str_to_int(http_fetch(\"rates.example.com\", HttpRequest(
        method=\"GET\",
        path=\"/usd\",
        headers={\"accept\": \"text/plain\"},
        body=\"\",
        port=443,
        tls=True,
        secrets={\"authorization\": secret_env(\"RATES_TOKEN\")})).body), 0)
";
    std::env::set_var("RATES_TOKEN", "Bearer s3cret");
    let (program, d, m) = beck_core::check_str("t.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&m));
    let backend = beck_eval::backend_for(Arc::new(program.clone()));
    let rate = backend
        .function(&program.defs["rate"].body)
        .expect("the definition prepares");
    assert_eq!(
        beck_eval::on_the_evaluator_stack(|| rate(Vec::new())).expect("the call returns"),
        Value::Int(41)
    );

    let sent = client.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].host.as_ref(), "rates.example.com");
    assert_eq!(sent[0].port, 443);
    assert_eq!(sent[0].path.as_ref(), "/usd");
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k.as_ref() == "authorization" && v.as_ref() == "Bearer s3cret"),
        "the credential is on the wire: {:?}",
        sent[0].headers
    );
    assert!(
        sent[0]
            .headers
            .iter()
            .any(|(k, v)| k.as_ref() == "accept" && v.as_ref() == "text/plain"),
        "and so are the ordinary headers: {:?}",
        sent[0].headers
    );
}
