# 116 — The host answers back

**Built.** The four primitives that cannot be computed — `now()`, `uuid()`, `secret_env` and
`http_fetch` — compile, to **both** code generators. [`08`](08-roadmap.md) §8.5.5's Lane E row has
read *"what is left is the effects that call **back** into the host"* since
[`112`](112-a-raise-arrives-report.md), and named it as the one item in the lane that is not a
missing emitter: it wants a protocol with a second direction, where every feature so far has been
the arena answering a question it was already asked.

It is that, and it is one other thing the row did not forecast. **The protocol took two of the four
immediately and the other two were blocked by a *type*.** `secret[T]` had no machine representation
— it is a wrapper no program declares — so `secret_env` could not answer with one, and neither could
`http_fetch`, whose `HttpRequest` has a `secrets: Map[Str, secret[Str]]` field that made the whole
record unlayoutable. The lane row was one item and the work was two, which is §8.5.6's *"a wave item
can split"* happening for the third time in this project.

**Across the tree, 870 → 889 definitions compile and refusals go 208 → 189**, over 64 programs each
compiled alone. `now`, `uuid`, `secret_env` and `http_fetch` no longer appear in a refusal anywhere.

§116.6 is the honest column, and it has two rows rather than one because the answer depends on the
question: a question that cannot point into the heap costs **24.5 µs at 16 live elements and 29.0 µs
at 4,096** — the round trip, flat — and one that can costs **26.8 µs and 162.7 µs**, because the live
arena travels with it. That is a decision rather than an accident, and §116.7's gate counts the bytes
with no clock in it.

---

## 116.1 Why this one needed a new mechanism and the others did not

Every feature in this series has been the same motion: the host asks a question, the worker computes
an answer, and one message goes each way. A `Str` on the heap
([`105`](105-text-on-the-heap-report.md)), a list that grows ([`113`](113-a-list-grows-report.md)), a
page as the call that builds it ([`111`](111-a-view-arrives-as-a-recipe-report.md)) — each of them is
a different thing to put in the arena, and none of them changed the shape of the conversation.

Failure did not either. [`112`](112-a-raise-arrives-report.md) §112.1's point is that a raise is *a
way of returning*, so the unwinder that already existed carried it. §112.9 then said, in as many
words, that this said nothing about the rest:

> **Nothing about the other effects.** `io`, `log`, `net.out`, `fs.read` and the rest need the worker
> to call *back* into the host mid-call, which is a protocol with a second direction. A raise needed
> none of that, which is why it was worth taking first and why taking it says nothing about the rest.

That is the whole of what is new here. The four primitives below are not computations at all:

| primitive | its atom (§3.2) | what the answer depends on |
|---|---|---|
| `now()` | `nondet` | when the call happened |
| `uuid()` | `nondet` | a source of randomness |
| `secret_env(name)` | `env` | the process's environment |
| `http_fetch(host, req)` | `net.out(host)`, `raises(HttpError)` | a peer |

No amount of machine code produces any of those. The worker has to stop mid-call and ask.

## 116.2 The frame, and the three decisions in it

A question is 32 bytes with the same five fields a reply has, told apart by its first word —
`Upcall::MARKER`, which is `u32::MAX` and is no `Trap` code and not zero. After the header come
`2 + 2 × arity` words: the shape the answer should have, the shape a **failure** would carry, and
then a shape and a word per argument.

**The shapes are the design.** They are indices into the module's word table
(`Heap::word_of`/`Heap::shape`), so the host decodes and encodes through
[`heap`](../compiler/crates/beck-llvm/src/heap.rs) without a second table saying what `secret_env`
takes and what `http_fetch` answers. It is the same trick a view's deferred leaves play one subsystem
over ([`111`](111-a-view-arrives-as-a-recipe-report.md)): the repr becomes a *datum*, and the host —
which holds `Value` and every function that builds one — does the rest. The four match arms in
`beck_llvm::service::perform` are the only per-primitive code on the host's side, and they are four
calls on one trait.

