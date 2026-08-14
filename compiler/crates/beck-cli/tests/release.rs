//! The release pipeline and the installer, gated.
//!
//! `docs/28-releases-and-deployment.md` opens with the discipline these tests exist to serve: "a
//! pipeline is an artefact, and an artefact nobody has executed is a design document". A release
//! workflow cannot be executed before a tag is cut, so what is executable is factored out of it —
//! `release/build.sh` builds one artefact, `install.sh` installs one, and both run on a laptop.
//! This file runs them, and checks the parts of the workflow that only a reader could otherwise
//! check.
//!
//! Four properties, and the last two are the ones that matter:
//!
//! 1. **The installer and the pipeline name the same platforms**, in both directions. An installer
//!    offering a target no release contains is a 404 in somebody's first five minutes; a target
//!    built and never offered is an artefact nobody can reach.
//! 2. **Nothing is published before the whole suite has run.** §28.2 item 1 is a `needs:` edge, and
//!    an edge is checkable.
//! 3. **The installer refuses an archive whose checksum is wrong**, tested by corrupting one. That
//!    is the gap this script exists to close, so it is the gap the gate is written against
//!    (`docs/82-the-edge-report.md` §82.10), rather than the presence of
//!    a `sha256sum` call somewhere in the file.
//! 4. **The installer refuses an archive whose provenance does not verify**, and refuses to
//!    pretend it verified one when the tool that would is missing
//!    (`docs/92-supply-chain-and-release-report.md`). Tested the same way: a verifier that says no, and a
//!    verifier that is not there.
//!
//! The last two need `sh`, `tar` and a SHA-256 tool, so they **skip loudly** when one is missing;
//! `BECK_REQUIRE_INSTALL=1` forbids the skip. Neither needs `gh`, and neither needs a network —
//! `support::relfix` explains what that buys and what it does not.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

mod support;

use support::relfix::{self, repo_root, tool};

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is checked in: {e}", path.display()))
}

