# 101 — Phase 3 report, part 69: the release, and the installer that reaches it

**Built.** [`08`](08-roadmap.md) §8.5.4 closes Phase 3's exit criterion with a list of the apologies
a tutorial would still have to make, and after [`94`](94-mode-b-report.md),
[`95`](95-oidc-relying-party-report.md) and [`100`](100-client-polish-report.md) the list was down to
two: **no installation story, no released binary**. Neither is a bullet in the phase list — they were
never anybody's work — and they are the two an outside developer meets before any of the language
exists for them.

Both are now built:

- **`.github/workflows/release.yml`** — a tag on a commit that passed the whole matrix produces four
  binaries, one `SHA256SUMS`, and a GitHub Release. It *calls* [`compiler.yml`](../.github/workflows/compiler.yml)
  rather than restating any of its gates, so "no release-only build steps"
  ([`28`](28-releases-and-deployment.md) §28.2 item 1) is a `needs:` edge rather than a promise.
- **[`install.sh`](../install.sh)** — `curl … | sh`. It works out the platform, resolves the
  version, downloads the tarball *and* the release's `SHA256SUMS`, refuses to continue unless they
  agree, and puts `beck` in `~/.beck/bin`.
- **[`release/build.sh`](../release/build.sh)** and **[`release/version.sh`](../release/version.sh)**
  — the parts of a release a person can run, factored out of the YAML for the reason §104.2 is
  about.
- **A version that means something.** The workspace was `0.1.0` on `publish = false` crates through
  four phases, which §28.1 already recorded as meaningless. It is **0.3.0** — §28.2 item 4's minor
  counts phase-sized increments, and this is the third — and `beck --version` now names the artefact
  rather than only the release:

```text
$ beck --version
beck 0.3.0 (3f3316bdc1d9 x86_64-unknown-linux-gnu)
```

What is **not** built is a cut release: no tag has been pushed, so nothing is published yet, and
§104.7 is careful about which parts of the above have been executed and which have only been
written.

## 101.1 Why these two were not on the phase list

Every other remaining item in Phase 3 is a piece of a bullet — the rest of the heap under both code
generators ([`101`](101-the-heap-report.md) §101.5), Mode B's codegen, lazy routes, the render lock.
These two are not. The phase list is a list of
things to *build*, and a release is a thing to *do*; a list of capabilities has no row for the act of
handing one to somebody.

That is exactly the failure mode §8.5's opening paragraph describes about F11: a decision written
down twice, correct both times, that never acquired a **position in an order**, so nothing ever came
due. "No released binary" had been true since Phase 0 and was nobody's item, which is why it
outlived nine bullets that were harder.

It also has a predecessor nothing had listed, and it is the reason this report exists at all rather
than a paragraph in [`99`](99-supply-chain-report.md): the supply-chain work built a command that
signs an *image*, and a compiler release is a **tarball**. §104.6 is what that costs.

## 101.2 A pipeline is the one artefact that cannot be run before it is used

This repository's rule is [`19`](19-phase-1-report.md) §19.4 item 10 — an artefact nobody has
executed is a design document — and Phase 2 paid for it once when a CI workflow turned out to have
been invalid YAML from the day it was written ([`20`](20-phase-2-report.md) §20.4 item 8).

A release workflow cannot obey that rule directly. It is triggered by a tag, so the first time it
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
Every step that could be wrong about *how* a binary is built or installed is in a shell script that
has been run, and §104.7 says which runs those were.

The same argument decided one absence. The publish job re-checks the assembled `SHA256SUMS` with
`sha256sum -c` and then **runs `install.sh` against the artefacts it just built**, over a `file://`
base URL, before it publishes anything. A release whose own installer cannot install it is the
failure that would be discovered by a stranger, and it costs one job step to make it impossible.

## 101.3 What a release is made of

Four tarballs, one per target, each containing a directory with the binary and the licence, plus one
`SHA256SUMS` and a copy of `install.sh`.

