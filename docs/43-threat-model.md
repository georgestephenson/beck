# 43 — The threat model

> **The question this answers**: when Beck says a property is secure, secure *against whom*?

[`42`](42-security-assurance.md) §42.10 says a project is "watertight" only if it has three things,
and names the one Beck was missing first: "a **threat model** that says what is in scope and what is
not". This document is that. It is a **charter**, not a survey and not a report: it states the
current position, it is edited when the position changes, and everything in it is meant to be true
of the tree today.

It is deliberately short. A threat model that enumerates attacks is a document that goes stale; one
that names *adversaries* and *what each is assumed to be able to do* stays useful, because a new
attack is then a question with an answer rather than a missing row.

## 43.1 The four adversaries

Beck defends against four, in the order they will actually be met.

| | Adversary | Can | Cannot |
|---|---|---|---|
| **A1** | **An anonymous author of source** — the playground submitter, the author of a package, the contributor of a pull request | Choose the whole input to the compiler: any bytes, any nesting, any macro, any dependency graph | Choose the compiler's flags, reach the filesystem or network through a macro, or observe another submission |
| **A2** | **An anonymous client of a running application** — a browser, a script, anything that opens a socket | Send any bytes on the wire, claim any identity **under the default provider** (§43.4), open connections, propose any command | Write to the log except through `validate`, read a view they did not subscribe to, or observe a `secret[T]` |
| **A3** | **A hostile dependency** — a crate in the compiler's graph, a tarn in a program's | Run arbitrary code at *its* build time and inside its own functions | Run code at install time (there are no install hooks — [`16`](16-packages-and-ecosystem.md)), or hide an effect from the effect row a caller sees |
| **A4** | **An operator reading a dashboard** — the one adversary who is not hostile, and is here because they are a *target* | — | — (they are attacked *through* content the other three supply) |

Two are live today and two are anticipated. A1 and A2 can be met by anybody, right now, by running
`beck` on a file or pointing a socket at `beck run`. A3 becomes real when the registry exists
(Phase 4); A4 becomes real when anything is deployed.

## 43.2 What is defended, and by what kind of evidence

The three kinds are [`42`](42-security-assurance.md) §42.1's, and the distinction is the point:
**structural** follows from a construction and no reviewer has to remember it, **tested** means a
harness goes red, **absent** means it is not there and §43.4 says so by name.

