# 92 — Supply chain and release

**Built.** [`08`](08-roadmap.md) Phase 3's supply-chain bullet — "`beck init ci`, apko image build
in-process, cosign signing, SBOM" — and the two things that were on nobody's list and that an outside
developer meets before any of the language exists for them: **a release pipeline and an installer**.

- **`beck sbom`** emits a CycloneDX 1.6 document, *derived* from the same object graph the image
  config is.
- **`beck image`** assembles an OCI image in one process — resolve against the Wolfi index, fetch,
  unpack, add the toolchain and the program, write a layout. No apko, no melange, no daemon, no
  `docker`, no registry client.
- **`beck sign` / `beck verify`** produce and check a Sigstore signature over an image manifest
  digest, in the shape `cosign verify --key` reads.
- **`beck init ci`** writes [`28`](28-releases-and-deployment.md) §28.3's workflow.
- **A release pipeline and `install.sh`** — four binaries, one `SHA256SUMS`, a GitHub Release, and
  `curl … | sh`.
- **Build provenance** — every artefact the release publishes is the subject of an in-toto statement
  signed by a short-lived Sigstore certificate whose identity is the release workflow, in the public
  transparency log.

Two things run through all of it. The first is a **rule about derivation**: an inventory assembled
beside the thing it describes is an inventory that can be wrong about it, and it will be wrong
quietly — so everything here comes from one function and every gate reads a *rendering* rather than
calling that function twice (§92.2). The second is a **distinction the release forced**: "built",
"runs" and "measured" are three different claims, and §92.13 is careful about which parts of this
have been executed and which have only been written. **No tag has been pushed.**

And the largest thing still missing is named rather than implied: **package signatures are not
verified** (§92.15). A repository that can serve a package can put anything in the image.

---

## 92.1 Why the parts arrive in this order

[`06`](06-kubernetes-and-packaging.md) §6.2 chose apko because "the build performs no arbitrary
execution", and drew the consequence for *reproducibility*. That property pays twice more.

**An image assembled by a build that executes nothing has a component list already.** There is no
`RUN` line to inspect, no layer to scan, no package manager resolving something at build time: what
is in the image is what the graph put there. So the bill of materials is a projection of the object
graph, in the same sense that [`23`](23-incremental-views-report.md)'s read models are a projection
of the arrangement — and, like those, it cannot lag, because there is no second copy to fall behind.
That is why an SBOM was available before a pipeline that could sign one: the other three pieces of
the bullet need a key, a registry, a transparency log and something to publish.

**And a build with no arbitrary execution has nothing in it that a compiler cannot do.** There is no
build container to start and no shell. What is in the image is packages plus two files, so the build
is `packages + two files → one tar → three JSON documents`, and every step of that is ordinary code.
§6.2's own implementation note called this — "move to writing the OCI layout directly from Rust …
once the format is settled" — and named two crates as the way. Neither is used: **the OCI image
format is four JSON documents and a tar, and the documents are shorter than the schema types for
them would be.**

**The melange step is gone from the in-process route, and the reason is worth stating precisely.**
apko cannot copy from the host — that absence *is* its reproducibility story — so the binary has to
be packaged first ([`19`](19-phase-1-report.md) §19.5 is the defect that taught this project so). A
build that *is* the compiler has the binary and the program in hand and copies two named files whose
digests it prints. **That is not a hole in the argument, because the argument was never about
copying: it is about execution, and this build executes nothing either.** The apko and melange
configs are still emitted, because they are the image's *description* — what a reader checks, and
what somebody who would rather use apko runs.

## 92.2 One source, and the gates that read a rendering

An inventory assembled beside the thing it describes is an inventory that can be wrong about it, and
it will be wrong quietly: a component added to the image and not to the list produces a valid
document that omits it, **which is worse than no document because somebody will search it.**

So there are four things that must agree about one image, and each has a gate that reads a
*rendering* rather than calling a function twice:

