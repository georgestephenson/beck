# 94 — Phase 3 report, part 62: the relying party, and the two links that hold it up

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 3, finished except for its provisioning
> half. [`48`](48-identity-report.md) built identity as a **seam** and said, in a table, exactly
> what it was not: "OIDC relying party — **not built**. No JWKS fetch, no RSA or ECDSA, no issuer
> or audience validation, no nonce, no token refresh." That table is what this closes, together
> with the row above it — claims that reach the program — and the dependency decision both were
> waiting behind.

## 94.1 What was in the way, and it was one decision rather than three

[`48`](48-identity-report.md) §48.5 gave the reason nothing had been built: "It needs an HTTP
client and a signature library, and taking either is an ADR rather than a line in a module."
[`49`](49-http-client-report.md) then built the HTTP client, and [`08`](08-roadmap.md)'s Wave 3 row
narrowed the remainder to a sentence worth quoting because it turned out to be exactly right:

> What is left is the signature library — and TLS, because JWKS is fetched over it and
> [`49`](49-http-client-report.md) §49.6 is plaintext. Those two are one dependency decision taken
> together, which is a better-shaped ADR than the three-way one this row used to describe.

They are one decision for a reason that is not obvious until you look: **rustls's cryptography
provider is a signature library**. `aws_lc_rs::signature` verifies RSA PKCS#1 and ECDSA exactly as
it verifies a certificate chain, so taking [`07`](07-dependencies.md) §7.2's TLS row buys both
capabilities and the second costs nothing. That is
[`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md)'s argument about BLAKE3 — the
cheapest cryptographic dependency is the one already in the graph — arriving one layer up.
[`adr/0021`](adr/0022-tls-and-the-signature-it-brings.md) is the record, including the alternatives
refused: `openidconnect` (brings a second HTTP client and would not go through
[`49`](49-http-client-report.md)'s bounded, stubbable seam), `ring` (identical here; not chosen
because §7.2 says aws-lc-rs and a dependency table nobody follows is fiction), and the `rsa` crate
(RUSTSEC-2023-0071 is open against it, and
[`adr/0004`](adr/0004-full-cargo-deny-gate.md)'s ignore list is empty on purpose).

## 94.2 TLS is a field of the request, not a mode of the client

`beck_core::net::Request` gains `tls`, `lib/http.beck`'s `HttpRequest` gains `tls: Bool`, and the
call that sets it is `over_tls(req)` — which also moves the port to 443, because a request that
asked for TLS on port 80 is a mistake nobody means.

A field rather than a client setting, because a program that calls two peers may reach one over a
plaintext hop inside its own cluster and the other across the internet, and those are two requests.
And a field rather than a scheme in the path, because
[`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) already made the
host the atom the call performs: a URL would give a program a second place to write the host, and
the whole point of that ADR is that there is one.

**The certificate is checked against `Request::host`** — the literal at the call site, which is the
`net.out(host)` atom and therefore the peer in the generated NetworkPolicy. There is deliberately no
SNI override, no additional trust anchor, and no `--insecure`: a way to reach a host under a name
the deployment was not told about would undo `adr/0013` from the other end. The trust anchors are
Mozilla's, compiled in as data rather than read from the container's filesystem, because §6.2's
images execute nothing at build time and a `ca-certificates` package is a version the bill of
materials cannot state ([`92`](92-sbom-report.md) §92.5).

The gate is a **real handshake**: `beck-rt/src/outbound.rs` makes a certificate authority a moment
before it uses it, issues a certificate for `127.0.0.1`, and connects. Then it does the same with a
certificate for another name under the same authority and requires a refusal — because a
verification that cannot fail is not one — and then points a TLS request at a plaintext server and
requires that too. The `HttpOutbound::trusting` constructor those tests use is `#[cfg(test)]`, so
the knob does not exist in a deployment.

## 94.3 The relying party

