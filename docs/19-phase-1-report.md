# 19 — Phase 1 report: the walking skeleton

Phase 1 of [`08-roadmap.md`](08-roadmap.md) asks for "the narrowest possible compiler that takes the
todo sketch from source to a running deployment. Deliberately bad at everything, complete
end-to-end."

The compiler exists and the sketch runs. [`compiler/`](../compiler/) is 14,299 lines of Rust across
eight crates; [`compiler/examples/todo.beck`](../compiler/examples/todo.beck) is the sketch from
[`00-original-idea.md`](00-original-idea.md), 132 lines, and `beck run` serves it. Every number
below was measured on the machine described in §19.2.

**The exit criterion is met**: the compiled sketch runs in a real Kubernetes cluster, serving the
page from a pod, with its events durable in the Postgres its own `durable` effect provisioned
(§19.5). Two things the roadmap asks for are **not** done, and are named as such rather than
implied: native codegen (§19.6) and effect *inference* (§19.6).

## 19.1 What was built, against what was asked

| Roadmap item | Status | Where |
|---|---|---|
| Lexer, layout | done — `logos` tokens, hand-written INDENT/DEDENT, brackets suppress layout | `beck-syntax/src/lexer.rs` |
| Parser: Python surface | done — recursive descent + Pratt, block rule, decorators, error recovery | `beck-syntax/src/parser.rs` |
| Parser: S-expression reader | done — reads the sketch's own notation | `beck-syntax/src/sexpr.rs` |
| `Node` | done — §2.2's shape, plus one addition (§19.4 item 1) | `beck-syntax/src/node.rs` |
| Pretty-printer, `beck fmt` | done — both surfaces, round-trip and idempotence asserted over a corpus | `beck-syntax/src/print.rs` |
| Macro expander **with hygiene** | done — Flatt's sets-of-scopes, add-then-flip; template macros | `beck-macro/src/lib.rs` |
| Modules, name resolution | partial — one module; resolution is hygiene-aware, `import` parses but does not link (§19.6) | `beck-core/src/check.rs` |
| HM typechecker: ADTs, records | done — unification, occurs check, let-polymorphism, nominal newtypes, `match` exhaustiveness | `beck-core/src/{ty,check}.rs` |
| …traits | **parsed, not checked** — a warning says so at every `trait`/`impl` | `beck-core/src/check.rs` |
| `Stream`/`Signal`/`fold`/`durable` typed | done — §3.7's signatures, in the prelude | `beck-core/src/prelude.rs` |
| `Core` IR | done — typed, tier per node, signals and UI kept symbolic per §4.2 | `beck-core/src/core.rs` |
| Manual placement via `@on(...)` | done — **and verified against effects**, which the roadmap defers to Phase 2 (§19.3) | `beck-core/src/place.rs` |
| Signal-graph slicing | done — the graph is walked and inlined into per-role functions | `beck-core/src/split.rs` |
| Command channel, envelope/patch serialisers | done — content-derived operation id (§4.3) | `beck-core/src/split.rs`, `beck-rt/src/{protocol,patch}.rs` |
| Views: full recompute per event | done, and it is the dominant cost (§19.4 item 3) | `beck-core/src/split.rs` |
| Backend: **Cranelift** | **not done** — a `Core` evaluator stands in its place (§19.6) | `beck-core/src/eval.rs` |
| Backend: thin patch client (plain JS) | done — Phase 0's client, byte for byte, because nothing about it was domain-specific | `beck-rt/client/beck-thin.js` |
| Backend: Postgres/redb log engine | done — both, plus in-memory; same contract, same tests | `beck-rt/src/log.rs` |
| Backend: k8s object graph | done — derived from effects, with provenance on every node | `beck-infra/src/lib.rs` |
| `beck run` (single process) | done | `beck-cli/src/main.rs` |
| `beck up` | done — **the app runs in a cluster and serves the page**; a killed pod recovers by folding the log (§19.5) | `beck-infra/src/lib.rs` |
| Salsa from commit one | done — `parse`/`expand`/`signature` are memoised queries | `beck-db/src/lib.rs` |
| `insta` diagnostic snapshots | **not used** — diagnostics are asserted by code and message, not by snapshot (§19.6) | — |
| Differential single-process vs split harness | done, green | `beck-cli/tests/differential.rs` |
| Replay-determinism harness | done, green | `beck-cli/tests/replay.rs` |