| What must agree | The one source | What the gate reads |
|---|---|---|
| The package list — SBOM, apko config, and what `beck image` resolves | `sbom::packages` | the rendered apko YAML, parsed back |
| The two files the application contributes, and their modes | `INSTALLS` | the emitted melange pipeline, parsed back |
| The account the image runs as — apko `accounts:`, `/etc/passwd`, the OCI `User`, the pod's `securityContext` | `NONROOT` | all four, separately |
| The process the image starts — apko `entrypoint:`/`cmd:`, the OCI `Entrypoint`/`Cmd` | `command()` | the emitted apko YAML, parsed back |

Reading the rendering rather than calling the function twice is the point. A test that called
`packages` on both sides would agree with itself no matter what the config said, which is
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's pattern: **a proxy for a control is
defeated by naming.**

The account row is the one that had a real defect waiting in it. Four artefacts named `65532`, three
of them generated by different code, and a mismatch between the image and the pod is the classic
"works locally, `CrashLoopBackOff` in the cluster" — **invisible in review because each file is
correct on its own.**

The same discipline is applied to the release: three files construct the asset name independently —
one YAML, two shell — so the gate asserts the convention in each rather than letting two of them
drift into agreement about a different name. And it is applied to the attestation:
`subject-checksums` attests one subject per line, so **the set of digests the attestation vouches
for *is* the set `install.sh` verifies against**, read from the same bytes, rather than two lists
that agree on the day they were written. A glob over the tarballs would produce an identical
attestation today and a different one the first time an artefact stopped being listed.

## 92.3 The bill of materials

| Component | Derived from |
|---|---|
| The application | the graph's app name, with a **BLAKE3 digest of the program's own source** and the `wire_id` as properties |
| `ca-certificates-bundle`, `tzdata` | `sbom::packages` — the same list apko installs, each with the reason it is there |
| `postgres:16-alpine` | present **exactly when** a `durable` fold derived a `LogStore`. A bill of materials covering only the app's image would omit the database the generated manifests start, and a program with no fold must not claim one |
| The standard-library modules | the program's `import` lines, filtered against the library's own table. `import bignum` downloads nothing and is a dependency all the same: it is code this project ships, and "is this program affected?" is a question somebody will ask about it |

The `wire_id` is the detail worth naming. It is the content-derived id of the command channel's
contract, so **an SBOM carrying it answers *which build is deployed* and not only *what is in it***
— which is the question an incident actually asks.

**No timestamp, and a serial number that is a digest.** CycloneDX documents usually carry a
timestamp and a fresh UUID per build. This one carries neither, and the reason is §6.2's
reproducibility property: the apko config's own comment says to check reproducibility by building
twice and comparing the results, and **a document stamped with the time of day cannot be compared
that way**. So the serial number is derived from the document it identifies — a UUIDv8 over the
BLAKE3 digest of the document without it. Two builds of one program produce byte-identical files; a
changed program produces a different serial. Both directions are gated, because the first without
the second is satisfied by a constant. RFC 9562's version 8 is the right one to claim: it is
reserved for a UUID whose bits mean something to whoever made it, and these mean "the digest of this
document".

**The argument's limit, stated precisely.** It makes the SBOM right about *what the config asks
for*. It says nothing about what a Wolfi package *contains*, which is that package's own SBOM to
publish, or about which version resolved on the day the image was built — which is §92.15's largest
row.

## 92.4 The image, assembled in one process

`beck image` resolves the packages the apko config names against the Wolfi index, fetches them,
unpacks them, adds the toolchain and the program, and writes an OCI layout.

**What checks this, other than this.** A tar writer tested by its own reader agrees with itself; a
signature checked by the library that made it establishes that the library is self-consistent. Both
were available failure modes, so neither is how the work is gated:

- **The system `tar` is on both sides.** The package fixtures are built by `tar czf` and read by
  this project's reader; the layer this project writes is then listed **and extracted** by `tar xzf`,
  and the extracted program compared byte-for-byte against the source. **A tar this project could
  write and only this project could read would pass a self-test and fail in a container runtime.**
- **`openssl` verifies the signature** — the same public key PEM and DER signature `cosign verify
  --key` reads — and the test also asserts it **refuses** a forged payload, because a verifier that
  accepts everything would pass the positive half.