`beck_rt::oidc` is 1,211 lines and does five things.

**Discovery.** `{issuer}/.well-known/openid-configuration`, so an operator configures one URL rather
than four. Two checks on the document that are not decoration: its `issuer` must be the one that was
asked for, and every endpoint must be on the issuer's **own host**. The second narrows the egress
rule to one name and stops a discovery document moving the token exchange — and the client secret
with it — somewhere else. An issuer that splits its endpoints across hosts is not usable here, which
§94.6 records as a limit rather than leaving somebody to find out.

**A key set.** Fetched over TLS, cached, and refetched on two triggers: a five-minute interval, and
a token naming a `kid` the set does not carry. The second one is the interesting half, and it is not
done where it is noticed. Verification runs on the connection path, and a `kid` is a string an
anonymous client chooses — so a refetch *there* would make "verify this token" a way to make a Beck
process call its identity provider. Instead the miss sets a flag, a background task reads it, and
there is a **ten-second floor** between fetches whatever asks. That is §43.1's A2 met one hop
further out than usual, and the harness checks both directions of it.

**Verification.** RS256/384/512, PS256/384/512, ES256 and ES384. What is not in that list is the
point: `none` is a registered JWS algorithm meaning "there is no signature", and the HMAC family is
symmetric, so a relying party that accepted `HS256` can be handed a token signed with the issuer's
own **public key** as the shared secret. Both are refused by not being in an enum rather than by a
check somebody remembered to write, and `oidc.rs` performs the second attack rather than describing
it — it computes the HMAC over the issuer's modulus and offers the result.

**The claim checks**, in one function: issuer, audience, authorized party when there are several
audiences, expiry, not-before, and the nonce when there is a nonce to compare against. The nonce is
`Some` exactly once in a token's life — at the callback, where the relying party still remembers
what it asked for — and `None` on every later connection, because a check that always passes is
worse than no check.

**The authorization-code flow with PKCE.** `/auth/login`, `/auth/callback`, `/auth/logout`. The
state, the nonce and the PKCE verifier travel in a **sealed cookie** rather than in a table this
process keeps: a login is then not a way to make a Beck app allocate, which is
[`84`](84-a-quota-is-only-as-good-as-its-actor-report.md)'s lesson applied before rather than after.
The seal is a keyed BLAKE3 under a key generated at startup, which is
[`48`](48-identity-report.md)'s `SignedIdentity` format doing a job it *is* suited for — the process
verifies what the same process minted ten seconds ago.

## 94.4 The claims reach the program, and stop at the log

`Session` gains `claims: Map[Str, Str]`, which is the row [`48`](48-identity-report.md) §48.5 called
"half built". A `map[Str, Str]` rather than a type per issuer, because what an issuer emits is that
issuer's decision and the type should say so; and **empty** under a provider that verifies nothing,
which is what makes `map_get(session.claims, "role")` a check rather than a decoration.

The line worth drawing is where they *stop*. An `Envelope` still carries `actor` and nothing else,
so:

- **`validate` may read a claim.** It is the authority chokepoint, it decides what a command becomes,
  and what it produces is an event — which is logged. D6's "claims → typed capabilities" is a rule
  about admission, and admission is what `validate` is.
- **A fold may not**, because there is nothing to read. §3.7 makes the log the only description of a
  program's history, and a fold that read a claim would replay differently depending on what the
  issuer was saying at the time. Nobody had to enforce this; it follows from `Envelope`'s fields,
  and this section exists so that stays deliberate.

Getting the claims there without two render paths took one small thing: `beck_rt::program::Viewer`,
a trait with `actor()` and `claims()`, implemented for `Actor` and for `str`. A connection supplies
the first; `beck test`'s `when session("ana") sends …`, the differential harness and every benchmark
supply the second and have no claims because there was nobody to make any. One `Session`
constructor either way — the alternative, an `Actor`-taking API beside a `&str`-taking one, is two
places for a claim to appear in one and not the other.

