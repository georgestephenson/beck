# 48 — Identity

**Built, and the bullet has no unbuilt row left.** [`10`](10-decisions.md) D6 and
[`08`](08-roadmap.md) Phase 3's identity bullet: a dev-mode identity that is named rather than
implied, a verifying symmetric provider, an **OIDC relying party** with discovery, a cached JWKS,
RS/PS/ES signatures, every claim check and the authorization-code flow with PKCE, claims that reach
the program, `identity = external(issuer=…)` and `identity = managed()` as *declarations*, and
**presence** as a first-class non-durable `Signal`.

The chapter's three load-bearing ideas are not about OIDC.

**An actor is something the runtime decides rather than something a client asserts** (§48.2), which
was the oldest absent control in the project: every ownership check in every corpus program had been
enforced against a value the caller chose.

**The issuer is a declaration rather than a flag** (§48.7), because [`06`](06-kubernetes-and-packaging.md)
§6.5 derives the cluster's egress rule from the hosts a program *names* — and a runtime with a peer
the compiler had never heard of would emit a policy that refuses its own JWKS fetch.

**Claims stop at the log** (§48.6). `validate` may read one, because it is the authority chokepoint
and what it produces is an event; a fold may not, because there is nothing to read. Nobody had to
enforce the second — it follows from `Envelope`'s fields — and it is written down so it stays
deliberate.

---

## 48.1 The gap, which was the oldest absent control in the project

[`42`](42-security-assurance.md) §42.6's first bullet:

> **Claim any identity.** `actor` arrives in the client's own `hello` frame; `protocol.rs` admits it
> in a comment … Every ownership check in every corpus program is therefore enforced against a value
> the caller chooses.

And §42.5's sharper version, which is the sentence this work exists to make true:

> "A capability required outside the chokepoint has no holder" is true and proven; "only the owner
> may toggle their todo" is enforced against a self-asserted string. **The two read alike in a slide
> deck and are entirely different guarantees.**

[`43`](43-threat-model.md) §43.4 listed it first among the absences, and `pending_security.rs` had
been asserting it since Wave 0 — which is what made this work start from a failing test rather than
from a memory.

## 48.2 Identity is a seam

`beck_rt::identity`, a seam in the sense `beck_core::clock` is one.

**`Actor` has a private name.** Nothing but an `Identity` implementation constructs one, so a
`String` from a frame cannot become an actor by being assigned to one, and **"where did this actor
come from" has exactly one answer everywhere it is asked.** What a caller outside the crate can
convert into is the actor of a *proposal* and not an `Actor`, because a public conversion would be a
public way to mint the thing the private field exists to prevent.

**`DevIdentity`** is the previous behaviour with a name: it believes the claim. It is the default —
`beck run` on a laptop must not need a secret — and it now *says* what it is, through
`kind() == "dev"` and `verifies() == false`.

**`SignedIdentity`** verifies `<actor;expiry;claims>.<mac>`, where the tag is a keyed BLAKE3 of the
payload. Keyed BLAKE3 *is* a MAC and BLAKE3 was already in the dependency graph for content
addressing, so this costs no new dependency and contains no hand-rolled cryptography — **the two
ways a module like this usually goes wrong.**

Both edges ask. The socket refuses **before** anything is rendered — no welcome, no frame, no view —
and the document handler returns 401. The suite drives the socket loop rather than the unit, because
**the thing a unit test cannot check is that the loop asks.**

Three decisions inside it:

- **A refusal is coarse to the client and specific to the operator.** A client is told
  `unauthenticated` and nothing else; which of missing, invalid or expired it was goes to the log.
  The difference is useful to an attacker and to nobody else.
- **It is counted separately.** `beck.connections.unauthenticated` is not `beck.proposals.rejected`:
  one is "who are you" and the other is "you may not do that". `verifies()` exists for the same
  reason — **an operator who cannot tell from the logs whether authentication is on does not have
  authentication**, and §42.6's whole point is that an absent control was invisible.
