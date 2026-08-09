# 92 — Phase 3 report, part 60: a bill of materials that cannot be wrong about the image

**Built, and it is the first piece of the supply-chain bullet.** `beck sbom` emits a CycloneDX 1.6
document for a Beck program, and `beck build` writes one beside the manifests. It is **derived**,
which is the only reason it is worth having: the package list in the bill of materials and the
`packages:` block of the apko config come from one function, and a test parses the *rendered* YAML
back and asserts they agree.

What this is not is the rest of that bullet. There is no signing, no provenance attestation, no
`beck init ci`, and no in-process apko build — §92.5 lists them, and the reason this one came first
is in §92.1.

## 92.1 Why an SBOM is available before the pipeline that would sign one

[`08`](08-roadmap.md) lists "`beck init ci`, apko image build in-process, cosign signing, SBOM" as
one bullet, and three of those four need a release pipeline this project does not have — a key, a
registry, a transparency log, and something to publish. An SBOM needs none of them, because of a
property [`06`](06-kubernetes-and-packaging.md) §6.2 established for a different reason:

> because an apko build performs no arbitrary execution, the same config and package versions yield
> the same digest on any machine

An image assembled by a build that executes nothing and copies nothing from the host **has a
component list already**. There is no `RUN` line to inspect, no layer to scan, no package manager
resolving something at build time: what is in the image is what the graph put there. So the bill of
materials is a projection of the object graph, in the same sense that
[`88`](88-read-models-and-pgwire-report.md)'s read models are a projection of the arrangement —
and, like those, it cannot lag, because there is no second copy to fall behind.

That is the whole argument, and it is worth being precise about its limit: it makes the SBOM right
about **what the config asks for**. It says nothing about what a Wolfi package *contains*, which is
that package's own SBOM to publish, or about which version resolved on the day the image was built
(§92.5).

## 92.2 The one rule, and the test that holds it

An inventory assembled beside the thing it describes is an inventory that can be wrong about it, and
it will be wrong quietly — a component added to the image and not to the list produces a valid
document that omits it, which is worse than no document because somebody will search it.

So `sbom::packages` is the single source, `k8s::apko` renders its `packages:` block from that
function, and `supply_chain.rs` reads the **rendered YAML** back:

```rust
let installed = apko_packages(&beck_infra::k8s::apko(&graph));
let listed = sbom::packages(&graph).iter().map(|p| p.apko_name());
assert_eq!(listed, installed);
```

Reading the rendering rather than calling the function twice is the point. A test that called
`packages` on both sides would agree with itself no matter what the config said, which is
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's pattern: a proxy for a control is
defeated by naming.

## 92.3 What is in the document, and where each part comes from

| Component | Derived from |
|---|---|
| The application | the graph's app name, with a **BLAKE3 digest of the program's own source** and the `wire_id` as properties |
| `ca-certificates-bundle`, `tzdata` | `sbom::packages` — the same list apko installs, each with the reason it is there |
| `postgres:16-alpine` | present **exactly when** a `durable` fold derived a `LogStore`. A bill of materials covering only the app's image would omit the database the generated manifests start, and a program with no fold must not claim one |
| The standard-library modules | the program's `import` lines, filtered against `beck_core::stdlib::MODULES`. `import bignum` downloads nothing and is a dependency all the same: it is code this project ships, compiled into the binary, and "is this program affected?" is a question somebody will ask about it |

The `wire_id` is the detail worth naming. It is the content-derived id of the command channel's
contract ([`04`](04-compiler-architecture.md) §4.3), so an SBOM carrying it answers *which build is
deployed* and not only *what is in it* — which is the question an incident actually asks.

## 92.4 No timestamp, and a serial number that is a digest

CycloneDX documents usually carry `metadata.timestamp` and a fresh `serialNumber` per build. This
one carries neither, and the reason is the property §6.2 exists for: the apko config's own comment
says to check reproducibility by building twice and comparing the results. A document stamped with
the time of day cannot be compared that way, and neither can one with a random UUID in it — "did the
bill of materials change?" stops being answerable by `cmp` and starts needing a diff tool that knows
which fields to ignore.