Beyond the list, because the exit criteria needed them: `beck explain place`/`wire`/`flow`/`deploy`
(§4.7 says "shipped in v0.1", and it is), a `ui:` block macro so the view reads like the sketch, and
SSR with hydration by `seq`.

## 19.2 The machine

| | |
|---|---|
| Kernel / CPUs / memory | Linux 6.18.5, 4 vCPU, 16 GB |
| Toolchain | rustc 1.94.1; `--release` is `lto=thin`, `codegen-units=1` |
| Substrates | redb 2.x, PostgreSQL 16 (local) |
| Container runtime | Docker 29.3.1 — **present**, unlike in Phase 0 |
| Image tooling | apko 0.29.10, melange 0.29.6, k3d 5.8.3 (k3s v1.31.5), kubectl 1.32.3 |
| Nesting | this container → dockerd → k3d node → pod sandbox. The innermost level is where §19.5 stops |

The same caveat as Phase 0 applies: a shared 4-core container is not a benchmarking rig. These are
baselines to regress against, not headline claims.

## 19.3 The exit criteria

> **Exit**: `git clone && beck up` yields the working todo app in a local cluster, CI-asserted;
> differential and replay harnesses green; `beck replay` reproduces state from a recorded log.

### The sketch compiles, runs, and is served

```console
$ beck check examples/todo.beck
ok: 9 definitions, 4 signals, wire id f0c15c6d9eb8601a
```

`beck run` then serves it. Driving the real websocket with six commands — two of which the program's
own `validate` refuses — produces four acks, two nacks naming the rejection (`BlankText`,
`IdTaken`), five patch frames, and an SSR document that reflects the fold:

```html
<ul><li class="done" data-b-k="a1"><span data-b-click="{"c":"Toggle","id":"a1"}">write the fold</span>…
```

No hand-written JavaScript exists anywhere in the program, and none was generated: the client is
Phase 0's patch interpreter, byte for byte (5,963 B raw), because nothing in it was ever
domain-specific. Handlers are declarative attributes, so `script-src` can stay near-empty.

### `beck replay` reproduces state from a recorded log

```console
$ beck replay examples/todo.beck --path beck.log --verify
store              redb
head               200
folded to          200
state digest       07ebb08c4f13e4812b1f9dd68e2b799237b49d95eec16213e4258ec3dc7b4d6f
fold               0.003 s (57,203 events/s)

replay is exact: two folds agree, and the snapshot path agrees with genesis.
```

### The harnesses are green

