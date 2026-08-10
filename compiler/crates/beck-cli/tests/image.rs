//! The in-process image build, and the signature over it.
//!
//! [`docs/06-kubernetes-and-packaging.md`](../../../../docs/06-kubernetes-and-packaging.md) §6.2
//! named the destination — "writing the OCI layout directly from Rust … so `beck build` is one
//! process with no external tools" — and [`docs/98`](../../../../docs/98-supply-chain-report.md) is
//! what building it found. This is the gate.
//!
//! # Somebody else's tar, and somebody else's verifier
//!
//! A tar writer tested by its own reader agrees with itself, and a signature checked by the library
//! that made it proves that the library is consistent. So the fixtures here are built by the
//! **system `tar`** and read by [`beck_infra::apk`], the layer this crate writes is read back by
//! the system `tar`, and the signature is verified by **`openssl`** as well as by
//! [`beck_infra::sign`]. That is the same discipline `read_models.rs` applies with somebody else's
//! Postgres client and `manifests.rs` with somebody else's YAML parser.
//!
//! Both are environment-dependent, so both **skip loudly**: `BECK_REQUIRE_TAR=1` and
//! `BECK_REQUIRE_OPENSSL=1` forbid the skip, and CI sets them. `docs/19` §19.4 item 10 is why the
//! skip prints — an artefact nobody executed that reports success is worse than one that reports
//! nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

use beck_infra::apk::{self, Kind};
use beck_infra::{oci, sign, InfraGraph};