| | |
|---|---|
| Asset | `beck-<version>-<target>.tar.gz` |
| Contents | `beck-<version>-<target>/{beck,LICENSE}` |
| Targets | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin` |
| Checksums | one `SHA256SUMS`, in `sha256sum -c` format |

Three files construct that asset name independently — one YAML, two shell — so `release.rs` asserts
the convention in each rather than letting two of them drift into agreement about a different name.

The sizes, from the artefact this report was written against
(`release/build.sh --out dist`, host target):

| | bytes | |
|---|---|---|
| `beck` | 21,092,816 | 20.1 MiB |
| `beck-0.3.0-x86_64-unknown-linux-gnu.tar.gz` | 8,535,398 | 8.14 MiB |

That binary is the whole toolchain — front end, checker, placement solver, evaluator, incremental
engine, runtime, both code generators, the infrastructure emitter and the image builder
([`04`](04-compiler-architecture.md) §4.6's one binary). No size budget is asserted on it, and this
report does not propose one: [`28`](28-releases-and-deployment.md) §28.5 already holds an image-size
row waiting on the same question, and a budget invented here would be a number nobody has defended.

**The tarball is not byte-reproducible**, and nothing claims it is — this is observed rather than
assumed: two runs of `release/build.sh` over the same commit produced `2b25689781b0…` and
`270a3aac64ad…`. gzip stamps an mtime, and the portable `tar` invocation this uses — macOS ships
bsdtar, which has neither `--sort` nor `--owner` — does not normalise ownership or order. That is a weaker property than the image half's, which *is*
reproducible for a fixed package set ([`99`](99-supply-chain-report.md) §99.4), and the difference is
written into `build.sh` beside the command rather than left to be discovered.

## 101.4 The version was not a number, and the first gate is about that

`0.1.0`, in fourteen crates, none of them published, for four phases. §28.1 called it meaningless and
was right: nothing read it, nothing compared it to anything, and no artefact carried it anywhere.

A release makes it load-bearing in two directions at once, and both can be wrong:

1. **The tag can disagree with the workspace.** `git tag v0.2.0` on a 0.3.0 tree would publish a
   binary that answers `0.3.0` from a page headed `v0.2.0`. `release/build.sh --expect-tag`
   compares the two and refuses, and the workflow runs that check first and cheaply, before any
   build starts.
2. **The version can identify a release and not an artefact.** Four tarballs share `0.3.0`. A bug
   report against a downloaded binary has to name which one, so `build.rs` stamps the commit and the
   target triple into `--version`. Both are best-effort — a source tree with no `.git` reads
   `unknown` rather than failing to build — and `BECK_COMMIT` overrides the git lookup for a packager
   who has the commit but not the repository.

The first of those is a gate that could not have existed before this work, because there was nothing
for a tag to disagree with. It is the smallest thing here and the one most likely to fire.

The stamp had a defect of its own, found by reading its output on a rebuilt binary: `build.rs`
watched `.git/HEAD`, and **`HEAD` does not change when a commit is made**. On a branch it holds
`ref: refs/heads/<branch>`; the ref file underneath it is what moves. So an incrementally-rebuilt
binary kept printing whichever commit it was first built at — a *wrong* answer rather than a
missing one, which is the worse of the two for a field whose whole purpose is identifying an
artefact. It watches both now. A release build is a fresh checkout and was never affected, which is
exactly why this would have survived: the pipeline is the one place the bug could not appear.

## 101.5 The installer, and the one thing it refuses

`install.sh` is POSIX `sh`, about 150 lines, and configured entirely by environment variables:
`BECK_VERSION`, `BECK_TARGET`, `BECK_INSTALL_DIR`, `BECK_BASE_URL`, `BECK_REPO`.

Three decisions in it are worth stating, because each is a place where an installer usually gets
this wrong:

- **Verification is not optional.** If neither `sha256sum` nor `shasum` is on the machine, the
  script *dies* rather than installing without checking. An installer that skips verification when
  the tool is missing has taught its users that verification is optional, which is worse than not
  verifying at all.
- **A refusal installs nothing.** The download goes to a temporary directory, and the binary is
  copied into place only after the digest matches — then through a temporary name in the target
  directory and `mv -f`, so a `beck` running elsewhere keeps its inode and a half-written binary is
  never on anybody's path.
- **The platform list is one line.** `SUPPORTED=` names the four triples, and it is checked even
  when `BECK_TARGET` supplied one, so an unsupported platform is a sentence naming the four rather
  than a 404 from a URL nobody meant to construct.

There is no version resolution magic: `releases/latest` redirects to the tag, so the latest version
is readable without a token and without parsing JSON.

Installing from a local base URL — no network, which is the part this report can measure — takes
**0.30 s, 0.35 s and 0.41 s** over three runs: a download, a digest, an untar and one `--version`.
The alternative it replaces is a Rust toolchain download plus a release build of fourteen crates; no
clean-build figure is quoted here, because none was measured for this report and
[`64`](64-compile-speed-report.md) measures Beck programs compiling rather than the compiler.

## 101.6 A checksum is not a signature, and this project already had the machinery

[`99`](99-supply-chain-report.md) §99.5 built `beck sign` and `beck verify`: a Sigstore-shaped
signature over a manifest digest, in the form `cosign verify --key` reads, checked by `openssl` and
not only by this project's own code. It would be natural to assume a binary release inherits that.

It does not. `beck sign` takes **a layout `beck image` wrote** — its subject is an OCI manifest
digest, and a `.tar.gz` on a releases page is not one. So what this release publishes is a
`SHA256SUMS`, and the honest description of that is narrow:

> A checksum published beside the artefact it describes proves the download was not corrupted in
> transit. It proves nothing about the release page. Whoever can rewrite the tarball can rewrite the
> line describing it.

That sentence is in `install.sh`, in `release/README.md`, in the release notes the workflow writes
and in [`43`](43-threat-model.md) §43.4, because it is the kind of thing a reader assumes the
opposite of — and it is asserted as an **absence** from both ends in `pending_security.rs`: the
pipeline signs nothing and the installer verifies no signature, so the day either changes, a test
goes red and the person who closed it has to correct these documents in the same change. What would close it is a
signature over the sums file, a builder identity and a transparency log — §99.7's row, unchanged by
this work except that the pipeline a provenance attestation would attach to now exists.
[`adr/0027`](adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md) is the decision, and it
records the three cheaper-looking routes and why none of them is cheap.
[`92`](92-sbom-report.md) §92's table gave "this repository has no release pipeline to attach one
to" as the reason that row was empty; that reason is gone, and the row is still empty.

## 101.7 What has been executed, and what has only been written

The distinction this project insists on ([`AGENTS.md`](../AGENTS.md): "built", "runs" and "measured"
are three different claims), applied to this change.

**Executed, on this machine, against real artefacts:**

- `release/version.sh` → `0.3.0`.
- `release/build.sh --out dist` → a 8,535,398-byte tarball and its checksum, from a 2m 04s release
  build of all fourteen workspace crates with third-party dependencies already compiled.
- `sha256sum -c SHA256SUMS` against the assembled sums file → `OK`.
- `install.sh` against that release over `file://` → installed, and the installed binary ran
  `beck check compiler/examples/todo.beck` (`ok: 9 definitions, 4 signals, wire id 3245cd08808b11a8`).
