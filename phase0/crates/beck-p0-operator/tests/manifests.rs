//! Golden manifests (§4.8's infra row).
//!
//! The committed `deploy/k8s` tree must be exactly what the effect row implies. If someone edits
//! the YAML by hand — the failure mode this whole design exists to abolish — this fails.

use std::process::Command;

#[test]
fn the_committed_manifests_match_the_derived_object_graph() {
    let phase0 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("phase0 root")
        .to_path_buf();

    let output = Command::new(env!("CARGO_BIN_EXE_beck-p0-operator"))
        .current_dir(&phase0)
        .args(["emit", "--out", "deploy/k8s", "--check"])
        .output()
        .expect("run emit --check");

    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