/// A program with a durable fold and a UI — the one the rest of the suite builds against.
fn sketch() -> (beck_core::Placed, String) {
    let src = include_str!("../../../examples/todo.beck").to_string();
    let (placed, diags, map) = beck_core::compile_str("todo", &src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    (placed.expect("the sketch slices"), src)
}

fn graph() -> InfraGraph {
    beck_infra::graph(&sketch().0)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("beck-image-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn tool(name: &str, require: &str) -> Option<PathBuf> {
    let found = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file());
    if found.is_none() {
        assert!(
            std::env::var(require).as_deref() != Ok("1"),
            "{require}=1 and there is no `{name}` on the path"
        );
        println!("skipped: no `{name}`. Set {require}=1 to make this a failure.");
    }
    found
}

/// A package, built by the system `tar` rather than by the code that will read it.
///
/// The shape is an APK's: metadata dotfiles first — which the reader must drop — then the files the
/// package installs. Not a gzip *concatenation* of three members, which is what a real APK is; that
/// property is exercised by [`a_real_package_reads`], against a package this project did not build.
fn package(tar: &Path, dir: &Path, files: &[(&str, &str)], links: &[(&str, &str)]) -> Vec<u8> {
    let root = dir.join("root");
    for (path, body) in files {
        let at = root.join(path);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("a directory");
        std::fs::write(&at, body).expect("a file");
    }
    for (path, target) in links {
        let at = root.join(path);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("a directory");
        std::os::unix::fs::symlink(target, &at).expect("a symlink");
    }
    std::fs::write(root.join(".PKGINFO"), "pkgname = fixture\n").expect("the metadata");
    let out = dir.join("fixture.apk");
    let status = Command::new(tar)
        .arg("czf")
        .arg(&out)
        .arg("-C")
        .arg(&root)
        .arg(".")
        .status()
        .expect("running tar");
    assert!(status.success(), "tar failed");
    std::fs::read(&out).expect("the package")
}

fn fetched(name: &str, bytes: Vec<u8>) -> oci::Fetched {
    oci::Fetched {
        entry: apk::Entry {
            name: name.to_string(),
            version: "1-r0".to_string(),
            arch: "x86_64".to_string(),
            ..Default::default()
        },
        bytes,
    }
}

/// The two files the application contributes, as `beck image` assembles them.
fn application(source: &str) -> Vec<(String, u32, Vec<u8>)> {
    beck_infra::INSTALLS
        .iter()
        .map(|i| {
            let bytes = if i.path == beck_infra::APP_SOURCE {
                source.as_bytes().to_vec()
            } else {
                b"\x7fELF a toolchain".to_vec()
            };
            (i.path.to_string(), i.mode, bytes)
        })
        .collect()
}

fn built(dir: &Path, tar: &Path) -> oci::Image {
    let (_, source) = sketch();
    let base = package(
        tar,
        &dir.join("base"),
        &[
            ("etc/passwd", "root:x:0:0:root:/root:/sbin/nologin\n"),
            ("etc/group", "root:x:0:\n"),
            ("usr/share/zoneinfo/UTC", "a zone"),
        ],
        &[("bin", "usr/bin")],
    );
    let certs = package(
        tar,
        &dir.join("certs"),
        &[("etc/ssl/certs/ca-certificates.crt", "a trust store")],
        &[],
    );
    oci::build(
        &graph(),
        "dev",
        "x86_64",
        &[
            fetched("wolfi-baselayout", base),
            fetched("ca-certificates-bundle", certs),
        ],
        &application(&source),
    )
    .expect("the image builds")
}

#[test]
fn a_package_built_by_somebody_elses_tar_reads_back() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let dir = scratch("read");
    let bytes = package(
        &tar,
        &dir,
        &[("usr/bin/thing", "a binary"), ("etc/conf", "a setting")],
        &[("usr/bin/other", "thing")],
    );
    let contents = apk::Contents::read(&bytes).expect("reads");

    assert_eq!(contents.find("usr/bin/thing"), Some(b"a binary".as_slice()));
    // The metadata dotfiles are what an image must *not* contain: `.PKGINFO` in a running
    // container is a file apk put there for apk, and this build has no apk.
    assert!(
        !contents.files.iter().any(|f| f.path.contains("PKGINFO")),
        "{:?}",
        contents.files.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
    assert_eq!(
        contents
            .files
            .iter()
            .find(|f| f.path == "usr/bin/other")
            .map(|f| &f.kind),
        Some(&Kind::Symlink("thing".to_string())),
        "a symlink must survive as a symlink, not as a copy"
    );
    // Sorted, because the layer's order is the image's digest.
    let paths: Vec<&str> = contents.files.iter().map(|f| f.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
}

#[test]
fn a_real_package_reads() {
    // A Wolfi package is three gzip members concatenated, which a fixture built by `tar czf` is
    // not. Nothing in this repository ships one, so this runs only where a build has already
    // fetched one — after `beck image`, or in CI with a warm cache.
    let cache = std::env::var("BECK_APK_CACHE").unwrap_or_default();
    let Some(apk) = std::fs::read_dir(&cache)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "apk"))
    else {
        assert!(
            std::env::var("BECK_REQUIRE_APK").as_deref() != Ok("1"),
            "BECK_REQUIRE_APK=1 and BECK_APK_CACHE holds no .apk"
        );
        println!(
            "skipped: no .apk under BECK_APK_CACHE. Set BECK_REQUIRE_APK=1 to make this a failure."
        );
        return;
    };
    let contents = apk::Contents::read(&std::fs::read(&apk).expect("readable")).expect("reads");
    assert!(
        !contents.files.is_empty(),
        "{} unpacked to nothing",
        apk.display()
    );
    assert!(
        !contents.files.iter().any(|f| f.path.starts_with('.')),
        "a metadata dotfile reached the file list"
    );
}

#[test]
fn the_same_inputs_twice_produce_the_same_image() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let dir = scratch("twice");
    // §6.2's whole argument for this image format, mechanised: "the same config and package
    // versions yield the same image digest on any machine". Two builds, one digest.
    let a = built(&dir.join("a"), &tar);
    let b = built(&dir.join("b"), &tar);
    assert_eq!(a.digest(), b.digest());
    assert_eq!(a.diff_id, b.diff_id);
    assert_eq!(a.layer, b.layer);

    // And the property is worth nothing if every build produced the same digest: a changed program
    // must change it. This is the direction a constant satisfies, so both are asserted.
    let mut changed = a.rootfs.clone();
    changed.file(
        beck_infra::APP_SOURCE,
        0o644,
        b"a different program".to_vec(),
    );
    assert_ne!(
        oci::digest(&changed.tar().expect("tars")),
        a.diff_id,
        "a changed program produced the same layer"
    );
}

