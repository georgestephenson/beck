//! `beck image`, `beck sign` and `beck verify` — the supply-chain commands, wired together.
//!
//! The build itself is [`beck_infra::oci`] and takes bytes; this is the part that decides *which*
//! bytes: the index of a repository, the packages [`beck_infra::sbom::packages`] names, and the two
//! files [`beck_infra::INSTALLS`] says the application contributes.
//!
//! # The toolchain the image ships is, by default, the one doing the building
//!
//! An image built here contains `beck` and the program, and `--binary` names the first. Defaulting
//! it to the running executable is not a convenience: it means the image a developer builds runs
//! the compiler that compiled it, so "it worked locally" and "it works in the container" are
//! statements about one binary. A release builds with `--binary` pointing at a static musl build,
//! which is what [`docs/28`](../../../../../docs/28-releases-and-deployment.md) §28.2 item 1
//! describes, and the digest of whatever was used is printed either way.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use beck_infra::{oci, sign};

use crate::fetch::Fetcher;

/// Everything `beck image` was told.
pub struct Options<'a> {
    pub out: &'a Path,
    pub tag: &'a str,
    pub arch: &'a str,
    pub repository: &'a str,
    pub cache: &'a Path,
    pub offline: bool,
    pub binary: Option<&'a Path>,
    pub sign_with: Option<&'a Path>,
}

/// Build the image, and say what went into it.
pub fn build(placed: &beck_core::Placed, source: &str, options: &Options) -> Result<()> {
    let graph = beck_infra::graph(placed);
    // Refused before anything is fetched: an unknown architecture discovered after a 40 MB
    // download is a worse error message for no reason.
    oci::oci_arch(options.arch)?;

    let fetcher = Fetcher::new(options.cache, options.offline)?;
    let index_url = format!(
        "{}/{}/APKINDEX.tar.gz",
        options.repository.trim_end_matches('/'),
        options.arch
    );
    let index = beck_infra::apk::Index::read(&fetcher.get(&index_url)?)
        .with_context(|| format!("reading the package index at {index_url}"))?;

    // The same list the SBOM and the apko config are rendered from, minus the application's own
    // package: `@local` is this build's output rather than something to resolve (docs/92 §92.2).
    let wanted: Vec<String> = beck_infra::sbom::packages(&graph)
        .into_iter()
        .filter(|p| !p.local)
        .map(|p| p.name)
        .collect();
    let resolution = index.resolve(&wanted)?;
    if !resolution.constraints.is_empty() {
        // Printed rather than swallowed: the resolver reads version constraints and does not
        // enforce them, and a constraint nobody checked is a constraint nobody should believe was
        // met (`beck_infra::apk`).
        println!(
            "note: {} version constraint(s) read and not enforced: {}",
            resolution.constraints.len(),
            resolution.constraints.join(" ")
        );
    }

    let mut packages = Vec::new();
    for entry in &resolution.packages {
        let url = entry.url(options.repository, options.arch);
        let bytes = fetcher.get(&url)?;
        println!(
            "  {:<28} {:<16} {} bytes  {}",
            entry.name,
            entry.version,
            bytes.len(),
            oci::digest(&bytes)
        );
        packages.push(oci::Fetched {
            entry: entry.clone(),
            bytes,
        });
    }

    let files = application(source, options.binary)?;
    let image = oci::build(&graph, options.tag, options.arch, &packages, &files)?;
    let written = image.write(options.out)?;

    println!("{}", options.out.join("index.json").display());
    println!(
        "{} blobs, {} paths in the layer",
        written.len(),
        image.rootfs.paths().count()
    );
    println!("reference {}", image.reference);
    println!("manifest  {}", image.digest());
    println!("diff id   {}", image.diff_id);

    if let Some(key) = options.sign_with {
        attach_signature(options.out, Some(key))?;
    }
    Ok(())
}

/// The two files the application contributes, read from disk.
fn application(source: &str, binary: Option<&Path>) -> Result<Vec<(String, u32, Vec<u8>)>> {
    let toolchain = match binary {
        Some(p) => p.to_path_buf(),
        None => std::env::current_exe().context("finding the running `beck` binary")?,
    };
    let mut out = Vec::new();
    for installed in beck_infra::INSTALLS {
        let bytes = if installed.path == beck_infra::APP_SOURCE {
            source.as_bytes().to_vec()
        } else {
            std::fs::read(&toolchain)
                .with_context(|| format!("reading the toolchain {}", toolchain.display()))?
        };
        println!(
            "  {:<28} {:<16} {} bytes  {}",
            installed.path,
            "(this build)",
            bytes.len(),
            oci::digest(&bytes)
        );
        if let Some(interpreter) = dynamic_interpreter(&bytes) {
            // A distroless Wolfi base has no dynamic loader, so an image whose entrypoint needs one
            // starts and dies with a message about a missing file — which is docs/19 §19.5's defect
            // exactly: an image config that could not work, invisible until something ran it. Said
            // here rather than refused, because a base that *does* carry a loader is a package list
            // away.
            eprintln!(
                "warning: {} needs the dynamic loader {interpreter}, and a distroless image has \n\
                 none. Build the toolchain for a musl target and pass it with --binary, or add the \n\
                 package that provides the loader.",
                installed.path
            );
        }
        out.push((installed.path.to_string(), installed.mode, bytes));
    }
    Ok(out)
}