`Actor` keeps its private field and gains one constructor, `pub(crate)`, used by all three
providers. What a caller outside the crate can convert into is a `Proposer` — the actor of a
*proposal* — and not an `Actor`, because a public `From<String> for Actor` would be a public way to
mint the thing [`48`](48-identity-report.md) §48.2 built a private field to prevent.

## 94.5 What it costs, measured

    cargo test --release -p beck-cli --test oidc -- --nocapture the_cost_of_a_verification

| Token | Bytes | Per verification |
|---|---|---|
| 5 claims | 538 | **24.3 µs** |
| 68 claims | 2,362 | **57.9 µs** |

Minimum of five runs, which is [`adr/0019`](adr/0019-a-modern-allocator-for-the-evaluator.md)'s
convention; the spread across those five is under 5% on the small token and under 17% on the large
one. Two **sizes** because one measurement cannot tell a fixed cost from a growing one, and the two
together say something the first alone would have got wrong: **the signature is not the expensive
half.** Solving for a fixed cost and a per-byte one gives about **14 µs** fixed — which is an
RSA-2048 verification, and is what it should be — and about **18 ns a byte** for everything else,
which is base64, JSON and building the claim map. A large token costs what its *claims* cost, not
what its cryptography costs, and the obvious optimisation (cache verified tokens) would therefore
be caching the cheap half.

24 µs is a per-**connection** cost, not a per-event one: `verify` runs at the document request and
at the websocket upgrade, and an event proposed on an open socket does not touch it. That is worth
stating because §94.6's third row makes the opposite trade look attractive and it is not needed.

Nothing else moved. `beck explain incremental` and the plan are untouched by the extra `Session`
field, the corpus, differential, fusion and incremental-engine harnesses are unchanged, and the
empty claim map allocates nothing — `PMap::new()` is a `None` root.

## 94.6 What is **not** built

Against [`48`](48-identity-report.md) §48.5's table:

| | Status |
|---|---|
| Dev-mode identity for rung 0 | **built** ([`48`](48-identity-report.md)), and still the default |
| A verifying provider, symmetric | **built** ([`48`](48-identity-report.md)) |
| OIDC relying party | **built** — discovery, JWKS, RS/PS/ES, issuer, audience, azp, expiry, nbf, nonce, and the code flow with PKCE |
| Claims → `Session` | **built** — §94.4 |
| `identity = external(issuer=…)` as a declaration | **built** — §94.7, and it is what makes the egress rule derivable |
| `identity = managed()` provisioning | **not built**. `B0359` says so by name rather than calling it unknown |
| Presence as a first-class signal | **not built**, and untouched by this |

And the things a reader would reasonably assume and should not:

- **No identity provider is provisioned.** `external(…)` names one that is already somewhere else;
  `managed()` would put a Keycloak or Ory workload in the object graph, and nothing does.
  `pending_security.rs` asserts that in both directions — the declared issuer *is* in the derived
  policy, and no provider workload is — because "nothing is provisioned" is only interesting beside
  "the one the program names is reachable".
- **No `Secure` on the cookie.** §6.5's gateway terminates TLS in front of a plaintext hop, so
  setting it would make the cookie unusable in the deployment this project generates. It is the same
  reason [`83`](83-the-runtime-edge-report.md) §83.3 does not compare schemes in the `Origin` check,
  and it means a deployment that terminates TLS *in* the pod has a cookie it would rather mark.
- **No refresh, because there is no session to refresh.** The cookie is the ID token, so a session
  lasts exactly as long as the issuer said and then the browser is sent back to `/auth/login`. An
  issuer minting five-minute tokens will do that every five minutes. The alternative — a
  Beck-minted session cookie over a verified login — decouples the two and costs a second credential
  format, a second secret and a second thing that can be stale; the trade is stated here rather
  than taken quietly.