- **A real APK, not only a fixture.** A fixture built by `tar czf` is one gzip member; a Wolfi
  package is three concatenated. The suite runs against a package a build actually fetched.
- **Somebody else's YAML parser reads the generated workflow.**

Both external tools **skip loudly** when absent — `BECK_REQUIRE_TAR=1` and `BECK_REQUIRE_OPENSSL=1`
forbid the skip, and CI sets both.

## 92.5 Reproducible, and exactly how far

Two builds of one program from one package set produce a byte-identical index, manifest, config and
layer. That is asserted three times over: against fixtures, in CI against the real repository, and —
in the direction a constant would satisfy — by changing the program and asserting the layer digest
**moves**. [`28`](28-releases-and-deployment.md) §28.2 item 3 asked for "build the image twice,
`diff` the digests" as a per-release gate; it is a per-commit one.

| Claim | Holds |
|---|---|
| Same inputs, same digest, same machine, same toolchain | **Yes**, and gated |
| Same inputs, same digest, a different machine | **Yes** for the uncompressed layer; for the compressed digest, only if the compressor is the same version — a compressor may emit different valid output ([`adr/0025`](adr/0025-deflate-so-the-image-build-needs-no-tools.md)) |
| Same *program*, same digest, a week later | **No.** apko resolves `tzdata` to whatever version the repository serves today, and so does this |

Nothing here reads a clock: no `created` on the config, no `created` in the history entry, mtime zero
on every tar member, and the gzip header's mtime and OS bytes written as constants — the same
discipline §92.3 applied to the SBOM's serial number, for the same reason.

**The release tarball is a weaker property, and this is observed rather than assumed**: two runs of
`release/build.sh` over the same commit produced different digests. gzip stamps an mtime, and the
portable `tar` invocation — macOS ships bsdtar, which has neither `--sort` nor `--owner` — does not
normalise ownership or order. That difference is written into `build.sh` beside the command rather
than left to be discovered.

## 92.6 The signature says one thing, and it is worth being exact about which

The payload is Sigstore's simple-signing document, and signing it asserts that **the holder of this
key says this digest is that image**. Three properties are gated, and the third is the one that
matters:

1. the signature verifies under the public key alone — a different key does not verify it;
2. the payload is a `cosign container image signature` and not some other document;
3. **the digest in the payload is the manifest digest in this layout.** A signature that verifies
   over somebody else's digest is a valid signature and a worthless one.

Signing a *tag* is refused outright: a tag moves, and a signature naming one asserts nothing about
what is deployed.

**Keyed, not keyless, and deliberately.** Sigstore's keyless flow needs Fulcio to issue a certificate
against an OIDC identity and Rekor to log it — two network services and a workload identity, none of
which existed when this was built. **A signing path exercisable only by a pipeline nobody has run is
[`19`](19-phase-1-report.md) §19.4 item 10's design document wearing a feature's clothes.** The keyed
half can be produced, verified and gated on a laptop, so it is the half that was built.

The private key is written `0600` and read from an environment variable in CI, **because a key on a
command line is a key in the process table**. The format is PKCS#8 PEM rather than cosign's own
container: what a consumer must be able to read is the *public* key, and that is a standard
SubjectPublicKeyInfo — which `openssl pkey -pubin` reads, which is the check that it is not this
project's own dialect.

## 92.7 `beck init ci`, and what it will not pretend

The generated workflow builds the toolchain with `cargo install --git`, because there is no released
`beck` to install and therefore no `setup-beck` action to `uses:`. **Emitting one would put a
workflow that fails on its first run into every repository that generated it.** A gate asserts that
every action it does name is a first-party one.

Two more refusals are in the file itself. It grants `contents: read` and **not** `packages: write`,
because nothing in it pushes — **a permission granted for something that does not happen is a
permission somebody will later use**. And the image job runs only from the default branch, because a
signature made on a pull request is a signature over code nobody has approved.

