# 48 — Phase 3 report, part 18: identity as a seam

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 3, begun — an actor is now something the
> runtime *decides* rather than something a client asserts. It is the first half of Phase 3's
> identity bullet and explicitly not the second: there is no OIDC relying party here.

## 48.1 The gap, which was the oldest absent control in the project

[`42`](42-security-assurance.md) §42.6's first bullet:

> **Claim any identity.** `actor` arrives in the client's own `hello` frame; `protocol.rs` admits
> it in a comment … Every ownership check in every corpus program is therefore enforced against a
> value the caller chooses.

And §42.5's sharper version, which is the sentence this work exists to make true:

> "A capability required outside the chokepoint has no holder" is true and proven; "only the owner
> may toggle their todo" is enforced against a self-asserted string. The two read alike in a slide
> deck and are entirely different guarantees.

[`43`](43-threat-model.md) §43.4 lists it first among the absences, and
`pending_security.rs` has been asserting it since Wave 0 — which is what made this piece of work
start from a failing test rather than from a memory.

## 48.2 What was built

`beck_rt::identity`, a seam in the sense `beck_core::clock` is one:

- **`Actor`** has a private name. Nothing but an `Identity` implementation constructs one, so a
  `String` from a frame cannot become an actor by being assigned to one, and "where did this actor
  come from" has exactly one answer everywhere it is asked.
- **`DevIdentity`** is the previous behaviour with a name: it believes the claim. It is the default
  — `beck run` on a laptop must not need a secret — and it now says what it is, through
  `kind() == "dev"` and `verifies() == false`.
- **`SignedIdentity`** verifies `<actor;expiry;claims>.<mac>`, where the tag is a keyed BLAKE3 of
  the payload. Keyed BLAKE3 *is* a MAC, and BLAKE3 is already in this workspace's dependency graph
  for content addressing, so this costs no new dependency and contains no hand-rolled cryptography
  — the two ways a module like this usually goes wrong.

Both edges ask. The socket refuses **before** anything is rendered — no welcome, no frame, no view
— and the document handler returns 401. `beck-cli/tests/identity.rs` drives the socket loop rather
than the unit, because the thing a unit test cannot check is that the loop asks.

## 48.3 Three decisions inside it

**A refusal is coarse to the client and specific to the operator.** A client is told
`unauthenticated` and nothing else; which of missing, invalid or expired it was goes to the log.
The difference is useful to an attacker and to nobody else.

**It is counted separately.** `beck.connections.unauthenticated` is not
`beck.proposals.rejected`: one is "who are you" and the other is "you may not do that", and an
operator watching for an attack needs to tell them apart. `verifies()` exists for the same reason —
an operator who cannot tell from the logs whether authentication is on does not have
authentication, and [`42`](42-security-assurance.md) §42.6's whole point is that an absent control
was invisible.

**Expiry reads the injected clock.** `SignedIdentity` holds a `beck_core::clock::Clock`, so the
tests state an instant instead of depending on when they ran — Wave 0's seam paying for itself two
waves later, which is the first time it has.

## 48.4 What the symmetric scheme is for, and what it is not for

`SignedIdentity` is a **shared secret**: everything that can verify a credential can also mint one.
That suits the shape a rung-1 deployment actually has — a gateway in front of a Beck process,
minting credentials the process checks — and it does not suit a public identity provider, because
the process would then be able to impersonate every user of it.

Writing that limit down is the point of the type having a name. The alternative — one `Identity`
implementation and a comment — is how a symmetric scheme ends up in front of the internet.

## 48.5 What is **not** built

The Phase 3 bullet reads: "OIDC relying-party runtime, `identity = managed()` provisioning
(Keycloak/Ory), claims → `Session` capability mapping, dev-mode identity for rung 0, presence as a
first-class signal". Against that:

| | Status |
|---|---|
| Dev-mode identity for rung 0 | **built**, and now named rather than implied |
| A verifying provider | **built**, symmetric only |
| OIDC relying party | **not built**. No JWKS fetch, no RSA or ECDSA, no issuer or audience validation, no nonce, no token refresh. It needs an HTTP client and a signature library, and taking either is an ADR rather than a line in a module |
| `identity = managed()` provisioning | **not built**. It is an infra derivation — a Keycloak or Ory workload in the object graph — and belongs with `beck-infra` |
| Claims → `Session` capability mapping | **half built**. Claims are verified and available at the edge; they do not reach the program. The actor travels through the view path as a `String`, and threading an `Actor` through `render`, `maintain` and the fold is a refactor this did not do |
| Presence as a first-class signal | **not built**, and untouched by this |

Each of the three unbuilt rows is asserted as absent in `pending_security.rs`, so none of them is
prose only. `a_verified_identitys_claims_do_not_reach_the_program` is the sharpest: it reads the
prelude's `Session` declaration and fails the day a `claims` field appears, which is the day this
section has to be rewritten.

Two more limits worth stating because a reader will assume otherwise:

- **No credential is issued by anything.** `SignedIdentity::mint` exists so the format has one
  implementation rather than a description, and so tests and a gateway can produce one. There is no
  login, no session store, no refresh, and no cookie.
- **Authorisation is unchanged.** This decides *who* is asking. What they may do is still the
  program's `validate` and §3.5's capability effects, and the value of a verified actor is exactly
  that those checks now compare against something.

## 48.6 What this corrects

- **[`43`](43-threat-model.md) §43.4's first bullet** is narrowed rather than deleted: the actor is
  self-asserted *under the default provider*, which is a choice an operator makes, rather than a
  property of the protocol. §43.2 gains a row — an actor is a decision of the runtime — and it is a
  **tested** row, not a structural one, because a deployment that keeps `DevIdentity` has kept the
  old behaviour deliberately.
- **`protocol.rs`'s comment** — "Dev-mode identity … D6's OIDC relying party is Phase 3" — is
  replaced by the distinction it was standing in for: the frame carries a **claim**, and
  `identity` is what turns one into an actor.
- **[`42`](42-security-assurance.md) §42.1's table** should read *tested* rather than *absent* for
  authentication, with the qualification that the default provider verifies nothing. That document
  is a dated survey and keeps its text; this bullet is the correction.
