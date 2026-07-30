# 18 — Phase 0 report: the premise, measured

Phase 0 of [`08-roadmap.md`](08-roadmap.md) asks for one thing: hand-write, in Rust, the *output*
the compiler will generate for the todo sketch, then find out whether the tierless partition idea
survives contact with a real deployment — "answer that in month 3, not year 2".

It survives. The code is in [`phase0/`](../phase0/); every number below was produced by
`phase0/tests/measure.sh` on the machine described in §18.2 and is reproducible with one command.
Two items are **unproven rather than proven**, and they are named as such in §18.6.

## 18.1 What was built, against what was asked

| Roadmap item | Status | Where |
|---|---|---|
| Ingress + envelope stamping | done — one merge point, one sequencer, `seq` assigned in exactly one place | `beck-p0-server/src/app.rs` |
| Durable fold over a Postgres log | done | `beck-p0-log/src/postgres.rs` |
| …and redb embedded | done — same total order, same contract, asserted by the same tests | `beck-p0-log/src/redb_store.rs` |
| Server-side `view` + structural diff | done — keyed children, hash-skipped subtrees, `apply(old, diff(old,new)) == new` as a property | `beck-p0-core/src/{view,diff}.rs` |
| The thin patch-interpreter client | done — 1,931 bytes brotli, no application logic in it | `phase0/client/beck-thin.js` |
| `(subscription, seq)` resumption | done — replays the gap by folding from the newest snapshot at or before the client's position | `app.rs::resume` |
| apko image | **config written, never built** — no container runtime in the environment | `deploy/apko/beck-p0.yaml` |
| k8s manifests | done — 18 objects, derived from the effect row by typed `k8s-openapi` structs, not templated | `beck-p0-operator/src/infra.rs`, `deploy/k8s/` |
| kube-rs operator stub | done — control loop, CRD, and the deploy-ordering decision as a pure tested function | `beck-p0-operator/src/{controller,crd}.rs` |
| Deployed to k3d | **not run** — same reason as apko | `deploy/k3d/up.sh` |
| Kill the process mid-stream and replay | done, as a test and in a browser | `beck-p0-server/tests/kill_and_replay.rs`, `tests/browser.mjs` |