- `install.sh` against the same release with one byte of the archive flipped → exit 1, "checksum
  mismatch", and **no file in the install directory**.
- `release/build.sh --expect-version 9.9.9 --check-only` → exit 1, naming both versions.
- Every workflow file, including the new one, through PyYAML.

**Written and not executed:**

- **The workflow itself.** No tag has been pushed, so `release.yml` has never run: not the tag
  trigger, not `gh release create`, not the `actions/upload-artifact` round trip. What is checked is
  that it parses, that its `needs:` edges are the ones §28.2 item 1 requires, and that the platforms
  it builds are exactly the ones `install.sh` offers.
- **Three of the four targets.** Only `x86_64-unknown-linux-gnu` has been built here. The two Darwin
  targets and `aarch64-unknown-linux-gnu` are native builds on GitHub-hosted runners and have never
  been compiled.
- **The `ubuntu-22.04-arm` runner label.** Chosen for the aarch64 Linux build; never used.
- **The concurrency hazard this file avoids.** `release.yml` declares no `concurrency` group. A
  reusable workflow may report its *caller's* name in `github.workflow`, which would make a group
  naming it identical in the caller and the callee — a job waiting on a workflow waiting on its own
  group. That is a hazard read from the documentation and not one observed here; the group is simply
  absent, because tags are unique and there is nothing to serialise.

## 101.8 The gates, and what makes each go red

`compiler/crates/beck-cli/tests/release.rs`, nine tests, plus one in `pending_security.rs`. Two of
them run a script and look at what it did; the rest read the files a person would otherwise have to
read.

| gate | goes red when |
|---|---|
| `there_is_a_pipeline_and_an_installer_to_check` | one of the four files stops being checked in, or a platform list parses empty — the test that stops the others passing by looking at nothing |
| `the_installer_and_the_pipeline_name_the_same_platforms` | a target is added to one and not the other, in either direction |
| `nothing_is_published_before_the_whole_suite_has_run` | `publish` stops needing `suite` or `binaries`, or the release stops calling `compiler.yml`, or `compiler.yml` stops being callable |
| `the_release_verifies_its_own_installer_against_its_own_artefacts` | the publish job stops installing what it built, or stops re-checking the assembled sums |
| `the_asset_name_is_one_convention` | any of the three files renames the asset alone |
| `a_tag_that_disagrees_with_the_workspace_version_fails_the_build` | `--expect-version` stops refusing a wrong version, or stops accepting the right one |
| `the_binary_says_which_build_it_is` | `--version` stops naming the release, the commit or the triple |
| `the_installer_refuses_an_archive_whose_checksum_is_wrong` | verification is skipped, or a refusal installs something anyway |
| `the_guide_installs_before_it_builds_from_source` | [`86`](86-getting-started.md) goes back to opening with a Rust toolchain |
| `a_release_artefact_carries_a_checksum_and_no_signature` (`pending_security.rs`) | the pipeline signs something, or the installer verifies a signature — a **red is good news** here, and the fix is to correct §104.6, [`43`](43-threat-model.md) §43.4 and [`adr/0027`](adr/0027-a-release-publishes-a-checksum-and-not-a-signature.md) in the same change |