**The answer is appended, never assigned.** The worker sends its arena's high-water mark; the host
sends back bytes to put *at* it, and never a whole arena. So nothing a live compiled value points at
can be rewritten by an answer, and that is a property of the protocol rather than a discipline
somebody has to keep. It is also what makes the host's encoder work unchanged: `Heap::encode`
computes an offset from the length of the blob it is appending to, so a blob that starts at the mark
produces offsets the worker will read at the right place.

**The arena travels only when an argument could point into it.** `now()` and `uuid()` take nothing,
so their question is a header and four words however much the program has allocated. `secret_env` is
handed text the program built and `http_fetch` a record, and the host cannot read either without the
bytes. §116.6 is what that costs and §116.7 is the gate that says it is a rule and not a habit.

## 116.3 One host, asked by three backends

The tree-walker reached the same four answers through four methods on `beck_eval::interp::Host` — a
trait in the evaluator's crate. A compiled backend that reached them a second way would make the
differential between the backends a comparison of *two descriptions of a host* rather than of the
program, which is the one thing a differential must not be.

So the four moved to `beck_core::host::Atoms`, and the evaluator's `Host` extends it. Every method
has a default and every default is the seam [`14`](14-review-findings.md) F11 asks for —
[`clock`](../compiler/crates/beck-core/src/clock.rs) for the wall clock,
[`net`](../compiler/crates/beck-core/src/net.rs) for the outbound call, the process environment for a
secret, and a UUIDv7 minted from the injected clock. `Artifact::answering` and
`Evaluator::answering` are how a harness hands all three backends *one* host.

That is what makes §116.7's differential mean anything. Two backends reading the process clock one
after the other are not in the same millisecond, and a test that compared them would be asserting
that two calls happened at once. Handed one stated host, what is left to compare is what the
backends did with the answer.

Two things moved with the trait, because they are the same claim:

- **`request_of`, `reply_value` and `failure_value`** — the translation between the `HttpRequest` a
  program built and [`net::Request`](../compiler/crates/beck-core/src/net.rs) — are now in
  `beck_core::host`. They were in the evaluator, and two backends making one call must not send two
  different requests. They are also where §3.5's one legitimate unwrapping of a `secret[Str]` happens,
  and it should happen in one place for the same reason.
- **`uuid_v7`** moved with them, since it is the default `Atoms::new_uuid` answers with.

## 116.4 The type that was actually in the way

`secret_env` answers a `secret[Str]`, and `heap::Heap::repr` refused it: *"`secret[Str]`, which is
not a type this module declares"*. That is true — no program declares it, the prelude does not
declare it as a `TyDecl`, and the checker knows it as a built-in constructor. It was also fatal twice
over, because `HttpRequest` has a `secrets: Map[Str, secret[Str]]` field, so the record `http_fetch`
takes had no layout either and the primitive was unreachable however good the protocol was.

At run time a `secret[T]` is already a one-field object — `Value::data("secret", {value})` — and it is
that rather than a bare `T` **on purpose**: the wire format and the digest have to be able to tell one
from the `T` it holds, which is what §3.5's claim rests on. So the fix is to lay it out as the
newtype it already behaves like, and the same for `internal[T]`. Unwrapping would have been smaller
and would have made the two indistinguishable in compiled code, which is exactly the property the
type system is asserting.

**A secret's bytes now cross into the worker process.** That is worth saying out loud rather than
leaving in a diff. It does not weaken anything §43 claims — a `secret[T]` still cannot reach a client
partition, the placement solver is unchanged, and `digest_keyed` is still the one declassifier — and
the worker is a child of the process that already holds the secret, spawned by it, reading only from
its pipe. What it does mean is that the set of processes a credential is in has grown by one on a
machine running `beck native`, and [`43`](43-threat-model.md)'s A1 row is where that lives.

## 116.5 A failure is an answer