Beyond the list, because the exit criteria needed them: a per-session view (§3.8's `mine`) so the
fanout number means something; SSR first paint with hydration by `seq`; Prometheus metrics; graceful
drain; the effect-derived NetworkPolicy and database grants; and a browser end-to-end suite.

## 18.2 The machine

| | |
|---|---|
| Kernel / CPUs / memory | Linux 6.18.5, 4 vCPU, 16 GB |
| Toolchain | rustc 1.94.1, release profile (`lto=thin`, `codegen-units=1`) |
| Durable substrates | PostgreSQL 16.13 (local, default `synchronous_commit`), redb 2.x |
| Open-file limit | **4,096, hard** — the reason the 10k-subscriber measurement has two forms (§18.3.3) |
| Container runtime | **none** — no Docker daemon, no apko, no k3d, no kubectl |

A shared 4-core container is not a benchmarking rig. Treat these as *baselines to regress against*
([`13`](13-testing.md) §13.7), not as headline performance claims.

## 18.3 Exit criteria

### 18.3.1 Interaction latency (Mode A): click → command → event → fold → patch → DOM

Measured over a real websocket, from the client, with 100 rows already in the view. The simulated
RTT is applied half on each leg, so an interaction pays it twice — which is what Mode A costs by
construction. n = 1,000 per row.

| Simulated RTT | p50 | p90 | p99 | max |
|---|---|---|---|---|
| 0 ms (loopback) | **0.41 ms** | 0.51 ms | **0.65 ms** | 0.85 ms |
| 25 ms | 29.0 ms | 30.3 ms | **33.9 ms** | 70.7 ms |
| 100 ms | 105.1 ms | 106.2 ms | **123.6 ms** | 142.9 ms |

The server's own contribution is the loopback row: **sub-millisecond at p99**, and it does not grow
with RTT. Every interaction produced exactly **2 patch operations** — one attribute or keyed
insert, plus the footer's text — costing 343 bytes for an add and ~60 for a delete.

The 25 ms and 100 ms rows are software-simulated delay (`tokio::time::sleep`), which adds its own
timer granularity of a millisecond or two; they model latency, not bandwidth or queueing.

### 18.3.2 Events/s through a single sequencer, and fold throughput on replay

32 concurrent clients, 20,000 commands, in-process (no websocket in the way, because this criterion
is about the sequencer and the substrate).

| Substrate | Durable | Commit rate | Mean group commit | Replay (fold) rate |
|---|---|---|---|---|
| PostgreSQL | yes | **7,660 events/s** | 16.2 | 871,086 events/s |
| redb | yes | 8,927 events/s | 16.2 | 1,423,633 events/s |
| memory | no | 140,608 events/s | 5.8 | 2,293,052 events/s |

Two observations matter more than the absolute numbers:

- **Group commit is the whole story.** The same redb substrate driven *serially* — one command,
  wait, next command, as `beck-p0 seed` does — manages 792 events/s. Batching what the ingress
  channel has already queued into one durable append buys **11×** without changing a semantic.
  Since the batch is exactly "whatever arrived while the last append was in flight", the system
  self-tunes: latency at low load, throughput at high load.
- **Folding is thousands of times cheaper than appending.** Replaying 20,000 events from genesis
  takes 13.4 ms (1.50M events/s), so recovery is bounded by I/O, not by the fold. A process that
  has just been killed is serving again in ~20–30 ms including redb open and the fold
  (`beck_recovery_millis` reports 9–14 ms of that).

### 18.3.3 Per-idle-session server memory — the fanout number

Every subscriber holds a per-session view (`todos.filter(owner == session.actor)`), which is the
shape §3.8 says is the norm and §5.3 says kills naive implementations.

| Subscribers | What is measured | Per idle session |
|---|---|---|
| 1,000 | server process RSS, real websockets | **12.6 KB** |
| 3,000 | server process RSS, real websockets | **9.6 KB** |
| 1,000 | one process holding *both* ends (no kernel socket buffers) | 23.9 KB |
| 10,000 | one process holding *both* ends | 23.5 KB |
| — | the rendered view tree the runtime actually retains | 1.1 KB |

The 4,096 open-file ceiling made 10,000 real sockets impossible, so the 10k row is an in-process
harness: real subscriptions, real websocket framing, over an in-memory duplex. It holds *both*
halves of every connection in one process, so it is an upper bound on the server's share; the real-
socket rows are the honest server-side figure. Both are far under R5's ~50 KB tripwire, and both
are dominated by buffers rather than by the view: the view itself is 1.1 KB.

**The ceiling is CPU, not memory.** Every event wakes every subscriber to re-render and diff:

| Subscribers | Commit latency p50 | p99 |
|---|---|---|
| 1,000 | 0.84 ms | 1.75 ms |
| 10,000 | 25.6 ms | 34.4 ms |

Ten thousand idle subscribers cost ~100 MB of memory and about 25 ms of wall clock *per event* on
four cores. That is the number that decides the architecture, and it points squarely at §5.3's
shared-prefix arrangements: the fix is not to store less per session, it is to stop recomputing N
views when one dataflow could serve them. Phase 0's contribution is to say how much that has to
buy — roughly 2.5 µs per idle subscriber per event, today.

One thing the design got right for free: an idle subscriber of a per-session view receives **no
message at all** when someone else's todo changes (asserted in
`beck-p0-server/tests/subscriptions.rs`). The cost is a diff, not a frame.

### 18.3.4 Thin-client payload and time to first paint

| | Raw | Brotli | Budget |
|---|---|---|---|
| Thin client (`beck-thin.js`) | 5,963 B | **1,931 B** | 10,240 B → **19% used** |
| Stylesheet | 351 B | 174 B | — |

First paint is the SSR document itself; there is no loading state anywhere in the program. Over
loopback, time to first byte p50 **1.61 ms**, last byte p50 1.64 ms. The document is a linear
function of list size: 3.5 KB at 10 rows, 33 KB at 100, 328 KB at 1,000 — pagination is a real
requirement at scale, not a nicety, and it is a *view* concern, which is where it belongs.

What the patch stream buys, for one toggle:

| Rows in view | Patch (JSON) | Patch (binary, §4.4) | Full page |
|---|---|---|---|
| 10 | 71 B | 33 B | 3,523 B |
| 100 | 74 B | 34 B | 33,044 B |
| 1,000 | 77 B | 37 B | 328,245 B → **4,263× smaller** |

The patch is essentially constant in the size of the list, which is the property the whole Mode A
design rests on. JSON costs about 2× the binary encoding on the wire and *zero bytes of decoder* in
the client; at these sizes that trade is obviously right, and §4.4's field-tagged binary format can
wait for Mode B, where the client already carries a decoder.

### 18.3.5 Reconnect-after-deploy: does resumption actually replay the gap?

Yes.

