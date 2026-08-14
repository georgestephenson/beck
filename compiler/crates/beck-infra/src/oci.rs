//! The image, built here rather than described for somebody else to build.
//!
//! [`docs/06-kubernetes-and-packaging.md`](../../../../../docs/06-kubernetes-and-packaging.md) §6.2:
//! "shell out to `apko` initially; move to writing the OCI layout directly from Rust … once the
//! format is settled, so `beck build` is one process with no external tools". This is that move.
//! [`crate::k8s::apko`] still renders the config — it is what a reader checks, and what an apko
//! build would consume — but nothing has to run apko, melange or a container daemon to get an
//! image out of `beck`.
//!
//! # Why this can be a pure function at all
//!
//! Because of the property §6.2 chose apko *for*: an image assembled by a build that executes
//! nothing and copies nothing arbitrary has contents that are already a list. So the whole build is
//! `packages + two files → one tar → three JSON documents`, with no daemon, no root, no shell and
//! no clock. [`docs/92`](../../../../../docs/92-supply-chain-and-release-report.md) §92.1 made the same observation
//! about the bill of materials; an SBOM was the projection that needed no pipeline, and this is the
//! artefact the projection describes.
//!
//! # Reproducible, and gated on being so
//!
//! Every field that would otherwise carry the time of day is absent: no `created` on the config, no
//! `created` on a history entry, mtime zero on every tar member, and the gzip header's mtime and OS
//! bytes written explicitly rather than defaulted. Two builds of one program from one package set
//! produce one digest, and `image.rs` asserts it — the mechanised form of the check the emitted
//! apko config's own comment describes.
//!
//! The limit worth stating: that is reproducibility of *this* build, holding the package set fixed.
//! The layer's compressed digest depends on the DEFLATE implementation, so it can move when
//! `flate2` does; the **`diff_id`** — the digest of the uncompressed tar — cannot, and both are in
//! the config where a reader can compare either.
//!
//! # What it does not do
//!
//! It builds one architecture, it does not push, and it has no opinion about a registry. Multi-arch
//! (§6.2's image index) needs a cross-compiled toolchain binary per architecture, which is a
//! release-pipeline fact rather than a compiler one; the index written here has one manifest in it
//! and is the shape that grows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::apk::{self, Kind};
use crate::{Account, InfraGraph, NONROOT};

/// The media type of an uncompressed layer's digest — what a `diff_id` is computed over.
pub const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
/// The media type of an image configuration blob.
pub const CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";
/// The media type of an image manifest.
pub const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
/// The media type of an image index.
pub const INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

pub use crate::ARCHITECTURES;

