//! The properties every emitted manifest set must have, as functions rather than as tests.
//!
//! Written once and used twice, which is the point:
//!
//! * [`manifests.rs`](../manifests.rs) calls them one per `#[test]` on the canonical program, so a
//!   failure names the property that broke rather than "something is wrong with the manifests";
//! * [`manifest_properties.rs`](../manifest_properties.rs) calls [`all`] on thousands of generated
//!   graphs, so the claim stops being "these manifests are consistent" and becomes "the emitter
//!   cannot produce inconsistent manifests".
//!
//! Each returns `Result<(), String>` rather than asserting, because a `proptest` shrinker needs a
//! failure it can carry rather than a panic it has to catch, and because the message is the whole
//! value of the check.
//!
//! # Why these and not others
//!
//! Every one of them is a **cross-object** property: something no schema can see, because it is
//! about two objects agreeing. A field being well-typed is the Rust compiler's job and is not
//! restated here. A Service whose selector matches no pod is well-typed, admissible, and a 503.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// A manifest set: the file name and the object it holds.
pub type Objects = [(String, Value)];

/// Every check, in one call. Order is deliberate — the cheap structural ones first, so a generated
/// counterexample fails on the simplest thing wrong with it.
pub fn all(objects: &Objects) -> Result<(), String> {
    yaml_round_trips(objects)?;
    kinds_are_known(objects)?;
    names_are_legal(objects)?;
    everything_is_namespaced(objects)?;
    file_names_are_unique_and_sorted(objects)?;
    workload_selectors_match_their_own_pods(objects)?;
    service_selectors_match_some_pod(objects)?;
    services_target_a_container_port(objects)?;
    routes_resolve_to_a_service_and_port(objects)?;
    secret_refs_resolve(objects)?;
    stateful_sets_name_a_headless_service(objects)?;
    credentials_point_at_an_emitted_service(objects)?;
    egress_peers_are_real(objects)?;
    ingress_matches_the_routes_gateway(objects)?;
    policy_selects_a_workload(objects)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------------------------

/// The writer is ours, so the parser must not be.
pub fn yaml_round_trips(objects: &Objects) -> Result<(), String> {
    for (name, value) in objects {
        let text = beck_infra::yaml::to_yaml(value);
        let back: Value = serde_norway::from_str(&text)
            .map_err(|e| format!("{name} is not YAML: {e}\n{text}"))?;
        if &back != value {
            return Err(format!("{name} did not survive the round trip:\n{text}"));
        }
    }
    Ok(())
}

/// A body under the wrong apiVersion is accepted by YAML and rejected by the API server.
pub fn kinds_are_known(objects: &Objects) -> Result<(), String> {
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
    for (name, value) in objects {
        let api = str_at(value, &["apiVersion"]);
        let kind = str_at(value, &["kind"]);
        if !KNOWN.contains(&(api.as_str(), kind.as_str())) {
            return Err(format!(
                "{name}: `{api}`/`{kind}` is not a pair this emitter is known to produce"
            ));
        }
    }
    Ok(())
}

/// RFC 1123: at most 63 characters, lowercase alphanumerics and dashes, alphanumeric at each end.
///
/// The rule that is easy to satisfy for the app name and easy to lose for everything derived from
/// it: `<app>-log-credentials` is sixteen characters longer than `<app>`.
pub fn names_are_legal(objects: &Objects) -> Result<(), String> {
    for (file, value) in objects {
        let name = str_at(value, &["metadata", "name"]);
        legal_label(&name).map_err(|why| format!("{file}: `{name}` {why}"))?;
        if let Some(ns) = value["metadata"]["namespace"].as_str() {
            legal_label(ns).map_err(|why| format!("{file}: namespace `{ns}` {why}"))?;
        }
        // Label *values* have the same shape, and every object carries some.
        for (k, v) in string_map(&value["metadata"]["labels"]) {
            legal_label(&v).map_err(|why| format!("{file}: label `{k}` is `{v}`, which {why}"))?;
        }
    }
    Ok(())
}

fn legal_label(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("is empty".to_string());
    }
    if s.len() > beck_infra::MAX_NAME {
        return Err(format!(
            "is {} characters, and the limit is {}",
            s.len(),
            beck_infra::MAX_NAME
        ));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("has a character outside [a-z0-9-]".to_string());
    }
    if s.starts_with('-') || s.ends_with('-') {
        return Err("starts or ends with a dash".to_string());
    }
    Ok(())
}

