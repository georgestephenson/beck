//! What the emitted Kubernetes manifests are checked against, on the canonical program.
//!
//! There are three kinds of wrong a generated manifest can be, and they need three mechanisms.
//! Phase 2 shipped with none of them:
//!
//! 1. **Malformed** — a misspelled field, a missing required one, a number where a string belongs.
//!    Handled in [`beck_infra::k8s`] by building `k8s-openapi` structs instead of strings, so the
//!    Rust compiler checks them against the Kubernetes OpenAPI schema. Nothing here tests that,
//!    because a test cannot: the program that would fail does not compile.
//! 2. **Not the YAML we meant** — well-typed objects, mangled on the way out. The writer is our own
//!    (see [`beck_infra::yaml`] for why), so it is not trusted to check itself: every document is
//!    parsed back with a third-party YAML parser and compared against the original JSON.
//! 3. **Individually valid, collectively broken** — a Service whose selector matches no pod, a
//!    `secretKeyRef` naming a Secret that is never emitted, a route sending to a port nothing
//!    listens on. No schema can see any of these, and every one is a deploy that comes up and does
//!    not work.
//!
//! The checks for (2) and (3) live in [`invariants`], because they are also what
//! [`manifest_properties.rs`](manifest_properties.rs) runs over generated graphs. This file calls
//! them one per test on the program a reader can open, and adds the things only a *known* program
//! can assert — that the container runs the path the package installs, that deleting an effect
//! deletes a manifest — plus an `insta` snapshot of the complete set, so a change to what gets
//! deployed is a diff somebody approves.

use std::collections::BTreeMap;

use beck_infra::{graph, InfraGraph, Node, Platform};
use serde_json::Value;

mod invariants;

/// A program that exercises every derivation: ingress, a durable fold, a client-placed page.
const PROGRAM: &str = r#"
union Command:
    Ping

union Event:
    Pinged

union Rejection:
    No

model State:
    n: Int

def apply_event(s: State, env: Envelope[Event]) -> State:
    return s.with(n=(s.n + 1))

def validate(s: State, p: Proposal) -> Result[list[Event], Rejection]:
    return Ok(value=[Pinged])

def notify(s: State) -> Bool uses net.out(hooks.example.com):
    return True

def view(s: State, session: Session) -> Html:
    return ui:
        main: str(s.n)

proposals: Stream[Proposal] = merge_clients()
events: Stream[Event] = decide(proposals, st, validate)
st: Signal[State] = durable(fold(apply_event, State(n=0), events))
page: Signal[Html] = per_session(st, view)
"#;

fn infra() -> InfraGraph {
    let (placed, d, map) = beck_core::compile_str("app.beck", PROGRAM);
    assert!(!d.has_errors(), "{}", d.render(&map));
    graph(&placed.expect("it compiles"))
}

const WIRE_ID: &str = "0123456789abcdef";

fn objects() -> Vec<(String, Value)> {
    beck_infra::k8s::objects(&infra(), WIRE_ID)
}

// ---------------------------------------------------------------------------------------------
// The invariants, one test each
// ---------------------------------------------------------------------------------------------
//
// Every check lives in [`invariants`] and is called from exactly two places: here, on the
// canonical program, so a failure names the property rather than "the manifests are wrong"; and
// `manifest_properties.rs`, on generated graphs, so the claim covers programs nobody wrote. There
// is no third copy, because two copies of an invariant is one invariant and one comment.

macro_rules! invariant {
    ($name:ident, $why:expr) => {
        #[test]
        fn $name() {
            if let Err(e) = invariants::$name(&objects()) {
                panic!("{}\n\n{}", e, $why);
            }
        }
    };
}

