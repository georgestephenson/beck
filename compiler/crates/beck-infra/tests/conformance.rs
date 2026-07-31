//! The manifests, submitted to a real Kubernetes API server.
//!
//! Everything else in this crate reasons about the objects. The API server is the only thing that
//! *decides* about them, and until it has been asked, "these manifests are correct" is a statement
//! about our model of Kubernetes rather than about Kubernetes. Concretely, three classes of defect
//! live entirely beyond the reach of the other suites:
//!
//! * **admission** — a field the schema allows and a validating webhook or the apiserver's own
//!   validation rejects: an immutable field, a `Quantity` that parses but is out of range, a
//!   `serviceName` that is legal as a string and illegal as a reference;
//! * **CRDs** — `HTTPRoute` is not in `k8s-openapi`, so its field names are checked here or by
//!   nobody. This is the suite that covers [`beck_infra::k8s::gateway`];
//! * **version drift** — the emitter is compiled against `v1_34` types, and the cluster it is
//!   applied to is whatever the cluster is.
//!
//! # `--dry-run=server`, and why that is the right amount of cluster
//!
//! Server-side dry run runs the whole admission chain — decoding, defaulting, validation, webhooks
//! — and then discards the object instead of persisting it. So it answers *"would the API server
//! accept this?"*, which is the question, without scheduling a pod, pulling an image, or needing a
//! working `postgres:16-alpine`. It is the difference between conformance and an end-to-end test,
//! and end-to-end is a different job with a different failure budget.
//!
//! # Skipping, and the one thing that must never happen
//!
//! There is no cluster on a developer's laptop by default and there is one in CI. A test that
//! silently passes when the thing it checks did not run is worse than no test — that is
//! [`docs/19-phase-1-report.md`](../../../../docs/19-phase-1-report.md) §19.4 item 10 again, an
//! artefact nobody executed. So:
//!
//! * with no cluster reachable, the test **prints why it skipped** and passes;
//! * with `BECK_REQUIRE_CLUSTER=1`, a missing cluster is a **failure**, and CI sets it.
//!
//! Run it locally with a throwaway cluster:
//!
//! ```console
//! $ k3d cluster create beck-conformance
//! $ kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.1/standard-install.yaml
//! $ BECK_REQUIRE_CLUSTER=1 cargo test -p beck-infra --test conformance -- --nocapture
//! ```

use std::path::Path;
use std::process::Command;

/// The canonical program, as the file a reader can open.
const TODO: &str = include_str!("../../../examples/todo.beck");

/// The program the other suites use, which exercises `net.out` as well.
const MODERATION: &str = include_str!("../../../corpus/20-moderation.beck");

fn require_cluster() -> bool {
    std::env::var("BECK_REQUIRE_CLUSTER").is_ok_and(|v| v == "1")
}

