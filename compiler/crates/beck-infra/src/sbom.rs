//! The bill of materials for what `beck build` emits — CycloneDX, derived rather than declared.
//!
//! [`docs/08-roadmap.md`](../../../../../docs/08-roadmap.md) lists "`beck init ci`, apko image build
//! in-process, cosign signing, SBOM" as one Phase 3 bullet, and
//! [`docs/42`](../../../../../docs/42-security-assurance.md) §42.8 puts CISA's 2026 minimum elements
//! and SLSA v1.2 on the ladder. This is the SBOM, and the reason it can exist now rather than
//! after a release pipeline is that **an image whose contents are derived has a component list
//! already**: §6.2's apko config performs no arbitrary execution and copies nothing from the host,
//! so what is in the image is what the graph put there.
//!
//! # The one rule that makes it worth having
//!
//! An SBOM assembled beside the thing it describes is an SBOM that can be wrong about it. So the
//! package list here and the `packages:` block of the apko config come from **one function** —
//! [`packages`] — and `supply_chain.rs` parses the emitted YAML back and asserts the two agree.
//! A component added to the image without a line here fails that test rather than shipping an
//! inventory that quietly omits it.
//!
//! # No timestamp, and a serial number that is a digest
//!
//! Reproducibility is the property the apko config exists for — its own comment says to check it
//! by building twice and comparing — and an SBOM stamped with the time of day cannot be compared
//! that way. So this document has no `timestamp`, and its `serialNumber` is derived from the
//! content it describes: two builds of one program produce byte-identical documents, and a
//! *changed* program produces a different serial. CycloneDX permits both fields to be absent or
//! any valid UUID; what it does not permit is being wrong, and a fresh UUID per build would make
//! "did the bill of materials change?" unanswerable by comparison.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::substrate::DEFAULT as SUBSTRATE;
use crate::{InfraGraph, Node};

/// The Wolfi package repository the image's contents come from.
pub const REPOSITORY: &str = "https://packages.wolfi.dev/os";

/// One thing inside the built image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    /// The name apko installs it by.
    pub name: String,
    /// Why it is in the image, in the same register as [`crate::Derived::because`].
    pub because: &'static str,
    /// True for the application's own package, which comes from the local build rather than from
    /// the distribution — apko spells that `@local`, and an SBOM should not claim Wolfi ships it.
    pub local: bool,
}

