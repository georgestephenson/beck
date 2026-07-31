//! The `Platform` seam, exercised by something that is neither of the platforms in the crate.
//!
//! A trait with one implementation is a claim, not a fact — the same sentence
//! [`backend_seam.rs`](../../beck-cli/tests/backend_seam.rs) opens with, for the same reason.
//! `beck-infra` now has two implementations, which is better, and both were written by the same
//! person on the same afternoon, which is worse: a trait shaped so that only *those two* could
//! satisfy it would look fine.
//!
//! So this harness drives the whole emitter through a platform the crate has never heard of, whose
//! output is not YAML, whose manifest directory is not `k8s` or `compose`, and which reports that
//! it cannot express most of the graph. If `emit_with` works for that, the seam is a real one.
//!
//! # What it also pins
//!
//! Three properties that are the crate's contract with *any* platform, and each of which was
//! violated by the code before the trait existed:
//!
//! 1. **The manifest directory holds manifests and nothing else** — no image config, no program, no
//!    provenance table. This is docs/20 §20.4 item 14, generalised: `kubectl apply -f <dir>` was
//!    handed an apko file, and the fix is only durable if it is a property of the interface.
//! 2. **The graph is the whole input.** A platform cannot add or remove an object, because
//!    "infrastructure is a function of the program" (§5.4) stops being true the moment a deployment
//!    target gets a vote.
//! 3. **What a platform cannot express appears in the output.** A silently dropped `Policy`
//!    produces a deployment that looks like the one the effects asked for and enforces nothing.

use std::collections::BTreeSet;
use std::path::Path;

use beck_core::Effect;
use beck_infra::platform::{Artefact, Platform};
use beck_infra::{InfraGraph, Node};

/// A platform that is not Kubernetes and is not Compose.
///
/// Deliberately unlike both: one file, not YAML, no notion of a namespace or a selector, and it
/// admits up front that it can express almost nothing. If the trait can describe this, the trait is
/// about deployment targets rather than about the two that exist.
struct Toy;

impl Platform for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn manifest_dir(&self) -> &'static str {
        "toy"
    }

    fn manifests(&self, graph: &InfraGraph, wire_id: &str) -> Vec<Artefact> {
        let mut body = format!("# {} @ {wire_id}\n", graph.app);
        for d in &graph.nodes {
            if let Node::Workload { name, replicas, .. } = &d.node {
                body.push_str(&format!("run {name} x{replicas}\n"));
            }
        }
        vec![("run.txt".to_string(), body)]
    }

    fn unsupported(&self, graph: &InfraGraph) -> Vec<(String, String)> {
        graph
            .nodes
            .iter()
            .filter(|d| !matches!(d.node, Node::Workload { .. } | Node::Image { .. }))
            .map(|d| {
                (
                    beck_infra::id_of(&d.node),
                    "a toy runs processes and nothing else".to_string(),
                )
            })
            .collect()
    }

    fn apply(&self, _manifests: &Path) -> anyhow::Result<()> {
        anyhow::bail!("the toy platform does not apply anything")
    }
}

fn program() -> beck_core::Placed {
    const SRC: &str = r#"
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
    let (placed, d, map) = beck_core::compile_str("app.beck", SRC);
    assert!(!d.has_errors(), "{}", d.render(&map));
    placed.expect("it compiles")
}