/// The OCI name for an apk architecture.
pub fn oci_arch(apk_arch: &str) -> Result<&'static str> {
    ARCHITECTURES
        .iter()
        .find(|(apk, _)| *apk == apk_arch)
        .map(|(_, oci)| *oci)
        .with_context(|| {
            format!(
                "unknown architecture {apk_arch:?} — known: {}",
                ARCHITECTURES
                    .iter()
                    .map(|(a, _)| *a)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// One member of the image's filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub kind: Kind,
}

/// The image's filesystem, as a map from path to member.
///
/// A map rather than a list, for two reasons that are the same reason: a later package overwrites
/// an earlier one's file exactly as `apk add` would, and iteration is in path order, so the tar
/// this produces does not depend on the order packages were fetched in.
#[derive(Clone, Debug, Default)]
pub struct Rootfs {
    members: BTreeMap<String, Member>,
}

impl Rootfs {
    pub fn new() -> Rootfs {
        Rootfs::default()
    }

    /// Every path in the image, in order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.members.keys().map(String::as_str)
    }

    /// The bytes of a regular file, if that is what is there.
    pub fn file_at(&self, path: &str) -> Option<&[u8]> {
        match self.members.get(path.trim_start_matches('/')) {
            Some(Member {
                kind: Kind::Regular(bytes),
                ..
            }) => Some(bytes),
            _ => None,
        }
    }

    /// Put a member at a path, creating the directories above it.
    pub fn insert(&mut self, path: &str, member: Member) {
        let path = path.trim_start_matches('/').to_string();
        // Every directory above the leaf, outermost first. A tar whose member has no parent entry
        // is legal and unpacks to root-owned 0755 anyway; writing them makes the layer's contents
        // the same list the image has, which is what the SBOM gate compares against.
        let mut prefix = String::new();
        let parents: Vec<&str> = path.split('/').collect();
        for part in &parents[..parents.len().saturating_sub(1)] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            self.members.entry(prefix.clone()).or_insert(Member {
                mode: 0o755,
                uid: 0,
                gid: 0,
                kind: Kind::Dir,
            });
        }
        self.members.insert(path, member);
    }

    /// A regular file owned by root.
    pub fn file(&mut self, path: &str, mode: u32, bytes: Vec<u8>) {
        self.insert(
            path,
            Member {
                mode,
                uid: 0,
                gid: 0,
                kind: Kind::Regular(bytes),
            },
        );
    }

    /// Everything a package installs.
    pub fn install(&mut self, contents: &apk::Contents) {
        for f in &contents.files {
            self.insert(
                &f.path,
                Member {
                    mode: f.mode,
                    uid: 0,
                    gid: 0,
                    kind: f.kind.clone(),
                },
            );
        }
    }

    /// The layer, as an uncompressed tar.
    ///
    /// ustar, mtime zero, no pax records and no device nodes — every field that varies between two
    /// runs of the same build is written as a constant, because the digest of this is the image's
    /// identity.
    pub fn tar(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for (path, member) in &self.members {
            let (typeflag, link, body): (u8, &str, &[u8]) = match &member.kind {
                Kind::Dir => (b'5', "", &[]),
                Kind::Regular(bytes) => (b'0', "", bytes.as_slice()),
                Kind::Symlink(target) => (b'2', target.as_str(), &[]),
                Kind::Hardlink(target) => (b'1', target.as_str(), &[]),
            };
            let name = match member.kind {
                Kind::Dir => format!("{path}/"),
                _ => path.clone(),
            };
            out.extend_from_slice(&header(&name, member, typeflag, link, body.len())?);
            out.extend_from_slice(body);
            let padding = (BLOCK - body.len() % BLOCK) % BLOCK;
            out.extend(std::iter::repeat_n(0u8, padding));
        }
        // The end-of-archive marker: two zero blocks, as the format requires.
        out.extend(std::iter::repeat_n(0u8, BLOCK * 2));
        Ok(out)
    }
}

const BLOCK: usize = 512;

/// One ustar header block.
fn header(name: &str, member: &Member, typeflag: u8, link: &str, size: usize) -> Result<[u8; 512]> {
    let mut h = [0u8; BLOCK];
    let (prefix, name) = split_name(name)?;
    h[0..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut h[100..108], u64::from(member.mode & 0o7777), 7);
    write_octal(&mut h[108..116], u64::from(member.uid), 7);
    write_octal(&mut h[116..124], u64::from(member.gid), 7);
    write_octal(&mut h[124..136], size as u64, 11);
    // mtime zero: an image whose digest moves with the clock is not the artefact §6.2 promised.
    write_octal(&mut h[136..148], 0, 11);
    h[148..156].fill(b' ');
    h[156] = typeflag;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let checksum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    write_octal(&mut h[148..154], u64::from(checksum), 6);
    h[154] = 0;
    h[155] = b' ';
    Ok(h)
}

/// Split a path across ustar's `prefix` and `name` fields.
///
/// A path longer than both is refused rather than truncated: a silently shortened path is a file in
/// the wrong place in a running container, and no package in a distroless image has one.
fn split_name(name: &str) -> Result<(&str, &str)> {
    if name.len() <= 100 {
        return Ok(("", name));
    }
    let split = name[..name.len().min(156)]
        .rfind('/')
        .filter(|i| name.len() - i - 1 <= 100 && *i <= 155)
        .with_context(|| format!("{name}: too long for a ustar header and has no split point"))?;
    Ok((&name[..split], &name[split + 1..]))
}

fn write_octal(field: &mut [u8], value: u64, digits: usize) {
    let text = format!("{value:0digits$o}");
    field[..text.len()].copy_from_slice(text.as_bytes());
    if text.len() < field.len() {
        field[text.len()] = 0;
    }
}