/// An object with no namespace lands in whatever `kubectl` was pointed at.
pub fn everything_is_namespaced(objects: &Objects) -> Result<(), String> {
    let namespaces: BTreeSet<String> = objects
        .iter()
        .filter(|(_, v)| v["kind"] == "Namespace")
        .map(|(_, v)| str_at(v, &["metadata", "name"]))
        .collect();
    for (file, value) in objects {
        if value["kind"] == "Namespace" {
            continue;
        }
        let ns = value["metadata"]["namespace"]
            .as_str()
            .ok_or_else(|| format!("{file} does not name its namespace"))?;
        if !namespaces.is_empty() && !namespaces.contains(ns) {
            return Err(format!(
                "{file} is in `{ns}`, which this manifest set does not create: {namespaces:?}"
            ));
        }
    }
    Ok(())
}

/// `kubectl apply -f <dir>` reads files in lexical order, so the prefixes have to carry it.
pub fn file_names_are_unique_and_sorted(objects: &Objects) -> Result<(), String> {
    let names: Vec<&str> = objects.iter().map(|(n, _)| n.as_str()).collect();
    let unique: BTreeSet<&str> = names.iter().copied().collect();
    if unique.len() != names.len() {
        return Err(format!("two objects share a file name: {names:?}"));
    }
    let mut sorted = names.clone();
    sorted.sort();
    if names != sorted {
        return Err(format!(
            "emission order is not lexical order, so `kubectl apply -f` would reorder it: {names:?}"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Objects against each other
// ---------------------------------------------------------------------------------------------

/// A Deployment whose `matchLabels` does not match its own template rolls out zero ready replicas
/// and reports nothing wrong.
pub fn workload_selectors_match_their_own_pods(objects: &Objects) -> Result<(), String> {
    for (file, v) in workloads(objects) {
        let selector = string_map(&v["spec"]["selector"]["matchLabels"]);
        let labels = string_map(&v["spec"]["template"]["metadata"]["labels"]);
        if !selects(&selector, &labels) {
            return Err(format!(
                "{file}: selector {selector:?} matches none of its own pods {labels:?}"
            ));
        }
    }
    Ok(())
}

/// A Service with no endpoints is valid, admissible, and a 503.
pub fn service_selectors_match_some_pod(objects: &Objects) -> Result<(), String> {
    let templates = pod_templates(objects);
    for (file, v) in of_kind(objects, "Service") {
        let selector = string_map(&v["spec"]["selector"]);
        if !templates
            .iter()
            .any(|(_, labels)| selects(&selector, labels))
        {
            return Err(format!(
                "{file}: selector {selector:?} matches no pod any workload creates: {templates:?}"
            ));
        }
    }
    Ok(())
}

/// Three numbers that have to agree, each of which reads as correct on its own line.
pub fn services_target_a_container_port(objects: &Objects) -> Result<(), String> {
    let ports = container_ports(objects);
    for (file, v) in of_kind(objects, "Service") {
        for p in array(&v["spec"]["ports"]) {
            let target = p["targetPort"]
                .as_i64()
                .ok_or_else(|| format!("{file}: a targetPort that is not a number: {p}"))?;
            if !ports.contains(&target) {
                return Err(format!(
                    "{file}: targets port {target}, which no container listens on: {ports:?}"
                ));
            }
        }
    }
    Ok(())
}

/// A route pointing at a Service nobody emits is a 404 the manifests describe in full.
pub fn routes_resolve_to_a_service_and_port(objects: &Objects) -> Result<(), String> {
    let services: BTreeMap<String, BTreeSet<i64>> = of_kind(objects, "Service")
        .into_iter()
        .map(|(_, v)| {
            (
                str_at(&v, &["metadata", "name"]),
                array(&v["spec"]["ports"])
                    .iter()
                    .filter_map(|p| p["port"].as_i64())
                    .collect(),
            )
        })
        .collect();
    for (file, route) in of_kind(objects, "HTTPRoute") {
        let rules = array(&route["spec"]["rules"]);
        if rules.is_empty() {
            return Err(format!("{file}: a route with no rules routes nothing"));
        }
        for rule in rules {
            for backend in array(&rule["backendRefs"]) {
                let name = str_at(&backend, &["name"]);
                let port = backend["port"].as_i64().unwrap_or_default();
                let ports = services
                    .get(&name)
                    .ok_or_else(|| format!("{file}: sends to `{name}`, which is not emitted"))?;
                if !ports.contains(&port) {
                    return Err(format!(
                        "{file}: sends to {name}:{port}, which that Service does not expose: \
                         {ports:?}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// A pod whose `secretKeyRef` misses stays in `CreateContainerConfigError` forever.
pub fn secret_refs_resolve(objects: &Objects) -> Result<(), String> {
    let secrets: BTreeMap<String, BTreeSet<String>> = of_kind(objects, "Secret")
        .into_iter()
        .map(|(_, v)| {
            (
                str_at(&v, &["metadata", "name"]),
                v["stringData"]
                    .as_object()
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default(),
            )
        })
        .collect();
    for (file, v) in workloads(objects) {
        for c in array(&v["spec"]["template"]["spec"]["containers"]) {
            for e in array(&c["env"]) {
                let Some(r) = e["valueFrom"]["secretKeyRef"].as_object() else {
                    continue;
                };
                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let key = r.get("key").and_then(|v| v.as_str()).unwrap_or_default();
                let keys = secrets
                    .get(name)
                    .ok_or_else(|| format!("{file}: `{name}` is referenced but never emitted"))?;
                if !keys.contains(key) {
                    return Err(format!("{file}: `{name}` has no key `{key}`: {keys:?}"));
                }
            }
        }
    }
    Ok(())
}

/// Without a headless Service the pods get no stable DNS, and the fold cannot find the log again.
pub fn stateful_sets_name_a_headless_service(objects: &Objects) -> Result<(), String> {
    let headless: BTreeSet<String> = of_kind(objects, "Service")
        .into_iter()
        .filter(|(_, v)| v["spec"]["clusterIP"] == "None")
        .map(|(_, v)| str_at(&v, &["metadata", "name"]))
        .collect();
    for (file, v) in of_kind(objects, "StatefulSet") {
        let want = str_at(&v, &["spec", "serviceName"]);
        if !headless.contains(&want) {
            return Err(format!(
                "{file}: serviceName `{want}` is not a headless Service: {headless:?}"
            ));
        }
    }
    Ok(())
}

/// §6.6 rung 3 wants the default credentials to work from `git clone`, which means the host in the
/// URL is a Service that exists rather than a plausible-looking string.
pub fn credentials_point_at_an_emitted_service(objects: &Objects) -> Result<(), String> {
    let Some(url) = of_kind(objects, "Secret")
        .into_iter()
        .find_map(|(_, v)| v["stringData"]["url"].as_str().map(str::to_string))
    else {
        return Ok(());
    };
    let host = url
        .rsplit_once('@')
        .and_then(|(_, rest)| rest.split(':').next())
        .ok_or_else(|| format!("no host in `{url}`"))?
        .to_string();
    let (service, _) = host
        .split_once('.')
        .ok_or_else(|| format!("`{host}` is not a service-qualified host"))?;
    let names: BTreeSet<String> = of_kind(objects, "Service")
        .into_iter()
        .map(|(_, v)| str_at(&v, &["metadata", "name"]))
        .collect();
    if !names.contains(service) {
        return Err(format!(
            "the credentials point at `{service}`, which is not emitted: {names:?}"
        ));
    }
    let port: i64 = url
        .rsplit(':')
        .next()
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no port in `{url}`"))?;
    let exposed: BTreeSet<i64> = objects
        .iter()
        .filter(|(_, v)| v["kind"] == "Service" && v["metadata"]["name"] == service)
        .flat_map(|(_, v)| array(&v["spec"]["ports"]))
        .filter_map(|p| p["port"].as_i64())
        .collect();
    if !exposed.contains(&port) {
        return Err(format!(
            "`{url}` names a port `{service}` does not expose: {exposed:?}"
        ));
    }
    Ok(())
}

/// The check that would have caught `podSelector: {app: payments.example.com}`.
///
/// An egress peer is one of exactly three things: the DNS service, a pod that some workload in this
/// manifest set actually creates, or an address range that excludes the cluster and the metadata
/// endpoint. A DNS name is none of them, and a selector matching nothing grants nothing while
/// rendering as a policy that works.
pub fn egress_peers_are_real(objects: &Objects) -> Result<(), String> {
    const MUST_EXCLUDE: &[&str] = &[
        "10.0.0.0/8",
        "172.16.0.0/12",
        "192.168.0.0/16",
        "169.254.0.0/16",
    ];
    let templates = pod_templates(objects);
    for (file, policy) in of_kind(objects, "NetworkPolicy") {
        let mut dns = 0;
        for rule in array(&policy["spec"]["egress"]) {
            if array(&rule["ports"]).is_empty() {
                return Err(format!(
                    "{file}: an egress rule with no ports opens all of them: {rule}"
                ));
            }
            for to in array(&rule["to"]) {
                if to["ipBlock"].is_object() {
                    let except: BTreeSet<String> = array(&to["ipBlock"]["except"])
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                    for private in MUST_EXCLUDE {
                        if !except.contains(*private) {
                            return Err(format!(
                                "{file}: public egress must exclude {private}: {to}"
                            ));
                        }
                    }
                    continue;
                }
                let sel = string_map(&to["podSelector"]["matchLabels"]);
                if sel.get("k8s-app").is_some_and(|a| a == "kube-dns") {
                    dns += 1;
                    continue;
                }
                if !templates.iter().any(|(_, labels)| selects(&sel, labels)) {
                    return Err(format!(
                        "{file}: egress to {sel:?} matches no pod this manifest set creates: \
                         {templates:?}"
                    ));
                }
            }
        }
        // A policy with an egress section denies port 53 along with everything else it does not
        // name. Forgetting DNS is the classic generated-policy bug (§6.5).
        if dns != 1 {
            return Err(format!(
                "{file}: expected exactly one DNS egress rule, found {dns}"
            ));
        }
    }
    Ok(())
}

/// The gateway sends the traffic; the policy decides whether it arrives.
pub fn ingress_matches_the_routes_gateway(objects: &Objects) -> Result<(), String> {
    let from: BTreeSet<String> = of_kind(objects, "HTTPRoute")
        .into_iter()
        .flat_map(|(_, v)| array(&v["spec"]["parentRefs"]))
        .map(|p| str_at(&p, &["namespace"]))
        .collect();
    for (file, policy) in of_kind(objects, "NetworkPolicy") {
        let admitted: BTreeSet<String> = array(&policy["spec"]["ingress"])
            .iter()
            .flat_map(|r| array(&r["from"]))
            .filter_map(|p| {
                p["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"]
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        if admitted != from {
            return Err(format!(
                "{file}: admits {admitted:?} and the gateway is in {from:?}"
            ));
        }
        // …and an ingress rule with no ports admits every port.
        for rule in array(&policy["spec"]["ingress"]) {
            if array(&rule["ports"]).is_empty() {
                return Err(format!("{file}: an ingress rule with no ports: {rule}"));
            }
        }
    }
    Ok(())
}

/// A policy whose `podSelector` matches nothing constrains nothing, and looks like it is working.
pub fn policy_selects_a_workload(objects: &Objects) -> Result<(), String> {
    let templates = pod_templates(objects);
    for (file, policy) in of_kind(objects, "NetworkPolicy") {
        let sel = string_map(&policy["spec"]["podSelector"]["matchLabels"]);
        if !templates.iter().any(|(_, labels)| selects(&sel, labels)) {
            return Err(format!(
                "{file}: selects {sel:?}, which is no workload's pods: {templates:?}"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Small readers, so every check above is about the property and not about JSON
// ---------------------------------------------------------------------------------------------

pub fn of_kind(objects: &Objects, kind: &str) -> Vec<(String, Value)> {
    objects
        .iter()
        .filter(|(_, v)| v["kind"] == kind)
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect()
}

pub fn workloads(objects: &Objects) -> Vec<(String, Value)> {
    objects
        .iter()
        .filter(|(_, v)| v["kind"] == "Deployment" || v["kind"] == "StatefulSet")
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect()
}

pub fn pod_templates(objects: &Objects) -> Vec<(String, BTreeMap<String, String>)> {
    workloads(objects)
        .into_iter()
        .map(|(_, v)| {
            (
                str_at(&v, &["metadata", "name"]),
                string_map(&v["spec"]["template"]["metadata"]["labels"]),
            )
        })
        .collect()
}

pub fn container_ports(objects: &Objects) -> BTreeSet<i64> {
    workloads(objects)
        .into_iter()
        .flat_map(|(_, v)| array(&v["spec"]["template"]["spec"]["containers"]))
        .flat_map(|c| array(&c["ports"]))
        .filter_map(|p| p["containerPort"].as_i64())
        .collect()
}

pub fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

pub fn selects(selector: &BTreeMap<String, String>, labels: &BTreeMap<String, String>) -> bool {
    !selector.is_empty() && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
}

fn array(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn str_at(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for step in path {
        cur = &cur[step];
    }
    cur.as_str().unwrap_or_default().to_string()
}