/// The `SUPPORTED=` line of `install.sh` — one line, so that this test and the workflow's own
/// check can read the same list rather than each parsing a `case` arm.
fn installer_targets() -> BTreeSet<String> {
    let text = read("install.sh");
    let line = text
        .lines()
        .find(|l| l.starts_with("SUPPORTED="))
        .expect("install.sh names the platforms it supports on one `SUPPORTED=` line");
    line.trim_start_matches("SUPPORTED=")
        .trim_matches('"')
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The `target:` entries of the release workflow's build matrix.
fn pipeline_targets() -> BTreeSet<String> {
    read(".github/workflows/release.yml")
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- target: ").map(str::to_string))
        .collect()
}

#[test]
fn there_is_a_pipeline_and_an_installer_to_check() {
    // Every other test here reads one of these three files, and a missing file would make them all
    // pass by looking at nothing.
    for f in [
        ".github/workflows/release.yml",
        "install.sh",
        "release/build.sh",
        "release/version.sh",
    ] {
        assert!(
            repo_root().join(f).exists(),
            "{f} is what the release is made of, and it is not checked in"
        );
    }
    assert!(
        !installer_targets().is_empty() && !pipeline_targets().is_empty(),
        "one of the two platform lists parsed as empty, which would make the set equality below \
         hold for the wrong reason"
    );
}

#[test]
fn the_installer_and_the_pipeline_name_the_same_platforms() {
    let installer = installer_targets();
    let pipeline = pipeline_targets();
    assert_eq!(
        installer,
        pipeline,
        "install.sh offers {:?} and .github/workflows/release.yml builds {:?}. \
         The difference is either a download that 404s or a binary nobody can install.",
        installer.difference(&pipeline).collect::<Vec<_>>(),
        pipeline.difference(&installer).collect::<Vec<_>>(),
    );
}

#[test]
fn nothing_is_published_before_the_whole_suite_has_run() {
    let text = read(".github/workflows/release.yml");

    // The suite is `compiler.yml` itself rather than a subset of its steps — §28.2 item 1's "no
    // release-only build steps" is only true if the release runs the workflow a pull request runs.
    assert!(
        text.contains("uses: ./.github/workflows/compiler.yml"),
        "the release no longer re-runs the compiler workflow on the tagged commit"
    );
    assert!(
        read(".github/workflows/compiler.yml").contains("workflow_call:"),
        "compiler.yml is no longer callable, so the release cannot be running it"
    );

    // …and the publishing job waits for it. `needs:` is the whole of "refuses to publish on any
    // red", so it is asserted by name rather than by reading the job order.
    let publish = text
        .split("\n  publish:")
        .nth(1)
        .expect("the release workflow has a publish job");
    let needs = publish
        .lines()
        .find(|l| l.trim_start().starts_with("needs:"))
        .expect("the publish job declares what it needs");
    for required in ["suite", "binaries"] {
        assert!(
            needs.contains(required),
            "publish does not need `{required}`: {needs}"
        );
    }
}

#[test]
fn the_release_verifies_its_own_installer_against_its_own_artefacts() {
    // The pipeline installs what it just built, over `file://`, before it publishes anything. A
    // release whose installer cannot install it is the failure this step exists to prevent, and the
    // step is easy to delete by accident when the publish job is edited.
    let text = read(".github/workflows/release.yml");
    assert!(
        text.contains("./install.sh"),
        "the publish job no longer runs the installer against the artefacts it assembled"
    );
    assert!(
        text.contains("sha256sum -c SHA256SUMS"),
        "the publish job no longer re-checks the assembled SHA256SUMS"
    );
}

#[test]
fn the_asset_name_is_one_convention() {
    // Three files independently construct `beck-<version>-<target>.tar.gz`. They cannot share a
    // definition — one is YAML, two are shell — so the convention is asserted in each.
    for (file, needle) in [
        ("release/build.sh", "beck-$version-$target.tar.gz"),
        ("install.sh", "beck-$version-$target.tar.gz"),
        (
            ".github/workflows/release.yml",
            "beck-$VERSION-$target.tar.gz",
        ),
    ] {
        assert!(
            read(file).contains(needle),
            "{file} no longer names the release asset `{needle}`"
        );
    }
}

#[test]
fn a_tag_that_disagrees_with_the_workspace_version_fails_the_build() {
    let Some(sh) = tool("sh") else { return };
    let version = env!("CARGO_PKG_VERSION");

    // The real version passes and prints the asset it would produce…
    let good = script(&sh, &["--expect-version", version, "--check-only"]);
    assert!(
        good.status.success(),
        "release/build.sh rejects the version the workspace carries: {}",
        String::from_utf8_lossy(&good.stderr)
    );
    let printed = String::from_utf8_lossy(&good.stdout);
    assert!(
        printed.trim().starts_with(&format!("beck-{version}-")),
        "release/build.sh named the asset `{}` for version {version}",
        printed.trim()
    );

    // …and a wrong one fails, which is what stops `git tag v0.2.0` on a 0.3.0 workspace from
    // publishing a binary whose own `--version` contradicts the release page.
    let bad = script(&sh, &["--expect-version", "9.9.9", "--check-only"]);
    assert!(
        !bad.status.success(),
        "release/build.sh accepted a version the workspace does not carry"
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains(version),
        "the refusal does not say which version the workspace carries"
    );
}

#[test]
fn the_binary_says_which_build_it_is() {
    // A version alone names a release; four artefacts share it. `docs/28` §28.2 publishes one per
    // target from one commit, so `--version` names the commit and the triple as well.
    let out = Command::new(env!("CARGO_BIN_EXE_beck"))
        .arg("--version")
        .output()
        .expect("the binary this test run built");
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        text.starts_with(&format!("beck {}", env!("CARGO_PKG_VERSION"))),
        "`beck --version` printed {text:?}, which does not begin with the release number"
    );
    assert!(
        text.contains(std::env::consts::ARCH) && text.ends_with(')'),
        "`beck --version` printed {text:?}, which does not identify the artefact"
    );
}

