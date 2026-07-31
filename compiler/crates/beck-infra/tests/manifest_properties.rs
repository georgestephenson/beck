//! The manifest emitter, as properties over generated graphs.
//!
//! [`manifests.rs`](manifests.rs) checks one program's manifests against fifteen invariants. That
//! licenses "these manifests are consistent". This file runs the same invariants over thousands of
//! graphs nobody wrote, which licenses the claim that is actually worth having:
//!
//! > **the emitter cannot produce inconsistent manifests.**
//!
//! The difference is not academic. Every defect §20.4 item 13 records was present in the manifests
//! of the one program under test — a `net.out` rendered as a pod selector, a missing DNS rule, a
//! namespace two objects disagreed about — and each survived because the example-based test asked
//! about something else. An example suite can only find what somebody suspected.
//!
//! # What is generated
//!
//! Not YAML, and not `Node`s at random. The input to the emitter that a program can actually vary
//! is an **effect row and an application name**, so that is what is generated, and everything
//! downstream is the real [`beck_infra::derive`] and the real [`beck_infra::k8s::objects`]. A
//! counterexample is therefore always a program somebody could write.
//!
//! Application names are generated adversarially on purpose — long ones, ones that are all
//! punctuation, ones that end in a dash — because the name is the stem of every derived object and
//! Kubernetes has opinions about names that the Rust type system does not. That is where the
//! generator found its first defect: a module name over 47 characters produced a
//! `<app>-log-credentials` past the API's 63-character limit, so the application compiled, derived,
//! rendered, and would have failed at `kubectl apply` with a message about a field nobody wrote.

use beck_core::Effect;
use beck_infra::{InfraGraph, Platform};
use proptest::prelude::*;

mod invariants;

/// The effect atoms that reach the infrastructure tier at all.
///
/// The others (`log`, `dom`, `partial`, …) imply no object, so generating them would only dilute
/// the sample. `net.out` is generated with several hosts because the number of them is what the
/// egress rules are a function of.
fn effect() -> impl Strategy<Value = Effect> {
    prop_oneof![
        Just(Effect::Ingress),
        Just(Effect::Durable),
        Just(Effect::NetOut("origin".into())),
        Just(Effect::NetOut("payments.example.com".into())),
        Just(Effect::NetOut("hooks.example.com".into())),
        Just(Effect::NetOut("a.very.long.hostname.example.com".into())),
        Just(Effect::Env),
        Just(Effect::Nondet),
    ]
}

/// Names a module could plausibly have, and names it could implausibly have.
fn app_name() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-z][a-z0-9-]{0,20}",
        2 => "[A-Za-z0-9_. -]{1,40}",
        2 => "[a-z]{40,80}",
        1 => "[-_. ]{1,10}",
        1 => Just(String::new()),
    ]
}