| | |
|---|---|
| Subscribed at | seq 3,600 |
| Events missed while away | 25 |
| Server's verdict on reconnect | `resumed` |
| Catch-up patch | 7,490 B, 26 ops |
| What a *fresh* subscription would have cost | 82,441 B, 1 op (the whole view) |
| Reconnect → caught up | 2.9 ms |

The same path is exercised twice more: `kill_and_replay.rs` SIGKILLs the server and reconnects a
subscriber to a *different process* that rebuilt its state by folding the log, and the browser suite
does it with a real tab that was never reloaded. Resumption is keyed by log position and nothing
else, which is why it works across a process death — and will work across replicas.

### 18.3.6 Replay determinism

`beck-p0 verify` folds the log twice and compares, then compares the snapshot path against a fold
from genesis:

```
head               20000
state digest       df305b29d5b9d94dbc748a5f4566319b0e2001b03779029a77e385c468d55404
state fold         0.012 s (1,622,888 events/s)
patch limit        2000
patch digest       b5dd28efd936072e771e9eb12514d8cbce41932b379c82ddd41adb468518dffe
patch fold         5.995 s (334 events/s — full recompute per event, O(events × rows))

replay is exact: state and patch stream are bit-identical, and the
snapshot path agrees with a fold from genesis.
```

Bit-identical *state* and bit-identical *patch stream* — the second is the stronger claim, and the
one that makes time-travel debugging and log-backed property tests fall out of the semantics rather
than out of a framework. `kill_and_replay.rs` adds the property that matters operationally:
**everything acknowledged survives a SIGKILL**, with no drain, no snapshot and no destructors.

### 18.3.7 apko reproducibility and image size

**Not measured.** The environment has no container runtime, so `apko build` was never run and the
bit-for-bit reproducibility claim of §6.2 remains a claim. What exists: the apko config, and the
knowledge that the artefact it wraps is a single binary (3.8 MB, dynamically linked here; the image
path builds `x86_64-unknown-linux-musl` static). This is the first task of Phase 1, and it should
be done on a machine with a daemon before anything else in that phase.

> **Phase 1 revisit.** Attempted again on a machine that *does* have a daemon
> ([`19`](19-phase-1-report.md) §19.5). The static musl binary now exists and is measured —
> **3,947,136 B, static-pie, no dynamic dependencies** — but `apko build` still never ran, because
> `packages.wolfi.dev` is blocked by that environment's egress policy, and no cluster was created
> because the container registry is blocked too. Both items therefore stand as unproven for a new
> reason: not a missing daemon, but a closed network.

## 18.4 The kill/pivot gates

| Gate | Threshold | Measured | Verdict |
|---|---|---|---|
| Interaction p99 on realistic RTT ⇒ Mode B moves into the core plan | > ~150 ms | 33.9 ms at 25 ms RTT; 123.6 ms at 100 ms RTT | **not tripped** — Mode B stays in Phase 3 |
| Per-idle-session memory ⇒ redesign the session representation | can't get under ~50 KB | 9.6–12.6 KB | **not tripped** |

The latency gate deserves a caveat: at a 100 ms RTT — a transatlantic user on a mobile network —
p99 reaches 124 ms, which is inside the gate but not comfortably. Mode A's cost is one round trip
per interaction, and no amount of server work changes that. The plan already answers this (Mode B,
Phase 3, for latency-sensitive components); Phase 0's numbers say the answer is needed for *distant*
users rather than for *all* users, which is exactly the per-component decision §5.1 describes.

## 18.5 What turned out harder than expected

The roadmap asks for this list explicitly. In rough order of how much it changed the design:

1. **A patch stream carries states, not events.** A command whose net effect is invisible — an add
   and its delete coalesced into one wake, or a change filtered out by a per-session view — produces
   an empty diff and therefore *no frame*, and a client waiting for "the patch for my command" waits
   forever. The measurement harness deadlocked on exactly this. The fix is one message
   (`{"t":"u","q":N}`, sent only to a client waiting on its own command, never to idle ones) and it
   is now a protocol rule rather than an accident: **the ack tells you the command landed; the frame
   tells you where your view stands, and the two are different facts.**
2. **Full recompute is fine live and quadratic on replay.** Re-rendering the view after every event
   costs 334 events/s against 1.6M events/s for the state fold alone — a 5,000× gap that grows with
   list length. Interactive latency never notices (the list is one size, the diff is 2 ops), but
   *re-deriving a patch stream over a log* — time-travel debugging, patch-level replay tests — is
   O(events × rows). Incremental views (§5.3, Phase 3) are not only a throughput optimisation; they
   are what makes patch-stream replay usable at all. Until then, `verify --patch-limit` bounds it.