`http_fetch` fails by raising `HttpError`, and **nothing was added to make that work**. The host
answers with `Trap::Raised` and a two-word pair — the shape and the value — which is precisely what a
compiled `raise` builds ([`112`](112-a-raise-arrives-report.md) §112.2). The compiled program's own
`try:` catches it without knowing an upcall happened.

One thing is written by the *worker* rather than by the host: the offset of the error type's name in
the literal pool, which is what a `try:` compares against. Only the module knows which offset that is
— the host holding an opinion about it would be two spellings of one name — so the emitter passes it
into the call and `beck.host` stores it. `Upcall::raises` is that name, and it is a fact about the
primitive rather than a field of the frame.

The same shape as §112.1, one layer out: the mechanism was already there.

## 116.6 What it costs

From `measure_native.rs::what_asking_the_host_costs`, release, median of nine, on this machine. Each
row is the same program with and without the question, so what is printed is the question rather than
the loop that built the arena in front of it:

| the question | live elements | without it | with it | the question |
|---|---|---|---|---|
| `now()` | 16 | 26.4 µs | 50.9 µs | **24.5 µs** (0 B carried) |
| | 4,096 | 48.4 µs | 77.3 µs | **29.0 µs** (0 B carried) |
| `secret_env` | 16 | 25.2 µs | 52.0 µs | **26.8 µs** (664 B) |
| | 4,096 | 47.4 µs | 210.1 µs | **162.7 µs** (163,864 B) |

Two sizes, because one measurement cannot tell a constant from a slope. The reading:

- **A question that carries nothing is a round trip, and stays one.** 24.5 µs against 29.0 µs across
  a 256× change in what is live. That is [`93`](93-llvm-backend-report.md) §93.5's pipe, paid a
  second time inside a call.
- **A question that carries the arena grows with it**, 664 B → 163,864 B and 26.8 µs → 162.7 µs. So
  `secret_env` inside a loop over a large heap is a cost a developer can meet, and `now()` is not.
  This is stated rather than smoothed because a reader deciding where to put the call needs it.

Neither number is asserted: [`13`](13-testing.md) §13.7's rule about timing gates on a shared runner.
What is asserted is the **shape**, and it is asserted without a clock — see below.

## 116.7 The gates

- **`native.rs::the_two_backends_agree_on_the_host_effects`** and
  **`cranelift.rs::the_three_backends_agree_on_the_host_effects`** — 16 calls over a program written
  around the ways an upcall can go wrong rather than around whether one works: a question with no
  arguments and a scalar answer; one with no arguments and a `Str` answer, which is the first time
  the host writes *into* the worker's arena; **two questions in one call**, where the second one's
  offsets are measured from a mark the first one moved; a question inside a loop; a question whose
  answer is built on rather than returned; a question whose argument is text, so the arena travels
  the other way; and `http_fetch` succeeding, failing uncaught, failing into a `try:`, and succeeding
  *into* a `try:`.
- **The count is a control.** The stated host tallies its outbound calls, and the differential
  asserts it is exactly twice the number of cases that reach `http_fetch` — because a run in which one
  side quietly fell back to the evaluator would agree on every value and be worth nothing.
- **`native.rs::what_a_question_carries_is_a_decision_and_not_an_accident`** — the clock-free half,
  and the one that would go red first. At 16 and 4,096 live elements, a `now()` question carries
  **0 bytes at both sizes** and a `secret_env` question carries 664 and 163,864; and a definition that
  mints four ids asks **four** questions, not one. `Artifact::questions` is the counter, and it exists
  for this the way `call_sized` exists for the arena's shape gate.
- **`cranelift.rs::the_two_emitters_accept_and_refuse_the_same_definitions`**, unchanged and now
  covering four more primitives across every program in the tree.
- The refusal lists in `scalar.rs` and `heapfix.rs` moved `reads_the_clock` from the refused side to
  the **control** side in the same commit, which is what those lists are for
  ([`83`](83-the-runtime-edge-report.md) §83.7).