**One gate exists only because this is a generator.** It reads every `beck <word>` out of the emitted
workflow and asserts each is a subcommand `beck --help` lists. A hand-written workflow that names a
renamed command fails once, in this repository, where somebody sees it; **a generator that does it
fails on the first run in every repository that ran `beck init ci`.** Checked by making it red:
renaming one invocation fails it with the list of commands that do exist.

## 92.8 A pipeline is the one artefact that cannot be run before it is used

This repository's rule is that an artefact nobody has executed is a design document, and Phase 2 paid
for it once when a CI workflow turned out to have been invalid YAML from the day it was written.

**A release workflow cannot obey that rule directly.** It is triggered by a tag, so the first time it
runs is the first release, and "run it by hand once first" is not available: running it *is* cutting
a release. Writing 200 lines of YAML and trusting them would be the Phase 1 workflow again, with a
published artefact behind it instead of a silent gate.

So the executable parts are taken **out** of the workflow:

| | Executable by a person | What the workflow adds |
|---|---|---|
| `release/version.sh` | the version, read from one place | — |
| `release/build.sh` | one target's binary, tarball and checksum | the matrix, four times |
| `install.sh` | a download verified and installed | nothing — it runs the same script |
| `release.yml` | — | the tag trigger, the suite as a predecessor, the upload |

What is left in the YAML is a **schedule**: what runs, in what order, and what must be green first.
It *calls* the compiler workflow rather than restating any of its gates, so "no release-only build
steps" is a `needs:` edge rather than a promise. Every step that could be wrong about *how* a binary
is built or installed is in a shell script that has been run.

The same argument decided one absence. The publish job re-checks the assembled `SHA256SUMS` and then
**runs `install.sh` against the artefacts it just built**, over a `file://` base URL, before it
publishes anything. **A release whose own installer cannot install it is the failure that would be
discovered by a stranger**, and it costs one job step to make it impossible.

A release is four tarballs — `beck-<version>-<target>.tar.gz`, each containing the binary and the
licence — one `SHA256SUMS` in `sha256sum -c` format, and a copy of the installer. The binary is
20.1 MiB and the tarball 8.14 MiB; that binary is the whole toolchain, and **no size budget is
asserted on it**, because [`28`](28-releases-and-deployment.md) §28.5 already holds an image-size row
waiting on the same question and a budget invented here would be a number nobody has defended.

## 92.9 The version was not a number

`0.1.0`, in fourteen crates, none of them published, for four phases. §28.1 called it meaningless and
was right: nothing read it, nothing compared it to anything, and no artefact carried it anywhere.

A release makes it load-bearing in two directions at once, and both can be wrong:

1. **The tag can disagree with the workspace.** `git tag v0.2.0` on a 0.3.0 tree would publish a
   binary that answers `0.3.0` from a page headed `v0.2.0`. The build script compares the two and
   refuses, and the workflow runs that check first and cheaply, before any build starts. **It is a
   gate that could not have existed before this work, because there was nothing for a tag to
   disagree with** — and it is the smallest thing here and the one most likely to fire.
2. **The version can identify a release and not an artefact.** Four tarballs share `0.3.0`, so a bug
   report against a downloaded binary has to name which one:

```text
$ beck --version
beck 0.3.0 (3f3316bdc1d9 x86_64-unknown-linux-gnu)
```

Both stamps are best-effort — a source tree with no `.git` reads `unknown` rather than failing to
build — and an environment variable overrides the git lookup for a packager who has the commit but
not the repository.

**The stamp had a defect of its own, found by reading its output on a rebuilt binary.** `build.rs`
watched `.git/HEAD`, and **`HEAD` does not change when a commit is made**: on a branch it holds
`ref: refs/heads/<branch>`, and the ref file underneath it is what moves. So an incrementally-rebuilt
binary kept printing whichever commit it was first built at — **a wrong answer rather than a missing
one**, which is the worse of the two for a field whose whole purpose is identifying an artefact. It
watches both now. A release build is a fresh checkout and was never affected, **which is exactly why
this would have survived: the pipeline is the one place the bug could not appear.**

## 92.10 The installer, and the one thing it refuses