/// Is there a `kubectl` that can reach a cluster?
///
/// Returns the reason it cannot, so the skip message says something useful rather than "skipped".
fn cluster() -> Result<(), String> {
    let out = Command::new("kubectl")
        .args(["version", "-o", "json"])
        .output()
        .map_err(|e| format!("`kubectl` is not on PATH ({e})"))?;
    if !out.status.success() {
        return Err(format!(
            "`kubectl version` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // `kubectl version` succeeds against no cluster at all; `serverVersion` is what says otherwise.
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.contains("serverVersion") {
        return Err("`kubectl` found no cluster (no serverVersion in `kubectl version`)".into());
    }
    Ok(())
}

/// Run the body against a cluster, or explain why it did not run.
fn with_cluster(what: &str, body: impl FnOnce()) {
    match cluster() {
        Ok(()) => body(),
        Err(why) => {
            let message = format!("conformance: {what} did not run — {why}");
            assert!(
                !require_cluster(),
                "{message}\n\nBECK_REQUIRE_CLUSTER=1 is set, so this is a failure rather than a \
                 skip: CI is supposed to have a cluster, and a conformance suite that silently \
                 skips is a suite that is not running."
            );
            eprintln!("{message} (set BECK_REQUIRE_CLUSTER=1 to make this a failure)");
        }
    }
}

/// Emit a program's manifests into a directory.
fn emit(source: &str, name: &str, dir: &Path) -> String {
    let (placed, d, map) = beck_core::compile_str(name, source);
    assert!(!d.has_errors(), "{}", d.render(&map));
    let placed = placed.expect("it compiles");
    beck_infra::emit(&placed, source, dir).expect("the manifests are written");
    beck_infra::graph(&placed).app
}

fn kubectl(args: &[&str]) -> (bool, String) {
    let out = Command::new("kubectl")
        .args(args)
        .output()
        .expect("`kubectl` runs, having already answered `kubectl version`");
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

/// Submit a program's manifests to the API server and report what it said.
///
/// The Namespace is applied for real first, because a server-side dry run of a namespaced object
/// into a namespace that does not exist is rejected for that reason and not for any reason about
/// the object. It is deleted afterwards, and named after the program, so two runs cannot collide.
fn admits(source: &str, name: &str) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("beck-conformance-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    let app = emit(source, name, &dir);

    let manifests = dir.join(beck_infra::MANIFEST_DIR);
    let namespace = manifests.join("000-namespace.yaml");
    assert!(
        namespace.exists(),
        "the namespace must sort first, or nothing else can be applied: {}",
        manifests.display()
    );
    let ns = namespace.display().to_string();
    let (ok, said) = kubectl(&["apply", "-f", &ns]);
    if !ok {
        return Err(format!("the namespace itself was rejected:\n{said}"));
    }

    // `<out>/k8s`, not `<out>`: the image configs are YAML too, and `kubectl apply -f <dir>` reads
    // every `.yaml` it finds.
    let path = manifests.display().to_string();
    let (ok, said) = kubectl(&["apply", "--dry-run=server", "-f", &path]);

    // Clean up whatever happened, then report.
    let _ = kubectl(&["delete", "namespace", &app, "--wait=false"]);
    let _ = std::fs::remove_dir_all(&dir);

    if ok {
        println!("conformance: the API server admits {name}:\n{said}");
        Ok(())
    } else {
        Err(said)
    }
}

#[test]
fn the_api_server_admits_the_sketchs_manifests() {
    with_cluster("the todo sketch", || {
        if let Err(said) = admits(TODO, "todo.beck") {
            panic!(
                "the API server refused the manifests the compiler emits:\n{said}\n\n\
                 This is the one authority on whether an object is admissible. Every other suite \
                 in this crate reasons about our model of Kubernetes; this one asks Kubernetes."
            );
        }
    });
}

#[test]
fn the_api_server_admits_a_program_that_calls_out() {
    // A different effect row, so a different object set: this one has a `net.out`, which is the
    // NetworkPolicy path, and an `internal[T]`, which changes nothing about the infrastructure and
    // is included so that the two suites stay pointed at the same programs.
    with_cluster("the moderation corpus program", || {
        if let Err(said) = admits(MODERATION, "moderation.beck") {
            panic!("the API server refused the manifests the compiler emits:\n{said}");
        }
    });
}

#[test]
fn the_gateway_api_types_are_the_ones_the_cluster_has() {
    // The suite's real reason for existing. `HTTPRoute` is a CRD, so `k8s-openapi` does not carry
    // it and `beck_infra::k8s::gateway` writes the field names by hand — checked, until now, by
    // review. A cluster with the Gateway API CRDs installed is what checks them.
    with_cluster("the HTTPRoute", || {
        let (installed, _) = kubectl(&["get", "crd", "httproutes.gateway.networking.k8s.io"]);
        if !installed {
            let message = "conformance: the Gateway API CRDs are not installed, so the HTTPRoute \
                           was not validated. `kubectl apply -f \
                           https://github.com/kubernetes-sigs/gateway-api/releases/download/\
                           v1.2.1/standard-install.yaml`";
            assert!(
                !require_cluster(),
                "{message}\n\nBECK_REQUIRE_CLUSTER=1 is set: this is the one object no other suite \
                 can check, so a cluster without the CRDs is not a cluster this test accepts."
            );
            eprintln!("{message}");
            return;
        }
        if let Err(said) = admits(TODO, "todo-gateway.beck") {
            panic!("the API server refused the HTTPRoute:\n{said}");
        }
    });
}

#[test]
fn the_harness_refuses_to_pass_quietly_when_it_is_required_to_run() {
    // A test about this file, because the failure mode of a conformance suite is not "it fails" —
    // it is "it skipped, in CI, for a month". The skip path is only acceptable because
    // `BECK_REQUIRE_CLUSTER=1` turns it into a failure, so that switch is itself checked.
    //
    // No cluster is needed to check it: the question is what `with_cluster` does with a `Err`.
    assert!(
        cluster().is_ok()
            || !require_cluster()
            || std::panic::catch_unwind(|| {
                with_cluster("a deliberate probe", || ());
            })
            .is_err(),
        "with BECK_REQUIRE_CLUSTER=1 and no cluster, `with_cluster` must panic rather than skip"
    );
}