#[test]
fn the_layer_holds_what_the_packages_and_the_program_put_there() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let image = built(&scratch("contents"), &tar);
    let paths: Vec<&str> = image.rootfs.paths().collect();

    for installed in beck_infra::INSTALLS {
        assert!(
            paths.contains(&installed.path.trim_start_matches('/')),
            "{} is not in the image",
            installed.path
        );
    }
    for from_a_package in [
        "usr/share/zoneinfo/UTC",
        "etc/ssl/certs/ca-certificates.crt",
    ] {
        assert!(
            paths.contains(&from_a_package),
            "{from_a_package} is missing"
        );
    }
    assert!(
        !paths.iter().any(|p| p.contains("PKGINFO")),
        "package metadata reached the image"
    );
}

#[test]
fn the_account_the_image_runs_as_exists_in_it() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let image = built(&scratch("accounts"), &tar);
    let passwd = String::from_utf8_lossy(
        image
            .rootfs
            .file_at("etc/passwd")
            .expect("the image has a passwd file"),
    )
    .to_string();
    let account = beck_infra::NONROOT;

    // A container whose `User` names a uid with no passwd entry starts, and then every library that
    // looks the user up fails at a distance. The pod's securityContext, the apko config and this
    // file all name the same number, and the number is one constant.
    assert!(
        passwd.contains(&format!(
            "{}:x:{}:{}",
            account.user, account.uid, account.gid
        )),
        "{passwd}"
    );
    // …and the base package's own entries survived being added to.
    assert!(passwd.contains("root:x:0:0:"), "{passwd}");

    let config: serde_json::Value = serde_json::from_slice(&image.config).expect("the config");
    assert_eq!(
        config["config"]["User"],
        format!("{}:{}", account.uid, account.gid)
    );
}

#[test]
fn the_image_starts_the_process_the_apko_config_describes() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let image = built(&scratch("command"), &tar);
    let config: serde_json::Value = serde_json::from_slice(&image.config).expect("the config");

    // Read out of the *rendered* apko YAML rather than out of `beck_infra::command`, which is what
    // makes this a comparison rather than a function agreeing with itself (docs/92 §92.2). An image
    // that ran a different process than the config a reader checks would be two artefacts nobody
    // could compare.
    let apko = beck_infra::k8s::apko(&graph());
    let entrypoint = field(&apko, "  command: ");
    let cmd = field(&apko, "cmd: ");
    assert_eq!(
        config["config"]["Entrypoint"],
        serde_json::json!([entrypoint])
    );
    assert_eq!(
        config["config"]["Cmd"],
        serde_json::json!(cmd.split(' ').collect::<Vec<_>>())
    );
}

fn field(yaml: &str, key: &str) -> String {
    yaml.lines()
        .find_map(|l| l.strip_prefix(key))
        .unwrap_or_else(|| panic!("no {key:?} in the config"))
        .trim()
        .to_string()
}

#[test]
fn somebody_elses_tar_reads_the_layer_this_writes() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let dir = scratch("layer");
    let image = built(&dir, &tar);
    let layer = dir.join("layer.tar.gz");
    std::fs::write(&layer, &image.layer).expect("writing the layer");

    let out = Command::new(&tar)
        .arg("tzf")
        .arg(&layer)
        .output()
        .expect("running tar");
    assert!(
        out.status.success(),
        "tar refused the layer: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let listed: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end_matches('/').to_string())
        .collect();
    let mine: Vec<String> = image.rootfs.paths().map(str::to_string).collect();
    assert_eq!(listed, mine, "tar read a different set of paths");

    // And it can extract them: a header that lists but does not unpack is a header a runtime
    // rejects when it comes to build a rootfs.
    let into = dir.join("unpacked");
    std::fs::create_dir_all(&into).expect("a directory");
    let status = Command::new(&tar)
        .arg("xzf")
        .arg(&layer)
        .arg("-C")
        .arg(&into)
        .status()
        .expect("running tar");
    assert!(status.success(), "tar could not extract the layer");
    assert_eq!(
        std::fs::read(into.join(beck_infra::APP_SOURCE.trim_start_matches('/')))
            .expect("the program is in the extracted image"),
        sketch().1.as_bytes()
    );
}

