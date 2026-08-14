//! A fixture release, and [`install.sh`](../../../../../install.sh) run against it.
//!
//! Two harnesses install from this: `release.rs`, which asserts that the installer refuses a
//! corrupted archive and unverifiable provenance, and `pending_security.rs`, which asserts that
//! the **default** path checks neither signature nor provenance. They have to agree about what a
//! release looks like or one of them is testing a shape the other does not produce, so the
//! fixture is defined once here.
//!
//! Everything is local. The assets are a tarball with a stub `beck` in it and a `SHA256SUMS`
//! beside it, reached over `file://`, so nothing here needs a network or a published release —
//! and [`stub_gh`](Release::stub_gh) stands in for the GitHub CLI, which means the provenance
//! path is exercised on a machine that has never seen `gh`. What that buys is the installer's
//! *behaviour* around the verifier: whether a refusal stops the install, whether a missing tool is
//! fatal, and what arguments the check is made with. What it cannot buy is a real attestation —
//! `docs/92-supply-chain-and-release-report.md` §92.12 says so under its own heading rather than leaving it to
//! be inferred from the word "stub".

#![allow(dead_code)] // each test binary uses the half of this it needs

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A fixture release on disk: assets to install from, and somewhere to install to.
pub struct Release {
    /// The scratch directory holding everything below.
    pub root: PathBuf,
    /// What `BECK_BASE_URL` points at — the tarball and its `SHA256SUMS`.
    pub assets: PathBuf,
    pub version: &'static str,
    pub target: &'static str,
    sh: PathBuf,
}

/// The fixture, or `None` when a tool it needs is missing — in which case the caller returns and
/// the skip has already printed itself.
pub fn fixture(name: &str) -> Option<Release> {
    let sh = tool("sh")?;
    let tar = tool("tar")?;
    if which("sha256sum").is_none() && which("shasum").is_none() {
        skip("no sha256sum and no shasum");
        return None;
    }
    if which("curl").is_none() && which("wget").is_none() {
        skip("no curl and no wget");
        return None;
    }

    let version = env!("CARGO_PKG_VERSION");
    let target = "x86_64-unknown-linux-gnu";
    let root = scratch(name);
    let assets = root.join("assets");
    let stage = root.join("stage");
    let dir = stage.join(format!("beck-{version}-{target}"));
    std::fs::create_dir_all(&assets).expect("scratch");
    std::fs::create_dir_all(&dir).expect("scratch");

    // A stub that answers `--version`, because the installer runs what it installed.
    let stub = dir.join("beck");
    std::fs::write(&stub, "#!/bin/sh\necho \"beck fixture\"\n").expect("write");
    make_executable(&stub);

    let release = Release {
        root,
        assets,
        version,
        target,
        sh,
    };
    let built = Command::new(&tar)
        .args(["-czf", release.asset_path().to_str().expect("utf-8"), "-C"])
        .arg(&stage)
        .arg(format!("beck-{version}-{target}"))
        .status()
        .expect("tar runs");
    assert!(built.success(), "the fixture tarball was not built");
    release.write_sums();
    Some(release)
}

impl Release {
    pub fn asset(&self) -> String {
        format!("beck-{}-{}.tar.gz", self.version, self.target)
    }

    pub fn asset_path(&self) -> PathBuf {
        self.assets.join(self.asset())
    }

    /// The digest the release publishes, rewritten from whatever the tarball currently is.
    pub fn write_sums(&self) {
        let sum = sha256(&self.asset_path());
        std::fs::write(
            self.assets.join("SHA256SUMS"),
            format!("{sum}  {}\n", self.asset()),
        )
        .expect("write");
    }

    /// The digest in `SHA256SUMS` — what the installer is expected to print on success.
    pub fn published_digest(&self) -> String {
        let sums = std::fs::read_to_string(self.assets.join("SHA256SUMS")).expect("written above");
        sums.split_whitespace()
            .next()
            .expect("a digest")
            .to_string()
    }

    /// Flip a byte of the archive and leave `SHA256SUMS` alone — the artefact moved and the
    /// published digest did not, which is the shape of the failure the checksum is for.
    pub fn corrupt_the_archive(&self) {
        let mut bytes = std::fs::read(self.asset_path()).expect("read");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(self.asset_path(), bytes).expect("write");
    }

    /// A stand-in for the GitHub CLI that records how it was called and then exits with `code`.
    ///
    /// Read back with [`gh_argv`](Self::gh_argv). Recording the arguments is the point of the
    /// successful one: `--signer-workflow` is what makes the check mean anything, and a test that
    /// only asserted "the installer ran something" would pass without it.
    pub fn stub_gh(&self, name: &str, code: i32) -> PathBuf {
        let path = self.root.join(format!("{name}-gh"));
        let log = self.gh_log(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >>'{}'\nexit {code}\n",
                log.display()
            ),
        )
        .expect("write");
        make_executable(&path);
        path
    }

    pub fn gh_log(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}-gh.argv"))
    }

    /// What the stub was called with, or the empty string if it was never called.
    pub fn gh_argv(&self, name: &str) -> String {
        std::fs::read_to_string(self.gh_log(name)).unwrap_or_default()
    }

    /// Run the installer against this fixture, with `env` on top of what it always needs.
    pub fn install(&self, into: &Path, env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(&self.sh);
        command
            .arg(repo_root().join("install.sh"))
            .env("BECK_VERSION", self.version)
            .env("BECK_TARGET", self.target)
            .env("BECK_BASE_URL", format!("file://{}", self.assets.display()))
            .env("BECK_INSTALL_DIR", into);
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().expect("install.sh runs")
    }
}

pub fn repo_root() -> PathBuf {
    // .../compiler/crates/beck-cli → the repository
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the crate lives three levels under the repository root")
        .to_path_buf()
}

pub fn skip(why: &str) {
    // A skip that prints, per `docs/19-phase-1-report.md` §19.4 item 10: a gate that reports
    // success without running is worse than one that reports nothing.
    assert!(
        std::env::var("BECK_REQUIRE_INSTALL").is_err(),
        "BECK_REQUIRE_INSTALL is set and this test cannot run: {why}"
    );
    eprintln!("relfix: skipping — {why}");
}

pub fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(tool))
        .find(|candidate| candidate.is_file())
}

pub fn tool(name: &str) -> Option<PathBuf> {
    match which(name) {
        Some(path) => Some(path),
        None => {
            skip(&format!("no {name} on the path"));
            None
        }
    }
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("beck-release-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

pub fn sha256(path: &Path) -> String {
    let (tool, args): (PathBuf, Vec<&str>) = match which("sha256sum") {
        Some(t) => (t, vec![]),
        None => (
            which("shasum").expect("checked by the fixture"),
            vec!["-a", "256"],
        ),
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

pub fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    #[cfg(not(unix))]
    let _ = path;
}