| Claim | Against | Kind | Where |
|---|---|---|---|
| A `secret[T]` cannot reach a client partition | A2 | structural + tested | The placement solver; `security.rs`, one test per §3.5 row |
| `durable` and `ingress` are never placed client-side | A2 | structural + tested | Same |
| A fold is deterministic, so replay is exact | A2, A4 | structural + tested | §3.7's rule in the checker; the replay-determinism harness |
| Only validated events are logged | A2 | structural | F1/F3's rule: a command envelope is transient |
| No memory-unsafety in first-party code | all | structural | `unsafe_code = "forbid"`, inherited by every crate but one. The claim is about the compiler and the runtime; **machine code `beck native` generates is not first-party code in this sense** — it is an artefact, and it runs as a separate process for that reason ([`93`](93-llvm-backend-report.md) §93.1). The exception is `beck-wasm`, which **denies** rather than forbids because rustc classifies `#[no_mangle]` as unsafe code and a WebAssembly module that exports nothing cannot be called: four `#[allow]`s, all on export attributes, and no `unsafe` block, `unsafe fn` or raw-pointer read anywhere in the crate. Where the lint could shape the design it did ([`adr/0021`](adr/0021-the-native-backend-writes-ir-and-runs-a-process.md)); where it could not, the extent is a test — `mode_b.rs::the_wasm_boundary_is_the_only_exception_to_forbid_unsafe`, which also asserts that every other crate still inherits the workspace lint ([`94`](94-mode-b-report.md) §94.4) |
| No SQL injection | A2 | structural | Every value is a bind parameter |
| No hash flooding | A2 | structural | `Map`/`Set` are ordered, not hashed (F8) |
| Deeply nested source is refused, not fatal | **A1** | tested | [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md); `front_end_bound.rs` |
| A macro cannot read the disk or the network | A1, A3 | structural | The expander has no I/O to offer |
| Time enters at the merge point and nowhere else | A2, A4 | tested | `beck_core::clock`; `clock.rs`'s one-reader gate |
| Text in a page is escaped, in both text and attribute context | A2, A4 | tested | `beck-core/src/html.rs`; the dashboard's own escaper |
| An actor is a decision of the runtime, not a claim of the client | A2 | tested | `beck_rt::identity`; `identity.rs` drives the socket loop. **Only with a verifying provider** — the default verifies nothing, which is §43.4 |
| An actor a *third party* vouched for cannot be minted by this process | A2 | tested | `beck_rt::oidc` ([`95`](95-oidc-relying-party-report.md)): the signature is checked against the issuer's public key, and `oidc.rs` performs the `alg: none` and `HS256` confusion attacks rather than describing them. **Only for a program that declares an identity provider** |
| A named peer's identity is verified before anything is sent to it | A2 | tested | `beck_rt::outbound` with rustls ([`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md)): the certificate must answer for `Request::host`, which is the atom the call performs. **Only for a request that said `over_tls`** |
| Every host a program can reach is one the program named, and the egress rule is that list | A2 | structural + tested | [`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md): a computed host is `B0395`, so the derivation in §6.5 is total; `outbound.rs` |
| Exactly one function turns a `secret[T]` into a value that is not one, and it needs a capability | A2 | structural + tested | [`adr/0014`](adr/0014-a-keyed-digest-is-the-one-declassifier.md): `digest_keyed` performs `cap.sign`, which no client tier discharges; `security.rs` enumerates the prelude and asserts the count is one |

## 43.3 What is explicitly **not** defended

Stated positively, because an unstated exclusion reads as an oversight and a stated one is a
decision.

1. **Side channels.** Timing, cache, speculation. `secret[T]` is a *flow* property — where a value
   may travel — not a claim about what its use costs to observe, and nothing in Beck's design
   attempts constant-time anything **except `digest_eq`**, which exists because comparing a message
   authentication code with `==` returns at the first differing byte
   ([`52`](52-crypto-and-identifiers-report.md) §52.3). One comparison is not a side-channel
   programme, and the exclusion is otherwise unchanged.
2. **A compromised host.** If the process's memory or its filesystem is under someone else's
   control, no property here survives, and none is claimed to.
3. **An authenticated insider.** An actor acting within its capabilities is the system working —
   and there is now something to be inside of, since [`95`](95-oidc-relying-party-report.md) made
   the actor a third party's decision rather than the client's. A deployment left on the default
   provider has no authentication to be inside of, which is §43.4's first bullet rather than this
   exclusion.
4. **The dependency graph's own `unsafe`.** `forbid(unsafe)` is first-party only. tokio, hyper,
   redb and tungstenite are outside it, and the answer for them is pinning, an advisory gate with an
   empty ignore list, and upgrade discipline — not an absence of unsafe code.
5. **A program's semantics.** Beck contains flows; it does not know that a program's rules are the
   rules its author meant. "Only the owner may toggle their todo" is a program's claim, and Beck's
   contribution is that the claim is *checkable* and enforced identically on every tier — not that
   it is right.
6. **Availability of a service somebody runs.** Quotas, connection limits and admission control are
   §43.4's absent list. An operator who deploys Beck today and is knocked over by traffic has met a
   documented gap, not a vulnerability.
7. **The compiler's own correctness as a security property.** Beck is not a verified compiler and
   never claims a CompCert-style theorem. Miscompilation is a bug; it is not in this model.

## 43.4 What is absent, and how you can tell

The controls a reader would reasonably assume exist, that do not:

- **Identity, under the default provider.** Narrowed twice. [`48`](48-identity-report.md) made
  identity a seam, so an `Actor` is something only a provider can mint; [`95`](95-oidc-relying-party-report.md)
  added the asymmetric one, so ~~OIDC is still absent~~ **an OIDC relying party exists** — discovery,
  a cached JWKS, RS/PS/ES signatures, issuer, audience, authorized party, expiry, not-before and
  nonce, plus the authorization-code flow with PKCE — and ~~the claims → `Session` mapping~~ **the
  claims reach the program**, as `Session.claims`. What remains is that the **default** is still
  `DevIdentity`, which believes the claim: a deployment that has not chosen a provider has the old
  behaviour, deliberately rather than structurally, and `beck run` prints which one is in force.
  What remains absent is `identity = managed()`, which would provision an identity provider into
  the object graph; `external(…)` names one that is already somewhere else. The issuer **is** in the
  derived NetworkPolicy — it is a declaration, so §6.5's egress derivation covers it like any other
  peer ([`95`](95-oidc-relying-party-report.md) §95.7) — and `pending_security.rs` asserts both
  halves: the declared issuer is reachable, and no provider workload is emitted.
- **Transport security on an outbound call, unless the program asked for it.**
  [`49`](49-http-client-report.md) built `http_fetch` over **plaintext HTTP/1.1**;
  [`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md) took rustls, so `over_tls(req)` now puts
  the exchange inside a TLS session whose certificate must answer for the host written at the call
  site. What is absent is therefore narrower and is a property of the *program*: a request that does
  not say `over_tls` is still plaintext, so a credential sent with `with_secret_header` over one is
  confidential exactly as far as the network under it is. There is no certificate pinning, no OCSP
  and no revocation check, and there is deliberately no way to add a trust anchor or override the
  name checked. What *is* bounded either way is the blast radius of a hostile peer: 8 MiB of reply
  read at most, a 10-second deadline per exchange, and an egress rule the cluster derives from the
  program's own atoms, so a call to a host nobody wrote is a call the network refuses.
