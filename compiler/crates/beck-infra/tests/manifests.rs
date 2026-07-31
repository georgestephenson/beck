//! What the emitted Kubernetes manifests are checked against.
//!
//! There are three kinds of wrong a generated manifest can be, and they need three mechanisms.
//! Only the first is a type system's job, and Phase 2 shipped with none of them:
//!
//! 1. **Malformed** — a misspelled field, a missing required one, a number where a string belongs.
//!    Handled in [`beck_infra::k8s`] by building `k8s-openapi` structs instead of strings, so the
//!    Rust compiler checks them against the Kubernetes OpenAPI schema. Nothing in this file tests
//!    that, because a test cannot: the program that would fail does not compile.
//! 2. **Not the YAML we meant** — well-typed objects, mangled on the way out. The writer is our own
//!    (see [`beck_infra::yaml`] for why), so it must not be trusted to check itself:
//!    [`every_object_reads_back_as_the_object_it_was`] parses every document with a third-party
//!    YAML parser and compares against the original JSON.
//! 3. **Individually valid, collectively broken** — a Service whose selector matches no pod, a
//!    `secretKeyRef` naming a Secret that is never emitted, a route sending to a port nothing
//!    listens on, a StatefulSet naming a `serviceName` that does not exist. No schema can see any
//!    of these, and every one of them is a deploy that comes up and does not work. They are the
//!    bulk of this file, and each is asserted **by walking the objects** rather than by naming the
//!    string that happens to be right today.
//!
//! And underneath all three, [`insta`] snapshots of the complete manifest set, so that any change
//! to what gets deployed is a diff a person approves rather than a surprise in a cluster.

use std::collections::{BTreeMap, BTreeSet};

use beck_infra::{graph, InfraGraph, Node};
use serde_json::Value;

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
// 2. The document says what the object said
// ---------------------------------------------------------------------------------------------

#[test]
fn every_object_reads_back_as_the_object_it_was() {
    // The writer is ours, so the parser must not be. `serde_norway` is a maintained fork of
    // `serde_yaml`; if this passes, the emitted bytes are YAML, and they are *this* YAML.
    let all = objects();
    assert!(!all.is_empty());
    for (name, value) in &all {
        let text = beck_infra::yaml::to_yaml(value);
        let back: Value = serde_norway::from_str(&text)
            .unwrap_or_else(|e| panic!("{name} is not YAML: {e}\n{text}"));
        assert_eq!(
            &back, value,
            "{name} did not survive the round trip:\n{text}"
        );
    }
}

#[test]
fn every_document_declares_a_kind_that_matches_its_api_version() {
    // A body under the wrong apiVersion is accepted by YAML and rejected by the API server. The
    // pairs are enumerated so that emitting a new kind is a decision somebody makes on purpose.
    const KNOWN: &[(&str, &str)] = &[
        ("v1", "Namespace"),
        ("v1", "Service"),
        ("v1", "Secret"),
        ("v1", "ConfigMap"),
        ("apps/v1", "Deployment"),
        ("apps/v1", "StatefulSet"),
        ("networking.k8s.io/v1", "NetworkPolicy"),
        ("gateway.networking.k8s.io/v1", "HTTPRoute"),
    ];
    for (name, value) in objects() {
        let api = value["apiVersion"].as_str().unwrap_or_default();
        let kind = value["kind"].as_str().unwrap_or_default();
        assert!(
            KNOWN.contains(&(api, kind)),
            "{name}: `{api}`/`{kind}` is not a pair this emitter is known to produce"
        );
    }
}