So the serial number is **derived from the document it identifies**: a UUIDv8 over the BLAKE3 digest
of the document without it. Two builds of one program produce byte-identical files; a changed
program produces a different serial. Both directions are gated, because the first without the second
is satisfied by a constant.

RFC 9562's version 8 is the right one to claim: it is reserved for a UUID whose bits mean something
to whoever made it, and these mean "the digest of this document".

## 92.5 What is not built

| | Status |
|---|---|
| A CycloneDX 1.6 document, derived, written by `beck build` | **built** |
| The package list gated against the image config | **built** — §92.2 |
| The substrate, and the standard-library modules a program imports | **built**, both derived |
| **Package versions** | **not built**, and it is the largest gap. The document names `tzdata`, not `tzdata-2026a-r1`: apko resolves a version at build time from the repository, and this document is written at *compile* time by a compiler that never contacts the repository. Closing it means either pinning versions in the config — which is a change to §6.2's reproducibility story, not to this file — or emitting the SBOM from the built image, which is what apko itself can already do |
| **Package digests** | **not built**, for the same reason and with the same two fixes |
| **The compiler's own dependencies** | **not built.** The document describes the *application*; the Rust crates that make up `beck` are the toolchain's bill of materials, and `cargo` already emits one. The `metadata.tools` entry names the compiler and its version, which is what ties the two together |
| **Signing, and a provenance attestation** | **not built.** SLSA v1.2's build track needs a builder identity and a transparency log ([`42`](42-security-assurance.md) §42.8), and this repository has no release pipeline to attach one to — which is [`28`](28-releases-and-deployment.md)'s subject and unchanged |
| **`beck init ci`** | **not built** |
| **An in-process apko build** | **not built.** `beck build` writes the config and prints the three commands; nothing here runs them |
| SPDX as a second format | **not built and not planned** — one format, gated, beats two that can disagree |

## 92.6 What this leaves open

**Nothing consumes it.** The document is written and no tool in this repository reads it; the tests
assert its shape and its agreement with the image config, not that `grype` or `trivy` accept it.
That is the same honesty [`88`](88-read-models-and-pgwire-report.md) §88.7 applied to the read-model
port — "nothing has been tried against a BI tool" — and the fix is the same shape: run somebody
else's client against it.

**A version-less SBOM answers fewer questions than it looks like it answers.** A reader who sees
`tzdata` in a bill of materials will reasonably assume they can match it against an advisory, and
they cannot, because the version is not there (§92.5). This is stated in the document's own
`description` fields and in this report, and it is not stated *in the document* in a machine-readable
way — CycloneDX has no "this component's version is unknown" that a scanner respects.

**The apko package list is three entries and two of them are unconditional.** The `net.out` and
`time_parse` reasons in `sbom::packages` are the reasons those packages are there, not conditions —
a program that performs neither still gets both. Deriving the trust store from the effect row is one
line and a worse image, because a program that gains an outbound call would need a rebuild of the
base rather than a redeploy; that trade is recorded here rather than taken.

## 92.7 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`08`](08-roadmap.md) | The supply-chain bullet's SBOM is built; signing, provenance, `beck init ci` and the in-process image build are not |
| [`42`](42-security-assurance.md) §42.8 | "CISA's 2026 SBOM elements" as a row that has moved: the document carries supplier, component name, unique identifier, dependency relationships and the author of the SBOM data. **Version strings it does not carry**, and that is one of the seven minimum elements — so this is a partial answer, and §92.5 is the part that is missing |
| [`06`](06-kubernetes-and-packaging.md) §6.2 | Its reproducibility argument turns out to buy a second thing: an image whose contents are derived has an inventory without anything having to scan it |

## 92.8 What Phase 3 is still not

Unchanged, and none of it moved here: **no LLVM backend and no native codegen**; **no Mode B and no
client polish**; **no playground**; identity's OIDC relying party, `managed()` provisioning, the
claims mapping and presence ([`48`](48-identity-report.md) §48.5). The supply-chain bullet has one
of its four pieces.

The exit criterion is a claim about a person, and no outside developer has read the guide
[`88`](88-read-models-and-pgwire-report.md) published.