- **Transport security on the way *in*.** `beck run` serves plaintext HTTP and §6.5's gateway
  terminates TLS in front of it. That is why the session cookie is not marked `Secure`
  ([`95`](95-oidc-relying-party-report.md) §95.6) and why the `Origin` check does not compare
  schemes ([`83`](83-the-runtime-edge-report.md) §83.3): a deployment that terminates TLS inside the
  pod is not the one this project generates, and would want both.
- ~~**Per-actor quotas** (F3)~~ — **built** ([`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)),
  on by default at 600 events a minute. What remains absent is what §84.4 measures rather than
  claims: the bound is per *actor*, so under the default `DevIdentity` a client that rotates names
  is bounded by the table (1,024 buckets) rather than by the limit. **Subscription and connection
  quotas** (F15) and **the deploy choreography's bounded buffer** (F12) are still unbuilt.
- ~~**Message size limits and origin checks** on the websocket~~ — **built**
  ([`83`](83-the-runtime-edge-report.md)). The limits are numbers this project chose and a unit test
  holds it to them; the upgrade compares `Origin`'s authority against `Host` and refuses a mismatch
  with `403`. What is *still* absent here is a cross-origin allowlist: a deployment whose client is
  served from another host has nothing to configure, because a Beck app serves its own page and
  same-origin is a description of that rather than a policy chosen over alternatives.
- **Authentication on the read-model port.** [`88`](88-read-models-and-pgwire-report.md) built
  `beck run --pgwire`, which serves a program's read models on the PostgreSQL wire protocol. It
  answers `AuthenticationOk` to everyone and speaks plaintext, so it is **off by default** and
  **refuses to bind to anything but loopback** — the compensating control is reachability rather
  than a credential, and a deployment that wants it elsewhere forwards it through something that
  already authenticates. There is deliberately no flag to lift the bound;
  [`adr/0020`](adr/0020-the-read-model-speaks-pgwire-by-hand.md) is why, and it is the record that
  has to change before the bound does. The absence is asserted by connecting without a password and
  expecting it to work, which goes red the day one is required.
- **A signature on the compiler you downloaded.** [`104`](104-the-release-and-the-installer-report.md)
  built the release pipeline and [`install.sh`](../install.sh), which verifies the tarball against
  the release's `SHA256SUMS` and refuses to install on a mismatch. That is a **checksum, not a
  chain of trust**: it establishes that the download was not corrupted in transit and nothing at all
  about the page it came from, because whoever can rewrite the tarball can rewrite the line
  describing it. It would be reasonable to assume otherwise — this project *has* signing machinery
  ([`99`](99-supply-chain-report.md) §99.5) — but `beck sign`'s subject is an OCI manifest digest and
  a compiler release is a tarball (§104.6). The absence is asserted from both ends in
  `pending_security.rs`: the pipeline signs nothing, and the installer checks nothing. A provenance
  attestation and a transparency log are §99.7's rows and are also absent.
- **Macro fuel** (F17). Expansion is bounded in *depth* — twice over, since
  [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md) separated the two counters that
  had been one — and not in *work*.

None of these is a secret, and none of them is prose only:
`compiler/crates/beck-cli/tests/pending_security.rs` asserts each one **as an absence**, in the
pattern `sicp/refusals/` used for expressiveness. The day one is built, its test goes red, and
whoever built it has to correct this section in the same change. That is the mechanism this
document depends on; without it, §43.4 would be exactly the kind of list that is accurate on the
day it is written and quietly wrong six months later.

## 43.5 The trust boundaries, drawn once

```text
   A1 source ─────────▶│ beck check / build │─────▶ artefacts ─────▶│ registry │──▶ A3
                       │  ← bounded (0012)  │      (signing, SBOM: unbuilt — 28)
                       └────────────────────┘
   A2 client ──socket──▶│ ingress │──▶ validate ──▶ log ──▶ fold ──▶ view ──▶ patch ──▶ A2
                        └─ actor is self-asserted under the default provider (§43.4); a decision
                           of the runtime under a verifying one, and the issuer's under
                           `identity = external(issuer=…)` (95). Claims reach `validate` and
                           stop at the log, which carries the actor's name only (95 §95.4).
                           A Mode B document carries them so the browser's own `validate`
                           decides as the server's does — escaped, and still only advice
                                   └─ the only writer; the only place time enters (§3.7)
                                                              └─ secret[T] cannot cross (§3.5)
   A4 operator ◀── dashboard ◀── telemetry ◀── (content originating with A2)

   program ──http_fetch──▶│ net.out(host) │──▶ a peer ──▶ a reply the program parses
                          └─ the host is a literal, so the egress rule is the program (0013)
                             and the certificate must answer for that same literal (0023);
                             plaintext unless `over_tls`; bounded at 8 MiB and 10 s (§43.4)

   runtime ──JWKS/token──▶│ the issuer │  over TLS, at startup and on a timer — never on the
                          └─ connection path. The host is `identity = external(issuer=…)`, so
                             §6.5's egress rule covers it like any other peer (95 §95.7)
```

Five crossings, and what each one is:

1. **Source into the compiler** — the boundary A1 attacks. Bounded in nesting; not bounded in
   macro work.
2. **The wire into the sequencer** — the boundary A2 attacks. Typed and decoded before anything
   else happens. ~~Unauthenticated, unquota'd~~: quota'd since
   [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md), and authenticated when a provider was
   chosen — the default is still `DevIdentity`, and §84.4's arithmetic (the bound is worth what the
   actor is worth) is why those two facts belong in one sentence.
3. **The fold into a view, and a view into a patch** — where §3.5's placement properties do their
   work, and the crossing this project has the most evidence about.
4. **Content into an operator's screen** — the one A4 is attacked through, and the reason the
   dashboard escapes in attribute context as well as text.
5. **The program out to a peer, and the peer's reply back in** — added by
   [`49`](49-http-client-report.md). The outward half is bounded by construction: the set of hosts
   reachable is the set written in the source, and — for a request that said `over_tls` — the peer
   answering to one of those names has a certificate for it
   ([`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md)). The inward half is *still* not
   authenticated beyond that: a reply is a `Str` a program has to parse like any other input.

