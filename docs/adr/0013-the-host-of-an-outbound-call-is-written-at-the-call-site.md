# 0013 — The host of an outbound call is written at the call site

**Context.** [`docs/46`](../46-standard-library-report.md) §46.6 left the HTTP client as "the one
item on the list with an effect row nobody has designed — `net.out(host)` per call site, with the
host in the type". The difficulty is real and is not about HTTP. Every other effect atom in the
language is a *constant* of the thing that performs it: `durable` is `durable`, `env` is `env`,
`nondet` is `nondet`. `net.out(host)` is parameterised, and its parameter is a value — so a
primitive that makes a request has a row that depends on an argument, which no scheme in
[`prelude.rs`](../../compiler/crates/beck-core/src/prelude.rs) can express.

The atom is also load-bearing downstream in a way the others are not.
[`docs/06`](../06-kubernetes-and-packaging.md) §6.5 derives the deployment's egress NetworkPolicy
from the program's `net.out` atoms and nothing else, and §3.5's headline claim — "least-privilege
infra, computed" — is that derivation. A host the compiler cannot see is a call the cluster cannot
be told about, and a policy that is *silently incomplete* is worse than no policy: the program
still runs in development and fails in production, or the rule is widened to `0.0.0.0/0` by whoever
is on call.

Three shapes were available.

**Decision.** `http_fetch(host, request)` takes the host as its **first argument**, that argument
must be a **string literal**, and the checker reads it there and charges `net.out(host)` at the
call site. A computed host is `B0395`; a literal that is not a DNS name a `uses` clause could also
write — a URL, a host with a port, `origin` — is `B0396`.

This makes `http_fetch` the second primitive whose row is a function of an argument rather than a
constant. `raise` is the first ([`docs/45`](../45-error-rows-report.md)), and the precedent is
exact: the atom names the argument so that something downstream can read it — a handler there, a
NetworkPolicy here.

**Alternatives.**

*The host as a field of the request*, computed like any other. Rejected: it is the shape that makes
the derivation impossible, and it is impossible to *narrow* later. Every program written against it
would have to be edited on the day the policy became real.

*A host-parameterised type* — `Client["api.example.com"]`, minted once and passed around — so the
atom rides the value. This is the shape that would let a library write `get(client, path)`, and it
is the one to revisit. It needs type-level strings, which the language does not have and which are
a much larger decision than an HTTP client; taking them under this feature's time pressure is the
mistake [`docs/39`](../39-bounds-report.md) §39.7 declined to make about operators. Nothing in this
ADR forecloses it: a `Client` type would resolve to the same atom by the same rule, and the literal
would move from the call to the mint.

*Effect-polymorphism in the atom* — `def get[h](host: h, …) uses net.out(h)` — was rejected because
`h` would be a value in a row, which is a dependent type, and because the caller of such a wrapper
would still have to be the one naming the host for the policy to be derivable. It buys syntax and
not capability.

**Consequences.**

A library cannot make a call on a caller's behalf. `compiler/lib/http.beck` therefore has no
`get(host, path)` and never will under this decision: it builds requests and reads responses, and
the application writes the two words that name its peer. That is a real cost — it is one function
per host in an application that talks to several — and it is the cost that buys a policy which is
complete by construction.

Higher-order composition survives, because §3.2's row variable already does the work: a helper that
takes a closure (`def with_retry(f: () -> a ! e) -> a ! e`) inherits whatever the closure performs,
including the atom, so retry and fallback wrappers are writable in Beck without any of them naming
a host.

A credential needs a second route, and gets one. §3.5 gives a program no way to read a
`secret[Str]`, so `"Bearer " + key` cannot be written and an authenticated request would otherwise
be inexpressible. `HttpRequest` has a `secrets: map[Str, secret[Str]]` field which the runtime
merges into the headers at the edge; the credential reaches the wire without ever having been a
value the program could put anywhere else, and a request carrying one is not Sendable, so the type
system already refuses to let it near a client.