- **Expiry reads the injected clock**, so the tests state an instant instead of depending on when
  they ran — Wave 0's seam paying for itself two waves later, which was the first time it had.

**What the symmetric scheme is for, and what it is not for.** `SignedIdentity` is a **shared
secret**: everything that can verify a credential can also mint one. That suits the shape a rung-1
deployment actually has — a gateway in front of a Beck process, minting credentials the process
checks — and it does not suit a public identity provider, because the process would then be able to
impersonate every user of it. **Writing that limit down is the point of the type having a name.**
The alternative — one `Identity` implementation and a comment — is how a symmetric scheme ends up in
front of the internet.

## 48.3 One dependency decision, not three

The reason nothing further had been built was that a relying party "needs an HTTP client and a
signature library, and taking either is an ADR rather than a line in a module".
[`46`](46-standard-library-report.md) built the HTTP client, which narrowed the remainder to the
signature library — **and TLS, because a JWKS is fetched over it.**

They are one decision for a reason that is not obvious until you look: **rustls's cryptography
provider is a signature library.** `aws_lc_rs::signature` verifies RSA PKCS#1 and ECDSA exactly as it
verifies a certificate chain, so taking [`07`](07-dependencies.md) §7.2's TLS row buys both
capabilities and the second costs nothing. That is
[`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md)'s argument about BLAKE3 — **the
cheapest cryptographic dependency is the one already in the graph** — arriving one layer up.
[`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md) is the record, including the three
alternatives refused: `openidconnect` brings a second HTTP client and would not go through
[`46`](46-standard-library-report.md)'s bounded, stubbable seam; `ring` is identical here and is not
what §7.2 names, and a dependency table nobody follows is fiction; and the `rsa` crate has an open
advisory against it, with [`adr/0004`](adr/0004-full-cargo-deny-gate.md)'s ignore list empty on
purpose.

## 48.4 TLS is a field of the request, not a mode of the client

`Request` gains `tls`, `HttpRequest` gains `tls: Bool`, and the call that sets it also moves the port
to 443, **because a request that asked for TLS on port 80 is a mistake nobody means.**