#[test]
fn the_guide_installs_before_it_builds_from_source() {
    // The exit criterion is a person building an application from the documentation
    // (`docs/08-roadmap.md` §8.5.4), and "clone the repository and wait for a Rust toolchain" was
    // the first thing that documentation asked of them.
    let guide = read("docs/86-getting-started.md");
    assert!(
        guide.contains("install.sh"),
        "docs/86 no longer shows the installer this repository ships"
    );
    let install_at = guide.find("install.sh").expect("shown");
    let build_at = guide
        .find("cargo build --release")
        .expect("building from source stays documented");
    assert!(
        install_at < build_at,
        "docs/86 shows the source build before the install, which is the order the release changed"
    );
    assert!(
        read("README.md").contains("install.sh"),
        "the repository README no longer shows how to install a released binary"
    );
}

/// The property the installer exists for: a download that does not match its checksum installs
/// nothing.
///
/// Run against a fixture release rather than a real one — a tarball with a stub `beck` in it, a
/// `SHA256SUMS` beside it, and a `file://` base URL — so it needs no network and no published
/// release.
#[test]
fn the_installer_refuses_an_archive_whose_checksum_is_wrong() {
    let Some(release) = relfix::fixture("checksum") else {
        return;
    };

    // 1. A release whose checksum agrees installs, and the binary is on disk afterwards.
    let bin = release.root.join("bin");
    let ok = release.install(&bin, &[]);
    assert!(
        ok.status.success(),
        "the installer refused a release whose checksum is correct:\n{}\n{}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(bin.join("beck").exists(), "nothing was installed");
    assert!(
        String::from_utf8_lossy(&ok.stdout).contains(&release.published_digest()),
        "the installer did not print the checksum it verified"
    );

    // 2. The same release with one byte of the archive changed installs nothing.
    release.corrupt_the_archive();

    let fresh = release.root.join("bin-2");
    let bad = release.install(&fresh, &[]);
    assert!(
        !bad.status.success(),
        "the installer accepted an archive whose checksum does not match:\n{}",
        String::from_utf8_lossy(&bad.stdout)
    );
    assert!(
        !fresh.join("beck").exists(),
        "the installer refused the archive and installed it anyway"
    );
    assert!(
        String::from_utf8_lossy(&bad.stderr).contains("checksum mismatch"),
        "the refusal does not say what was wrong:\n{}",
        String::from_utf8_lossy(&bad.stderr)
    );
}

// ---- the provenance ----------------------------------------------------------------------------

#[test]
fn the_pipeline_attests_the_file_the_installer_verifies_against() {
    let text = read(".github/workflows/release.yml");

    // The subject. `subject-checksums` reads `SHA256SUMS` and attests one subject per line, so the
    // digests the attestation covers are the digests the installer checks — the same bytes, not two
    // lists that agree today. A glob over the tarballs would pass this file's other tests and
    // silently cover a different set the day an artefact stops being listed.
    assert!(
        text.contains("subject-checksums: staging/SHA256SUMS"),
        "the release no longer attests the assembled SHA256SUMS, so the set of digests it vouches \
         for is no longer the set install.sh verifies against"
    );

    // The permissions the signing step needs, on the one job that has them. `id-token: write` is
    // the right to sign as this repository; a workflow that granted it at the top level would grant
    // it to every job, including the ones that run a build.
    let (before, publish) = text
        .split_once("\n  publish:")
        .expect("the release workflow has a publish job");
    for permission in ["id-token: write", "attestations: write"] {
        assert!(
            publish.contains(permission),
            "the publish job no longer asks for `{permission}`, so the attestation cannot be signed"
        );
        assert!(
            !before.contains(permission),
            "`{permission}` is granted outside the publish job — it is the right to sign as this \
             repository and belongs to the one step that signs"
        );
    }

    // A dry run attests too. This is the only way the step runs before the first tag is cut, which
    // is `release/README.md`'s point about the one artefact that cannot be executed before it is
    // used — and a `if: github.event_name == 'push'` here would take that away without failing
    // anything.
    //
    // The whole step, from its `- name:` to the next one — not the part after `uses:`. A condition
    // is legal anywhere among a step's keys, and the first version of this assertion read only the
    // tail: `if:` written above `uses:` passed it. Slice the step the way YAML delimits one.
    let step = publish
        .split("\n      - name:")
        .find(|step| step.contains("uses: actions/attest@"))
        .expect("the publish job attests what it publishes");
    assert!(
        !step.lines().any(|l| l.trim_start().starts_with("if:")),
        "the attest step is now conditional, so a workflow_dispatch dry run no longer exercises \
         it — and nothing does until the first tag is cut:\n{step}"
    );
}

#[test]
fn the_installer_refuses_an_archive_whose_provenance_does_not_verify() {
    let Some(release) = relfix::fixture("provenance") else {
        return;
    };
    let no = release.stub_gh("refuses", 1);

    let bin = release.root.join("bin");
    let out = release.install(
        &bin,
        &[
            ("BECK_VERIFY_PROVENANCE", "1"),
            ("BECK_GH", no.to_str().expect("utf-8")),
        ],
    );
    assert!(
        !out.status.success(),
        "the installer accepted an archive whose provenance did not verify:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !bin.join("beck").exists(),
        "the installer refused the provenance and installed the binary anyway"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("provenance"),
        "the refusal does not say what was wrong:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // …and a verifier that agrees installs. Asserted together with the refusal, because a script
    // that refused everything would pass the half above on its own.
    let yes = release.stub_gh("agrees", 0);
    let second = release.root.join("bin-2");
    let ok = release.install(
        &second,
        &[
            ("BECK_VERIFY_PROVENANCE", "1"),
            ("BECK_GH", yes.to_str().expect("utf-8")),
        ],
    );
    assert!(
        ok.status.success(),
        "the installer refused a release whose provenance verified:\n{}\n{}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(second.join("beck").exists(), "nothing was installed");

    // What the check was actually made with. `--signer-workflow` is the whole value of it: without
    // that flag any attestation this repository can produce satisfies the check, including one
    // minted by a workflow added by whoever could rewrite the release page.
    let argv = release.gh_argv("agrees");
    assert!(
        argv.contains("attestation verify"),
        "the installer ran the GitHub CLI for something other than verifying an attestation: {argv}"
    );
    assert!(
        argv.contains("--signer-workflow"),
        "the installer verifies provenance without pinning the workflow that signed it, so any \
         attestation this repository can produce satisfies it: {argv}"
    );
    let workflow = argv
        .split("--signer-workflow ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("a value for --signer-workflow");
    let (_, path) = workflow
        .split_once(".github/")
        .expect("--signer-workflow names a workflow file in the repository");
    assert!(
        repo_root().join(".github").join(path).is_file(),
        "the installer pins `{workflow}`, which is not a workflow file in this repository"
    );
}

#[test]
fn provenance_verification_is_refused_rather_than_skipped_when_the_tool_is_missing() {
    // The installer resolves its SHA-256 tool fatally for a stated reason — "an installer that
    // skips verification when the tool is missing has taught its users that verification is
    // optional" — and the verifier is the same argument with a second tool. A run that asked for
    // provenance and could not check it must not end with a binary on disk.
    let Some(release) = relfix::fixture("no-gh") else {
        return;
    };
    let absent = release.root.join("there-is-no-gh-here");

    let bin = release.root.join("bin");
    let out = release.install(
        &bin,
        &[
            ("BECK_VERIFY_PROVENANCE", "1"),
            ("BECK_GH", absent.to_str().expect("utf-8")),
        ],
    );
    assert!(
        !out.status.success(),
        "the installer was asked to verify provenance, had no tool to do it with, and installed \
         anyway:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !bin.join("beck").exists(),
        "the installer failed and left a binary behind"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("BECK_VERIFY_PROVENANCE") && stderr.contains("CLI"),
        "the refusal does not say which tool is missing or how to proceed without it:\n{stderr}"
    );
    // Before the download, not after it: nothing should be fetched for an install that cannot
    // finish. The fixture is local, so this is about the order rather than about the bytes.
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("sha256"),
        "the installer downloaded and checksummed an archive it was never going to install:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ---- the plumbing -----------------------------------------------------------------------------

fn script(sh: &Path, args: &[&str]) -> std::process::Output {
    Command::new(sh)
        .arg(repo_root().join("release/build.sh"))
        .args(args)
        .output()
        .expect("release/build.sh runs")
}
