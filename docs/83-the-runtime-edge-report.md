# 83 — Phase 3, part 52: the runtime edge

**Built.** The websocket upgrade compares `Origin` against `Host` and refuses a mismatch, and the
socket's limits are numbers this project chose instead of numbers its library chose.

[`42`](42-security-assurance.md) §42.6 asks "what an untrusted client can do to a running Beck app
today" and answers with four bullets. Two of them were about the handshake, and both are closed
here. The other two — claim any identity, spend the log — are not, and §83.6 says so.

The mechanism is worth as much as the change. Both bullets had a **failing test** in
`pending_security.rs` asserting the gap, so building them turned those tests red, and this file's
own rule then applies: *the person who built it has to come here and to the documents and say so.*

## 83.1 What was there

```rust
WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None)
```

`None` is the configuration, so the limits were tungstenite's: 64 MiB a message, 16 MiB a frame,
128 KiB of read buffer per connection, and an **unbounded** write buffer. Bounded, then, and bounded
well — but by somebody else's judgement about somebody else's protocol.

And nothing above that line looked at `Origin`. A Beck app's page is served by the app itself and
the socket carries whatever identity the visitor's browser has; so a page on any other host could
open one, send a `hello` and a command, and be a subscriber. That is cross-site WebSocket hijacking,
and the reason it is worth fixing even while identity is still `DevIdentity` is that the fix is
independent of who the actor turns out to be: the same-origin check is about *which page asked*,
not about who is asking.

## 83.2 The numbers, and the argument for each

| | was | is | why |
|---|---|---|---|
| `max_message_size` | 64 MiB | **256 KiB** | a client sends a `hello` naming a subscription and an actor, or a `Cmd` carrying one value of the program's own `union Command`. The largest field either can hold is text a person typed into a form, and 256 KiB is around a hundred pages of it |
| `max_frame_size` | 16 MiB | **256 KiB** | a message that fits needs no larger frame |
| `read_buffer_size` | 128 KiB | **8 KiB** | **eagerly allocated per connection.** §5.3 makes per-subscriber memory a number this project reports rather than hopes about, and the library's default is tuned for high read load — a Beck client sends a few hundred bytes when somebody clicks something |
| `max_write_buffer_size` | unbounded | **8 MiB** | it only grows past `write_buffer_size` when writes are *failing*, so this is backpressure against a client that has stopped reading rather than a ceiling on what a healthy one is sent |

`write_buffer_size` is left at 128 KiB: batching outgoing patches is what it is for, and it is a
threshold rather than a per-connection allocation.

Outgoing patches are unaffected — `max_message_size` and `max_frame_size` bound what is *read*,
which is the half an untrusted client controls.

The read-buffer figure is arithmetic rather than a measurement, and is labelled as such: the library
documents that buffer as eagerly allocated, so a thousand connections hold 128 MB of it at the old
default and 8 MB at the new one. Nothing here has measured RSS at a thousand connections, and §83.6
lists that as owed rather than done.

## 83.3 The origin rule, and the three ways it could have gone

`Origin` is set by the **browser** and cannot be forged by a script, which is why it answers the one
question worth asking: is the page requesting this socket the page this server rendered? The check
compares `Origin`'s authority to `Host`, and answers `403` when they differ.

Three decisions, each of which could have gone the other way:

* **An absent `Origin` is allowed.** Non-browser clients do not send one — a script, a load
  generator, a future `beck` subcommand — and the attack this defends against *needs* a browser,
  which always sends one. Refusing an absent header would break every non-browser client for no
  security gain, because an attacker running their own client was never subject to a browser's rules
  in the first place.
* **The scheme is not compared.** Behind a TLS-terminating gateway — which is exactly what
  [`06`](06-kubernetes-and-packaging.md) §6.5's HTTPRoute is — the page is `https://app.example` and
  the request arriving here is plain HTTP. Comparing schemes would refuse every deployment this
  project generates.