fn out_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("beck-platform-seam-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_platform_the_crate_has_never_heard_of_can_be_emitted_for() {
    let placed = program();
    let dir = out_dir("toy");
    let written = beck_infra::emit_with(&placed, "source", &dir, &Toy).expect("it emits");
    assert!(!written.is_empty());
    let run = std::fs::read_to_string(dir.join("toy/run.txt")).expect("the toy's one file");
    assert!(run.contains("run app x1"), "{run}");
    // No image configs: the toy builds nothing, and the default `build_inputs` is empty.
    assert!(!dir.join("image.apko.yaml").exists());
    // …and the program travels with the manifests whatever the platform is.
    assert!(dir.join("app.beck").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_manifest_directory_holds_manifests_and_nothing_else() {
    // docs/20 §20.4 item 14, as a property of the interface rather than of one emitter: everything
    // a person or a controller points at is a manifest, so nothing else may be in there.
    let placed = program();
    for platform in beck_infra::platform::all() {
        let dir = out_dir(platform.name());
        beck_infra::emit_with(&placed, "source", &dir, platform.as_ref()).expect("it emits");

        let expected: BTreeSet<String> = platform
            .manifests(&beck_infra::graph(&placed), &placed.wire_id)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let found = walk(
            &dir.join(platform.manifest_dir()),
            &dir.join(platform.manifest_dir()),
        );
        assert_eq!(
            found,
            expected,
            "`{}`: the manifest directory holds something that is not a manifest",
            platform.name()
        );
        // The program, the provenance table and any build inputs are *outside* it.
        for stray in ["app.beck", "explain.txt", "image.apko.yaml"] {
            assert!(
                !dir.join(platform.manifest_dir()).join(stray).exists(),
                "`{}`: {stray} is inside the manifest directory",
                platform.name()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Every file under `dir`, as paths relative to `root`.
fn walk(dir: &Path, root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            out.extend(walk(&path, root));
        } else {
            out.insert(
                path.strip_prefix(root)
                    .expect("under the root")
                    .display()
                    .to_string(),
            );
        }
    }
    out
}

#[test]
fn no_platform_changes_which_objects_exist() {
    // §5.4: infrastructure is a function of the *program*. A platform renders the graph; it does
    // not get a vote on what is in it. The check is that `derive` is not reached from any platform
    // — asserted by giving every platform the same graph and requiring the graph to be untouched.
    let placed = program();
    let before = beck_infra::graph(&placed);
    let names: Vec<String> = before
        .nodes
        .iter()
        .map(|d| beck_infra::id_of(&d.node))
        .collect();

    for platform in beck_infra::platform::all()
        .into_iter()
        .chain([Box::new(Toy) as Box<dyn Platform>])
    {
        let after = beck_infra::graph(&placed);
        let _ = platform.manifests(&after, &placed.wire_id);
        let now: Vec<String> = after
            .nodes
            .iter()
            .map(|d| beck_infra::id_of(&d.node))
            .collect();
        assert_eq!(
            names,
            now,
            "`{}` changed the object graph, which is the program's and not the platform's",
            platform.name()
        );
    }
}

#[test]
fn what_a_platform_cannot_express_reaches_the_output_a_person_reads() {
    // The property `Platform::unsupported` exists for. A deployment that silently omits the
    // NetworkPolicy looks exactly like one where the policy is working, and the difference has to
    // be visible without reading the emitter.
    let placed = program();
    let dir = out_dir("toy-explain");
    beck_infra::emit_with(&placed, "source", &dir, &Toy).expect("it emits");
    let explain = std::fs::read_to_string(dir.join("explain.txt")).expect("a provenance table");
    assert!(explain.contains("platform: toy"), "{explain}");
    assert!(
        explain.contains("a toy runs processes and nothing else"),
        "the gap must be in the output, not inferable from an absence:\n{explain}"
    );
    // …and the object it cannot express is named, not counted.
    assert!(explain.contains("Policy/app-policy"), "{explain}");
    let _ = std::fs::remove_dir_all(&dir);

    // Kubernetes expresses everything, and says so rather than saying nothing.
    let dir = out_dir("k8s-explain");
    beck_infra::emit_with(&placed, "source", &dir, &beck_infra::k8s::Kubernetes).expect("emits");
    let explain = std::fs::read_to_string(dir.join("explain.txt")).expect("a provenance table");
    assert!(
        explain.contains("every object above is expressible"),
        "{explain}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_platform_is_reachable_by_the_name_it_reports() {
    // `--platform` resolves through `by_name`, so a platform whose name does not round-trip is one
    // nobody can select.
    for platform in beck_infra::platform::all() {
        let found = beck_infra::platform::by_name(platform.name())
            .unwrap_or_else(|| panic!("`{}` is not reachable by name", platform.name()));
        assert_eq!(found.name(), platform.name());
        assert!(!platform.manifest_dir().is_empty());
    }
    assert!(beck_infra::platform::by_name("nomad").is_none());
    // Two platforms sharing a manifest directory would overwrite each other in one output tree.
    let dirs: BTreeSet<&str> = beck_infra::platform::all()
        .iter()
        .map(|p| p.manifest_dir())
        .collect();
    assert_eq!(dirs.len(), beck_infra::platform::all().len());
}

#[test]
fn both_real_platforms_agree_about_the_facts_they_share() {
    // The interesting cross-platform property: two targets, one derivation. Where both express the
    // same fact, they must express the *same* fact — the port the app listens on, the name of the
    // log store it reaches, and the grants it is given. This is the check that would catch one
    // platform being updated and the other not.
    let g = beck_infra::derive(
        "app",
        &[
            (Effect::Ingress, "proposals".into()),
            (Effect::Durable, "st".into()),
        ],
        true,
    );

    let k8s: String = beck_infra::k8s::Kubernetes
        .manifests(&g, "id")
        .into_iter()
        .map(|(_, b)| b)
        .collect();
    let compose: String = beck_infra::compose::Compose
        .manifests(&g, "id")
        .into_iter()
        .map(|(_, b)| b)
        .collect();

    for shared in [
        &format!("0.0.0.0:{}", beck_infra::APP_PORT),
        "GRANT SELECT, INSERT",
        "/app/app.beck",
    ] {
        assert!(k8s.contains(shared), "kubernetes lost `{shared}`");
        assert!(compose.contains(shared), "compose lost `{shared}`");
    }
    // Neither may grant a privilege the program does not exercise.
    for forbidden in ["DELETE", "UPDATE", "TRUNCATE"] {
        assert!(!k8s.contains(forbidden), "kubernetes: {forbidden}");
        assert!(!compose.contains(forbidden), "compose: {forbidden}");
    }
}
