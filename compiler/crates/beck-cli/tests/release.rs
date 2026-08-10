//! The release pipeline and the installer, gated.
//!
//! `docs/28-releases-and-deployment.md` opens with the discipline these tests exist to serve: "a
//! pipeline is an artefact, and an artefact nobody has executed is a design document". A release
//! workflow cannot be executed before a tag is cut, so what is executable is factored out of it —
//! `release/build.sh` builds one artefact, `install.sh` installs one, and both run on a laptop.
//! This file runs them, and checks the parts of the workflow that only a reader could otherwise
//! check.
//!
//! Three properties, and the third is the one that matters:
//!
//! 1. **The installer and the pipeline name the same platforms**, in both directions. An installer
//!    offering a target no release contains is a 404 in somebody's first five minutes; a target
//!    built and never offered is an artefact nobody can reach.
//! 2. **Nothing is published before the whole suite has run.** §28.2 item 1 is a `needs:` edge, and
//!    an edge is checkable.
//! 3. **The installer refuses an archive whose checksum is wrong**, tested by corrupting one. That
//!    is the gap this script exists to close, so it is the gap the gate is written against
//!    (`docs/84-a-quota-is-only-as-good-as-its-actor-report.md` §84.5), rather than the presence of
//!    a `sha256sum` call somewhere in the file.
//!
//! The last one needs `sh`, `tar` and a SHA-256 tool, so it **skips loudly** when one is missing;
//! `BECK_REQUIRE_INSTALL=1` forbids the skip.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    // .../compiler/crates/beck-cli → the repository
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate lives three levels under the repository root")
        .to_path_buf()
}

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
    let Some(sh) = tool("sh") else { return };
    let Some(tar) = tool("tar") else { return };
    if which("sha256sum").is_none() && which("shasum").is_none() {
        skip("no sha256sum and no shasum");
        return;
    }
    if which("curl").is_none() && which("wget").is_none() {
        skip("no curl and no wget");
        return;
    }

    let version = env!("CARGO_PKG_VERSION");
    let target = "x86_64-unknown-linux-gnu";
    let root = scratch("install");
    let assets = root.join("assets");
    let stage = root.join("stage").join(format!("beck-{version}-{target}"));
    std::fs::create_dir_all(&assets).expect("scratch");
    std::fs::create_dir_all(&stage).expect("scratch");

    // A stub that answers `--version`, because the installer runs what it installed.
    let stub = stage.join("beck");
    std::fs::write(&stub, "#!/bin/sh\necho \"beck fixture\"\n").expect("write");
    make_executable(&stub);

    let asset = format!("beck-{version}-{target}.tar.gz");
    let built = Command::new(&tar)
        .args(["-czf", assets.join(&asset).to_str().expect("utf-8"), "-C"])
        .arg(root.join("stage"))
        .arg(format!("beck-{version}-{target}"))
        .status()
        .expect("tar runs");
    assert!(built.success(), "the fixture tarball was not built");

    let good = sha256(&assets.join(&asset));
    std::fs::write(assets.join("SHA256SUMS"), format!("{good}  {asset}\n")).expect("write");

    // 1. A release whose checksum agrees installs, and the binary is on disk afterwards.
    let bin = root.join("bin");
    let ok = install(&sh, &assets, &bin, version, target);
    assert!(
        ok.status.success(),
        "the installer refused a release whose checksum is correct:\n{}\n{}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(bin.join("beck").exists(), "nothing was installed");
    assert!(
        String::from_utf8_lossy(&ok.stdout).contains(&good),
        "the installer did not print the checksum it verified"
    );

    // 2. The same release with one byte of the archive changed installs nothing. The checksum file
    //    is left alone, which is the shape of the failure this is about: the artefact moved and the
    //    published digest did not.
    let mut bytes = std::fs::read(assets.join(&asset)).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(assets.join(&asset), bytes).expect("write");

    let fresh = root.join("bin-2");
    let bad = install(&sh, &assets, &fresh, version, target);
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

// ---- the plumbing -----------------------------------------------------------------------------

fn skip(why: &str) {
    // A skip that prints, per `docs/19-phase-1-report.md` §19.4 item 10: a gate that reports
    // success without running is worse than one that reports nothing.
    assert!(
        std::env::var("BECK_REQUIRE_INSTALL").is_err(),
        "BECK_REQUIRE_INSTALL is set and this test cannot run: {why}"
    );
    eprintln!("release.rs: skipping — {why}");
}

fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
}

fn tool(name: &str) -> Option<PathBuf> {
    match which(name) {
        Some(path) => Some(path),
        None => {
            skip(&format!("no {name} on the path"));
            None
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("beck-release-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn script(sh: &Path, args: &[&str]) -> std::process::Output {
    Command::new(sh)
        .arg(repo_root().join("release/build.sh"))
        .args(args)
        .output()
        .expect("release/build.sh runs")
}

fn install(
    sh: &Path,
    assets: &Path,
    into: &Path,
    version: &str,
    target: &str,
) -> std::process::Output {
    Command::new(sh)
        .arg(repo_root().join("install.sh"))
        .env("BECK_VERSION", version)
        .env("BECK_TARGET", target)
        .env("BECK_BASE_URL", format!("file://{}", assets.display()))
        .env("BECK_INSTALL_DIR", into)
        .output()
        .expect("install.sh runs")
}

fn sha256(path: &Path) -> String {
    let (tool, args): (PathBuf, Vec<&str>) = match which("sha256sum") {
        Some(t) => (t, vec![]),
        None => (which("shasum").expect("checked above"), vec!["-a", "256"]),
    };
    let out = Command::new(tool)
        .args(args)
        .arg(path)
        .output()
        .expect("a sha256 tool runs");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .expect("a digest")
        .to_string()
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    #[cfg(not(unix))]
    let _ = path;
}