#[test]
fn a_layout_round_trips_through_the_disk() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let dir = scratch("layout");
    let image = built(&dir, &tar);
    let layout = dir.join("image");
    image.write(&layout).expect("writes");

    assert_eq!(
        std::fs::read_to_string(layout.join("oci-layout"))
            .expect("the marker file")
            .trim(),
        "{\"imageLayoutVersion\":\"1.0.0\"}"
    );
    let index = oci::read_index(&layout).expect("an index");
    let (digest, reference) = oci::image_of(&index).expect("an image");
    assert_eq!(digest, image.digest());
    assert_eq!(reference, image.reference);
    // Every blob the manifest names is present, and hashes to the name it is stored under —
    // `read_blob` is what checks the second, and a layout that failed it would be one no runtime
    // could pull.
    let manifest: serde_json::Value =
        serde_json::from_slice(&oci::read_blob(&layout, &digest).expect("the manifest"))
            .expect("parses");
    for d in [
        &manifest["config"]["digest"],
        &manifest["layers"][0]["digest"],
    ] {
        oci::read_blob(&layout, d.as_str().expect("a digest")).expect("the blob is there");
    }
}

#[test]
fn a_signature_names_the_image_it_is_attached_to() {
    let Some(tar) = tool("tar", "BECK_REQUIRE_TAR") else {
        return;
    };
    let dir = scratch("sign");
    let image = built(&dir, &tar);
    let key = sign::Key::generate().expect("a key");
    let signature = sign::image(&key, &image.reference, &image.digest()).expect("signs");

    sign::verify(&signature.payload, &signature.signature, &key.public_pem()).expect("verifies");
    let payload: serde_json::Value = serde_json::from_slice(&signature.payload).expect("parses");
    assert_eq!(
        payload["critical"]["image"]["docker-manifest-digest"],
        image.digest(),
        "the signature must name this image and not a tag"
    );
    assert_eq!(
        signature.tag,
        format!("{}.sig", image.digest().replace(':', "-")),
        "cosign looks the signature up by this tag"
    );

    // The failure that matters: a signature that verifies over *some* digest says nothing about
    // this image. Rebuild the payload against a different digest and the signature must not hold.
    let elsewhere = sign::payload(&image.reference, &oci::digest(b"another image"));
    assert!(sign::verify(
        elsewhere.as_bytes(),
        &signature.signature,
        &key.public_pem()
    )
    .is_err());
}

#[test]
fn openssl_verifies_what_this_signed() {
    let Some(openssl) = tool("openssl", "BECK_REQUIRE_OPENSSL") else {
        return;
    };
    let dir = scratch("openssl");
    let key = sign::Key::generate().expect("a key");
    let digest = oci::digest(b"a manifest");
    let signature = sign::image(&key, "todo:dev", &digest).expect("signs");

    let (payload, der, public) = (
        dir.join("payload.json"),
        dir.join("signature.der"),
        dir.join("cosign.pub"),
    );
    std::fs::write(&payload, &signature.payload).expect("the payload");
    std::fs::write(&der, unbase64(&signature.signature)).expect("the signature");
    std::fs::write(&public, key.public_pem()).expect("the key");

    // `openssl dgst -verify` reads a SubjectPublicKeyInfo PEM and an ASN.1 DER ECDSA signature —
    // which is what `cosign verify --key` reads too. A signature only this crate can check is a
    // signature nobody downstream can act on.
    let out = Command::new(&openssl)
        .args(["dgst", "-sha256", "-verify"])
        .arg(&public)
        .arg("-signature")
        .arg(&der)
        .arg(&payload)
        .output()
        .expect("running openssl");
    assert!(
        out.status.success(),
        "openssl refused the signature: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // The negative control: openssl must also *refuse* a payload that was not signed.
    std::fs::write(&payload, b"not what was signed").expect("the payload");
    let out = Command::new(&openssl)
        .args(["dgst", "-sha256", "-verify"])
        .arg(&public)
        .arg("-signature")
        .arg(&der)
        .arg(&payload)
        .output()
        .expect("running openssl");
    assert!(!out.status.success(), "openssl accepted a forged payload");
}

/// Standard base64, for turning the annotation back into the DER openssl wants.
fn unbase64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut acc, mut bits, mut out) = (0u32, 0u32, Vec::new());
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let v = ALPHABET.iter().position(|a| *a == c).expect("base64");
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}