/// The ELF interpreter a binary needs, if it needs one.
///
/// Read from the program headers rather than guessed from a string search: `PT_INTERP` is the field
/// that decides whether the kernel has to find a loader before the program runs, and its absence is
/// what "statically linked" means to the thing that has to start it.
fn dynamic_interpreter(bytes: &[u8]) -> Option<String> {
    const PT_INTERP: u32 = 3;
    let u16_at = |at: usize| -> Option<usize> {
        let b = bytes.get(at..at + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b = bytes.get(at..at + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let u64_at = |at: usize| -> Option<usize> {
        let b = bytes.get(at..at + 8)?;
        Some(u64::from_le_bytes(b.try_into().ok()?) as usize)
    };
    // 64-bit, little-endian ELF only — every target this builds an image for, and a header this
    // does not recognise produces no warning rather than a wrong one.
    if bytes.get(..5)? != b"\x7fELF\x02" || bytes.get(5) != Some(&1) {
        return None;
    }
    let (offset, entry_size, count) = (u64_at(0x20)?, u16_at(0x36)?, u16_at(0x38)?);
    for i in 0..count {
        let header = offset.checked_add(i.checked_mul(entry_size)?)?;
        if u32_at(header)? != PT_INTERP {
            continue;
        }
        let (at, size) = (u64_at(header + 0x08)?, u64_at(header + 0x20)?);
        let path = bytes.get(at..at.checked_add(size)?)?;
        return Some(
            String::from_utf8_lossy(path)
                .trim_end_matches('\0')
                .to_string(),
        );
    }
    None
}

/// Sign the image an OCI layout holds, and attach the signature to the layout.
pub fn attach_signature(layout: &Path, key: Option<&Path>) -> Result<()> {
    let pem = read_key(key)?;
    let key = sign::Key::from_pem(&pem)?;
    let index = oci::read_index(layout)?;
    let (digest, reference) = oci::image_of(&index)?;

    let signature = sign::image(&key, &reference, &digest)?;
    oci::write_blob(layout, &signature.payload)?;
    oci::write_blob(layout, &signature.config)?;
    oci::write_blob(layout, &signature.manifest)?;
    oci::attach(
        layout,
        serde_json::json!({
            "mediaType": oci::MANIFEST_MEDIA_TYPE,
            "digest": oci::digest(&signature.manifest),
            "size": signature.manifest.len(),
            "annotations": { oci::REF_ANNOTATION: signature.tag },
        }),
    )?;

    println!("signed {reference}");
    println!("  digest {digest}");
    println!("  tag    {}", signature.tag);
    Ok(())
}

/// Check the signature in a layout against a public key.
///
/// Three things, and the third is the one that matters: the signature verifies, the payload is a
/// cosign container-image signature, and **the digest it names is the image manifest actually in
/// this layout**. A signature that verifies over somebody else's digest is a valid signature and a
/// worthless one.
pub fn verify(layout: &Path, public_key: &Path) -> Result<()> {
    let pem = std::fs::read_to_string(public_key)
        .with_context(|| format!("reading {}", public_key.display()))?;
    let index = oci::read_index(layout)?;
    let (digest, reference) = oci::image_of(&index)?;

    let manifests = index
        .get("manifests")
        .and_then(|m| m.as_array())
        .context("the layout's index has no manifests array")?;
    let signature_descriptor = manifests
        .iter()
        .find(|m| {
            m.get("annotations")
                .and_then(|a| a.get(oci::REF_ANNOTATION))
                .and_then(|r| r.as_str())
                .is_some_and(|r| r.ends_with(".sig"))
        })
        .with_context(|| format!("{} holds no signature", layout.display()))?;

    let manifest: serde_json::Value = serde_json::from_slice(&oci::read_blob(
        layout,
        signature_descriptor["digest"].as_str().unwrap_or_default(),
    )?)
    .context("parsing the signature manifest")?;
    let layer = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .and_then(|l| l.first())
        .context("the signature manifest has no layer")?;
    let payload = oci::read_blob(layout, layer["digest"].as_str().unwrap_or_default())?;
    let signature = layer
        .get("annotations")
        .and_then(|a| a.get(sign::SIGNATURE_ANNOTATION))
        .and_then(|s| s.as_str())
        .context("the signature layer carries no signature annotation")?;

    sign::verify(&payload, signature, &pem)?;

    let claimed: serde_json::Value =
        serde_json::from_slice(&payload).context("the payload is not JSON")?;
    let signed_digest = claimed["critical"]["image"]["docker-manifest-digest"]
        .as_str()
        .unwrap_or_default();
    if signed_digest != digest {
        bail!("the signature is over {signed_digest}, and this layout holds {digest}");
    }
    if claimed["critical"]["type"].as_str() != Some(sign::SIGNATURE_TYPE) {
        bail!("the payload is not a container image signature");
    }

    println!("verified {reference}");
    println!("  digest {digest}");
    println!("  key    {}", public_key.display());
    Ok(())
}

/// A signing key, from a file or from the environment.
///
/// The environment is how CI holds one — a private key on a command line is a private key in the
/// process table — and `beck init ci` emits a workflow that sets `BECK_SIGNING_KEY`.
pub fn read_key(path: Option<&Path>) -> Result<String> {
    match path {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        }
        None => std::env::var("BECK_SIGNING_KEY")
            .context("no --key, and no BECK_SIGNING_KEY in the environment"),
    }
}

/// `beck key generate` — a signing key, and the public half a consumer verifies with.
pub fn generate_key(out: &Path) -> Result<()> {
    let key = sign::Key::generate()?;
    let private = out.with_extension("key");
    let public = out.with_extension("pub");
    if private.exists() {
        bail!(
            "{} exists — refusing to overwrite a signing key",
            private.display()
        );
    }
    write_private(&private, &key.private_pem())?;
    std::fs::write(&public, key.public_pem())
        .with_context(|| format!("writing {}", public.display()))?;
    println!("{}", private.display());
    println!("{}", public.display());
    println!(
        "The private half is the secret. Put its contents in BECK_SIGNING_KEY; commit {} so a \n\
         consumer can run `cosign verify --key {}`.",
        public.display(),
        public.display()
    );
    Ok(())
}

/// Write a private key so that only its owner can read it.
///
/// A key written 0644 into a repository checkout is a key that has been disclosed to every process
/// on the machine, and the default umask is not a control.
fn write_private(path: &PathBuf, pem: &str) -> Result<()> {
    std::fs::write(path, pem).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64-bit little-endian ELF with one program header, `PT_INTERP`, naming a loader.
    fn elf(interpreter: Option<&str>) -> Vec<u8> {
        let (offset, entry_size) = (64usize, 56usize);
        let mut bytes = vec![0u8; offset + entry_size];
        bytes[..5].copy_from_slice(b"\x7fELF\x02");
        bytes[5] = 1; // little-endian
        bytes[0x20..0x28].copy_from_slice(&(offset as u64).to_le_bytes());
        bytes[0x36..0x38].copy_from_slice(&(entry_size as u16).to_le_bytes());
        bytes[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
        // PT_LOAD when there is no interpreter: a static binary has program headers too, so the
        // absence this checks for has to be the absence of the *right* one.
        let kind: u32 = if interpreter.is_some() { 3 } else { 1 };
        bytes[offset..offset + 4].copy_from_slice(&kind.to_le_bytes());
        if let Some(path) = interpreter {
            let at = bytes.len();
            bytes[offset + 0x08..offset + 0x10].copy_from_slice(&(at as u64).to_le_bytes());
            let body = format!("{path}\0");
            bytes[offset + 0x20..offset + 0x28].copy_from_slice(&(body.len() as u64).to_le_bytes());
            bytes.extend_from_slice(body.as_bytes());
        }
        bytes
    }

    #[test]
    fn a_binary_that_needs_a_loader_says_which_one() {
        assert_eq!(
            dynamic_interpreter(&elf(Some("/lib/ld-linux-x86-64.so.2"))).as_deref(),
            Some("/lib/ld-linux-x86-64.so.2")
        );
    }

    #[test]
    fn a_static_binary_needs_none() {
        assert_eq!(dynamic_interpreter(&elf(None)), None);
    }

    #[test]
    fn something_that_is_not_an_elf_produces_no_claim_either_way() {
        // A warning derived from a header this cannot read would be a guess, and a wrong guess
        // about the entrypoint is worse than silence.
        assert_eq!(dynamic_interpreter(b"#!/bin/sh\n"), None);
        assert_eq!(dynamic_interpreter(&[]), None);
        // Truncated after a valid magic: every read is bounds-checked, so this answers rather than
        // panics.
        assert_eq!(dynamic_interpreter(b"\x7fELF\x02\x01"), None);
    }
}