* **There is no allowlist.** A Beck app serves its own page (§5.2's first paint), so same-origin is
  a *description* of the architecture rather than a policy chosen over alternatives. A deployment
  that genuinely needs a cross-origin client has nothing to configure, and [`43`](43-threat-model.md)
  §43.4 now records that as the part still absent rather than leaving it implied.

`Origin: null` — a sandboxed iframe, a `file://` page — has no authority, matches no host, and is
refused. That is the answer it should get and it falls out of the rule rather than being a case.

## 83.4 The first test of this edge

`beck-cli/tests/runtime_edge.rs` is the **first test in the project to drive `beck-rt`'s HTTP
edge.** Every harness that touches a session goes through `beck_rt::session::run` over an in-memory
duplex — which is what the `Socket` trait exists for, and is right for testing a subscription — and
the consequence is that nothing had ever exercised the handshake in front of it.

That matters here specifically: a refusal wired into `upgrade` and tested only as a pure function is
a refusal one refactor away from never being called. So the client is a `TcpStream` and a literal
request, which is what a browser sends, and the assertions are on the status line:

| request | answer |
|---|---|
| `Origin: http://<host>` — the server's own page | `101` |
| no `Origin` — not a browser | `101` |
| `Origin: https://evil.example` | `403` |
| `Origin: null` | `403` |

Both directions in one test on purpose. "Cross-origin is refused" is worth nothing without
"same-origin still works" beside it, and a check that refused everything would satisfy the first
assertion alone.

The rule itself is tested where it is a rule — six unit tests beside `same_origin`, including the
one that would catch the obvious wrong implementation: `app.example.evil.test` must not pass as
`app.example`, which a `starts_with` or a `contains` would let through. And `socket_limits()` has a
test asserting each number *and* asserting each is tighter than the library's default, so a drift
back to `None` is a decision rather than an edit.

## 83.5 What this corrects

**§42.6's smaller item had already been fixed, and the record had not.** That paragraph ends: "one
smaller item, recorded so it does not rot: `dash.html`'s `esc` escapes `&<>` only, and the graph
renderer interpolates `class="${n.tier}"` into an attribute without it." It does not. `esc` escapes
`&<>"'` and carries a comment saying why quotes are in the set; the renderer writes
`class="${esc(n.tier)}"`.

The audit, since a claim of the form "every interpolation is safe" should say what it looked at:
every `${…}` in `dash.html` is one of an `esc(…)` call, a number computed by the layout
(`cx`, `cy`, `viewBox`, `toLocaleString`, `toFixed`), an ISO timestamp from `new Date(…)`, or a
string literal chosen by a ternary (`'warn' : 'dim'`). There is no unescaped attribute
interpolation.

So the item rotted in the direction nobody watches for: it was **fixed and the record was not**, and
this document has spent some months describing a defect that stopped existing. That is precisely the
failure a `pending_security` test does not have — a test that asserts an absence goes red when the
absence ends — and it is the argument for turning a paragraph into one. The two bullets this report
closes were paragraphs *and* tests, and the tests are what brought anybody back here.

## 83.6 What is not built

| | |
|---|---|
| The other two §42.6 bullets | **not built.** Identity still defaults to believing the client (`DevIdentity`, deliberately — [`48`](48-identity-report.md)), and there is still no per-actor quota, no connection quota and no bounded deploy buffer. Their `pending_security` tests are still green, which is to say still red-in-spirit |
| A cross-origin allowlist | **not built** (§83.3), and recorded in [`43`](43-threat-model.md) §43.4 as the part of this that remains absent |
| A measurement of the read buffer | **not done.** §83.2's memory figures are the library's documented per-connection allocation times a connection count, not an RSS anybody observed. The measurement wants the fanout harness [`23`](23-incremental-views-report.md) built, pointed at real sockets rather than in-memory duplexes — which is a bigger change than this one and would be the second test of this edge |
| Anything about what happens *after* the handshake | **unchanged.** A client that passes the origin check and stays under the size limits can still open unlimited subscriptions (F15) and write to the log without a quota (F3) |

## 83.7 What this establishes

**That a list of absences is a better artefact than a list of work.** Neither of these would have
been found by reading the code — the code was doing something reasonable, with a library's defaults
and no obviously missing line. They were found because somebody wrote down what was *not* there and
attached a test to it, and the test is what made the writing-down survive contact with the months
afterwards. §42.6's fourth paragraph, which was prose rather than a test, is the control: it
described a defect that had been fixed and nobody noticed.

**And that an edge nothing tests is an edge nothing tests.** The handshake in front of every session
in this project had no test at all, which is not a criticism of the in-memory duplex — that is the
right instrument for a subscription — but a fact about where the seam was drawn. Two of the four
things §42.6 says an untrusted client can do were in the ten lines on the wrong side of it.