/// A built image: the blobs, and the three documents that name them.
#[derive(Clone, Debug)]
pub struct Image {
    /// `<name>:<tag>` — what the index annotates the manifest with.
    pub reference: String,
    /// The gzipped layer.
    pub layer: Vec<u8>,
    /// The digest of the layer *before* compression — the `diff_id` in the config, and the one
    /// number here that no compression library can move.
    pub diff_id: String,
    /// The image configuration blob, as it is written.
    pub config: Vec<u8>,
    /// The manifest blob, as it is written.
    pub manifest: Vec<u8>,
    /// The index blob, as it is written.
    pub index: Vec<u8>,
    /// The filesystem the layer was built from, for anything that wants to ask what is in it.
    pub rootfs: Rootfs,
}

impl Image {
    /// `sha256:…` for the manifest — the digest a deployment pins and a signature covers.
    pub fn digest(&self) -> String {
        digest(&self.manifest)
    }

    /// Write an OCI image layout: `oci-layout`, `index.json` and `blobs/sha256/…`.
    ///
    /// The layout rather than a Docker archive, because it is the format the spec defines and the
    /// one `skopeo copy oci:…` and `crane push` read; a `docker load` tarball is a second encoding
    /// of the same blobs and a second thing to keep right.
    pub fn write(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        let blobs = dir.join("blobs/sha256");
        std::fs::create_dir_all(&blobs).with_context(|| format!("creating {}", blobs.display()))?;
        let mut written = Vec::new();
        let blob = |bytes: &[u8], written: &mut Vec<PathBuf>| -> Result<()> {
            let path = blobs.join(digest(bytes).trim_start_matches("sha256:"));
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
            written.push(path);
            Ok(())
        };
        blob(&self.layer, &mut written)?;
        blob(&self.config, &mut written)?;
        blob(&self.manifest, &mut written)?;

        for (name, body) in [
            (
                "oci-layout",
                b"{\"imageLayoutVersion\":\"1.0.0\"}\n".to_vec(),
            ),
            ("index.json", self.index.clone()),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
            written.push(path);
        }
        Ok(written)
    }
}