A field rather than a client setting, because a program that calls two peers may reach one over a
plaintext hop inside its own cluster and the other across the internet, and **those are two
requests**. And a field rather than a scheme in the path, because
[`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) already made the
host the atom the call performs: a URL would give a program a second place to write the host, and the
whole point of that ADR is that there is one.

**The certificate is checked against the host literal at the call site** — the same string that is
the `net.out(host)` atom and therefore the peer in the generated NetworkPolicy. There is deliberately
no SNI override, no additional trust anchor and no `--insecure`: **a way to reach a host under a name
the deployment was not told about would undo `adr/0013` from the other end.** The trust anchors are
Mozilla's, compiled in as data rather than read from the container's filesystem, because §6.2's
images execute nothing at build time and a `ca-certificates` package is a version the bill of
materials cannot state.

The gate is a **real handshake**: the suite makes a certificate authority a moment before it uses it,
issues a certificate for the loopback address, and connects. Then it does the same with a certificate
for another name under the same authority and requires a refusal — **because a verification that
cannot fail is not one** — and then points a TLS request at a plaintext server and requires that too.
The trusting constructor those tests use is `#[cfg(test)]`, so the knob does not exist in a
deployment.

## 48.5 The relying party

`beck_rt::oidc` does five things.

**Discovery.** `{issuer}/.well-known/openid-configuration`, so an operator configures one URL rather
than four. Two checks on the document that are not decoration: its `issuer` must be the one that was
asked for, and **every endpoint must be on the issuer's own host** — which narrows the egress rule to
one name and stops a discovery document moving the token exchange, and the client secret with it,
somewhere else.

**A key set.** Fetched over TLS, cached, and refetched on two triggers: a five-minute interval, and a
token naming a key id the set does not carry. **The second is the interesting half, and it is not
done where it is noticed.** Verification runs on the connection path and a key id is a string an
anonymous client chooses — so a refetch *there* would make "verify this token" a way to make a Beck
process call its identity provider. Instead the miss sets a flag, a background task reads it, and
there is a **ten-second floor** between fetches whatever asks. That is §43.1's A2 met one hop further
out than usual, and both directions are gated.

**Verification.** RS256/384/512, PS256/384/512, ES256 and ES384. **What is not in that list is the
point.** `none` is a registered JWS algorithm meaning "there is no signature", and the HMAC family is
symmetric, so a relying party that accepted `HS256` can be handed a token signed with the issuer's
own **public key** as the shared secret. Both are refused by not being in an enum rather than by a
check somebody remembered to write — and the suite *performs* the second attack rather than
describing it, computing the HMAC over the issuer's modulus and offering the result.

**The claim checks**, in one function: issuer, audience, authorized party when there are several
audiences, expiry, not-before, and the nonce when there is a nonce to compare against. The nonce is
present exactly once in a token's life — at the callback, where the relying party still remembers
what it asked for — and absent on every later connection, **because a check that always passes is
worse than no check.**

**The authorization-code flow with PKCE.** The state, the nonce and the PKCE verifier travel in a
**sealed cookie** rather than in a table this process keeps: **a login is then not a way to make a
Beck app allocate**, which is [`82`](82-the-edge-report.md)'s lesson
applied before rather than after. The seal is a keyed BLAKE3 under a key generated at startup —
§48.2's format doing a job it *is* suited for, since the process verifies what the same process
minted ten seconds ago.

## 48.6 The claims reach the program, and stop at the log

`Session` gains `claims: Map[Str, Str]`. A map rather than a type per issuer, **because what an
issuer emits is that issuer's decision and the type should say so**; and **empty** under a provider
that verifies nothing, which is what makes a lookup a check rather than a decoration.

The line worth drawing is where they *stop*. An `Envelope` still carries `actor` and nothing else, so:

- **`validate` may read a claim.** It is the authority chokepoint, it decides what a command becomes,
  and what it produces is an event — which is logged. D6's "claims → typed capabilities" is a rule
  about admission, and admission is what `validate` is.
- **A fold may not, because there is nothing to read.** §3.7 makes the log the only description of a
  program's history, and a fold that read a claim would replay differently depending on what the
  issuer was saying at the time. **Nobody had to enforce this; it follows from `Envelope`'s fields**,
  and this is written down so that stays deliberate.

Getting the claims there without two render paths took one small thing: a `Viewer` trait with
`actor()` and `claims()`, implemented for `Actor` and for `str`. A connection supplies the first;
`beck test`'s `when session("ana") sends …`, the differential harness and every benchmark supply the
second and have no claims because there was nobody to make any. **One `Session` constructor either
way** — the alternative is two places for a claim to appear in one and not the other.

### The claims have to travel to Mode B, and the reason is `validate` rather than the view

Mode B's own rule looks as if it settles the question: `B0514` refuses `@render(client)` for a page
that reads the session, so a Mode B *view* is a function of the state alone and the claims are
nothing to it. **But that is a fact about the view, and `validate` is in the bundle too** — it runs
in the browser, speculatively, handed a `Proposal` carrying a `Session`. `validate` reading a claim
is the case above that says is the *correct* one.

So a client whose claims map was left empty would refuse a command the server accepts. **The page
flashes a rejection the log never saw**, which is precisely the divergence optimism exists to be free
of — and it would be invisible to Mode B's differential gate, because that gate compares *renders*
and this is a difference in a decision.

The document therefore carries the claims beside the actor, and the kernel's load header became a
viewer rather than a bare name. Three things about what that is and is not:

- **It tells the browser nothing the page did not already.** The server verified the token, rendered
  against those claims, and sent the result.
- **It is advice, exactly as the client's `validate` is.** The socket re-verifies the cookie and
  every command goes through the server's chokepoint. A browser that edited its own claims would get
  a different guess and the same answer.
- **A claim is the issuer's string**, so it is escaped for the attribute it sits in — with the view's
  own escaper, exported rather than rewritten, and a gate that fires on a claim containing
  `"><script>`. **The actor's name went in unescaped before this and is escaped now**; it is the
  issuer's string too.

The Mode B gate is written against the shape of the gap rather than the shape of the fix: one bundle,
one command, two viewers, and the only difference between them is a claim. Reverting the plumbing
makes both clients refuse and the test go red.

## 48.7 The issuer is a declaration, because an egress rule is derived from what a program names

```python
identity = external(issuer="https://login.acme.com")
```

**The reason to build it is not tidiness.** §6.5 derives the cluster's egress rule from the hosts a
program *names*, and what makes that derivation **total** is that a computed host is refused — so
there is no outbound call the deployment cannot be told about. An issuer supplied as a `beck run`
flag broke that from the other end: nothing was wrong with the flag, but **the runtime had a peer the
compiler had never heard of**, so `beck build` would emit a policy that refuses the JWKS fetch and a
deployment would come up authenticating nobody.

So the declaration is not a configuration file in the language. It is a **host the program named**,
and the infrastructure derivation treats it as exactly that: one more `net.out(login.acme.com)` on
the effect list, and not a line below that knows the difference between a host `http_fetch` reaches
and a host the runtime fetches a key set from. The egress rule, the `derived_from` explanation and
`beck explain` all follow without being told.

Three decisions inside it:

- **It is guarded on the `=`, not on the word.** `identity` is an ordinary name, and `sicp/ch1.beck`
  defines `def identity(n: Int)` at §1.3.1 — a program in this repository, checked by a harness. Only
  `identity =` at the top level is the declaration, so `identity` is not a reserved form, and the
  suite gates that with a program that defines and calls one.
- **The issuer is checked at compile time, by the same two rules the runtime uses** — `https` only,
  and a host the network module accepts. Those are not new rules: they are the two refusals the
  runtime already made at startup, **moved to where they are a diagnostic with a span instead of a
  process that starts and cannot authenticate anybody.**
- **`--issuer` is gone.** It was the first thing this work built and it did not survive it: **two
  sources for one fact is the drift this project spends its gates on**, and the flag could not have
  been the one the derivation read. What stays on the command line is what is genuinely a
  *deployment* fact rather than a program fact — the client id, the secret and the redirect URI,
  since staging and production register different clients against one issuer.

`corpus/31-tenants.beck` exercises it and earns its place twice over: it is a corpus program, so the
round-trip property, the placement property and the formatter all run over the new form, and it is
the first program in the tree whose `validate` reads a claim. Its test block is the honest half of
§48.6's limit: **a test names an actor and cannot forge a session, so the tests it can write are the
refusals.**

## 48.8 `managed()`, and the one place `http` is not a defect

D6's other form asks Beck to *provision* the provider. `identity = managed()` does that: the emitter
derives a StatefulSet with a volume, a Service, a Secret and a realm.

**"Wired" is the word that costs something.** A Keycloak that is running and does not know this
application's redirect URI refuses every login, so the derivation has to produce a *realm*, and the
realm has to agree with objects the same graph produced. It does, because both read the same two
facts: the realm is the application's name, and the redirect URI comes from the same function the
`Route` uses, **extracted for this so the host is written once rather than twice**. The client is
**public**, because that is what a browser-facing application with PKCE is and because a confidential
client would need a secret this file invented and wrote into a manifest in a git repository.

**The interesting result is the egress rule, and it is better than `external`'s.** §6.5's derivation
has two lists, and the reason has been carried since Phase 1: a core `NetworkPolicy` egress peer is
an `ipBlock`, a namespace selector or a pod selector — **never a DNS name**, so the hosts an
`external` issuer and `http_fetch` contribute are emitted and not enforced. A **managed** issuer is a
pod in this application's own namespace, so it is a peer, and the rule is one the cluster actually
applies. **Asking for a provider you control therefore buys a guarantee that naming somebody else's
cannot.**

That is also the answer to the obvious question. The issuer of a managed provider is
`http://todo-identity:8080/realms/todo` — **plaintext**, in a chapter that has just spent §48.4
arguing there is no flag to relax `https`. There is still no flag. There are two constructors, one
requiring `https` and one not, and **which one is used is decided by the declaration** rather than by
the URL's scheme. The trust argument is different because the situation is: an external key set
crosses a network nobody in this deployment controls and TLS is its only integrity protection, while
a managed one crosses one pod-to-pod hop, to a Service this derivation emitted, permitted by a
NetworkPolicy this derivation wrote and enforced by the cluster. **What protects it there is the
policy.** Writing that down is the whole of why the in-cluster flag is a private field with a
constructor rather than a public boolean somebody could set.

Two limits. Keycloak's **production mode** wants a hostname, a database and TLS material this
derivation does not have, and starting it in a mode it cannot satisfy is a crash-loop rather than a
deployment — so the derived provider is rung-1 shaped. And **nothing here has been started**: the
conformance suite skips without a cluster, so what is established is that *the object graph contains
these objects, wired to each other*. "You can log in" is a different claim.

At rung 0 there is no cluster at all, so `beck run` on a program that says `managed()` prints
`identity dev — this program's provider is provisioned by 'beck build', and there is none here` and
believes the client. That is D6's own answer, and **the startup line is what keeps it from being a
silent one.**

## 48.9 Presence: who is here

```beck
here: Signal[Map[Str, Int]] = presence()
page: Signal[Html] = per_session(map2(combine, board, here), view)
```

The roster is a map from actor to how many connections that actor holds. **Not a declared model, and
that is a decision rather than a shortcut**: `corpus/15-presence.beck` has had exactly that type in
it since the corpus was written — it is what a program that had to fake this reached for — the
ordering is the key's and therefore a function of the value, and every question a page asks of it is
a `Map` primitive that already exists. A `Presence` model would have added a type, a docgen entry and
a wire shape, and bought nothing.

**What is interesting about it is that it is the first input to a view that moves without an event.**
The session is not a function of the log either, but it is fixed for the life of a subscription; the
accumulator moves only when the log does. The roster moves on its own, and every decision below is
that sentence applied somewhere.

**The atom is a capability, and that is F16 taken literally.** [`14`](14-review-findings.md) F16 —
"presence signals leak who-is-online; gate behind a capability like any other view" — so `presence()`
performs `cap.presence` and no new atom was added. That does three things at once: it **places** the
roster, because §3.3's table gives `cap.*` to the server and to no other tier, so a `presence()` in a
browser is a placement error with a reason attached; it **keeps the roster out of a fold**, because
the machinery that has refused a clock inside a fold since Phase 2 refuses this without being told
about it; and it is **in the signature**, so "this program reads who is online" is a fact about the
module rather than a line of code somebody has to find. What it does *not* do is stop a program that
wants the roster from having it — **nothing here is an access-control system**, and it is worth
saying which kind of gate it is.

**The chokepoint may not read it** (`B0515`). This is the whole replay argument in one rule: who was
connected when an event was recorded is written down nowhere, so a `validate` that decided from the
roster would decide one thing today and another on replay, **and the log would stop being the whole
history.** The fix the diagnostic suggests is the one `corpus/15-presence.beck` already implements:
record the fact, and decide from the state that fold produces. The check is **reachability in the
graph**, not the shape of an argument — a `signal_map` between the roster and the chokepoint is still
the roster, and writing it the other way would have been
[`82`](82-the-edge-report.md) §82.10's pattern, a limit at the one production somebody
thought of.

**Mode B may not read it** (`B0516`), for the same reason it may not read the session: the roster is
in neither the accumulator nor the log — it is a fact the server holds about its own sockets — so a
browser handed the state would have nothing to render that part of the page from. Shipping the roster
alongside would be a second wire that nothing reconciles by `seq`, which is exactly what Mode B's
correctness argument rests on not having. The program in the harness that triggers it is written so
that **only** the new refusal can fire, because **a program refused for two reasons proves neither.**

**Everything downstream of the roster runs per subscriber, and the reason is a *clock* rather than a
privacy rule.** §5.3's shared dataflow is versioned by the log's `seq`: a subscriber renders at a
version, and two subscribers at the same version must be handed the same input. **The roster moves
when `seq` does not, so there is no version at which the shared side could hold it.** Sharing it
needs a second clock, and that is §48.13's first unbuilt item.

## 48.10 The roster is bounded, and its cost is the roster's

The obvious implementation is a map from actor to a count, and **that map is unbounded memory keyed
by a string the client chooses.** Under `DevIdentity` the actor is whatever the connection said it
was, so a client opening sockets under fresh names would grow the table until the process died. That
is [`82`](82-the-edge-report.md) §82.5's finding exactly, in a second
place, **and finding it here rather than in a later hardening pass is the only thing that report's
existence is worth.**

The quota module's answer — shard into a fixed table — is **not available here**: a quota needs a
number per actor and may let two actors share a bucket, while a roster needs the actor's *name* and
would be nonsense if two names collided. So the bound is a capacity, 4,096 by default, past which a
new actor is not recorded and a counter says so. **Presence therefore under-reports rather than
growing**, which is the failure this direction should have: a page that says "127 here" when 200 are
connected is wrong in a way that costs nothing, and the opposite is a process that dies. An actor
already in the roster is never refused — the bound is on how many *names* are held.

The shape that would have been wrong is a page whose re-render on every connection walked the
accumulator: connections are the one input that moves without an event, so a roster change costing
`O(the accumulator)` would make connecting to a large application quadratic in the number of people
doing it. Gated on the shape rather than on a rate, with the evaluator's step budget as the
instrument:

| roster | 200 notes | 1,600 notes |
|---|---|---|
| 2 connected | **112 steps** | **112 steps** |
| 8 connected | **298 steps** | **298 steps** |

The same two numbers at both accumulator sizes, to the step. Eight times the notes costs nothing;
four times the people costs 2.7×, which is the roster being walked and each name's note looked up by
key. The gate holds a **constant** at two sizes rather than a ratio, so a page that started walking
the state would fail at 1,600 and pass at 200.

**A program that never asks is never woken.** Whether the view reads presence is a compile-time fact,
so a subscription to a program whose page does not mention it does not even hold a receiver on the
roster — and the harness asserts the consequence rather than the flag: a connected `todo.beck` client
is sent **no frame at all** when a second client arrives. What such a program does still pay is the
bookkeeping, one mutex and one rebuild per *connection* and never per event or per render, and that
is deliberate: it keeps "how many are connected" a fact the process has rather than one it would have
to start collecting.

## 48.11 What it costs, measured

| Token | Bytes | Per verification |
|---|---|---|
| 5 claims | 538 | **24.3 µs** |
| 68 claims | 2,362 | **57.9 µs** |

Minimum of five runs; the spread is under 5% on the small token and under 17% on the large one. Two
**sizes** because one measurement cannot tell a fixed cost from a growing one, and the two together
say something the first alone would have got wrong: **the signature is not the expensive half.**
Solving for a fixed cost and a per-byte one gives about **14 µs** fixed — which is an RSA-2048
verification, and is what it should be — and about **18 ns a byte** for everything else, which is
base64, JSON and building the claim map. **A large token costs what its *claims* cost, not what its
cryptography costs**, and the obvious optimisation — cache verified tokens — would therefore be
caching the cheap half.

24 µs is a per-**connection** cost, not a per-event one: verification runs at the document request
and at the websocket upgrade, and an event proposed on an open socket does not touch it. That is
worth stating because §48.13's refresh row makes the opposite trade look attractive and it is not
needed.

Nothing else moved. `beck explain incremental` and the plan are untouched by the extra `Session`
field, the corpus, differential, fusion and incremental-engine harnesses are unchanged, and the empty
claim map allocates nothing.

## 48.12 The gates, and a grep that would have fired and still proves nothing

`pending_security.rs` lost three tests to this work. One of them, `nothing_here_speaks_oidc`,
searched every source file for `jwks`, `id_token`, `issuer` and `RS256`. It *would* have gone red on
this change — all four appear — so it is not a fifth entry in
[`82`](82-the-edge-report.md) §82.10's list of gates that could not fail.

**It is still the weaker of the two kinds, and the reason is worth writing down beside that list.** A
name grep fires when a **subject** is touched. It would have fired equally on a module that fetched a
JWKS and checked nothing, on a module that verified the signature and forgot the audience, and on a
comment mentioning `RS256`. **The thing it can detect is "somebody worked on this"; the thing §43.4
needs detected is "this control now works."** Those coincide only when the absent control has no
behaviour to look at — which is why the file keeps the grep for F15's connection quota, whose absence
is the absence of a mechanism, and why the one test replacing these three **emits an object graph and
reads it**.

**The gate that replaced those three made the mistake in miniature.** "No identity provider is
provisioned" was first written as a search of the rendered YAML for `keycloak`, `ory` and `Kratos` —
and it went red on **`revisionHistoryLimit`**. A substring match over generated text is worth exactly
that, so it counts the object graph's workload nodes instead. The failure was harmless and the lesson
is not: **the first version would have gone green again the moment somebody renamed a field, and
nobody would have looked.**

Every other gate here was checked by breaking the thing it guards. Deleting the audience comparison,
the `azp` requirement, the issuer comparison and the expiry comparison each turned exactly one test
red; making the websocket upgrade ignore the cookie turned its gate red — **and the first attempt at
that last mutation left the refusal in place and the suite stayed green, which is a small reminder
that a mutation you did not verify is a mutation you did not perform.**

### The presence finding

The first version of the registry published its roster with a watch channel's `send`, and **`send`
fails when there are no receivers — leaving the value it was given unpublished.**

Nothing subscribes to the roster until a program that reads one has a connection, and a connection
joins the roster *before* it subscribes. So the first client's own join was always lost: it was in
the map, and the value every render read was the empty one it had been constructed with. The symptom
was a first page that said `0 here` while somebody was looking at it.

`send_replace` is the fix, and it is one word. Both halves of the suite go red without it, which was
checked by putting `send` back — and **the one test that stays green is the *watcher* test, the only
one that holds a receiver, which is the whole shape of the defect in one line.** A registry exercised
only by a harness that subscribes first would have passed everything, and nobody writing that harness
would have thought about the order twice. [`82`](82-the-edge-report.md) §82.2 is the same
shape a suite earlier: a refusal tested only as a pure function is one refactor away from never being
called.

## 48.13 What is not built

| | Status |
|---|---|
| **The roster is not shared between subscribers** | §48.9's clock is the reason, and it is a version rather than a design: a second clock on the shared dataflow — a `(seq, roster)` pair, or a render epoch with the `seq` riding along as a stamp — would let the operators above the session hold it once. Nothing makes that harder, and one file would change |
| A roster change re-renders rather than patching | `map_keys` has no delta rule, so the operator that reads it is a recompute. Eight people is 298 steps; eight thousand would be proportional |
| Presence is per **process** | Who is connected to *this* pod is what this reports. [`15`](15-scale-and-distribution.md)'s partitioned deployment has no way to answer "who is connected to the application", and that is a Phase 4 question about a fabric |
| The first paint can say `0 here` | The document is rendered before its own socket exists, and the connection joins the roster a moment later. A first frame replaces the whole root so the correction is immediate; it is written down because it is visible and because the fix is a different shape |
| Nothing is resumable about the roster | A reconnecting client is served the difference from its last `seq`, and the roster is not a function of `seq`; the page it is sent reflects the roster *now*, which is the only thing "now" can mean |
| No presence metric is exported | The counters exist and are read by tests; §5.3's gauges are exported and these are not, which is a line somebody should write the day an operator asks |
| **No `Secure` on the cookie** | §6.5's gateway terminates TLS in front of a plaintext hop, so setting it would make the cookie unusable in the deployment this project generates — the same reason [`82`](82-the-edge-report.md) §82.2 does not compare schemes in the `Origin` check. A deployment that terminates TLS *in* the pod has a cookie it would rather mark |
| **No refresh, because there is no session to refresh** | The cookie is the ID token, so a session lasts exactly as long as the issuer said and then the browser is sent back to login. The alternative — a Beck-minted session cookie over a verified login — decouples the two and costs a second credential format, a second secret and a second thing that can be stale; **the trade is stated rather than taken quietly** |
| Logout is local | It clears this app's cookie and does not call the issuer's end-session endpoint |
| One host per issuer | Every discovered endpoint must be on the issuer's host. An issuer that splits them is not usable here |
| No UserInfo, no `at_hash`, no dynamic client registration, no back-channel logout, no certificate pinning | There is no access token here to bind |
| **No refresh token, no access token, no resource server** | This is a relying party that authenticates a person to *this* application. Calling somebody else's API on their behalf is a different feature and none of it is here |
| `beck run` still serves plaintext | TLS arrived on the way *out*, not on the way in |
| **The OpenID Foundation conformance suite has not been run** | [`12`](12-standards-and-conformance.md) §12.3 names it as how this row is validated, and §12.1's rule is that a claim enters the project as an executable artefact wired into CI. This chapter claims what the tests check, one behaviour at a time, and **not** conformance — that row stays unticked deliberately, because it requires a hosted certification run against a live deployment and there is nothing deployed |
| Keycloak in production mode, and a database for it | §48.8. The derived provider is rung-1 shaped |
| Nothing has been started in a cluster | §48.8. The object graph is what is established |
| Scale | Thirteen presence tests, one corpus program, a bounded registry and a two-size shape gate say the feature is correct and that its cost is the roster's rather than the accumulator's. **Nobody has run this with a thousand connections**, and the first row of this table is what would decide what happens when somebody does |

### What this corrects, elsewhere

| Where | What |
|---|---|
| [`43`](43-threat-model.md) §43.4 | Its first two bullets are rewritten. Identity: the default is still `DevIdentity` and says so, but "OIDC is still absent" and "claims do not reach the program" are both false. Transport security: an outbound call **may** be encrypted, and what is left is the honest remainder — a request that does not say so is still plaintext, and that is the program's statement rather than a limit of the runtime. §43.2 gains a row — an actor is a decision of the runtime — and it is a *tested* row rather than a structural one, because a deployment that keeps `DevIdentity` has kept the old behaviour deliberately |
| [`10`](10-decisions.md) D6 | Says the runtime does the OIDC code flow with "the audited `openidconnect` Rust crate". It does the code flow, and not with that crate — [`adr/0023`](adr/0023-tls-and-the-signature-it-brings.md) is the argument, and the short version is that a relying party built on somebody else's HTTP client would not go through [`46`](46-standard-library-report.md)'s bounded, stubbable, egress-derivable seam. Everything else D6 asks for is here |
| [`adr/0015`](adr/0015-blake3-for-the-standard-librarys-digests.md) | Its closing section says the asymmetric decision is "still one ADR, still unwritten". It is written; ADRs are immutable, so this is the correction rather than an edit there |
| [`42`](42-security-assurance.md) §42.1 | Its table should read *tested* rather than *absent* for authentication, with the qualification that the default provider verifies nothing |
| `protocol.rs`'s comment | "Dev-mode identity … D6's OIDC relying party is Phase 3" is replaced by the distinction it was standing in for: **the frame carries a claim, and `identity` is what turns one into an actor** |
| [`03`](03-type-and-effect-system.md) §3.7 | `Session` is what an identity subsystem mints for two of its three fields; the third is [`94`](94-the-client-report.md) §94.3's |