`install.sh` is POSIX `sh`, about 150 lines, configured entirely by environment variables. Three
decisions in it are worth stating, because each is a place where an installer usually gets this
wrong:

- **Verification is not optional.** If neither `sha256sum` nor `shasum` is on the machine, the script
  *dies* rather than installing without checking. **An installer that skips verification when the
  tool is missing has taught its users that verification is optional, which is worse than not
  verifying at all.**
- **A refusal installs nothing.** The download goes to a temporary directory, and the binary is
  copied into place only after the digest matches — then through a temporary name in the target
  directory and `mv -f`, so a `beck` running elsewhere keeps its inode and a half-written binary is
  never on anybody's path.
- **The platform list is one line**, and it is checked even when the target was supplied, so an
  unsupported platform is a sentence naming the four rather than a 404 from a URL nobody meant to
  construct.

There is no version-resolution magic: `releases/latest` redirects to the tag, so the latest version
is readable without a token and without parsing JSON. Installing from a local base URL — no network,
which is the part this can measure — takes **0.30 s, 0.35 s and 0.41 s** over three runs: a download,
a digest, an untar and one `--version`.

## 92.11 A checksum is not a signature, and provenance is what breaks the second half

`beck sign` takes **a layout `beck image` wrote** — its subject is an OCI manifest digest, and a
`.tar.gz` on a releases page is not one. So what the release publishes is a `SHA256SUMS`, and the
honest description of that is narrow:

> A checksum published beside the artefact it describes proves the download was not corrupted in
> transit. It proves nothing about the release page. Whoever can rewrite the tarball can rewrite the
> line describing it.

That sentence is in `install.sh`, in `release/README.md`, in the release notes the workflow writes
and in [`43`](43-threat-model.md) §43.4, **because it is the kind of thing a reader assumes the
opposite of** — and it is asserted as an **absence** from both ends in `pending_security.rs`, so the
day either changes a test goes red and the person who closed it has to correct these documents in the
same change. [`adr/0027`](adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md) is the
decision, and it records the three cheaper-looking routes and why none of them is cheap.

**Build provenance breaks the second half of that sentence, and only the second half.** An
attestation is not published beside the artefact — it is in GitHub's attestations API and in a
transparency log, signed by an identity that is not "whoever can write to this repository's releases"
but "this workflow file on this repository". Rewriting the tarball and the sums line leaves the
attestation describing the bytes that were actually built, and the verifier looks the artefact up by
the digest it computes from the file on disk.
[`adr/0028`](adr/0028-a-release-carries-provenance-and-still-no-signature.md) supersedes 0027: its
reasoning is not revised — it named this route as the right one and deferred it, and **the reason to
defer expired.**

**Verification is opt-in, and three things about that are decisions rather than defaults.**

`--signer-workflow` is the whole of it. `--repo` alone accepts any attestation this repository can
produce — **including one minted by a workflow that whoever rewrote the release page also added.**
The check would pass and assert nothing. So the gate reads back the arguments the installer invoked
the CLI with rather than asserting that a call happened: **the flag is the property.**

It is off by default, because `gh` is not a tool a script piped from `curl` can assume, and the
alternative — a Fulcio chain and a Rekor inclusion proof in POSIX shell — is not a thing to write.
The cost is asserted as an absence rather than described.

**Asked for and unavailable is a failed install**, not a skipped step, and the tool is resolved
*before anything is downloaded* — the same reason the SHA-256 tool is resolved that way. A run that
cannot finish should not have fetched anything, and the gate asserts the ordering too.

## 92.12 The stub, and what a stub can be evidence for

A fixture release — a tarball with a stub `beck` in it, a `SHA256SUMS` beside it, reached over
`file://` — and a shell script standing in for the verifier, recording its argument list and exiting
with a chosen code. It is one module rather than two fixtures that could disagree about what a
release looks like.

**The honest boundary: a stub verifier is evidence about the *installer*, not about the
verification.** Every property gated is a property of `install.sh` — what it does when a verifier
says no, when one is absent, when one is not asked for. **Nothing here says an attestation
verifies.** That claim needs the first release, and it is the same shape as §92.15's "nothing
consumes the SBOM": the fix is to run somebody else's client against the real artefact.