- **Logout is local.** It clears this app's cookie and does not call the issuer's end-session
  endpoint, so the browser is still signed in to the identity provider.
- **One host per issuer**, as §94.3 says: every discovered endpoint must be on the issuer's host.
- **No UserInfo request, no `at_hash` check** (there is no access token here to bind), no dynamic
  client registration, no back-channel logout, and no certificate pinning.
- **`beck run` still serves plaintext.** TLS arrived on the way *out*, not on the way in.
- **The OpenID Foundation conformance suite has not been run.**
  [`12`](12-standards-and-conformance.md) §12.3 names it as how this row is validated, and
  §12.1's rule is that "a claim enters the project as an executable artefact wired into CI".
  This report claims what the tests in `oidc.rs` check, one behaviour at a time, and **not**
  conformance — that row in §12.3 stays unticked, and it stays unticked deliberately, because it
  requires a hosted certification run against a live deployment and there is nothing deployed
  ([`28`](28-releases-and-deployment.md)).
- **No refresh token, no access token, and no resource server.** This is a relying party that
  authenticates a person to *this* application. Calling somebody else's API on their behalf is a
  different feature and none of it is here.

## 94.7 The issuer is a declaration, because an egress rule is derived from what a program names

[`10`](10-decisions.md) D6 has always written the language surface as one block:

```python
identity = external(issuer="https://login.acme.com")
```

It is here, and the reason to build it is not tidiness. §6.5 derives the cluster's egress rule from
the hosts a program *names* — that is
[`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md)'s property, and
it is what makes the derivation **total**: a computed host is `B0395`, so there is no outbound call
the deployment cannot be told about. An issuer supplied as a `beck run` flag broke that from the
other end. Nothing was wrong with the flag; the *runtime* had a peer the *compiler* had never heard
of, so `beck build` would emit a policy that refuses the JWKS fetch, and a deployment would come up
authenticating nobody.

So the declaration is not a configuration file in the language. It is a **host the program named**,
and `beck-infra` treats it as exactly that: `graph()` pushes one more `net.out(login.acme.com)`
onto the effect list, and not a line below that knows the difference between a host `http_fetch`
reaches and a host the runtime fetches a key set from. The egress rule, the `derived_from`
explanation and `beck explain` all follow without being told.

Three decisions inside it:

- **It is guarded on the `=`, not on the word.** `identity` is an ordinary name, and `sicp/ch1.beck`
  defines `def identity(n: Int)` at §1.3.1 — a program in this repository, checked by a harness. Only `identity =` at the top level
  is the declaration, so `identity` is **not** in `RESERVED_FORMS` and `B0312` does not refuse it.
  `oidc.rs` gates that with a program that defines and calls one.
- **The issuer is checked at compile time, by the same two rules the runtime uses.** `https` only,
  and a host `beck_core::net::is_nameable_host` accepts. Those are not new rules: they are the two
  refusals `beck_rt::oidc` already made at startup, moved to where they are a diagnostic with a span
  instead of a process that starts and cannot authenticate anybody. `B0359` is the code.
- **`--issuer` is gone.** It is the first thing this work built and it does not survive it: two
  sources for one fact is the drift this project spends its gates on, and the flag could not have
  been the one the derivation read. What stays on the command line is what is genuinely a
  *deployment* fact rather than a program fact — `--client-id`, `--client-secret` and
  `--redirect-uri`, since staging and production register different clients against one issuer.

`corpus/31-tenants.beck` is the program that exercises it, and it earns its place twice over: it is
the thirty-first corpus program, so the round-trip property, the placement property and the
formatter all run over the new form, and it is the first program in the tree whose `validate` reads
`session.claims` — a note is refused unless the *issuer* said which tenant is asking. Its test
block is the honest half of §94.4's limit: a test names an actor and cannot forge a session, so the
tests it can write are the **refusals**.

## 94.8 What this corrects

- **[`43`](43-threat-model.md) §43.4's first two bullets** are rewritten in the same change, which
  is what that section's own mechanism requires. Identity: the default is still `DevIdentity` and
  says so, but "OIDC is still absent" and "claims do not reach the program" are both now false.
  Transport security: an outbound call **may** be encrypted, and what is left is the honest
  remainder — a request that does not say `over_tls` is still plaintext, and that is the program's
  statement rather than a limit of the runtime. §43.2 gains a row and §43.5's diagram is corrected
  where it says the inbound half is unauthenticated.
- **[`10`](10-decisions.md) D6** says Beck's runtime does the OIDC code flow with "the audited
  `openidconnect` Rust crate". It does the code flow, and not with that crate;
  [`adr/0021`](adr/0022-tls-and-the-signature-it-brings.md) is the argument, and the short version
  is that a relying party built on somebody else's HTTP client would not go through
  [`49`](49-http-client-report.md)'s bounded, stubbable, egress-derivable seam. Everything else D6
  asks for is here except `managed()`.
- **[`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md)'s closing section** says the
  asymmetric decision is "still one ADR, still unwritten". It is written:
  [`adr/0021`](adr/0022-tls-and-the-signature-it-brings.md). ADRs are immutable, so this is the
  correction rather than an edit there.
