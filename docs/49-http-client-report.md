# 49 — Phase 3 report, part 19: the outbound call

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 2 item that had a design question in front
> of it rather than a day's typing — the HTTP client, and the effect row
> [`46`](46-standard-library-report.md) §46.6 said nobody had designed. It is built: a program
> makes a request, the row says which host it reached, and the cluster's egress rule is that same
> string. It is **plaintext**, and §49.6 says so before anything else does.

## 49.1 The item, and why it was not just another primitive

[`46`](46-standard-library-report.md) §46.6, on the standard library's remaining list:

> **HTTP client** — **untouched**. It is the one item on the list with an effect row nobody has
> designed — `net.out(host)` per call site, with the host in the type.

Every other effect atom in the language is a constant of whatever performs it. `durable` is
`durable`; `env` is `env`; `json_parse` raises `JsonError` and always will. `net.out(host)` is
parameterised, and the parameter is a **value** — so a primitive that makes a request has a row
that depends on one of its arguments, and no scheme in `prelude.rs` can say that.

The atom is also the one that is read by something outside the compiler.
[`06`](06-kubernetes-and-packaging.md) §6.5 derives the deployment's egress NetworkPolicy from the
program's `net.out` atoms and nothing else, which is §3.5's "least-privilege infra, computed" in
its most concrete form. So the design question was not "what type does a request have". It was:
**what has to be true of a program for the policy to be complete?**

## 49.2 The answer: the host is written where the call is