invariant!(
    yaml_round_trips,
    "the writer is ours, so the parser must not be: every document is read back with `serde_norway`"
);
invariant!(
    kinds_are_known,
    "a body under the wrong apiVersion is accepted by YAML and rejected by the API server"
);
invariant!(
    names_are_legal,
    "RFC 1123: 63 characters, and `<app>-log-credentials` is sixteen longer than `<app>`"
);
invariant!(
    everything_is_namespaced,
    "an object with no namespace lands in whatever `kubectl` was pointed at"
);
invariant!(
    file_names_are_unique_and_sorted,
    "`kubectl apply -f <dir>` reads files in lexical order, so the prefixes have to carry it"
);
invariant!(
    workload_selectors_match_their_own_pods,
    "a Deployment whose matchLabels miss its own template rolls out zero replicas and says nothing"
);
invariant!(
    service_selectors_match_some_pod,
    "a Service with no endpoints is valid, admissible, and a 503"
);
invariant!(
    services_target_a_container_port,
    "containerPort, targetPort and the route's backend port all have to agree"
);
invariant!(
    routes_resolve_to_a_service_and_port,
    "a route pointing at a Service nobody emits is a 404 the manifests describe in full"
);
invariant!(
    secret_refs_resolve,
    "a pod whose secretKeyRef misses stays in CreateContainerConfigError forever (docs/19 §19.5)"
);
invariant!(
    stateful_sets_name_a_headless_service,
    "without a headless Service the log store's pods get no stable DNS (docs/19 §19.5)"
);
invariant!(
    credentials_point_at_an_emitted_service,
    "§6.6 rung 3 wants the default credentials to work from `git clone`"
);
invariant!(
    egress_peers_are_real,
    "the check that catches `podSelector: {app: payments.example.com}` — a DNS name is not a label"
);
invariant!(
    ingress_matches_the_routes_gateway,
    "the gateway sends the traffic and the policy decides whether it arrives"
);
invariant!(
    policy_selects_a_workload,
    "a policy whose podSelector matches nothing constrains nothing, and looks like it works"
);

// ---------------------------------------------------------------------------------------------
// What the canonical program says that a generated one cannot
// ---------------------------------------------------------------------------------------------

#[test]
fn the_container_runs_the_program_the_image_ships() {
    // The two ends of docs/19 §19.5's first defect, checked against each other: the melange
    // pipeline installs the program at a path, and the container is told to run that same path.
    let g = infra();
    let melange = beck_infra::k8s::melange(&g);
    assert!(melange.contains(beck_infra::k8s::APP_SOURCE), "{melange}");

    let workload = beck_infra::k8s::objects(&g, WIRE_ID)
        .into_iter()
        .find(|(_, v)| v["kind"] == "Deployment")
        .map(|(_, v)| v)
        .expect("a workload exists");
    let args: Vec<String> = workload["spec"]["template"]["spec"]["containers"][0]["args"]
        .as_array()
        .expect("the container is given arguments")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        args.contains(&beck_infra::k8s::APP_SOURCE.to_string()),
        "the container must run the program the package installed: {args:?}"
    );
    // …and listen on the address its Service targets.
    assert!(
        args.contains(&format!("0.0.0.0:{}", beck_infra::k8s::APP_PORT)),
        "{args:?}"
    );
}