fn graph() -> impl Strategy<Value = InfraGraph> {
    (
        app_name(),
        prop::collection::vec(effect(), 0..6),
        any::<bool>(),
    )
        .prop_map(|(app, effects, serves_ui)| {
            let effects: Vec<(Effect, String)> = effects
                .into_iter()
                .enumerate()
                .map(|(i, e)| (e, format!("d{i}")))
                .collect();
            beck_infra::derive(&beck_infra::sanitise(&app), &effects, serves_ui)
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// The whole point. Every invariant, every graph.
    #[test]
    fn no_effect_row_produces_an_inconsistent_manifest_set(g in graph()) {
        let objects = beck_infra::k8s::objects(&g, "0123456789abcdef");
        if let Err(why) = invariants::all(&objects) {
            let rendered: String = beck_infra::render(&g, "0123456789abcdef")
                .iter()
                .map(|(n, b)| format!("# {n}\n{b}\n"))
                .collect();
            prop_assert!(false, "{why}\n\n{rendered}");
        }
    }

    /// §3.4's determinism guardrail, one tier down: GitOps diffs and golden files both need the
    /// same input to produce the same bytes, and a `HashMap` anywhere in the emitter would break it
    /// on some inputs and not others.
    #[test]
    fn rendering_is_a_function_of_the_graph(g in graph()) {
        prop_assert_eq!(
            beck_infra::render(&g, "id"),
            beck_infra::render(&g, "id")
        );
    }

    /// The `needs` edges and the emission order agree, so nothing is applied before what it
    /// references. Kubernetes converges either way; this is the difference between coming up and
    /// coming up after a few CrashLoopBackOffs.
    #[test]
    fn nothing_is_applied_before_what_it_references(g in graph()) {
        let order = beck_infra::k8s::apply_order(&g);
        let position: std::collections::BTreeMap<String, usize> = order
            .iter()
            .enumerate()
            .map(|(i, d)| (beck_infra::id_of(&d.node), i))
            .collect();
        for (i, d) in order.iter().enumerate() {
            for need in &d.needs {
                let at = *position
                    .get(need)
                    .ok_or_else(|| TestCaseError::fail(format!(
                        "{} needs {need}, which is not in the graph",
                        beck_infra::id_of(&d.node)
                    )))?;
                prop_assert!(
                    at < i,
                    "{} is applied at {i} but needs {need} at {at}",
                    beck_infra::id_of(&d.node)
                );
            }
        }
    }

    /// The derivation claim, over every row rather than over the two examples in `lib.rs`: an
    /// object exists **because** of an effect, so an effect row that lacks the atom lacks the
    /// object. This is the sentence the platform-team pitch rests on, and it is the one that would
    /// quietly stop being true if a default crept into `derive`.
    #[test]
    fn an_object_exists_only_because_an_effect_asked_for_it(
        app in "[a-z][a-z0-9-]{0,12}",
        base in prop::collection::vec(effect(), 0..4),
    ) {
        let with_durable: Vec<(Effect, String)> = base
            .iter()
            .filter(|e| **e != Effect::Durable)
            .map(|e| (e.clone(), "d".to_string()))
            .chain(std::iter::once((Effect::Durable, "st".to_string())))
            .collect();
        let without: Vec<(Effect, String)> = with_durable
            .iter()
            .filter(|(e, _)| *e != Effect::Durable)
            .cloned()
            .collect();

        let kinds = |effects: &[(Effect, String)]| -> Vec<String> {
            let g = beck_infra::derive(&app, effects, true);
            beck_infra::k8s::objects(&g, "id")
                .into_iter()
                .map(|(_, v)| v["kind"].as_str().unwrap_or_default().to_string())
                .collect()
        };
        prop_assert!(kinds(&with_durable).contains(&"StatefulSet".to_string()));
        prop_assert!(!kinds(&without).contains(&"StatefulSet".to_string()));
        prop_assert!(!kinds(&without).contains(&"Secret".to_string()));
    }

    /// One `net.out` host or twenty, the egress rules stay bounded and the hosts stay out of the
    /// selectors. The second half is the defect §20.4 item 13 records, stated as a property.
    #[test]
    fn a_hostname_never_becomes_a_label(hosts in prop::collection::vec("[a-z][a-z.]{2,30}", 1..8)) {
        let effects: Vec<(Effect, String)> = hosts
            .iter()
            .map(|h| (Effect::NetOut(h.as_str().into()), "call".to_string()))
            .chain(std::iter::once((Effect::Durable, "st".to_string())))
            .collect();
        let g = beck_infra::derive("app", &effects, false);
        let policy = beck_infra::k8s::objects(&g, "id")
            .into_iter()
            .find(|(_, v)| v["kind"] == "NetworkPolicy")
            .map(|(_, v)| v)
            .expect("a policy is always emitted");
        // Every label *value* any egress selector matches on. Walked rather than grepped: an
        // earlier version of this check compared substrings and a generated host called `pod`
        // matched the word `podSelector`, which is the test being wrong rather than the code.
        let selector_values: std::collections::BTreeSet<String> = policy["spec"]["egress"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .flat_map(|r| r["to"].as_array().cloned().unwrap_or_default())
            .flat_map(|to| {
                ["podSelector", "namespaceSelector"]
                    .iter()
                    .flat_map(|k| {
                        to[*k]["matchLabels"]
                            .as_object()
                            .map(|m| m.values().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>())
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        for host in &hosts {
            // A host may appear in the annotation — that is where it belongs — and never as a
            // label value, where it would match nothing.
            prop_assert!(
                !selector_values.contains(host.as_str()),
                "`{host}` is an egress selector's label value, where it matches no pod: \
                 {selector_values:?}"
            );
        }
        prop_assert_eq!(
            policy["metadata"]["annotations"]["beck.dev/egress-hosts"]
                .as_str()
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .count(),
            hosts.iter().collect::<std::collections::BTreeSet<_>>().len(),
            "every host the program names is recorded exactly once"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// The platform-neutral half, over every platform the crate knows.
    ///
    /// Not "the manifests are right" — a target this file has never seen renders something this
    /// file cannot read. It is the *contract*: paths stay inside the output directory and do not
    /// collide, nothing is emitted empty, and a reported gap is about an object that exists. Each
    /// of those is a way to produce a broken output tree that no per-platform check would notice,
    /// because each platform would be individually correct.
    #[test]
    fn no_platform_breaks_the_contract_the_trait_states(g in graph()) {
        for platform in beck_infra::platform::all() {
            if let Err(why) = invariants::platform_artefacts(platform.as_ref(), &g, "id") {
                prop_assert!(false, "{why}");
            }
        }
    }

    /// The Compose file's own cross-object invariants, over generated effect rows.
    ///
    /// Compose has no selectors and no namespaces, so it has fewer ways to be inconsistent — and
    /// exactly the same *kinds*: a `depends_on` naming a service nobody defines, a volume mount
    /// naming a volume nobody declares, a connection string naming a host nothing runs. The
    /// Kubernetes analogue of the last one is docs/19 §19.5's third defect.
    #[test]
    fn the_compose_file_never_references_something_it_does_not_define(g in graph()) {
        let rendered = beck_infra::compose::Compose.manifests(&g, "id");
        let (_, body) = rendered
            .iter()
            .find(|(name, _)| name == "compose.yaml")
            .expect("compose always emits its one file");
        let parsed: serde_json::Value = serde_norway::from_str(body)
            .map_err(|e| TestCaseError::fail(format!("compose.yaml is not YAML: {e}\n{body}")))?;
        if let Err(why) = invariants::compose_file_is_consistent(&parsed) {
            prop_assert!(false, "{why}\n\n{body}");
        }
    }
}

/// The generator's first find, kept as a regression test with the name spelled out.
///
/// `proptest` shrank a 60-character module name to this. Without the cap in
/// [`beck_infra::sanitise`], the derived Secret is 76 characters and the API server rejects it —
/// after the program has compiled, derived, rendered and been committed.
#[test]
fn a_long_module_name_still_derives_names_kubernetes_will_accept() {
    let long = "customer-facing-order-management-and-fulfilment-service";
    assert!(long.len() > 47, "the case has to be long enough to matter");
    let g = beck_infra::derive(
        &beck_infra::sanitise(long),
        &[
            (Effect::Ingress, "proposals".into()),
            (Effect::Durable, "st".into()),
        ],
        true,
    );
    let objects = beck_infra::k8s::objects(&g, "id");
    invariants::names_are_legal(&objects).expect("every derived name fits");
    // …and the name is still recognisable, which is the reason for truncating rather than hashing.
    assert!(g.app.starts_with("customer-facing-order"), "{}", g.app);
}