#[test]
fn every_namespaced_object_says_which_namespace() {
    // An object with no namespace lands in whatever `kubectl` was pointed at, which is how a
    // manifest set that works on a laptop deletes something in production.
    let app = infra().app;
    for (name, value) in objects() {
        if value["kind"] == "Namespace" {
            continue;
        }
        assert_eq!(
            value["metadata"]["namespace"].as_str(),
            Some(app.as_str()),
            "{name} does not name its namespace"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The objects against each other — what no schema can check
// ---------------------------------------------------------------------------------------------

/// Every `(kind, name)` the emitter produced.
fn emitted() -> BTreeSet<(String, String)> {
    objects()
        .into_iter()
        .map(|(_, v)| {
            (
                v["kind"].as_str().unwrap_or_default().to_string(),
                v["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The pod template labels of every workload, by the object that owns them.
fn pod_templates() -> Vec<(String, BTreeMap<String, String>)> {
    objects()
        .into_iter()
        .filter(|(_, v)| v["kind"] == "Deployment" || v["kind"] == "StatefulSet")
        .map(|(_, v)| {
            let name = v["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (
                name,
                string_map(&v["spec"]["template"]["metadata"]["labels"]),
            )
        })
        .collect()
}

fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn selects(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    !selector.is_empty() && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
}

#[test]
fn every_workloads_selector_matches_the_pods_it_creates() {
    // The one the API server accepts and the cluster never recovers from: a Deployment whose
    // `matchLabels` does not match its own template's labels rolls out zero ready replicas and
    // reports nothing wrong. It is unrepresentable here because both maps are built by one
    // function — this test is what keeps that true.
    let workloads: Vec<(String, Value)> = objects()
        .into_iter()
        .filter(|(_, v)| v["kind"] == "Deployment" || v["kind"] == "StatefulSet")
        .collect();
    assert!(!workloads.is_empty(), "no workload to check");
    for (name, v) in workloads {
        let selector = string_map(&v["spec"]["selector"]["matchLabels"]);
        let labels = string_map(&v["spec"]["template"]["metadata"]["labels"]);
        assert!(
            selects(&selector, &labels),
            "{name}: selector {selector:?} matches none of its own pods {labels:?}"
        );
    }
}

#[test]
fn every_service_selects_pods_that_something_actually_creates() {
    // A Service with no endpoints is valid YAML, valid to the API server, and a 503.
    let services: Vec<(String, Value)> = objects()
        .into_iter()
        .filter(|(_, v)| v["kind"] == "Service")
        .collect();
    assert!(services.len() >= 2, "expected the app and the log store");
    let templates = pod_templates();
    for (name, v) in services {
        let selector = string_map(&v["spec"]["selector"]);
        assert!(
            templates
                .iter()
                .any(|(_, labels)| selects(&selector, labels)),
            "{name}: selector {selector:?} matches no pod any workload creates. \
             Pods: {templates:?}"
        );
    }
}

#[test]
fn every_service_sends_traffic_to_a_port_a_container_listens_on() {
    // Three numbers that have to agree — `containerPort`, `targetPort`, and the route's
    // `backendRefs.port` — each of which reads as correct on its own line.
    let all = objects();
    let container_ports: BTreeSet<i64> = all
        .iter()
        .filter(|(_, v)| v["kind"] == "Deployment" || v["kind"] == "StatefulSet")
        .flat_map(|(_, v)| {
            v["spec"]["template"]["spec"]["containers"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .flat_map(|c| c["ports"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| p["containerPort"].as_i64())
        .collect();
    assert!(!container_ports.is_empty());

    for (name, v) in all.iter().filter(|(_, v)| v["kind"] == "Service") {
        for p in v["spec"]["ports"].as_array().cloned().unwrap_or_default() {
            let target = p["targetPort"].as_i64().expect("a numeric targetPort");
            assert!(
                container_ports.contains(&target),
                "{name}: targets port {target}, which no container listens on: {container_ports:?}"
            );
        }
    }
}

#[test]
fn the_route_sends_to_a_service_that_exists_on_a_port_that_service_exposes() {
    let all = objects();
    let route = all
        .iter()
        .find(|(_, v)| v["kind"] == "HTTPRoute")
        .map(|(_, v)| v.clone())
        .expect("`ingress` implies a route");
    let services: BTreeMap<String, BTreeSet<i64>> = all
        .iter()
        .filter(|(_, v)| v["kind"] == "Service")
        .map(|(_, v)| {
            (
                v["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                v["spec"]["ports"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| p["port"].as_i64())
                    .collect(),
            )
        })
        .collect();

    let rules = route["spec"]["rules"].as_array().expect("rules");
    assert!(!rules.is_empty());
    for rule in rules {
        for backend in rule["backendRefs"].as_array().expect("backendRefs") {
            let name = backend["name"].as_str().expect("a backend name");
            let port = backend["port"].as_i64().expect("a backend port");
            let ports = services
                .get(name)
                .unwrap_or_else(|| panic!("the route sends to `{name}`, which is not emitted"));
            assert!(
                ports.contains(&port),
                "the route sends to {name}:{port}, which that Service does not expose: {ports:?}"
            );
        }
    }
}

#[test]
fn every_secret_key_ref_names_a_secret_that_is_emitted_and_a_key_it_holds() {
    // docs/19 §19.5's third defect, generalised: a manifest naming an object nobody emits. A pod
    // whose `secretKeyRef` misses stays in `CreateContainerConfigError` forever.
    let all = objects();
    let secrets: BTreeMap<String, BTreeSet<String>> = all
        .iter()
        .filter(|(_, v)| v["kind"] == "Secret")
        .map(|(_, v)| {
            (
                v["metadata"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                v["stringData"]
                    .as_object()
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default(),
            )
        })
        .collect();

    let mut checked = 0;
    for (file, v) in &all {
        for c in v["spec"]["template"]["spec"]["containers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            for e in c["env"].as_array().cloned().unwrap_or_default() {
                let Some(r) = e["valueFrom"]["secretKeyRef"].as_object() else {
                    continue;
                };
                let name = r["name"].as_str().expect("a secret name");
                let key = r["key"].as_str().expect("a secret key");
                let keys = secrets
                    .get(name)
                    .unwrap_or_else(|| panic!("{file}: `{name}` is referenced but never emitted"));
                assert!(
                    keys.contains(key),
                    "{file}: `{name}` has no key `{key}`: {keys:?}"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 2,
        "expected the app and the log store to read credentials"
    );
}

#[test]
fn a_stateful_sets_service_name_resolves_to_a_headless_service() {
    // docs/19 §19.5's fourth defect. A StatefulSet's `serviceName` must name a *headless* Service,
    // or its pods get no stable DNS and the fold cannot find the log across a restart.
    let all = objects();
    let headless: BTreeSet<String> = all
        .iter()
        .filter(|(_, v)| v["kind"] == "Service" && v["spec"]["clusterIP"] == "None")
        .map(|(_, v)| {
            v["metadata"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let sets: Vec<&Value> = all
        .iter()
        .filter(|(_, v)| v["kind"] == "StatefulSet")
        .map(|(_, v)| v)
        .collect();
    assert!(!sets.is_empty(), "the durable fold implies a StatefulSet");
    for v in sets {
        let want = v["spec"]["serviceName"].as_str().expect("a serviceName");
        assert!(
            headless.contains(want),
            "serviceName `{want}` is not a headless Service: {headless:?}"
        );
    }
}

#[test]
fn the_url_in_the_credentials_resolves_to_the_log_stores_own_service() {
    // The default credentials have to work from `git clone` (§6.6 rung 3), which means the host in
    // the URL is a Service that exists — not a plausible-looking string.
    let all = objects();
    let url = all
        .iter()
        .filter(|(_, v)| v["kind"] == "Secret")
        .find_map(|(_, v)| v["stringData"]["url"].as_str().map(str::to_string))
        .expect("the log credentials hold a url");
    let host = url
        .rsplit_once('@')
        .and_then(|(_, rest)| rest.split(':').next())
        .expect("a host in the url")
        .to_string();
    let (service, rest) = host.split_once('.').expect("a service-qualified host");
    assert!(
        emitted().contains(&("Service".to_string(), service.to_string())),
        "the credentials point at `{service}`, which is not emitted"
    );
    assert_eq!(rest, format!("{}.svc", infra().app), "{url}");

    let port: i64 = url
        .rsplit(':')
        .next()
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse().ok())
        .expect("a port in the url");
    let exposed: BTreeSet<i64> = all
        .iter()
        .filter(|(_, v)| v["metadata"]["name"] == service)
        .flat_map(|(_, v)| v["spec"]["ports"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| p["port"].as_i64())
        .collect();
    assert!(
        exposed.contains(&port),
        "{url} names a port {service} does not expose"
    );
}

#[test]
fn every_egress_peer_is_a_pod_that_exists_or_an_address_range_that_is_not_the_cluster() {
    // §3.5's "least-privilege infra, computed", checked against the emitted object rather than
    // against the graph that produced it. This is the test that would have caught Phase 1's
    // `podSelector: {app: payments.example.com}`: a DNS name is not a pod label, and a rule whose
    // selector matches nothing grants nothing — while rendering as a policy that looks like it
    // works.
    let policy = objects()
        .into_iter()
        .find(|(_, v)| v["kind"] == "NetworkPolicy")
        .map(|(_, v)| v)
        .expect("a policy is always emitted");
    let templates = pod_templates();
    let mut pods = 0;
    let mut blocks = 0;
    let mut dns = 0;
    for rule in policy["spec"]["egress"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        // Every rule names its ports. An egress rule with no `ports` opens all of them.
        assert!(
            rule["ports"].as_array().is_some_and(|p| !p.is_empty()),
            "an egress rule with no ports is not least privilege: {rule}"
        );
        for to in rule["to"].as_array().cloned().unwrap_or_default() {
            if to["ipBlock"].is_object() {
                let except = format!("{}", to["ipBlock"]["except"]);
                for private in [
                    "10.0.0.0/8",
                    "172.16.0.0/12",
                    "192.168.0.0/16",
                    "169.254.0.0/16",
                ] {
                    assert!(
                        except.contains(private),
                        "public egress must not reach the cluster or the metadata endpoint: {to}"
                    );
                }
                blocks += 1;
                continue;
            }
            let sel = string_map(&to["podSelector"]["matchLabels"]);
            if sel.get("k8s-app").is_some_and(|a| a == "kube-dns") {
                dns += 1;
                continue;
            }
            assert!(
                templates.iter().any(|(_, labels)| selects(&sel, labels)),
                "the policy allows egress to {sel:?}, which matches no pod: {templates:?}"
            );
            pods += 1;
        }
    }
    assert_eq!(dns, 1, "exactly one DNS rule, and it is not optional");
    assert!(pods >= 1, "expected the log store to be an egress peer");
    assert_eq!(
        blocks, 1,
        "the program's `net.out` should have opened public egress"
    );
}

#[test]
fn the_namespace_the_policy_admits_is_the_namespace_the_route_comes_from() {
    // Two objects, one fact. The route points the gateway at this workload; the policy is what
    // lets the gateway's packets in. Phase 1 wrote `gateway-system` in one and `gateway` in the
    // other, so the policy admitted a namespace that does not exist — a deploy that applies
    // cleanly, comes up healthy and serves nothing.
    let all = objects();
    let route = all
        .iter()
        .find(|(_, v)| v["kind"] == "HTTPRoute")
        .map(|(_, v)| v.clone())
        .expect("`ingress` implies a route");
    let from: BTreeSet<String> = route["spec"]["parentRefs"]
        .as_array()
        .expect("parentRefs")
        .iter()
        .map(|p| p["namespace"].as_str().unwrap_or_default().to_string())
        .collect();

    let policy = all
        .iter()
        .find(|(_, v)| v["kind"] == "NetworkPolicy")
        .map(|(_, v)| v.clone())
        .expect("a policy is always emitted");
    let admitted: BTreeSet<String> = policy["spec"]["ingress"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|r| r["from"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| {
            p["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
                .as_str()
                .map(str::to_string)
        })
        .collect();

    assert!(!from.is_empty() && !admitted.is_empty());
    assert_eq!(
        admitted, from,
        "the policy admits {admitted:?} and the gateway is in {from:?}"
    );
}

#[test]
fn the_policy_selects_the_workload_it_is_meant_to_constrain() {
    // A NetworkPolicy whose `podSelector` matches nothing constrains nothing — and looks exactly
    // like a policy that is working.
    let policy = objects()
        .into_iter()
        .find(|(_, v)| v["kind"] == "NetworkPolicy")
        .map(|(_, v)| v)
        .expect("a policy is always emitted");
    let sel = string_map(&policy["spec"]["podSelector"]["matchLabels"]);
    let templates = pod_templates();
    assert!(
        templates.iter().any(|(_, labels)| selects(&sel, labels)),
        "the policy selects {sel:?}, which is no workload's pods: {templates:?}"
    );
}

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