`http_fetch(host, request)` takes the host as its first argument, and that argument has to be a
literal. The checker reads it there and charges `net.out(that host)` at the call site;
[`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md) records the
decision, the two alternatives and what it costs.

That makes `http_fetch` the **second primitive whose row is a function of an argument** rather than
a constant. `raise` was the first ([`45`](45-error-rows-report.md)), and the precedent is exact: in
both cases the atom names the argument so that something downstream can read it — a handler there,
a NetworkPolicy here. `Prim::effects` states the constant half (`raises(HttpError)`); the checker
and `Core::effects` supply the other half from the literal, the second because
`testing::performs_itself` is what decides which definition a `stub` replaces.

A program writes no `uses` clause and no `@on`:

```
def fresh_rate() -> Int:
    return unwrap_or(str_to_int(http_fetch("rates.example.com",
        HttpRequest(method="GET", path="/usd", headers={}, body="", port=80, secrets={})).body), 0)
```

and gets, without saying anything else:

```
name                 tier     kind       effects
fresh_rate           server   definition {net.out(rates.example.com), raises(HttpError)}
```

```yaml
# k8s/100-policy.yaml
  annotations:
    beck.dev/egress-hosts: "rates.example.com"
```

The refusals are the same rule from the other side. A computed host is `B0395` — "the host of an
outbound call has to be written at the call site" — because it is a call the deployment could not
be told about. A literal that is not a name a `uses net.out(…)` clause could also write is `B0396`:
a URL, a host with a port on it, or `origin`, which is refused for its own reason — it is the one
outbound atom a *client* tier discharges, and a client reaches its own server over the command
channel rather than by fetching.

## 49.3 Three types, and a status is not a failure

`HttpRequest`, `HttpResponse` and `HttpError` are prelude declarations, the way `Json` and
`JsonError` are. The division inside them is the same one [`46`](46-standard-library-report.md)
§46.2 made: the primitive is one exchange, and everything either side of it is
`compiler/lib/http.beck` — builders, header lookup, status predicates, a JSON body — written in
Beck with its own tests, gated by being in the directory.

The decision worth stating is what counts as a failure. **A status is a reply.** A 500 arrived; a
client that turned it into an exception would have thrown away the sentence explaining why. So
`HttpError` has the three cases where *nothing* arrived — `HttpUnreachable`, `HttpTimedOut`,
`HttpBadResponse` — and a fourth, `HttpStatus`, which the primitive never raises and the library's
`require_ok` does. A caller who wants "give me the body or fail" gets it in one call; a caller who
wants to read a 429's `retry-after` still can.

`json_body` is the shape [`47`](47-effect-polymorphic-traits-report.md) §47.4 made precise, doing
its job in a library rather than in a test:

```
def json_body(response: HttpResponse) -> Json uses raises(HttpError), raises(JsonError):
    return json_parse(require_ok(response))
```

Two failures, inferred from two callees, and a `try:` catches the one its type names while the
other travels — so "the peer said 503" and "the peer said something that is not JSON" cannot be
confused for one another by a caller that wanted to tell them apart.

## 49.4 The wall this found: a credential could not be sent

`lib/http.beck` was meant to have a `with_bearer(req, token: secret[Str])`, and the first attempt
was the obvious one:

```
error[B0320]: argument mismatch: expected `internal[?5]`, found `secret[Str]`
   |     return with_header(req, "authorization", "Bearer " + reveal(token))
   |                                                                 ^^^^^
```

The compiler is right, and it is right for §3.5's best reason: **there is no `reveal` for a
`secret[T]`.** That is the claim that keeps one out of a browser. `reveal` exists for `internal[T]`,
which is the other quadrant. So a secret cannot become the `Str` a header value needs — and an
authenticated outbound request, which is nearly all of them, was inexpressible.

Note what the tree did *not* say about this. `corpus/03-billing.beck` has held an `ApiKey` since
Phase 1 and has never used it: `charge` performs `net.out(payments.example.com)` and returns
`amount > 0`. The gap was invisible for three phases because no program had ever tried to *spend* a
secret. This is [`46`](46-standard-library-report.md) §46.5's lesson again, one wave later —
writing a library finds what writing a compiler does not.

The fix is not a `reveal` for secrets, which would delete the property. `HttpRequest` has a
`secrets: map[Str, secret[Str]]` field, and the runtime merges it into the headers **at the edge**,
past every tier the checker places. The credential becomes bytes exactly where it becomes a
request, and never becomes a value the program could put somewhere else. A request carrying one is
not Sendable, so the type system already refuses to let it near a client, with nobody asserting
anything. `outbound.rs`'s two tests are the two halves: `"Bearer " + reveal(token)` is still a
compile error, and the credential is still on the wire.

## 49.5 The network, on the seam F11 asked for

[`14`](14-review-findings.md) F11 names three resources that cannot be retrofitted: clock, network
and disk. [`44`](44-wave-0-report.md) §44.3 closed the clock, three phases late.

This closes the network, and closes it *early* — `beck_core::net::Outbound` is a trait with three
implementations before there is a second caller. `Refusing` is the default, so a process that has
not decided to make outbound calls refuses them and says so in a sentence. `Canned` is replies
decided in advance and requests kept, which is what a Rust-level test uses when it wants to assert
what a program sent. `beck_rt::outbound::HttpOutbound` is the real one, over the hyper that has
been in this workspace since Phase 1 — **no new direct dependency**; enabling that crate's `client`
feature drew two transitive ones into the lock, `want` and `try-lock`, both MIT and both hyper's
own. `beck run` installs it while
`beck test` deliberately does not, because `net.out` is auto-stubbed there (§21.3) and a test that
reached a socket would depend on somebody else's uptime.

Two bounds are in the implementation rather than in the language, and both are stated where they
are set: a 10-second deadline per exchange, and 8 MiB of reply read at most — a peer that streams
for ever is the cheapest denial of service there is. Both are elapsed time and bytes, neither
enters the log, and neither can change what a replay produces.

`beck test`'s existing machinery needed **no change at all**, which is the strongest evidence that
the atom is an ordinary atom: `stub net.out(rates.example.com): 42` names the peer, and a test that
says nothing gets the auto-stub and a line in the report saying what it did.

## 49.6 What is **not** built

| | Status |
|---|---|
| TLS | **not built.** `http_fetch` speaks HTTP/1.1 over TCP. [`07`](07-dependencies.md) chooses rustls; taking a TLS stack is a dependency decision rather than a line in `outbound.rs`. Until it is taken, a credential sent with `with_secret_header` is confidential exactly as far as the network under it is. `pending_security.rs::an_outbound_call_has_no_transport_security` asserts the absence |
| A host that is a value | **refused, deliberately** — [`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md). The cost is one call site per host in an application that talks to several. A host-parameterised `Client` type is the shape that would lift it, and it needs type-level strings |
| Redirects, retries, cookies, connection reuse | **not built.** A redirect that silently changes which host is reached is the last thing a derived egress rule wants, so following one is an application's decision. Retry is writable in Beck today — a helper taking a closure inherits the closure's row, §3.2's `e` — and is not written |
| Percent-encoding and query building | **not built.** It needs a code point, and no primitive gives one. A path is sent as written |
| Repeated headers | **not representable.** `headers` is a `map[Str, Str]`, so a second `Set-Cookie` replaces the first. Said in `prelude.rs` where the field is declared, because the day a caller needs it every reader changes |
| HTTP/2, streaming bodies, timeouts per call | **not built.** A per-call deadline is a language question — §3.6 would have to give it a place in a signature — and the process-wide default stands in |
| A benchmark | **none**, for [`46`](46-standard-library-report.md) §46.6's reason: the tree-walker is 33× CPython, so a number here would measure the interpreter |

## 49.7 What this corrects

- **[`46`](46-standard-library-report.md) §46.6's HTTP row** moves from "untouched" to built, and
  its parenthetical — "`net.out(host)` per call site, with the host in the type" — turned out to be
  the right design and an incomplete statement of it: the host is in the *row*, and it has to be a
  literal for the row to be derivable at all. That last clause is the whole of
  [`adr/0013`](adr/0013-the-host-of-an-outbound-call-is-written-at-the-call-site.md).
- **[`08`](08-roadmap.md) §8.5.4's Wave 3** loses a predecessor. Its OIDC row says the relying party
  "needs an HTTP client and a signature library, so it is an ADR rather than a module". Half of that
  is now a module. The signature library is still a decision, and JWKS is fetched over TLS in
  practice, so §49.6's first row is now on the OIDC path too.
- **[`14`](14-review-findings.md) F11 should be read as two-thirds met.** [`08`](08-roadmap.md)
  §8.5.6 corrected `FIXED` to one-third when the clock landed. The network is now a seam as well.
  The disk is not, and elapsed time is not.
- **[`43`](43-threat-model.md) §43.4** gains an absence rather than losing one: a Beck process can
  now make outbound calls, and they are not encrypted. The threat model's "who is not defended
  against" list is where that belongs, and `pending_security.rs` is what keeps it from going stale.
