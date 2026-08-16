//! §3.5's table, as executable tests — one per row.
//!
//! [`docs/03-type-and-effect-system.md`](../../../../docs/03-type-and-effect-system.md) §3.5 makes
//! eight claims under the heading "security properties for free (the selling point)", and the
//! Phase 2 exit criterion is that "every §3.5 property is a passing test". This file is that.
//!
//! Two rules were applied while writing it, because a security suite that tests the wrong thing is
//! worse than none:
//!
//! * **A property is tested by the attempt it forbids**, not by the API that implements it. Every
//!   test below writes the program a mistaken or malicious author would write, and asserts the
//!   compiler refuses it *by name*.
//! * **Where a property is structural rather than checked, the test says so in its own body.**
//!   "Clients can only propose" is not enforced by an analysis; it is enforced by there being no
//!   other message in the protocol. That is a stronger guarantee, and the test asserts the absence
//!   rather than pretending a check exists.

use beck_core::{compile_str, Effect, Tier};

mod support;

/// Compile a source string and return the diagnostic codes it produced.
fn codes(src: &str) -> Vec<&'static str> {
    let (_, d, _) = compile_str("t.beck", src);
    d.iter().map(|x| x.code).collect()
}

fn compiles(src: &str) {
    let (placed, d, map) = compile_str("t.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&map));
    assert!(placed.is_some());
}

/// The canonical program, as the file a reader can open.
const TODO: &str = include_str!("../../../examples/todo.beck");

// ---------------------------------------------------------------------------------------------
// 1. "Secrets cannot reach the browser"
//    Mechanism: boundary crossings require Sendable; secret[T] isn't.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_secret_in_a_command_is_a_compile_error_naming_the_flow() {
    // A command is minted in the browser. A secret in one is a secret the browser already had, so
    // this is the mistake the property exists to make impossible rather than merely unwise.
    let src = TODO.replace(
        "    Add(id: Id, text: Str)           # the client names the id — see \"optimism\"",
        "    Add(id: Id, text: Str, token: secret[Str])",
    );
    assert!(codes(&src).contains(&"B0410"), "{:?}", codes(&src));

    // §3.5: "The leak is a compile error naming the flow". Naming it is the requirement — a
    // diagnostic that says only "not Sendable" leaves the author to find the field themselves.
    let (_, d, map) = compile_str("t.beck", &src);
    let rendered = d.render(&map);
    assert!(
        rendered.contains("secret[Str]") && rendered.contains("Command.Add.token"),
        "the diagnostic must name the type and the path it travels:\n{rendered}"
    );
}

#[test]
fn a_secret_buried_under_three_records_is_found_just_the_same() {
    // The interesting failure is never at the top level. `State` holds a `Config` holds an
    // `ApiKey` holds a `secret[Str]`, and the state is what the view renders.
    let src = TODO
        .replace(
            "model State:\n    todos: Map[Id, Todo]",
            "type ApiKey = newtype[secret[Str]]\n\nmodel Config:\n    key: ApiKey\n\n\
             model State:\n    todos: Map[Id, Todo]\n    config: Config",
        )
        .replace(
            "State(todos={})",
            "State(todos={}, config=Config(key=ApiKey(value=secret_env(\"K\"))))",
        );
    let all = codes(&src);
    assert!(
        all.contains(&"B0410") || all.contains(&"B0411"),
        "a secret reachable from the state must be refused: {all:?}"
    );
}

#[test]
fn a_secret_kept_on_the_server_is_fine_and_the_flow_query_says_where_it_went() {
    // The property is not "secrets are forbidden"; it is "secrets cannot cross". A program that
    // keeps one on the server must compile, or the mechanism is useless.
    let src = SECRET_APP;
    compiles(src);
    let (program, d, map) = beck_core::check_str("t.beck", src);
    assert!(!d.has_errors(), "{}", d.render(&map));
    let mut program = program;
    let solution = beck_core::place::solve(&program, None);
    beck_core::place::apply(&mut program, &solution);

    let reached = beck_core::secure::flow(&program, "ApiKey");
    assert!(!reached.is_empty(), "the key must reach something");
    for r in &reached {
        assert_eq!(
            r.tier,
            Tier::Server,
            "`{}` handles the key, so it cannot be anywhere but the server",
            r.what
        );
        assert!(r.blocked.is_none());
    }
}