## 116.8 The finding: a limit on compiled time is not a limit on a call

The worker carries an optional wall-clock limit, and the watchdog kills it when a call has not
answered in time. That limit exists because there is no fuel in compiled code: it bounds a program
that will not stop.

An `http_fetch` that waits thirty seconds for a peer is not a program that will not stop, and under
the first version of this work it was killed as though it were. The failure is instructive because
both halves were correct: the limit is right about compiled code, and blocking on a peer is right
about a network. What was wrong was that one clock was being asked to mean two things.

So the deadline is **stood down while the host works** and re-armed as a *fresh* deadline afterwards
— not the old one, because the compiled half of the call starts again from there and charging it for
the time the host spent is the same conflation in smaller print. The bound still covers every
instruction the worker executes, and covers nothing the host does, which is what it always claimed.

## 116.9 What this corrects

- [`93`](93-llvm-backend-report.md) §93.6's table has a row reading *"Any effect — the fold,
  `validate`, the view, `raise`, `parallel:`, `http_fetch` | **not built.** They cross the seam and
  land on the evaluator"*. `raise` came off it with [`112`](112-a-raise-arrives-report.md), and
  `http_fetch` comes off it here along with the other three host primitives. What is still true of
  that row is the **signal vocabulary** — the fold, `validate`, the view and `parallel:` — which are
  not primitives a body calls but nodes the splitter reads, and §116.10 says so again.
- **`beck_llvm`'s crate documentation** said every effect that has to reach the host is still the
  tree-walker's, and named the clock and `net.out` as examples. It is not, and the four it named are
  the four that compile.
- **`beck_llvm::emit`'s module documentation** said "every effect that has to reach the host, and
  growing a **map**, are refused". The second half had already been false since
  [`114`](114-a-map-grows-report.md) and nothing caught it, which is a small instance of §116.7's
  point about lists with tests attached: the refusal *lists* moved when a map started compiling and
  the prose beside them did not.
- [`14`](14-review-findings.md) F11's clock-and-network seam is now read by **three** backends rather
  than one. [`08`](08-roadmap.md) §8.5.6's F11 bullet says the clock is supplied and the network is a
  seam; what is new is that a compiled program reaches both through the same trait rather than not at
  all.

## 116.10 What this does not establish

- **Nothing about the signal vocabulary.** `merge_clients`, `fold`, `durable`, `decide`,
  `per_session`, `presence` and `freshness` are not primitives a compiled body calls: the splitter
  reads them out of the program and wires the runtime accordingly (§3.7), and the evaluator refuses
  to evaluate one for that reason. A compiled *fold* is a different item and this is not a down
  payment on it.
- **Nothing about `parallel:`.** [`80`](80-a-scope-owns-its-children-report.md) §80.5's missing piece
  is a backend that runs two children at once, and one pipe with one lock on it is further from that
  than the tree-walker is.
- **Nothing about a second concurrent question.** The worker's pipe is behind a `Mutex` and a call
  holds it for its whole duration, upcalls included — so a fold blocked on a peer blocks a view that
  wants the same worker. That was true of a slow call before this and is now true for a longer list
  of reasons. [`93`](93-llvm-backend-report.md) §93.7's "the first thing a second version would
  change" is still the first thing.
- **Nothing about `io`, `log` or `fs`.** §112.9 named them alongside these four; they are not
  primitives in the language today — a program does not call them — so what would need building is
  the language feature and not the protocol. The protocol is here when they arrive.
- **Nothing about a question the host answers wrongly.** `Trap::HostFailed` reports that this
  compiler could not turn the arena's bytes into a value or back, and `Artifact` substitutes the
  sentence for the trap's message. It is a compiler bug reported as one, and no test in this tree
  provokes it — which is the honest thing to say about a path that exists so that a mistake is a
  message rather than a wrong answer.
- **Nothing about code size or startup.** A module that asks the host gains one function of about
  sixty instructions and the arena it would have had anyway, and nothing here measures either.