/// The `index.json` of a layout on disk.
pub fn read_index(dir: &Path) -> Result<Value> {
    let path = dir.join("index.json");
    let bytes = std::fs::read(&path)
        .with_context(|| format!("{} is not an OCI layout — no index.json", dir.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
}

/// The image manifest an index describes: its digest and the reference it is tagged with.
///
/// The *image*, not a signature: a signed layout has two manifests in its index, and the one a
/// signature covers is the one whose reference is not a `.sig`.
pub fn image_of(index: &Value) -> Result<(String, String)> {
    index
        .get("manifests")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|m| {
            let reference = m
                .get("annotations")?
                .get(REF_ANNOTATION)?
                .as_str()?
                .to_string();
            let digest = m.get("digest")?.as_str()?.to_string();
            (!reference.ends_with(".sig")).then_some((digest, reference))
        })
        .context("the layout's index names no image manifest")
}

/// Write a blob, named by its own digest.
pub fn write_blob(dir: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let blobs = dir.join("blobs/sha256");
    std::fs::create_dir_all(&blobs).with_context(|| format!("creating {}", blobs.display()))?;
    let path = blobs.join(digest(bytes).trim_start_matches("sha256:"));
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Read a blob by digest, and check that it is the blob that digest names.
///
/// The check is the point: a layout is content-addressed, so a blob that does not hash to its own
/// name is a corrupted or substituted one, and reading it without noticing would make every
/// signature over it meaningless.
pub fn read_blob(dir: &Path, want: &str) -> Result<Vec<u8>> {
    let path = dir
        .join("blobs/sha256")
        .join(want.trim_start_matches("sha256:"));
    let bytes =
        std::fs::read(&path).with_context(|| format!("reading the blob {}", path.display()))?;
    let actual = digest(&bytes);
    if actual != want {
        bail!(
            "{} is {actual}, not the {want} its name claims",
            path.display()
        );
    }
    Ok(bytes)
}

/// Add a manifest to a layout's index, replacing any entry with the same reference.
pub fn attach(dir: &Path, descriptor: Value) -> Result<()> {
    let mut index = read_index(dir)?;
    let reference = descriptor
        .get("annotations")
        .and_then(|a| a.get(REF_ANNOTATION))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let manifests = index
        .get_mut("manifests")
        .and_then(Value::as_array_mut)
        .context("the layout's index has no manifests array")?;
    manifests.retain(|m| {
        m.get("annotations")
            .and_then(|a| a.get(REF_ANNOTATION))
            .and_then(Value::as_str)
            != Some(reference.as_str())
    });
    manifests.push(descriptor);
    let path = dir.join("index.json");
    std::fs::write(&path, document(&index)).with_context(|| format!("writing {}", path.display()))
}

/// A package as the build consumes it: what the index said, and the bytes.
pub struct Fetched {
    pub entry: apk::Entry,
    pub bytes: Vec<u8>,
}

/// Assemble the image.
///
/// `files` is the application's own content — [`crate::INSTALLS`] resolved against real bytes —
/// and it is the one thing here that does not come out of a package, because the toolchain and the
/// program are this build's *output* rather than somebody's dependency. The apko route ships them
/// as a melange-built package for the reason §6.2 gives (apko copies nothing from the host); a
/// build that *is* the compiler has them in hand and needs no intermediate.
pub fn build(
    graph: &InfraGraph,
    tag: &str,
    apk_arch: &str,
    packages: &[Fetched],
    files: &[(String, u32, Vec<u8>)],
) -> Result<Image> {
    let arch = oci_arch(apk_arch)?;
    let mut rootfs = Rootfs::new();

    for p in packages {
        let contents = apk::Contents::read(&p.bytes)
            .with_context(|| format!("reading the package {}", p.entry.file_name()))?;
        if contents.files.is_empty() {
            bail!(
                "{} installs no files — the archive is not an APK",
                p.entry.file_name()
            );
        }
        rootfs.install(&contents);
    }

    for (path, mode, bytes) in files {
        rootfs.file(path, *mode, bytes.clone());
    }
    accounts(&mut rootfs, NONROOT);

    let tar = rootfs.tar()?;
    let diff_id = digest(&tar);
    let layer = gzip(&tar)?;

    let command = crate::command();
    let (entrypoint, args) = command.split_at(1);
    let config = document(&json!({
        "architecture": arch,
        "os": "linux",
        "config": {
            "Entrypoint": entrypoint,
            "Cmd": args,
            "User": format!("{}:{}", NONROOT.uid, NONROOT.gid),
            "WorkingDir": "/",
            "Env": [format!("PATH={PATH}")],
            "ExposedPorts": { format!("{}/tcp", crate::APP_PORT): {} },
            "Labels": labels(graph),
        },
        "rootfs": { "type": "layers", "diff_ids": [diff_id] },
        // One entry, no timestamp, and `created_by` names the build rather than a shell command,
        // because there was no shell command: this is the whole history of an image assembled from
        // a package list.
        "history": [{
            "created_by": format!("beck image ({} packages, {} files)", packages.len(), files.len()),
            "comment": "assembled in-process from the object graph",
        }],
    }));

    let manifest = document(&json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "config": descriptor(CONFIG_MEDIA_TYPE, &config),
        "layers": [descriptor(LAYER_MEDIA_TYPE, &layer)],
        "annotations": labels(graph),
    }));

    let reference = format!("{}:{tag}", graph.app);
    let mut manifest_descriptor = descriptor(MANIFEST_MEDIA_TYPE, &manifest);
    manifest_descriptor["platform"] = json!({ "architecture": arch, "os": "linux" });
    manifest_descriptor["annotations"] = json!({ REF_ANNOTATION: reference });
    let index = document(&json!({
        "schemaVersion": 2,
        "mediaType": INDEX_MEDIA_TYPE,
        "manifests": [manifest_descriptor],
    }));

    Ok(Image {
        reference,
        layer,
        diff_id,
        config,
        manifest,
        index,
        rootfs,
    })
}

/// The annotation an OCI layout uses to name what a manifest is: a tool reads it as the tag.
pub const REF_ANNOTATION: &str = "org.opencontainers.image.ref.name";