## 92.13 What has been executed, and what has only been written

"Built", "runs" and "measured" are three different claims, and this is where the distinction is
unflattering.

**Executed, on this machine, against real artefacts:**

- The version script, the build script, and `sha256sum -c` against the assembled sums file.
- `install.sh` against that release over `file://` → installed, and the installed binary checked a
  program.
- The same with one byte of the archive flipped → exit 1, "checksum mismatch", and **no file in the
  install directory**.
- The same with a stub verifier that **refuses** → exit 1, and nothing installed; with one that
  **agrees** → installed, with the recorded argument list carrying `--signer-workflow` naming a
  workflow file that exists; with verification asked for and the tool missing → exit 1 **before the
  fetch**; and on the default path with a verifier that refuses everything → installed, and the
  verifier never called. **That last is the absence, asserted as behaviour.**
- A build against a mismatched version → exit 1, naming both versions.
- The image resolve, unpack, build, double-build comparison, key, signature, verification and its
  negative control.
- Every workflow file through PyYAML.

**Written and not executed:**

- **The release workflow.** No tag has been pushed, so it has never run: not the tag trigger, not the
  release creation, not the artefact round trip. What is checked is that it parses, that its `needs:`
  edges are the ones §28.2 item 1 requires, and that the platforms it builds are exactly the ones the
  installer offers.
- **The attestation itself.** `actions/attest` has never executed here: not the OIDC exchange, not
  the Sigstore certificate, not the transparency-log entry. The permissions it needs have never been
  granted to a job that ran. The dry-run path exists to shorten that list — **the attest step is
  deliberately not guarded by an event condition**, so a manual run, which publishes nothing, still
  mints an attestation over the artefacts it built, and the gate asserts the guard's absence so it
  cannot be added back without a red test.
- **Three of the four targets**, which are native builds on hosted runners and have never been
  compiled here.
- **The package fetch.** Everything else in the image job was run locally, but this environment's
  HTTPS goes through a TLS-intercepting proxy, so the packages were placed in the cache with `curl`
  and the build was run offline. **The index and the packages are real and the *transport* is not
  exercised**, and the first CI run is what executes it.

**And two things nothing has run at all.** No container runtime has been handed the image: GNU tar
reads and extracts the layer and the digests are what the documents say, but this environment has a
`docker` client and no daemon. And **the image CI builds today would not start**, for a reason worth
naming rather than hiding: the default binary is the running `beck`, which on an ordinary Linux
runner is linked against glibc, and a distroless Wolfi base carries no dynamic loader.
[`19`](19-phase-1-report.md) §19.5 is the precedent — an image config that could not work, invisible
until something ran it — so the build reads the toolchain's `PT_INTERP` and **says so**, warning
rather than refusing, because a base image that carries a loader is a package list away. This is also
why no image-size number appears here: **the artefact to measure is not the one being built.**

## 92.14 The gates, and what makes each go red