- **[`48`](48-identity-report.md) §48.5's last two bullets** — "no credential is issued by anything"
  and "there is no login, no session store, no refresh, and no cookie" — are false now for the first
  clause and still true for the others: there is a login and a cookie, and still no session store
  and no refresh. §94.6 says why the last two are absent by design rather than by omission.
- **`beck-cli/tests/pending_security.rs`** loses three tests and gains one. The three are
  `nothing_here_speaks_oidc`, `a_verified_identitys_claims_do_not_reach_the_program` and
  `an_outbound_call_has_no_transport_security`; the one is the egress gap above. §94.8 is about the
  first of them.

## 94.9 The uncomfortable half: a grep that would have fired, and still proves nothing

`nothing_here_speaks_oidc` searched every `.rs` file in the workspace for `jwks`, `id_token`,
`issuer` and `RS256`. It *would* have gone red on this change — all four appear — so it is not a
fifth entry in [`84`](84-a-quota-is-only-as-good-as-its-actor-report.md) §84.5's list of gates that
could not fail.

It is still the weaker of the two kinds, and the reason is worth writing down beside that list. A
name grep fires when a **subject** is touched. It would have fired equally on a module that fetched
a JWKS and checked nothing, on a module that verified the signature and forgot the audience, and on
a comment mentioning `RS256`. The thing it can detect is "somebody worked on this"; the thing
§43.4 needs detected is "this control now works". Those coincide only when the absent control has
no behaviour to look at — which is why the file keeps the grep for F15's connection quota, whose
absence is the absence of a mechanism, and why the one test replacing these three **emits an object
graph and reads it**.

The gate that replaced those three made the mistake in miniature, and it is worth recording because
it is the same one at a smaller scale. "No identity provider is provisioned" was first written as a
search of the rendered YAML for `keycloak`, `ory` and `Kratos` — and it went red on
**`revisionHistoryLimit`**. A substring match over generated text is worth exactly that, so it
counts the object graph's `Workload` nodes instead and asserts there is one. The failure was
harmless and the lesson is not: the first version would have gone green again the moment somebody
renamed a field, and nobody would have looked.

The same standard applied to this work's other gates, so each of them was checked by breaking the
thing it guards. Deleting the audience comparison, the `azp` requirement, the issuer comparison and
the expiry comparison each turned exactly one test red; making the websocket upgrade ignore the
cookie turned `the_websocket_upgrade_is_where_a_cookie_is_checked` red — and the *first* attempt at
that last mutation left the refusal in place and the suite stayed green, which is a small reminder
that a mutation you did not verify is a mutation you did not perform.