| Harness | What it asserts | Result |
|---|---|---|
| **Differential** (§4.8's "highest-value test") | A client that has only ever seen *patches* holds a DOM byte-identical to the single-process view, after every command, for every subscriber | green |
| Differential, part two | Both sides agree on *acceptance* — including the rejections: blank text, a taken id, and one actor toggling another's todo | green |
| Replay: state | Two folds of the same log produce the same digest, and both equal the state the live process held | green |
| Replay: snapshots | The snapshot path agrees with a fold from genesis (D3) | green |
| Replay: **patch streams** | Re-deriving a subscriber's whole patch stream over the log twice yields identical bytes — and a *different* subscriber's stream differs, and is equally reproducible | green |
| Recovery | A second `App::start` over the same log lands on the live process's digest and keeps serving | green |
| Infrastructure | Removing an effect removes the objects it implied; the grant is `SELECT, INSERT` because the program never updates or deletes | green |
| Hygiene | A macro-introduced binding does not capture a caller's reference, and a caller's identifiers come back to their own scopes | green |
| Round-trip | `parse(print(parse(src))) == parse(src)` over a corpus including the whole example; `fmt` is idempotent | green |

110 tests, no failures, no compiler warnings, no clippy warnings.

### The cluster: **met**

```console
$ kubectl -n todo get all
pod/todo-7f8dbfb868-xzbqf   1/1   Running
pod/todo-log-0              1/1   Running
service/todo                ClusterIP   10.43.162.70   8080/TCP
service/todo-log            ClusterIP   None           5432/TCP
deployment.apps/todo        1/1   1   1

$ curl -s 'http://todo/?actor=alice' | grep -o '<ul>.*</ul>'
<ul><li class="done" data-b-k="k1"><span data-b-click='{"c":"Toggle","id":"k1"}'>deployed to a cluster</span>…
```

Four commands over the websocket, two of which the program's own `validate` refuses; the events
land in the Postgres the `durable` effect provisioned; a killed pod comes back with an identical
view, rebuilt by folding. §19.5 has the whole run, and the four defects getting there exposed.

### One thing done ahead of schedule

The roadmap puts placement *verification* in Phase 2. It is in Phase 1 because the splitter needs it
to be sound — a splitter that trusts an `@on(client)` on a durable fold would emit a program that
ships the log to the browser. So `@on(client)` + `durable` is rejected by name, a fold function that
reaches nondeterminism through a *named* function is rejected, and a second `merge_clients()` is
rejected. §3.10 calls this stage "already novel, already shippable"; it is worth being clear that
this is **verification against declared and collected effects, not §3.2's inference** — see §19.6.

## 19.4 What turned out harder than expected

In rough order of how much it changed the design.

1. **`head: Sym | Lit` cannot distinguish `(params)` from `params`.** §2.2's `Node` model gives a
   node a head and a list of arguments, so an empty application and a bare variable are the same
   value — and an empty parameter list is not a reference to something called `params`. Elixir hit
   this and solved it by giving a variable `nil` args where a call has a list. `Node` now carries an
   `applied: bool`, which is the same distinction at a lower cost. Small, but it is a correction to
   a design document, not an implementation detail.

2. **The block rule needs a scope, or it eats the program.** §2.3 says any call written `f(args):`
   followed by a block desugars to `f(args, do=quote(block))`. Applied literally, `for t in todos:`
   parses `todos` as a block-form call and swallows the loop body; `if ready:` does the same. §2.7
   already has the fix and states it as a mitigation for a different problem — "a block-form call
   may not appear as a non-final argument" — so the rule is now *enforced by construction*: `:`
   opens a block only in final position (statement, `return`, or the right-hand side of a binding).
   This is the single change that made the surface work at all.

3. **Full recompute is the dominant cost, and the accumulator clone is worse.** Phase 0 measured
   recompute at 334 events/s against 1.5M events/s for the state fold alone and called it a Phase 3
   dependency. Phase 1 makes the gap concrete and adds a second, larger one:

   | | Phase 0 (hand-written Rust) | Phase 1 (evaluated `Core`) |
   |---|---|---|
   | Fold from genesis, 200 events | 1.5M events/s | 57,203 events/s |
   | Fold from genesis, 4,818 events | 1.5M events/s | 7,562 events/s |

   The *shape* of that second row is the finding. Folding 24× more events cost 7.6× more per event,
   which is not `O(log n)` — it was the evaluator's `map_insert` cloning the whole accumulator, so a
   fold over a log was `O(events × rows)`. Phase 0's hand-written fold does not have this problem,
   and its own comment says why: "written as an in-place update of an owned accumulator: that is the
   shape the compiler's linear analysis produces for a fold whose previous state is dead".

   **Fixed, and the first diagnosis was wrong.** The obvious reading — Phase 1 lacks Phase 0's
   linearity analysis, so it pays the copy — makes an *asymptotic* guarantee contingent on an
   optimisation firing, which is the wrong shape for a language whose central construct is a fold.
   The defect was the data structure: `Map[K, V]` was an `Arc<BTreeMap>`, which makes copying the
   handle cheap and updating expensive, exactly backwards. It is now a persistent weight-balanced
   tree ([`pmap.rs`](../compiler/crates/beck-core/src/pmap.rs)) whose `insert` rebuilds the `O(log n)`
   nodes on the root path and shares the rest by pointer, so the fold is `O(E log n)` on every
   backend, with or without any analysis:

   | log length | per-event fold cost | |
   |---|---|---|
   | 500 → 4,000 events (8×) | 31,992 ns → 224,188 ns | **7.01×** — `Arc<BTreeMap>` |
   | 500 → 4,000 events (8×) | 11,735 ns → 19,051 ns | **1.62×** — persistent map |
   | 500 → 16,000 events (32×) | 11,656 ns → 13,075 ns | **1.12×** — persistent map |

   `cargo test --release --test scaling -- --nocapture`, and the middle row is a CI gate. Pure
   `O(log n)` predicts 1.33× over the 8× step; the residual is cache, not algorithm — the 32× step
   settles it.

   Uniqueness analysis is still worth having: it turns `O(log n)` allocations per event into `O(1)`
   when the previous state is dead. But it is now a constant-factor optimisation, which is what an
   optimisation should be, rather than the thing standing between the language and correct
   asymptotics.

   The other half of the row stands: **full recompute of the view is `O(rows)` per event** and stays
   that way until Phase 3 makes views incremental. `scaling.rs` pins it as linear so it cannot
   quietly become worse.

4. **The signal graph is legitimately cyclic, and the checker has to accept that.** `events` is
   decided from `todos`; `todos` is folded from `events`. A checker that resolves top-level
   declarations in order rejects the program. §3.7 already makes the cycle sound — validation reads
   the accumulator under the same lock as the append — so signal names are pre-registered with fresh
   type variables and unified afterwards. Worth stating in the design: **the signal graph is a graph,
   not a pipeline.**

5. **`validate` needs the accumulator, and the sketch's `filter_map` cannot give it one.** The
   sketch types validation as `Session -> Command -> Option[Event]`, and §3.7 generalises it to
   `(Session, Command) -> list[Event]` — but F2's obligations (client-minted ids accepted only if
   *fresh*; ownership checked against the actor) both require reading current state. Neither
   signature can. Phase 1 adds one primitive, `decide(proposals, state, validate)`, whose third
   argument is `(S, Proposal) -> Result[list[Event], Rejection]`. That is the shape §3.7's prose
   actually describes, and it makes "authority is one chokepoint" a node in the graph rather than a
   convention.