/// The `PATH` a distroless image needs — the same one apko writes, and the reason `beck run` can be
/// spelled without a directory in the melange package's install step.
const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

fn labels(graph: &InfraGraph) -> Value {
    json!({
        "org.opencontainers.image.title": graph.app,
        "org.opencontainers.image.description": format!("The {} application, compiled by Beck.", graph.app),
        "org.opencontainers.image.vendor": "beck",
    })
}

/// `/etc/passwd` and `/etc/group`, with the account added rather than replacing whatever a base
/// package supplied — a distroless image that lost `root` from its passwd file is a surprise
/// waiting for the first tool that looks one up.
fn accounts(rootfs: &mut Rootfs, account: Account) {
    let mut passwd = String::from_utf8_lossy(
        rootfs
            .file_at("etc/passwd")
            .unwrap_or(b"root:x:0:0:root:/root:/sbin/nologin\n"),
    )
    .to_string();
    let mut group =
        String::from_utf8_lossy(rootfs.file_at("etc/group").unwrap_or(b"root:x:0:\n")).to_string();
    for text in [&mut passwd, &mut group] {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
    }
    if !passwd
        .lines()
        .any(|l| l.starts_with(&format!("{}:", account.user)))
    {
        passwd.push_str(&format!(
            "{}:x:{}:{}:{}:/home/{}:/sbin/nologin\n",
            account.user, account.uid, account.gid, account.user, account.user
        ));
    }
    if !group
        .lines()
        .any(|l| l.starts_with(&format!("{}:", account.group)))
    {
        group.push_str(&format!("{}:x:{}:\n", account.group, account.gid));
    }
    rootfs.file("etc/passwd", 0o644, passwd.into_bytes());
    rootfs.file("etc/group", 0o644, group.into_bytes());
}

/// A JSON blob as it is written to disk: compact, and byte-stable for one input.
fn document(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a JSON document built from literals serialises")
}

fn descriptor(media_type: &str, bytes: &[u8]) -> Value {
    json!({
        "mediaType": media_type,
        "digest": digest(bytes),
        "size": bytes.len(),
    })
}

/// The digest OCI names every blob by.
pub fn digest(bytes: &[u8]) -> String {
    let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, bytes);
    let mut out = String::from("sha256:");
    for b in d.as_ref() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// gzip, with every header byte that could carry a clock or a hostname written as a constant.
fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write as _;
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).context("compressing the layer")?;
    encoder.finish().context("finishing the layer")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rootfs() -> Rootfs {
        let mut fs = Rootfs::new();
        fs.file("/usr/bin/beck", 0o755, b"ELF".to_vec());
        fs.file("/app/app.beck", 0o644, b"log Todo\n".to_vec());
        fs
    }

    #[test]
    fn a_file_brings_its_directories_with_it() {
        let fs = rootfs();
        let paths: Vec<&str> = fs.paths().collect();
        assert_eq!(
            paths,
            ["app", "app/app.beck", "usr", "usr/bin", "usr/bin/beck"]
        );
    }

    #[test]
    fn the_tar_is_a_multiple_of_the_block_size_and_reads_back() {
        let tar = rootfs().tar().expect("tars");
        assert_eq!(tar.len() % BLOCK, 0);
        // Read it with this crate's own reader, which is the one that parses somebody else's tars.
        let back = apk::Contents::read(&gzip(&tar).expect("gzips")).expect("reads back");
        assert_eq!(back.find("usr/bin/beck"), Some(b"ELF".as_slice()));
    }

    #[test]
    fn two_tars_of_one_filesystem_are_identical() {
        assert_eq!(rootfs().tar().expect("a"), rootfs().tar().expect("b"));
    }

    #[test]
    fn a_long_path_splits_across_the_ustar_prefix() {
        let long = format!("usr/share/zoneinfo/{}/City", "a".repeat(120));
        let (prefix, name) = split_name(&long).expect("splits");
        assert_eq!(name, "City");
        assert!(prefix.len() <= 155 && !prefix.is_empty());
    }

    #[test]
    fn an_architecture_this_does_not_know_is_refused() {
        assert!(oci_arch("x86_64").is_ok());
        assert!(oci_arch("s390x").is_err());
    }
}