#[test]
fn removing_an_effect_removes_the_object_and_the_manifest_with_it() {
    // The derivation claim, restated at the level of emitted YAML: this is what a reviewer sees
    // change in a pull request when a `net.out` is deleted from the program.
    let with_durable = beck_infra::derive(
        "app",
        &[
            (beck_core::Effect::Ingress, "proposals".to_string()),
            (beck_core::Effect::Durable, "st".to_string()),
        ],
        true,
    );
    let without = beck_infra::derive(
        "app",
        &[(beck_core::Effect::Ingress, "proposals".to_string())],
        true,
    );
    let kinds = |g: &InfraGraph| -> Vec<String> {
        beck_infra::k8s::objects(g, WIRE_ID)
            .into_iter()
            .map(|(_, v)| v["kind"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert!(kinds(&with_durable).contains(&"StatefulSet".to_string()));
    assert!(!kinds(&without).contains(&"StatefulSet".to_string()));
    assert!(!kinds(&without).contains(&"Secret".to_string()));
    assert!(kinds(&without).contains(&"HTTPRoute".to_string()));
}

/// §6.5's read-only root filesystem, derived from the program's own row.
///
/// This is the assertion the golden file cannot make, because a golden file records one program.
/// The claim is that the flag *moves*: the same derivation with `fs.write` in the row emits a
/// writable root filesystem and without it emits a read-only one. It could not be made at all until
/// `docs/81` split `fs` into a read and a write — one atom naming a path cannot say whether the
/// program writes — and `fs.read` deliberately does **not** move it, which is the second assertion.
#[test]
fn the_root_filesystem_is_read_only_unless_the_program_says_it_writes() {
    let root_is_read_only = |effects: &[(beck_core::Effect, String)]| -> Option<bool> {
        let graph = beck_infra::derive("app", effects, true);
        beck_infra::k8s::objects(&graph, WIRE_ID)
            .into_iter()
            .find(|(_, v)| v["kind"] == "Deployment")
            .and_then(|(_, v)| {
                v["spec"]["template"]["spec"]["containers"][0]["securityContext"]
                    ["readOnlyRootFilesystem"]
                    .as_bool()
            })
    };
    let ingress = (beck_core::Effect::Ingress, "proposals".to_string());
    let path = || std::sync::Arc::from("/var/lib/app");

    assert_eq!(
        root_is_read_only(std::slice::from_ref(&ingress)),
        Some(true)
    );
    assert_eq!(
        root_is_read_only(&[
            ingress.clone(),
            (beck_core::Effect::FsRead(path()), "load".to_string()),
        ]),
        Some(true),
        "reading a file is not a reason to make the root filesystem writable"
    );
    assert_eq!(
        root_is_read_only(&[
            ingress,
            (beck_core::Effect::FsWrite(path()), "save".to_string()),
        ]),
        Some(false),
        "a program that writes needs somewhere to write"
    );
}

/// The three constants are on **every** container, including the substrate's.
///
/// They cost a third-party image nothing — nothing Beck deploys needs a Linux capability, needs to
/// gain privileges, or needs a syscall outside the runtime default. `readOnlyRootFilesystem` is the
/// one that is not universal, and its absence from the substrate's container is asserted here so
/// the asymmetry is a decision rather than an oversight (`docs/82` §82.3).
#[test]
fn every_container_drops_its_capabilities_and_refuses_privilege_escalation() {
    let mut containers = 0;
    for (name, v) in objects() {
        let Some(specs) = v["spec"]["template"]["spec"]["containers"].as_array() else {
            continue;
        };
        for c in specs {
            containers += 1;
            let sc = &c["securityContext"];
            assert_eq!(sc["allowPrivilegeEscalation"], false, "{name}");
            assert_eq!(sc["capabilities"]["drop"][0], "ALL", "{name}");
            assert_eq!(sc["seccompProfile"]["type"], "RuntimeDefault", "{name}");
        }
    }
    assert_eq!(containers, 2, "the app and the substrate");

    // …and the substrate's root filesystem is *not* claimed to be read-only: Postgres writes its
    // socket and its temporary files outside the volume, and no Beck effect row knows that.
    let substrate = objects()
        .into_iter()
        .find(|(_, v)| v["kind"] == "StatefulSet")
        .expect("a durable fold emits one");
    assert!(
        substrate.1["spec"]["template"]["spec"]["containers"][0]["securityContext"]
            ["readOnlyRootFilesystem"]
            .is_null(),
        "the substrate's image is not ours to make claims about"
    );
}

// ---------------------------------------------------------------------------------------------
// The golden files
// ---------------------------------------------------------------------------------------------

#[test]
fn the_manifest_set_is_what_it_was() {
    // Everything above says the manifests are *consistent*. This says they are *the same ones* —
    // so that changing what a deploy contains is a reviewed diff. Update with `cargo insta review`
    // after reading what moved.
    let files = beck_infra::render(&infra(), WIRE_ID);
    let mut all = String::new();
    for (name, body) in &files {
        all.push_str(&format!("# {name}\n---\n{body}\n"));
    }
    insta::assert_snapshot!("manifests", all);
}

#[test]
fn the_compose_file_is_what_it_was() {
    // The second platform gets a golden file too, for the same reason the first does: what a
    // person deploys should change only when somebody approved the diff.
    let g = infra();
    let mut all = String::new();
    for (name, body) in beck_infra::compose::Compose.manifests(&g, WIRE_ID) {
        all.push_str(&format!("# {name}\n---\n{body}\n"));
    }
    all.push_str("\n# explain.txt (platform section)\n");
    let explain = g.explain_for(&beck_infra::compose::Compose);
    all.push_str(
        explain
            .split("platform: ")
            .nth(1)
            .expect("a platform section"),
    );
    insta::assert_snapshot!("compose", all);
}

#[test]
fn the_manifests_are_byte_identical_across_runs() {
    // Golden files and GitOps both need this, and a `HashMap` anywhere in the emitter would break
    // it silently.
    let a = beck_infra::render(&infra(), WIRE_ID);
    let b = beck_infra::render(&infra(), WIRE_ID);
    assert_eq!(a, b);
}

#[test]
fn the_file_names_apply_in_dependency_order() {
    // `kubectl apply -f dir/` applies in lexical file order, so the namespace has to sort first and
    // nothing may reference an object emitted after it.
    let g = infra();
    let files = beck_infra::render(&g, WIRE_ID);
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "the emitter's order is not apply order");
    assert!(names[0].contains("namespace"), "{names:?}");

    // And the emitted order is a topological order of the `needs` edges: an object is applied
    // only after everything it references. Kubernetes would converge either way — this is the
    // difference between coming up and coming up after a few CrashLoopBackOffs.
    let order = beck_infra::k8s::apply_order(&g);
    let position: BTreeMap<String, usize> = order
        .iter()
        .enumerate()
        .map(|(i, d)| (beck_infra::id_of(&d.node), i))
        .collect();
    let mut checked = 0;
    for (i, d) in order.iter().enumerate() {
        for need in &d.needs {
            let at = position[need];
            assert!(
                at < i,
                "{} is applied at {i} but needs {need} at {at}",
                beck_infra::id_of(&d.node)
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 4,
        "nothing references anything, so this proves nothing"
    );
}

#[test]
fn every_kind_of_node_produces_a_manifest_except_the_one_that_is_not_an_object() {
    // A `Node` variant the emitter forgets is not a crash — it is an object that quietly does not
    // get deployed. `objects()` matches exhaustively, so adding a variant does not compile until
    // somebody decides what it renders to; this checks the other direction, that every variant
    // there *is* renders to something.
    //
    // `Image` is the deliberate exception: it is an apko config, not a cluster object.
    let every = InfraGraph {
        app: "app".to_string(),
        nodes: [
            Node::Namespace { name: "app".into() },
            Node::Image {
                name: "app:dev".into(),
                entrypoint: "/usr/bin/beck".into(),
            },
            Node::Workload {
                name: "app".into(),
                replicas: 1,
                serves_ui: true,
                reads_log: true,
                writes_files: false,
                reads_identity: false,
            },
            Node::Route {
                name: "app-route".into(),
                host: "app.example".into(),
                websocket: true,
            },
            Node::Service {
                name: "app".into(),
                selector: "app".into(),
                port: 8080,
                headless: false,
            },
            Node::LogStore {
                name: "app-log".into(),
                volume_gb: 10,
            },
            Node::SnapshotSchedule {
                name: "app-snapshots".into(),
                every_events: 1000,
            },
            Node::Secret {
                name: "app-log-credentials".into(),
                keys: vec!["url".into()],
            },
            Node::Policy {
                name: "app-policy".into(),
                allow_ingress_from: vec!["gateway".into()],
                allow_egress_to: vec![beck_infra::Peer {
                    app: "app-log".into(),
                    port: 5432,
                }],
                allow_egress_hosts: vec!["hooks.example.com".into()],
            },
            Node::Grant {
                role: "app-app".into(),
                on: "beck_log".into(),
                privileges: vec!["SELECT".into()],
            },
        ]
        .into_iter()
        .map(|node| beck_infra::Derived {
            node,
            because: "under test".into(),
            from: None,
            needs: Vec::new(),
        })
        .collect(),
    };
    let rendered = beck_infra::k8s::objects(&every, WIRE_ID);
    assert_eq!(
        rendered.len(),
        every.nodes.len() - 1,
        "every node but `Image` is a cluster object: {:?}",
        rendered.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    for (name, v) in &rendered {
        assert!(
            v["kind"].is_string(),
            "{name} rendered to something with no kind"
        );
    }
}