6. **Per-session views need to be in the language, not in the runtime.** §3.8 says per-session views
   are the norm and §5.3 says they are where a naive implementation quietly becomes Meteor-at-scale.
   Phase 0 hard-coded a `Scope` enum. In Phase 1 the view is a *signal*, so the session has to enter
   it somewhere visible: `per_session(todos, view)`, typed `(Signal[a], (a, Session) -> b) ->
   Signal[b]`. Making the fanout point a first-class node is what will let Phase 3 share
   arrangements across it.

7. **The log needs a different encoding from the wire.** `Value::to_json` drops a record's type name
   and unwraps a newtype, because that is what a browser wants — and it is exactly wrong for a log,
   where replay compares digests of what it reads back. There are now two encodings, with a test
   that says why.

8. **A hygiene scope on a *core form's* head is harmless but surprising.** Flipping a scope over a
   macro's output scopes every symbol, including the `let` and `if` that name core forms. Racket
   does the same, and it is fine because forms are matched by name — but expansion dumps read
   `(let{1} tmp{1} 1)`, and every test that asserts on printed output has to be scope-insensitive or
   deliberately scope-sensitive. Worth deciding once rather than per test.

9. **Statement-level loops have nothing to accumulate into.** `for`/`while` are parsed and then
   refused with a diagnostic pointing at `map_list`/`filter_list`/`fold`. In an expression language
   where `var` is not yet mutable, a loop is not a missing feature so much as a missing *reason* —
   but the diagnostic has to say that, or it reads as an unimplemented corner.