#[test]
fn placing_a_secret_handling_function_on_the_client_is_refused() {
    // The same program, with one annotation added. This is the whole claim in one diff.
    let src = SECRET_APP.replace("def charge(", "@on(client)\ndef charge(");
    let all = codes(&src);
    assert!(
        all.contains(&"B0401") || all.contains(&"B0410"),
        "a client that handles a secret must be refused: {all:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 1b. "…and the other half of it: facts that are recorded and never shown"
//     Mechanism: `internal[T]` is Storable and not Sendable, which is the quadrant `secret[T]`
//     alone leaves empty. Not one of §3.5's eight rows — it is the answer to a question §3.5 does
//     not ask, and docs/20 §20.4 item 12 records why it was added.
// ---------------------------------------------------------------------------------------------

/// The corpus program that exists for this, read as the file a person can open.
const MODERATION: &str = include_str!("../../../corpus/20-moderation.beck");

#[test]
fn an_internal_fact_is_written_to_the_log_and_the_page_still_renders() {
    // The positive half, first, because a check that refuses everything is not a feature: a
    // `durable` fold whose state reaches an `internal[Str]` is *fine*. That is what it is for.
    compiles(MODERATION);
}

#[test]
fn an_internal_fact_in_a_command_is_refused_the_way_a_secret_is() {
    // A command is minted in the browser, so an `internal[T]` in one is a value the browser
    // already had — the same mistake as a secret in a command, caught by the same rule.
    let src = MODERATION
        .replace(
            "    Suspend(account: Str, reason: Str)",
            "    Suspend(account: Str, reason: internal[Str])",
        )
        .replace(
            "        case Suspend(account, reason):
            if str_is_empty(str_trim(reason)):
                return Err(error=NoReason)
            return Ok(value=[Suspended(account=account, reason=internal_of(str_trim(reason)))])",
            "        case Suspend(account, reason):
            return Ok(value=[Suspended(account=account, reason=reason)])",
        );
    assert!(codes(&src).contains(&"B0410"), "{:?}", codes(&src));

    let (_, d, map) = compile_str("t.beck", &src);
    let rendered = d.render(&map);
    assert!(
        rendered.contains("internal[Str]") && rendered.contains("Command.Suspend.reason"),
        "the diagnostic must name the type and the path it travels:\n{rendered}"
    );
}

#[test]
fn an_internal_fact_cannot_be_rendered_into_a_page() {
    // The attempt the type exists to forbid, written the way someone would write it: read the
    // reason back and print it. `reveal` carries `cap.internal`, a page is the browser's because
    // of its type, and no capability is discharged in a browser — so the view does not compile.
    let src = MODERATION.replace(
        "                        span: x.account",
        "                        span: (x.account + \": \" + reveal(x.reason))",
    );
    assert!(codes(&src).contains(&"B0401"), "{:?}", codes(&src));

    // And the diagnostic must not send the author to `@on(server)`, which would move the page off
    // the browser rather than fix the leak. This note is the one that was wrong before this test
    // existed: an unannotated placement is not automatically a *solved* one.
    let (_, d, map) = compile_str("t.beck", &src);
    let rendered = d.render(&map);
    assert!(
        rendered.contains("is not a placement an annotation can move"),
        "a structural pin must say so rather than blame the solver:\n{rendered}"
    );
    assert!(
        !rendered.contains("compiler defect"),
        "this is the program's mistake, not the compiler's:\n{rendered}"
    );
}

#[test]
fn revealing_an_internal_fact_inside_the_chokepoint_is_exactly_what_it_is_for() {
    // Authority is one chokepoint (§3.5), and `cap.internal` is a capability like any other: the
    // validator can hold it, so the rules may decide *on* the reason even though no view may show
    // it. Refusing this too would make the type useless rather than safe.
    let src = MODERATION
        .replace("uses cap.moderator", "uses cap.moderator, cap.internal")
        .replace(
            "        case Some(value):
            return Ok(value=[Restored(account=account)])",
            "        case Some(value):
            if str_is_empty(reveal(value.reason)):
                return Err(error=NotSuspended)
            return Ok(value=[Restored(account=account)])",
        );
    compiles(&src);
}

#[test]
fn a_secret_cannot_stand_in_for_an_internal_fact_because_it_is_not_storable() {
    // The two are not interchangeable, and this is the test that says so: swap `internal[Str]` for
    // `secret[Str]` in the same program and the *log* refuses it. Which is the reason `internal[T]`
    // had to exist — without it, recording why an account was suspended has no legal type.
    let src = MODERATION.replace("internal[Str]", "secret[Str]").replace(
        "internal_of(str_trim(reason))",
        "secret_env(str_trim(reason))",
    );
    assert!(codes(&src).contains(&"B0411"), "{:?}", codes(&src));
}

// ---------------------------------------------------------------------------------------------
// 1c. "…and the one function that spends a secret without revealing it"
//     Mechanism: `digest_keyed` is a message authentication code, so its output depends on a key
//     its reader must not learn. That is a declassification and it is the point of the operation
//     rather than a hole in §3.5 — `docs/adr/0014` is the decision. The tests below are what keeps
//     it *one* function, held behind a capability, rather than a door that opened.
// ---------------------------------------------------------------------------------------------

#[test]
fn exactly_one_primitive_turns_a_secret_into_something_that_is_not_one() {
    // §3.5's claim is a claim about the whole language, so it is checked against the whole prelude
    // rather than against the primitive that happens to be under discussion. A second declassifier
    // added without a second argument fails here, which is the only place it would fail.
    let declassifiers: Vec<&str> = beck_core::prelude::prims()
        .iter()
        .filter_map(|(name, _, scheme)| {
            let beck_core::Ty::Fun(params, ret, _) = &scheme.ty else {
                return None;
            };
            let takes = params
                .iter()
                .any(|p| p.con_name() == Some(beck_core::Ty::SECRET));
            let gives = ret.con_name() == Some(beck_core::Ty::SECRET);
            (takes && !gives).then_some(*name)
        })
        .collect();
    assert_eq!(
        declassifiers,
        vec!["digest_keyed"],
        "one function spends a secret; adding another is a decision, not a patch"
    );
}

#[test]
fn minting_a_code_is_a_capability_and_the_client_does_not_hold_it() {
    // The same shape as `placing_a_secret_handling_function_on_the_client_is_refused`, one row
    // down: it is not the secret that reaches the client here — a `Str` is Sendable and a MAC is a
    // `Str` — it is the *minting*. `cap.sign` is undischargeable on a browser, so a view that
    // signs is a placement error rather than a code review.
    let src = format!(
        "{SECRET_APP}
@on(client)
def stamp(k: secret[Str], body: Str) -> Str:
    return digest_keyed(k, body)
"
    );
    let all = codes(&src);
    assert!(
        all.contains(&"B0401") || all.contains(&"B0410"),
        "a client that mints a code must be refused: {all:?}"
    );
}

#[test]
fn signing_inside_the_chokepoint_is_exactly_what_it_is_for() {
    // And the other direction, for the reason 1b's pair exists: a capability that nothing may hold
    // is not a capability, it is a ban. `validate` may hold `cap.sign`, so a program can issue a
    // token where authority already lives.
    let src = TODO
        .replace(
            "def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:",
            "def validate(s: State, p: Proposal) -> Result[list[Event], Rejection] uses env, cap.sign:
    stamp = digest_keyed(secret_of(), p.session.actor)
    if str_is_empty(stamp):
        return Err(error=BlankText)",
        )
        .replace(
            "def if_owned(",
            "@on(server)
def secret_of() -> secret[Str]:
    return secret_env(\"TOKEN_KEY\")

def if_owned(",
        );
    compiles(&src);
}

#[test]
fn a_digest_is_pure_and_a_keyed_one_is_not() {
    // The line this group is drawn on. `digest` performs nothing, so it may be computed inside a
    // fold and a replay recomputes it; `digest_keyed` performs a capability, so §3.7's rule about
    // what a fold may contain applies to it and not to the other.
    use beck_core::core::Prim;
    assert!(Prim::Digest.effects().is_empty());
    assert!(Prim::HexEncode.effects().is_empty());
    assert!(Prim::Base64Encode.effects().is_empty());
    assert!(Prim::DigestEq.effects().is_empty());
    assert_eq!(
        Prim::DigestKeyed.effects(),
        vec![Effect::Cap(std::sync::Arc::from("sign"))]
    );
}

// ---------------------------------------------------------------------------------------------
// 2. "Clients can only propose"
//    Mechanism: the client's entire write surface is send(cmd) into a typed Command union.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_protocol_has_no_message_that_writes_anything_but_a_command() {
    // This property is structural, and the test says so rather than inventing a check: there is no
    // analysis that stops a client mutating state, because the protocol has no message that could.
    // Mass assignment and over-posting have no representation — they have no *frame*.
    use beck_rt::protocol::ClientMsg;
    assert!(ClientMsg::parse(r#"{"t":"hello","sub":"page","seq":0,"actor":"alice"}"#).is_ok());
    assert!(ClientMsg::parse(r#"{"t":"c","id":"1","command":{"c":"Toggle","id":"a"}}"#).is_ok());
    assert!(ClientMsg::parse(r#"{"t":"ping"}"#).is_ok());
    // Anything else is not a message this server has.
    for forged in [
        r#"{"t":"set","path":"todos.a.done","value":true}"#,
        r#"{"t":"event","body":{"c":"Added","id":"x","text":"y"}}"#,
        r#"{"t":"append","seq":99}"#,
        r#"{"t":"sql","q":"delete from beck_log"}"#,
    ] {
        assert!(
            ClientMsg::parse(forged).is_err(),
            "the protocol must have no representation for `{forged}`"
        );
    }
}

#[test]
fn a_command_that_is_not_in_the_union_is_refused_at_the_boundary() {
    // Even the one write surface is typed: a well-formed frame carrying a command the program does
    // not declare is a rejection, not a panic and not an insert.
    let runtime = support::todo_runtime();
    for forged in [
        serde_json::json!({"c": "Elevate", "id": "a1"}),
        serde_json::json!({"id": "a1"}),
        serde_json::json!({"c": "Toggle"}),
    ] {
        assert!(
            runtime.decode_command(&forged).is_err(),
            "`{forged}` is not a command this program declares"
        );
    }

    // Over-posting is the sharper case, and the answer is not a rejection. §4.3 requires the wire
    // format to *tolerate unknown fields*, because during a rolling deploy an old client talks to a
    // new server. So an extra field is accepted and then does not exist: decoding is driven by the
    // union's declared fields, so `owner` never becomes part of any value, and there is nothing for
    // a later assignment to pick up. Mass assignment has no representation because the decoder has
    // no loop over the *input's* keys.
    let over_posted = runtime
        .decode_command(&serde_json::json!({"c": "Toggle", "id": "a1", "owner": "root"}))
        .expect("an unknown field is tolerated, per §4.3");
    let honest = runtime
        .decode_command(&serde_json::json!({"c": "Toggle", "id": "a1"}))
        .expect("the declared form decodes");
    assert_eq!(
        beck_core::digest(&over_posted),
        beck_core::digest(&honest),
        "the extra field must leave no trace in the decoded value"
    );
}

// ---------------------------------------------------------------------------------------------
// 3. "Authority is one chokepoint"
//    Mechanism: only validate — the ingress consumer holding Session capabilities — turns
//    commands into events; a cap.* effect that goes undischarged is a compile error.
// ---------------------------------------------------------------------------------------------

#[test]
fn events_that_do_not_come_from_the_validator_are_refused() {
    // The chokepoint is not a convention the splitter hopes for. A stream of events built any other
    // way is refused by name, so there is no second path from a command to a fact.
    let src = TODO.replace(
        "events: Stream[Event] = decide(proposals, todos, validate)",
        "events: Stream[Event] = filter_map(proposals, forge)",
    );
    let src = src.replace(
        "def validate(",
        "def forge(p: Proposal) -> Option[Event]:\n    return None\n\ndef validate(",
    );
    assert!(codes(&src).contains(&"B0504"), "{:?}", codes(&src));
}

#[test]
fn a_capability_required_outside_the_chokepoint_has_no_holder() {
    // §3.5: "forgetting an auth check means the `cap.*` effect goes undischarged — a compile error,
    // not a pentest finding." A `Session` reaches exactly one place in a Beck program, so a
    // capability demanded anywhere else is a requirement nothing can satisfy.
    let src = TODO.replace(
        "def done_class(",
        "def purge(t: Todo) -> Str uses cap.admin:\n    return t.text\n\ndef done_class(",
    );
    assert!(codes(&src).contains(&"B0412"), "{:?}", codes(&src));
}

#[test]
fn a_capability_required_inside_the_chokepoint_pins_the_whole_path_to_the_server() {
    // And the positive case, which matters as much: the same effect reached from `validate` is the
    // design working, and it moves every definition on that path to the one tier that holds a
    // session.
    let src = TODO.replace(
        "def if_owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection]:\n    match map_get(s.todos, id):\n        case Some(value):\n            if value.owner != p.session.actor:",
        "def may(p: Proposal, owner: Str) -> Bool uses cap.session:\n    return owner == p.session.actor\n\n\
         def if_owned(s: State, p: Proposal, id: Id, evs: list[Event]) -> Result[list[Event], Rejection]:\n    match map_get(s.todos, id):\n        case Some(value):\n            if not may(p, value.owner):",
    );
    compiles(&src);
    let (placed, _, _) = compile_str("t.beck", &src);
    let program = placed.expect("it compiles").program;
    for name in ["may", "if_owned", "validate"] {
        assert_eq!(
            program.defs[name].tier,
            Tier::Server,
            "`{name}` is on the authority path, and only the server holds a capability"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 4. "The log and rules never ship to clients"
//    Mechanism: ingress/durable are undischargeable on client; DCE strips server-only code from
//    client artefacts, verified.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_log_cannot_be_placed_in_a_browser() {
    let src = TODO.replace(
        "@on(data)\ntodos: Signal[State]",
        "@on(client)\ntodos: Signal[State]",
    );
    assert!(codes(&src).contains(&"B0401"), "{:?}", codes(&src));

    let src = TODO.replace(
        "@on(server)\nproposals: Stream[Proposal]",
        "@on(client)\nproposals: Stream[Proposal]",
    );
    assert!(codes(&src).contains(&"B0401"), "{:?}", codes(&src));
}

#[test]
fn the_client_artefact_contains_no_rule_from_the_program() {
    // "DCE strips server-only code from client artefacts, verified." Phase 2's client is the thin
    // patch interpreter, so the property holds by construction — but "by construction" is a claim
    // until something checks the bytes, and the bytes are what ships.
    let client = beck_rt::THIN_CLIENT;
    for rule in [
        "apply_event",
        "validate",
        "if_owned",
        "IdTaken",
        "NoSuchTodo",
        "beck_log",
        "map_insert",
    ] {
        assert!(
            !client.contains(rule),
            "`{rule}` is a server-side rule and must not be in the client bundle"
        );
    }
    // …and it is small enough that reading it is a realistic audit, which is the other half of the
    // claim being credible.
    assert!(
        client.len() < 10_000,
        "the whole client is {} bytes",
        client.len()
    );
}

// ---------------------------------------------------------------------------------------------
// 5. "No injection / no XSS"
//    Mechanism: html""/sql"" typed literals; interpolation is escaped by type.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_todo_whose_text_is_a_script_tag_renders_as_text() {
    // The oldest bug in web programming, attempted through the one channel a client has.
    use beck_core::Html;
    let hostile = "<script>fetch('//evil')</script>";
    let rendered = Html::text(hostile).render();
    assert!(!rendered.contains("<script>"), "{rendered}");
    assert!(rendered.contains("&lt;script&gt;"), "{rendered}");

    // And in an attribute, where the escape set is different and the mistake is easier.
    let rendered = Html::el("div")
        .attr("title", "\" onclick=\"steal()")
        .render();
    assert!(!rendered.contains("onclick=\"steal"), "{rendered}");
}

#[test]
fn a_handler_is_a_declarative_attribute_rather_than_generated_javascript() {
    // The structural half of the same property: no program text becomes script, so there is no
    // interpolation site to get wrong. That is what lets `script-src` stay near-empty.
    let runtime = support::todo_runtime();
    let state = runtime.initial_state().expect("initial state");
    let out = runtime
        .view(&state, "alice")
        .expect("the view renders")
        .render();
    assert!(
        out.contains("data-b-"),
        "a handler is an attribute the client interprets, not code: {out}"
    );
    assert!(
        !out.contains("<script"),
        "the compiled view must contain no script element: {out}"
    );
    assert!(
        !out.contains("javascript:") && !out.contains("onclick="),
        "no inline handler may be emitted: {out}"
    );
}

// ---------------------------------------------------------------------------------------------
// 6. "Least-privilege infra, computed"
//    Mechanism: effect rows → NetworkPolicy, RBAC, store grants.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_egress_policy_is_the_programs_net_out_atoms_and_nothing_else() {
    // The claim is not "we generate a NetworkPolicy". It is that the policy is a *function of the
    // effect row* — so a host nobody calls is a host the cluster refuses, and adding the call adds
    // the rule in the same commit.
    let without = beck_infra::derive(
        "app",
        &[
            (Effect::Ingress, "proposals".into()),
            (Effect::Durable, "todos".into()),
        ],
        true,
    );
    let with = beck_infra::derive(
        "app",
        &[
            (Effect::Ingress, "proposals".into()),
            (Effect::Durable, "todos".into()),
            (
                Effect::NetOut("payments.example.com".into()),
                "charge".into(),
            ),
        ],
        true,
    );
    assert!(!hosts(&without).contains(&"payments.example.com".to_string()));
    assert!(hosts(&with).contains(&"payments.example.com".to_string()));
    // Removing the effect removes the rule — the easiest test in the project to write, and the one
    // the platform-team pitch rests on.
    assert_eq!(
        rules(&without) + 1,
        rules(&with),
        "one effect, one egress rule in the emitted object"
    );
}

#[test]
fn a_host_outside_the_cluster_is_never_emitted_as_a_pod_selector() {
    // The defect this test exists for shipped in Phase 1 and survived Phase 2: a
    // `net.out(payments.example.com)` was rendered as `podSelector: {app: payments.example.com}`,
    // which matches no pod. The rule granted nothing, so the policy derived from the program
    // *denied the program's own network call* — and it rendered as YAML that looked like the
    // feature working. §3.5's "least-privilege infra, computed" was the claim; "no privilege at
    // all, silently" was the object.
    let g = beck_infra::derive(
        "app",
        &[
            (Effect::Ingress, "proposals".into()),
            (Effect::Durable, "todos".into()),
            (
                Effect::NetOut("payments.example.com".into()),
                "charge".into(),
            ),
        ],
        true,
    );
    let policy = rendered_policy(&g);
    let selectors = format!("{}", policy["spec"]["egress"]);
    assert!(
        !selectors.contains("payments.example.com"),
        "a DNS name is not a pod label:\n{selectors}"
    );
    // The host is recorded where a `Platform` that can enforce names will find it…
    assert_eq!(
        policy["metadata"]["annotations"]["beck.dev/egress-hosts"],
        "payments.example.com"
    );
    // …and what is emitted excludes the cluster's own address space and the metadata endpoint,
    // which is the tightest thing a core NetworkPolicy can say about an outbound call.
    let except = format!("{}", policy["spec"]["egress"]);
    for private in [
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "169.254.0.0/16",
    ] {
        assert!(
            except.contains(private),
            "{private} is not excluded:\n{except}"
        );
    }
}

#[test]
fn a_policy_that_denies_dns_denies_everything_else_too() {
    // A NetworkPolicy with `policyTypes: [Ingress, Egress]` denies every port it does not name,
    // including 53. A generated policy that forgets it produces a pod that starts, looks healthy,
    // and resolves nothing — the classic failure §6.5 warns about, and one the Phase 1 emitter
    // walked straight into.
    let g = beck_infra::derive("app", &[(Effect::Durable, "todos".into())], false);
    let policy = rendered_policy(&g);
    let egress = format!("{}", policy["spec"]["egress"]);
    assert!(egress.contains("kube-dns"), "no DNS egress:\n{egress}");
    assert!(egress.contains("\"UDP\""), "DNS is UDP first:\n{egress}");
}

/// The hosts outside the cluster the derivation recorded.
fn hosts(g: &beck_infra::InfraGraph) -> Vec<String> {
    g.nodes
        .iter()
        .find_map(|d| match &d.node {
            beck_infra::Node::Policy {
                allow_egress_hosts, ..
            } => Some(allow_egress_hosts.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The emitted NetworkPolicy, as an object.
fn rendered_policy(g: &beck_infra::InfraGraph) -> serde_json::Value {
    beck_infra::k8s::objects(g, "id")
        .into_iter()
        .find(|(_, v)| v["kind"] == "NetworkPolicy")
        .map(|(_, v)| v)
        .expect("a policy is always emitted")
}

/// How many egress rules the emitted policy has.
fn rules(g: &beck_infra::InfraGraph) -> usize {
    rendered_policy(g)["spec"]["egress"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[test]
fn the_database_grant_is_what_the_program_actually_does() {
    // A program that appends and reads and never updates gets `SELECT, INSERT` — which is a
    // statement about the fold, not a template.
    let g = beck_infra::derive(
        "app",
        &[
            (Effect::Ingress, "proposals".into()),
            (Effect::Durable, "todos".into()),
        ],
        true,
    );
    let grant = g
        .nodes
        .iter()
        .find_map(|d| match &d.node {
            beck_infra::Node::Grant { privileges, .. } => Some(privileges.clone()),
            _ => None,
        })
        .expect("a durable program has a grant");
    assert_eq!(grant, vec!["SELECT", "INSERT"]);
    assert!(!grant.iter().any(|p| p == "UPDATE" || p == "DELETE"));
}

// ---------------------------------------------------------------------------------------------
// 7. "No arbitrary build-time code"
//    Mechanism: the macro phase is capability-restricted.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_macro_cannot_reach_the_host_because_expansion_has_nothing_to_reach_it_with() {
    // The half of the property that is about the *program a macro produces*: code inside a
    // `quote:` is ordinary code, so a name for the host is refused there exactly as it is anywhere
    // else — "cannot find", like any other undefined identifier.
    //
    // The other half — what a macro *body* may do while it runs at compile time — used to be
    // satisfied by construction, because the expander was a pure `Node -> Node` function over a
    // template with no environment to reach anything from. It has an interpreter now, so the
    // property is a claim rather than a shape, and `macro_sandbox.rs` is the gate: a whitelisted
    // environment, the prelude's effectful names refused by name, and an enumeration that fails
    // when a new one is added.
    let src = "\
macro leak():
    return quote:
        read_file(\"/etc/passwd\")

def f() -> Str:
    return leak()
";
    let all = codes(src);
    assert!(
        all.contains(&"B0340"),
        "a macro reaching for the host must find nothing there: {all:?}"
    );
}

#[test]
fn a_macro_cannot_introduce_a_binding_that_captures_its_callers() {
    // The other build-time property, and the reason hygiene was built in Phase 1 rather than
    // retrofitted: a macro that could capture a caller's name could rewrite the caller's meaning,
    // which is arbitrary code execution with extra steps.
    let src = "\
macro shadow(do):
    return quote:
        tmp = 1
        $do

def f() -> Int:
    tmp = 99
    shadow():
        return tmp
";
    let (_, d, map) = compile_str("t.beck", src);
    let errs: Vec<&str> = d
        .iter()
        .filter(|x| x.severity == beck_diag::Severity::Error)
        .map(|x| x.code)
        .collect();
    assert!(
        !errs.contains(&"B0340"),
        "the caller's `tmp` must survive the macro's: {}",
        d.render(&map)
    );
}

// ---------------------------------------------------------------------------------------------
// 8. "Tamper-evident history"
//    Mechanism: state is a fold over an append-only log.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_log_store_offers_no_way_to_change_a_recorded_event() {
    // "How did this row get here" is `beck replay`, not forensics — which is only true if there is
    // no operation that edits history. The trait is the proof: append and read, and nothing else.
    //
    // The replay harness asserts the *positive* half (two folds of one log agree, and agree with
    // the live process). This asserts the half that would make that meaningless.
    // The check reads the trait definition, which is unusual and deliberate: the property is the
    // *absence* of an operation, and no value at runtime can witness an absence. The surface a
    // `LogStore` implementation must provide is what the trait says it is, so that is what is
    // asserted. Adding a method to the trait fails this test, which is the point — a new operation
    // on the log is a change to the tamper-evidence claim and should be argued for, not merged.
    let source = include_str!("../../beck-rt/src/log.rs");
    let trait_body = source
        .split_once("pub trait LogStore")
        .expect("the trait exists")
        .1
        .split_once("\n}")
        .expect("the trait ends")
        .0;
    let mut names: Vec<&str> = trait_body
        .lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("async fn")
                .or_else(|| l.trim().strip_prefix("fn"))
        })
        .filter_map(|l| l.trim().split(['(', '<']).next())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "append",
            "floor",
            "head",
            "kind",
            "put_snapshot",
            "read",
            "snapshot_at_or_before"
        ],
        "a new operation on the log is a change to the tamper-evidence claim"
    );
    for forbidden in ["update", "delete", "truncate", "rewrite", "replace", "set_"] {
        assert!(
            !names.iter().any(|n| n.contains(forbidden)),
            "`{forbidden}` has no place on an append-only log"
        );
    }
}

// ---------------------------------------------------------------------------------------------

/// A program that genuinely holds a secret, used by the tests above.
const SECRET_APP: &str = r#"
type ApiKey = newtype[secret[Str]]

model Config:
    host: Str
    key: ApiKey

model State:
    charged: Int

union Command:
    Charge(amount: Int)

union Event:
    Charged(amount: Int)

union Rejection:
    TooMuch

def load() -> Config uses env:
    return Config(host="payments.example.com", key=ApiKey(value=secret_env("API_KEY")))

def charge(c: Config, amount: Int) -> Str uses net.out(payments.example.com):
    return "receipt"

def apply_event(s: State, env: Envelope[Event]) -> State:
    match env.body:
        case Charged(amount):
            return s.with(charged=(s.charged + amount))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    match p.command:
        case Charge(amount):
            if amount > 100:
                return Err(error=TooMuch)
            return Ok(value=[Charged(amount=amount)])

def view(s: State, session: Session) -> Html:
    return ui:
        main:
            h1: "billing"
            footer: (str(s.charged) + " charged")

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, ledger, validate)
ledger: Signal[State] = durable(fold(apply_event, State(charged=0), events))
page: Signal[Html] = per_session(ledger, view)
"#;