## 43.6 What would change this document

A threat model that never changes is not being read. These are the events that require an edit,
named so the edit is not left to somebody noticing:

| Event | What has to change here |
|---|---|
| ~~Identity lands~~ | **Done, except for the default** ([`48`](48-identity-report.md), [`95`](95-oidc-relying-party-report.md)): §43.2 has three rows, and §43.4's remaining gap is that `DevIdentity` is what a deployment gets when it chooses nothing. A2 splits into authenticated and anonymous when a *verifying* provider becomes the default, which is still not yet |
| TLS on the way *in* | §43.4 loses its third bullet, the session cookie gains `Secure`, and [`83`](83-the-runtime-edge-report.md) §83.3's decision not to compare `Origin`'s scheme is re-argued |
| ~~`identity = external(…)` becomes a declaration~~ | **Done** ([`95`](95-oidc-relying-party-report.md) §95.7): §6.5's derivation is total again, and what `pending_security.rs` asserts is now the *provisioning* half |
| ~~`identity = managed()` is built~~ | **Done** ([`95`](95-oidc-relying-party-report.md) §95.10). An identity provider is now a workload this project's manifests start — and it starts in `start-dev`, which §95.10 records as the limit it is |
| A managed provider is deployed for real | `start-dev` becomes `start`, which needs a database and TLS material this derivation does not emit — and the plaintext hop §95.10 argues for stops being the only thing between the application and its key set |
| The playground ships | A1 stops being hypothetical; the isolation story (§17.3) enters §43.2 |
| The registry ships | A3 stops being anticipated; tarn signing and effect diffs enter §43.2 |
| Any quota is built | §43.4 loses a bullet, `pending_security.rs` loses a test, both in one change |
| A second writer exists | The single-writer row in §43.2 stops being structural, and everything resting on it needs re-argued |

## 43.7 What this document is not

It is not an assessment: [`42`](42-security-assurance.md) is, it is dated August 2026, and it
carries the measurements. It is not a conformance claim against a framework — §12.1's rule applies,
and a claim enters the project as an executable artefact rather than as a paragraph. It contains no
penetration test, no cryptographic review and no assessment of the dependency graph's own `unsafe`.
And it says nothing about *residual risk* in the quantitative sense, because a project that has not
deployed anything has no incident history to compute one from.