10. **Four defects survived every check that is not "run it".** An apko config that could not
    build; an image containing the toolchain and no program; a graph whose `HTTPRoute` and
    `StatefulSet` referenced Services it never emitted; a credentials Secret holding `""`. All four
    passed review and typechecking, and the manifests among them were admitted by a real Kubernetes
    API server. All four died the first time something tried to run. They are enumerated with their
    fixes in §19.5, and the pattern is worth naming once: **an artefact nobody has executed is a
    design document.**

11. **An image config that reads correctly can be impossible to build.** Phase 0's apko config
    hardlinks the service binary from a path nothing creates, and `beck build` emitted the same
    shape. Both are wrong, and neither could be wrong *visibly*, because apko's refusal to copy from
    the host is exactly the property that makes its builds reproducible — the config looks like a
    Dockerfile `COPY` and is nothing of the kind. §6.2 describes the reproducibility without
    mentioning `melange`, which is the tool the property makes necessary. The lesson generalises
    past apko: **a build step nobody has run is a design document, not a build step** (§19.5).

Two things were *easier* than expected. Reusing Phase 0's runtime was almost free: the differ, the
patch protocol, the sequencer and the thin client were never domain-specific, so they moved across
with their tests and only the *inputs* changed from hand-written Rust to compiled `Core` — which is
itself the strongest evidence that Phase 0 drew its boundaries in the right place. And the
infrastructure derivation stayed as easy as Phase 0 reported: "removing an effect removes a policy
rule" is still the easiest test in the project to write.

## 19.5 Phase 0's two unproven items, revisited

[`18-phase-0-report.md`](18-phase-0-report.md) §18.7 item 2 says: "Build the image and the cluster
first. They are the only unproven claims, and they are cheap to prove on a machine with a daemon."
They were not cheap, and running them found things reading them could not.

### The image: **proven**

| | Phase 0 | Phase 1 |
|---|---|---|
| Container daemon | absent | present (Docker 29.3.1) |
| Static musl binary | not built | built: **5,266,400 B**, static-pie, no dynamic dependencies |
| `apko build` | never run | **runs** |
| Image size | unknown | **3,133,440 B**, containing `/usr/bin/beck` |
| Digest reproducibility (§6.2) | a claim | **measured: two builds, one digest** — `142cda21…` |

That last row is the one §6.2 exists for: "because an apko build performs no arbitrary execution,
the same config and package versions yield the same image digest on any machine." Two builds of the
same config, byte-identical output. It is no longer a claim.

Running it also found a defect that could not have been found by reading, and that Phase 0's config
and the config `beck build` originally emitted both had:

> **apko copies nothing from the host.** An image's contents come from packages and from nothing
> else — which is *precisely* what "performs no arbitrary execution" buys. So a `paths:` stanza
> hardlinking `/usr/bin/beck` to a `/beck` that no package ever creates cannot work, and the build
> fails with `linking "/beck" -> "/usr/bin/beck": file does not exist` the first time it is run.

The binary therefore has to *be* a package. The tool that makes one is **melange**, which §6.2 does
not mention at all. `beck build` now emits both configs, in build order — `image.melange.yaml`
packages the binary, `image.apko.yaml` installs it — and a test asserts the apko config names the
binary as a package rather than hardlinking a host path, so the mistake cannot come back.

### The container: **it serves the page**

Trying to run it found a second defect, worse than the first and equally invisible on paper: **the
image contained the toolchain and no program.** `beck build` emitted a Deployment whose container
ran `beck run --store postgres` with no source file, and an image with `/usr/bin/beck` and nothing
to feed it. That container could never have served anything, `runc` or no `runc`. The melange
package now installs the program at `/app/app.beck`, the apko `cmd` and the Deployment's `args`
name it, the Deployment takes the log store's URL from the Secret the `durable` effect implied, and
three tests hold all of it in place.

With that fixed, the image runs and serves:

```console
$ docker run -d -p 8087:8080 todo:dev-amd64 run /app/app.beck --store redb --addr 0.0.0.0:8080
$ curl -s 'http://127.0.0.1:8087/?actor=alice'
<!doctype html>…<main><h1>todos</h1><input placeholder="what needs doing?"…

# driving the websocket: two accepted, one refused by the program's own `validate`
acks 2 | nacks BlankText | frames 3

$ curl -s 'http://127.0.0.1:8087/?actor=alice' | grep -o '<ul>.*</ul>'
<ul><li class="done" data-b-k="c1"><span data-b-click="{"c":"Toggle","id":"c1"}">served from a container</span>…
```

…and the log it wrote is replayable from outside it, by the host, against the same program:

```console
$ beck replay examples/todo.beck --store redb --path ./vol/beck.log --verify
head 2 · digest 4fd87eff… · replay is exact
```

### The cluster: **the app runs in it**

```console
$ kubectl -n todo get pods
todo-7f8dbfb868-xzbqf   1/1   Running          # the compiled program
todo-log-0              1/1   Running          # the Postgres its `durable` effect provisioned

$ curl -s 'http://todo:8080/?actor=alice'
<!doctype html>…<main><h1>todos</h1><input placeholder="what needs doing?"…

# four commands over the websocket; two refused by the program's own `validate`
acks 2 | nacks BlankText,IdTaken | patch frames 3

$ kubectl -n todo exec todo-log-0 -- psql -U postgres -tAc 'select seq, actor from beck_log'
1|alice
2|alice
```

Then the operational property, in the cluster rather than in a test:

```console
$ kubectl -n todo delete pod -l app=todo --force --grace-period=0
$ curl -s 'http://todo:8080/?actor=alice' | grep -o '<ul>.*</ul>'
<ul><li class="done" data-b-k="k1">…deployed to a cluster…      # a new pod, folded from the log

$ kubectl -n todo exec <pod> -- beck replay /app/app.beck --store postgres --verify
head 2 · digest 6ca7c137… · replay is exact
```

A pod that never existed before serves a view identical to the one its predecessor served, having
computed it from nothing but the log — and `beck replay --verify` agrees, running *inside* the
cluster against the cluster's own database.

### Getting there found four defects, and none of them were visible on paper

Each survived every check that does not involve running the thing.

1. **The apko config could not build.** It hardlinked the binary from a path no package creates.
   apko copies nothing from the host — that absence *is* the reproducibility story — so the binary
   has to arrive as an APK, which needs `melange`, which §6.2 never mentions.
2. **The image contained no program.** `beck build` emitted a container that ran `beck run --store
   postgres` with no source file, and an image holding `/usr/bin/beck` and nothing to feed it. The
   config was valid, the build reproducible, the manifests admitted by a real API server, and the
   container had nothing to serve.
3. **The graph pointed at objects it did not emit.** The `HTTPRoute` named a backend Service that
   did not exist; the log store's `StatefulSet` declared a `serviceName` that did not exist, so the
   fold could never have resolved its own database.
4. **The credentials Secret was empty.** `url: ""`, so even with the Service the app had nothing to
   connect to — while the emitter knew the log store's service name all along.

All four are fixed and held by tests: the package installs the program, the workload runs it and can
reach the store, every Service the manifests reference is emitted, and the Secret carries a URL that
resolves. §6.6's parity ladder asks for rung 3 to work from `git clone` the way rung 0 does, and
these were the four things standing between it and that.

### And three environment accommodations, for whoever runs this next

None of them are Beck's, but all three cost time:

* **`cpuset` is enabled in the kernel and not mounted.** k3s warns about it; `mount -t cgroup -o
  cpuset cgroup /sys/fs/cgroup/cpuset` fixes it. (It was not the cause of the failure below, but
  it is a real prerequisite.)
* **`CAP_SYS_RESOURCE` is masked out of this sandbox.** Kubelet asks for `oomScoreAdj: -998` on
  every pod sandbox, lowering `oom_score_adj` requires that capability, and without it runc's
  `nsexec` bails — surfacing as `can't get final child's PID from pipe: EOF`, which names neither
  the capability nor the field. Clamping the request to the ambient `oom_score_adj` makes pods
  start. Finding it took wrapping runc to capture the log containerd writes to a file and then
  deletes: the CRI error is three layers away from the cause.