The last of the run-a-script gates is the one written against **the gap rather than the fix**
([`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5): it corrupts an archive, leaves the
published digest alone — which is the shape the failure actually takes — and asserts both that the
script exits non-zero *and* that the install directory is empty afterwards. **Checked by breaking
it**: turning `[ "$expected" = "$actual" ]` into a comparison with itself makes the test fail, which
is the only way to know a gate is about the gap rather than about the code that closed it. Moving
the copy above the check reddens it the same way.

It skips loudly when `sh`, `tar` or a SHA-256 tool is missing, and `BECK_REQUIRE_INSTALL=1` forbids
the skip. CI sets it.

## 101.9 What this corrects, elsewhere

- **[`28`](28-releases-and-deployment.md) §28.1's "not existing" paragraph** loses one of its four
  clauses. "No version number is meaningful" is gone: it is read by a script, compared against the
  tag before anything builds, and stamped into the binary with the commit and the triple. "No
  release has been cut", "no artefact is published" and "nothing built here is deployed" are all
  still true, and the first two stay true until somebody pushes a tag. §28.1 now also lists a
  fourth workflow.
- **§28.2's "the commands exist; the pipeline does not"** is now the other way round for binaries and
  unchanged for images: there is a pipeline, and the command it would use to *sign* what it publishes
  does not reach a tarball (§104.6).
- **[`08`](08-roadmap.md) §8.5.4's apology list** — "no OIDC, no Mode B, no installation story, no
  released binary" — was already two items out of date when this began
  ([`94`](94-mode-b-report.md), [`95`](95-oidc-relying-party-report.md)) and is now down to the
  distinction §104.7 draws: the story is built and executed, the release is built and uncut.
- **[`86`](86-getting-started.md) §86.1** opened by telling a newcomer to build a compiler, and
  §86.8 listed "there is no installation story" as something belonging to
  [`28`](28-releases-and-deployment.md) rather than to the guide. Both are rewritten: the guide leads
  with `install.sh` and keeps the source build as the second option, which is the order somebody
  arriving actually wants.
- **[`99`](99-supply-chain-report.md) §99.6's** note that `beck init ci` generates a workflow using
  `cargo install --git` "because there is no released `beck` to install" is still accurate, and
  deliberately so — §104.10.

Reports are history, so none of them is edited; this section is where the correction lives.

## 101.10 What is not built

- **No tag, so no release.** Everything above produces artefacts on a laptop and in a workflow that
  has never run. The next action is one command, and it is the user's rather than this report's.
- **No musl.** §28.2 item 1 asks for `x86_64/aarch64-linux-musl`; what the matrix builds is
  `-gnu`. SQLite and mimalloc are vendored C and aws-lc-rs builds with CMake
  ([`adr/0017`](adr/0017-sqlite-is-a-substrate-for-its-transaction-not-its-speed.md),
  [`adr/0019`](adr/0019-a-modern-allocator-for-the-evaluator.md),
  [`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md)), and none of that has been built against
  musl here. The consequence is a **portability floor**: a glibc-linked binary runs on the
  distribution it was built on and newer, which is why the Linux runners are `ubuntu-22.04` and not
  `-latest` — the one place the release deviates from CI's runner, and it deviates in the image
  rather than in the steps.
- **No signature, no attestation, no transparency log.** §104.6.
- **No Windows.** No target, no line in the installer, and no test in this workspace has ever run
  on one.
- **Nothing is signed for macOS.** The Darwin binaries are neither codesigned nor notarised. A
  `curl | sh` install is unaffected; a tarball downloaded through a browser carries a quarantine
  attribute and Gatekeeper will refuse it.
- **`beck init ci` still writes `cargo install --git`.** It could install a release instead, and it
  should not until one exists — a generated workflow that resolves "the latest release" on a
  repository with no releases fails on somebody else's first commit rather than on ours.
- **No package managers and no self-update.** No Homebrew formula, no apt repository, no
  `beck upgrade`. `install.sh` run again is the upgrade path, and it overwrites in place.
- **No size budget on the binary**, per §104.3.

## 101.11 What Phase 3 is still not

Unchanged by this, and worth repeating so the two items above are not mistaken for the criterion:
Phase 3 exits when **an outside developer builds a non-trivial app from documentation alone, without
asking the team a question**. What this removes is the first two sentences of the apology that guide
would have had to open with. What remains on the list is the **rest of the heap** — a record, a union
and a newtype compile since [`101`](101-the-heap-report.md), and text, collections, closures and every
effect do not — plus Mode B's codegen, which waits on exactly that, lazy routes and the render lock.
And the criterion itself, which is a claim about a person and which nothing in this repository can
make true about itself.