3. **Structural hashes must be canonical, not incremental-by-accident.** The first differ folded a
   node's hash in builder-call order, so two structurally identical subtrees built in different
   orders hashed differently, and the differ skipped work it should have done — or redid work it
   should have skipped. A hash used to prove *equality* has to be a pure function of the structure;
   it now accumulates tag/key, attributes and children separately.
4. **Determinism needs a total order everywhere, including ties.** The sketch sorts todos by text.
   Two todos with the same text left the order to whatever the map iterator did, which is stable
   within a process and not a property you can build replay bit-identity on. Ties now break by id.
   Anywhere a view "doesn't care" about order, replay does.
5. **Validation must see the batch it is inside.** Group commit means several commands are validated
   before any of them is appended, and `Add(x)` followed by `Toggle(x)` in the same batch must
   work. The sequencer therefore applies each command's events to the accumulator as it validates,
   under the same write lock it holds across the append, and predicts the `seq`s the store will
   assign — then asserts they match. That assertion is how a second writer would be caught.
6. **A failed append has no repair path, and that is correct.** If the log refuses a write after the
   fold has advanced, the process's state is ahead of the durable truth. There is nothing to
   reconcile: the process aborts, and the next one folds the log. Writing this down made it obvious
   how much of the design's simplicity comes from the fold being *downstream* of the log.
7. **redb is single-process.** It takes an exclusive file lock, so `beck replay` cannot inspect the
   log of a running `beck run` at rung 0 — a real ergonomic wart on the rung developers spend 99% of
   their time on, and one that Postgres does not have. Either the dev rung grows a read-only path
   through the running process, or rung 0's tooling talks to the server rather than the file.
8. **Distroless has no `sleep`.** The generated pod's `preStop` drain delay cannot be an `exec`
   probe when the image contains nothing but the binary; it uses Kubernetes' native sleep action.
   Small, but it is the kind of detail that only appears when the artefact really is distroless.
9. **SSR whitespace is load-bearing.** Patch paths are child indices, so a pretty-printed document
   introduces text nodes the server's tree does not have and the first patch lands on the wrong
   node. The renderer emits no inter-element whitespace, deliberately.
10. **Client-minted ids leak into the tooling, correctly.** First-writer-wins means a benchmark
    re-run against the same log is *rejected*, because it proposes ids that already exist. The
    harness now mints a per-run nonce. This is the semantics working as designed, but it is the kind
    of thing that surprises you the first time.
11. **The environment's file-descriptor ceiling shaped the measurement.** 4,096 hard, so the headline
    10k-subscriber figure needed a second harness that keeps the subscriptions in-process. Worth
    remembering when Phase 1 sets up CI: the fanout budget needs a runner with a raised limit, or it
    silently measures something smaller than it claims.

Two things were *easier* than expected, and both are worth banking: SSR plus resumption-by-`seq`
made hydration free (the document carries the position it reflects, so the first socket message
either finds nothing to do or is exactly the gap), and building Kubernetes objects as typed Rust
values made the "infrastructure is a function of the program" claim concrete in an afternoon — the
tests that assert *removing an effect removes a policy rule* were the easiest tests in the project
to write.

## 18.6 What Phase 0 is not

No compiler, no parser, no type checker, no effect inference, no placement solver. No Mode B, no
incremental views, no identity beyond a dev-mode actor name, no OpenTelemetry, no Crossplane, no
multi-arch images, no signing. The image was not built and the cluster was not created — those two
are the only *roadmap* items left unproven, and both are blocked on tooling rather than on design.

The authorisation model is deliberately minimal but not absent: first-writer-wins on client-minted
ids and ownership checks against the envelope's actor (F2), which the browser suite exercises by
having one user fail to toggle another user's todo.

## 18.7 What this changes for Phase 1

1. **Keep the sequencer shape.** One merge point, one writer, group commit, fold under the same lock
   as the append. It is simple, it is fast enough, and every property in §18.3.6 depends on it.
2. **Build the image and the cluster first.** They are the only unproven claims, and they are cheap
   to prove on a machine with a daemon.
3. **Treat the patch-stream cost as a Phase 3 dependency, not a Phase 3 nicety.** The differential
   harness (§4.8) will want patch-level comparison over long logs; at 334 events/s it cannot have it.
4. **Budget the fanout wake, not the fanout memory.** 2.5 µs per idle subscriber per event is the
   number to beat with arrangement sharing; per-session memory is already a solved problem.
5. **Carry the protocol rule forward.** Ack means *committed*; frame means *your view is here*.
   Whatever the wire format becomes, Mode B's optimistic reconciliation needs both facts, and Phase 0
   found out the hard way what happens when only one of them exists.