* **Container images must be pulled through the egress proxy.** dockerd and k3d nodes both go
  direct by default; the nodes also need the proxy's CA over their trust store, and its address has
  to be reachable from the node network rather than `127.0.0.1`.

## 19.6 What Phase 1 is not

- **No native codegen.** The roadmap names Cranelift for the server tier. What is here is a `Core`
  evaluator — the "engine-in-Rust with the language as its configuration" route that
  [`00`](00-original-idea.md) names as one of the three that work for a GC'd functional language on
  a Rust host. It keeps the `Core → Target` seam narrow, which §5.2 says is what lets a backend slot
  in later, and it is the reason for §19.4 item 3's numbers. LLVM, and the differential tests
  between the two backends, are untouched.
- **No effect inference.** §3.2's rows, effect polymorphism and the wider atom set (`net.out`, `fs`,
  `cap.*`) do not exist. Phase 1 has four atoms — `ingress`, `durable`, `dom`, `nondet` — declared
  with `uses` and *collected* by walking what a body calls. That is enough to decide placement
  legality and fold purity, and nothing more.
- **No placement inference and no cost solver.** Placement is annotated and verified; the min-cut
  solver, `beck explain place`'s candidate costs, and stability across edits are Phase 2.
- **No row polymorphism.** Records are nominal `model`s.
- **No trait semantics.** `trait`/`impl` parse and warn.
- **No separate compilation.** One module. `import` parses; the `.becki` signature file of §3.6 does
  not exist. Salsa's `signature` query is the firewall's shape, tested, but nothing depends across a
  module boundary yet.
- **No `insta` snapshots.** §4.5 asks for a `tests/ui/` suite from week two. Diagnostics are
  asserted by code and by rendered text, which catches regressions in *content* but not in
  *rendering*.
- **No Mode B, no incremental views, no migrations, no operator, no identity beyond a dev-mode
  actor, no `beck fork`, no OpenTelemetry, no package system, no LSP, no playground.** All Phase 3
  and beyond, and all named in the roadmap as such.

## 19.7 What this changes for Phase 2

1. **Linearity is now an optimisation, not a correctness requirement.** §19.4 item 3's fold was
   `O(events × rows)`; a persistent `Map` made it `O(events × log rows)` unconditionally, so a
   native backend can no longer inherit the wrong asymptotics. Uniqueness analysis — "a fold whose
   previous state is dead", which Phase 0's hand-written fold already assumes — remains worth
   building alongside the native backend, for the constant factor.
2. **`decide` and `per_session` are language, not runtime.** Both were added under pressure from F2
   and §3.8 and both turned out to be the right shape. They belong in [`03`](03-type-and-effect-system.md)
   §3.7–3.8 as named constructs, not as prose about what `validate` "generally" is.
3. **The signal graph is a graph.** [`03`](03-type-and-effect-system.md) §3.7 and
   [`04`](04-compiler-architecture.md) §4.3 both read as pipelines. The cycle through `decide` is
   load-bearing and sound; the documents should say so.
4. **The block rule needs its scope written into §2.3.** §2.7 has the rule as a mitigation; it is
   really part of the rule.
5. **`melange` belongs in [`06`](06-kubernetes-and-packaging.md) §6.2.** The section describes
   apko's reproducibility without naming the tool that its central property — no arbitrary
   execution, therefore no copying from the host — makes mandatory for shipping a binary.
6. **Artefacts that are never executed accumulate defects that no amount of checking finds.**
   Four of them, in §19.5 — an unbuildable image config, an image with no program in it, a graph
   referencing objects it never emitted, and empty credentials. Every one passed review, typecheck
   and a real API server's admission. Every one died the first time something tried to run it.
   §8.3's "every phase ships a demo that runs" is not a morale exercise; it is the only check that
   catches this class, and it should gate each phase rather than conclude it.