impl Package {
    /// How apko names it in the `packages:` block.
    pub fn apko_name(&self) -> String {
        if self.local {
            format!("{}@local", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Everything the image contains, in the order apko installs it.
///
/// The single source for both the apko config and the SBOM. Adding to the image means adding here,
/// which is what keeps the inventory honest — see this module's opening.
pub fn packages(graph: &InfraGraph) -> Vec<Package> {
    vec![
        Package {
            name: "ca-certificates-bundle".to_string(),
            because: "a program may perform `net.out`, and a TLS trust store is not derivable per \
                      program",
            local: false,
        },
        Package {
            name: "tzdata".to_string(),
            because: "`time_parse` and the civil calendar are arithmetic, but a zone name is data",
            local: false,
        },
        Package {
            name: graph.app.clone(),
            because: "the application itself — apko copies nothing from the host, so the binary \
                      arrives as a package",
            local: true,
        },
    ]
}

/// The CycloneDX 1.6 document for a built program.
///
/// `source_digest` is the digest of the program's own source, which is what makes this document
/// about *a* program rather than about the shape of every program.
pub fn cyclonedx(graph: &InfraGraph, source: &str, wire_id: &str) -> Value {
    let app_ref = format!("beck:app/{}", graph.app);
    let source_digest = beck_core::digest::of(source);

    let mut components: Vec<Value> = Vec::new();
    let mut depends_on: Vec<String> = Vec::new();

    for p in packages(graph) {
        // The application's own package is the same artefact as the metadata component, so it is
        // not listed twice: what goes in the image is the binary, and the binary is the app.
        if p.local {
            continue;
        }
        let bom_ref = format!("pkg:apk/wolfi/{}", p.name);
        components.push(json!({
            "type": "library",
            "bom-ref": bom_ref,
            "name": p.name,
            "purl": format!("pkg:apk/wolfi/{}", p.name),
            "scope": "required",
            "description": p.because,
            "externalReferences": [{ "type": "distribution", "url": REPOSITORY }],
        }));
        depends_on.push(bom_ref);
    }

    // The substrate is a component of the *deployment* rather than of the image, and it is here
    // because a bill of materials that lists only the app's own image would omit the database the
    // generated manifests start. It appears exactly when a `durable` fold derived a log store.
    if graph.contains(|n| matches!(n, Node::LogStore { .. })) {
        let bom_ref = format!("pkg:oci/{}", SUBSTRATE.image);
        components.push(json!({
            "type": "container",
            "bom-ref": bom_ref,
            "name": SUBSTRATE.image,
            "purl": format!("pkg:oci/{}", SUBSTRATE.image),
            "scope": "required",
            "description": "the durable substrate a `durable` fold derived",
        }));
        depends_on.push(bom_ref);
    }

    // The standard library the program imports. These are not third-party packages — they are
    // compiled into the binary from `beck_core::stdlib` — so they are `type: library` with a
    // `beck:` purl of their own rather than a package-manager one. Listing them is what makes the
    // document answer "is this program affected?" about a library this project ships.
    for module in stdlib_modules(source) {
        let bom_ref = format!("beck:lib/{module}");
        components.push(json!({
            "type": "library",
            "bom-ref": bom_ref,
            "name": module,
            "purl": format!("pkg:generic/beck/{module}"),
            "scope": "required",
            "description": "a standard-library module, compiled into the binary",
        }));
        depends_on.push(bom_ref);
    }

    let refs: Vec<&String> = depends_on.iter().collect();
    let document = json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "bom-ref": app_ref,
                "name": graph.app,
                "purl": format!("pkg:generic/{}", graph.app),
                "hashes": [{ "alg": "BLAKE3", "content": source_digest }],
                "properties": [
                    { "name": "beck:wire-id", "value": wire_id },
                    { "name": "beck:entrypoint", "value": entrypoint(graph) },
                ],
            },
            "tools": { "components": [{
                "type": "application",
                "name": "beck",
                "version": env!("CARGO_PKG_VERSION"),
            }] },
        },
        "components": components,
        "dependencies": [
            { "ref": app_ref, "dependsOn": refs },
        ],
    });

    // The serial is a function of the document, so it says *which* bill of materials this is
    // without saying when it was made. Computed over the document without it, which is the only
    // order in which a content-derived identifier can be computed at all.
    let digest = beck_core::digest::of(&document.to_string());
    let mut out = document;
    out.as_object_mut()
        .expect("a JSON object")
        .insert("serialNumber".to_string(), json!(urn_uuid(&digest)));
    out
}

/// The document, formatted the way it is written to disk.
pub fn render(graph: &InfraGraph, source: &str, wire_id: &str) -> String {
    let mut s = serde_json::to_string_pretty(&cyclonedx(graph, source, wire_id))
        .unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

fn entrypoint(graph: &InfraGraph) -> String {
    graph
        .nodes
        .iter()
        .find_map(|d| match &d.node {
            Node::Image { entrypoint, .. } => Some(entrypoint.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// A UUID in the URN form CycloneDX asks for, from the first sixteen bytes of a digest.
///
/// Version 8 — "custom" — because that is what RFC 9562 reserves for a UUID whose bits mean
/// something to whoever made it, and these mean "the digest of this document".
fn urn_uuid(digest_hex: &str) -> String {
    let mut b = [0u8; 16];
    for (i, slot) in b.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&digest_hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
    }
    b[6] = (b[6] & 0x0f) | 0x80; // version 8
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 9562 variant
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The standard-library modules a program imports, by name.
///
/// Read from the source's `import` lines rather than from the linked program, because what an SBOM
/// should list is what this program asked for — a module pulled in transitively by another is that
/// module's dependency, and `beck_core::stdlib` is where that graph lives when there is one to
/// draw.
fn stdlib_modules(source: &str) -> Vec<String> {
    let known: BTreeSet<&str> = beck_core::stdlib::MODULES.iter().map(|(n, _)| *n).collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("import ") {
            let name = rest.split_whitespace().next().unwrap_or_default();
            if known.contains(name) {
                out.insert(name.to_string());
            }
        }
    }
    out.into_iter().collect()
}