[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's pattern: write the gate against the
shape of the gap, then check that the gap would make it fire. Each mutation below was applied, run,
and reverted.

| Mutation | What went red |
|---|---|
| Rename a `beck` invocation in the generated workflow | `the_workflow_runs_no_command_the_binary_does_not_have` |
| Compare the checksum with itself, or copy the binary above the check | `the_installer_refuses_an_archive_whose_checksum_is_wrong` |
| Drop `--signer-workflow` from the installer's command, or warn instead of dying | `the_installer_refuses_an_archive_whose_provenance_does_not_verify` |
| Let a missing verifier silently turn verification off | `provenance_verification_is_refused_rather_than_skipped_when_the_tool_is_missing` |
| Default verification to **on** | `the_default_install_checks_a_checksum_and_not_the_provenance` — the absence gate, firing as designed |
| Guard the attest step by event, replace `subject-checksums` with a glob, or hoist the permissions | `the_pipeline_attests_the_file_the_installer_verifies_against` |

**One of those gates did not catch its own mutation the first time, and that is the finding.** The
attest-step assertion sliced the step's text from `uses:` to the next `- name:` and looked for a
condition — and a condition written *above* `uses:`, which is where a person would naturally put it,
was outside the slice. **The gate passed on the mutation it existed for.** It now slices the step the
way YAML delimits one. That is [`85`](85-what-the-generator-found-report.md) §85.1's lesson arriving
in a much smaller place: **a gate calibrated against the shape you happened to write tests that you
wrote it, not that the property holds.**

Beyond the mutations, the standing gates are the four derivation checks of §92.2, the reproducibility
pair (identical twice, and *moves* when the program changes), the signature's three properties with
`openssl` on the other side, the platform lists agreeing in both directions, `nothing_is_published_
before_the_whole_suite_has_run`, `the_release_verifies_its_own_installer_against_its_own_artefacts`,
`the_asset_name_is_one_convention`, `the_binary_says_which_build_it_is`, and
`the_guide_installs_before_it_builds_from_source`. Plus one in `pending_security.rs` where **a red is
good news**: a release artefact carries a checksum and no signature, and the day that changes the fix
is to correct §92.11, [`43`](43-threat-model.md) §43.4 and the ADR in the same change.

The suites skip loudly without `sh`, `tar`, a SHA-256 tool, `openssl` or an `.apk` cache;
`BECK_REQUIRE_INSTALL=1`, `BECK_REQUIRE_TAR=1`, `BECK_REQUIRE_OPENSSL=1` and `BECK_REQUIRE_APK=1`
forbid the skips, and CI sets them.

## 92.15 What is not built

| | Status |
|---|---|
| **Package signature verification** | **Not built, and it is the largest security gap here.** The apko config names a keyring; this build fetches over TLS and checks nothing else. **A repository that can serve a package can put anything in the image.** The control-segment digest apk verifies against is parsed and carried but not checked |
| **Package versions and digests, pinned** | **Not built**, and it is the single highest-value item left. The resolver takes the highest version the index offers *today*, and the SBOM names `tzdata` rather than `tzdata-2026a-r1` — a compiler that never contacts the repository cannot write a version. A lock file recording the resolved `(name, version, digest)` triple is what would make an image reproducible across weeks rather than across minutes. **A version-less SBOM answers fewer questions than it looks like it answers**: a reader who sees `tzdata` will reasonably assume they can match it against an advisory, and CycloneDX has no "this component's version is unknown" a scanner respects |
| **Version constraints, enforced** | **Not built, and reported rather than silently ignored.** The resolver reads `foo>=1.2` and installs `foo`; every dropped constraint is collected and printed. apk's own solver is a SAT problem, and **a half-implemented one that quietly chose a too-old package would be worse than one that says what it did not check** |
| **A registry push** | **Not built.** `beck image` writes a layout to a directory. It is the next thing here and the one that makes the signature reachable by a consumer, since cosign discovers a signature by looking it up *in a registry* — **a signature nobody can find is a signature nobody checks** |
| **A signature over the release listing** | **Not built.** `SHA256SUMS` carries no signature of its own, and `beck sign` cannot take this subject. A reader who checks the sums and stops has checked one file on the page against another file on the page |
| **No tag, so no release** | Everything above produces artefacts on a laptop and in a workflow that has never run. The next action is one command, and it is the user's |
| **SLSA Build L3** | **Not claimed.** What is built is provenance with a builder identity and a transparency log, which is the *shape* of the Build track's requirement, on hosted runners whose isolation this project has not audited. [`12`](12-standards-and-conformance.md) §12.6 states the target; the Source track is untouched |
| **Trusted publishing on crates.io** | **Untouched**, and it is the one control that has to be configured *before* an action rather than after it |
| The SBOM inside the image, and a signature over it | **Not built.** `beck build` writes the bill of materials beside the manifests; nothing attaches it as a referring artefact. The compiler's SBOM is written per *program* rather than for the release, so there is nothing to point the attestation's SBOM mode at — CISA's mandatory-signature element is unmet |
| The compiler's own dependencies in the SBOM | **Not built.** The document describes the *application*; `cargo` already emits the toolchain's, and the tools entry is what ties the two together |
| Multi-arch images | **Not built.** The index has one manifest and is the shape that grows; what is missing is a cross-compiled toolchain binary per architecture |
| A decompression bound | **Not built.** A package is inflated into memory, and what limits it is the fetcher's reply cap |
| Nothing consumes the SBOM | The document is written and no tool here reads it; the tests assert its shape and its agreement with the image config, not that a scanner accepts it. **The fix is the same shape as everywhere else in this chapter: run somebody else's client against it** |
| No musl | §28.2 item 1 asks for musl targets; the matrix builds glibc. The consequence is a **portability floor** — a glibc-linked binary runs on the distribution it was built on and newer, which is why the Linux runners are pinned rather than `-latest`: the one place the release deviates from CI, and it deviates in the image rather than in the steps |
| No Windows; nothing signed for macOS | No target and no line in the installer for the first. For the second, a `curl \| sh` install is unaffected and a tarball downloaded through a browser carries a quarantine attribute Gatekeeper will refuse |
| `beck init ci` still writes `cargo install --git` | **Deliberately.** It could install a release and should not until one exists — a generated workflow that resolves "the latest release" on a repository with no releases fails on somebody else's first commit rather than on ours |
| No package managers, no self-update, no size budget | `install.sh` run again is the upgrade path, and it overwrites in place |
| Verification needs the network and needs GitHub | The attestation lives in GitHub's API and the trust root in Sigstore's public-good instance. **An air-gapped consumer has the checksum** |
| The resolver's exposure | **One repository, three packages**, a dependency closure two deep. Nothing here has met a repository with genuinely competing versions, a `provides` two packages both satisfy, or an architecture other than `x86_64` |
| The apko package list is three entries, two unconditional | The reasons recorded against them are reasons, not conditions: a program that performs neither still gets both. Deriving the trust store from the effect row is one line and a worse image, because a program that gained an outbound call would need a rebuild of the base rather than a redeploy. **That trade is recorded here rather than taken** |
| SPDX as a second format | **Not built and not planned** — one format, gated, beats two that can disagree |

### What this corrects, elsewhere

| Where | What |
|---|---|
| [`06`](06-kubernetes-and-packaging.md) §6.2 | Its implementation note — "shell out to `apko` initially; move to writing the OCI layout directly from Rust" — is spent, and the crates it named were not needed. Its reproducibility argument turns out to buy a second thing: an image whose contents are derived has an inventory without anything having to scan it |
| [`28`](28-releases-and-deployment.md) §28.1, §28.2, §28.3 | §28.3 item 1's `beck init ci` is built. "No version number is meaningful" is gone. "No release has been cut", "no artefact is published" and "nothing built here is deployed" are all still true. §28.2 item 3's reproducibility gate runs per commit rather than per release, and item 2's provenance attestation is built; the section's closing paragraph named a signature over `SHA256SUMS` as the next slice, and that is still the next slice |
| [`42`](42-security-assurance.md) §42.7, §42.8 | CISA's 2026 SBOM elements: the document carries supplier, component name, unique identifier, dependency relationships and the author. **Version strings it does not carry**, and that is one of the seven minimum elements. "What is left here is the transparency log, the provenance statement and a signature over the SBOM" loses its first two items for the compiler's own release; the trusted-publishing row is untouched |
| [`43`](43-threat-model.md) §43.4 | "A signature on the compiler you downloaded" is narrowed: the pipeline attests and the installer can check, so the absence is now the *default* rather than the mechanism |
| [`12`](12-standards-and-conformance.md) §12.6 | The SLSA row's "provenance attestations for compiler releases" is emitted rather than planned; the Source track and the level claim are not |
| [`86`](86-getting-started.md) §86.1 | Opened by telling a newcomer to build a compiler. It leads with `install.sh` now and keeps the source build as the second option, which is the order somebody arriving actually wants |
| [`07`](07-dependencies.md) | One new third-party dependency, `flate2`, with [`adr/0025`](adr/0025-deflate-so-the-image-build-needs-no-tools.md) as the decision |
