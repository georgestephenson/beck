# 17 — The playground: the whole stack in a tab

> **The insight** (George): a playground for a full-FULL-stack language is a deep thing — a cloud
> environment for the entire stack. And build the whole playground in Beck itself.

Deeper than it first appears, because of a convergence already latent in the plan: **the entire
Beck stack can run in the browser tab — database included — for free.** Rungs, from zero-cost to
cloud:

> **Status**: rungs A and B are **built** — [`98`](98-playground-report.md), which also says what
> each of the five sections below still lacks (§98.7). Four of those lacks are now built too:
> the tab's log survives a reload, a share link is a content-addressed fragment, a `@render(client)`
> program runs in the client iframe, and the editor has highlighting, completion and inline
> diagnostics — [`101`](101-playground-phase-3-report.md), which says what each still does not
> deliver (§101.6). Rung C is Phase 4's and untouched.

## 17.1 Rung A — compile-time playground (static, costs a CDN)

The compiler is pure Rust, so it compiles to WASM and runs client-side. With zero servers, a
visitor gets the *entire compile-time story*: type checking with real diagnostics, macro expansion
(`beck fmt --sexpr` toggling between the two surfaces), **inferred placement per definition**,
generated dataflow/SQL plans, generated Kubernetes objects, effect signatures, and `beck explain`
— source on the left, *what the compiler derives* on the right. This alone out-demos most language
playgrounds, and it is the 60-second version of the pitch ([`09`](09-risks-and-open-questions.md)
§9.3). Scope note: the in-browser toolchain carries the front end + the WASM backend + the
reference interpreter; LLVM stays server-side (it doesn't fit a tab and doesn't need to).

## 17.2 Rung B — the whole *application* in the tab (the one nobody else can build)

Here is the convergence. Beck's rung-0 story is "the whole app in one process with an embedded
log" ([`06`](06-kubernetes-and-packaging.md) §6.6). And the DST prerequisite (F11,
[`13`](13-testing.md) §13.4) already forces the runtime to be written against **virtualized clock/
network/disk interfaces**. A browser tab is just a third implementation of those interfaces:

| Runtime interface | Production | DST | **Playground** |
|---|---|---|---|
| Clock | OS | simulated | `performance.now()` |
| Network | Tokio/websocket | simulated | `MessageChannel` |
| Log storage | Postgres/redb | simulated | IndexedDB / memory |

Every row of that table is built, the storage one last ([`101`](101-playground-phase-3-report.md)
§101.2): the tab's log is an array, handed to IndexedDB as the same records a durable store writes
and keyed by the program's wire id, so a reload continues rather than starting from `init`. Mode B
runs in the tab too — the kernel in the client iframe, the bundle over the port (§101.4).

So the *same* compiled program runs: the "server" — ingress, `validate`, folds, Mode A rendering —
in a **web worker**; the log in IndexedDB; the thin patch client in an iframe, speaking the
identical patch/command protocol over a `MessageChannel` instead of a websocket. The visitor gets
a **complete running application — database, backend, live UI — with no signup, no cloud, no
cost**, and it is not a toy approximation: by the differential harness's own guarantee, rung-B
behaviour *is* the deployed behaviour.

Two demos this uniquely enables:

- **Multiplayer in one tab**: open two client iframes against the same worker-server and watch
  optimism, reconciliation by `seq`, and per-session fanout live — the distributed-systems story,
  demonstrated without a network.
- **Time travel in the pitch**: a `seq` scrubber under the app — drag history back and forth,
  because the tab holds a real log and real deterministic folds. `beck replay` as a toy anyone can
  touch in their first minute.

No other stack can do this, structurally: their "full stack" doesn't fit in one deterministic
process because it was never one program to begin with.

## 17.3 Rung C — the cloud playground (real cluster, metered, guarded)

For the infra tier — watching `beck up` produce real pods, policies, volumes — sessions get an
ephemeral, TTL'd environment (vcluster-per-session or Firecracker-class microVMs) on a quota.
The security model leads with Beck's own machinery:

1. **The compiler is the first sandbox**: playground builds compile against a restricted effect
   budget — `net.out` to anywhere is a *compile error* in cloud-playground mode, `fs`/`env`
   likewise. Programs that phone home are rejected before a container exists. (The typed-sandbox
   demo is itself a selling point: the error message names the effect and the line.)
2. Then the generated infra applies its own guarantees: deny-all egress NetworkPolicies
   ([`06`](06-kubernetes-and-packaging.md) §6.5), non-root distroless images, CPU/memory quotas.
3. Then platform hygiene: gVisor/Firecracker isolation, signup required for rung C (rungs A/B are
   anonymous), 15–30 min TTL, no image pulls beyond the playground base, and rate limits on
   environment creation — the crypto-miner economics simply don't close.

## 17.4 Sharing: playgrounds are content-addressed values

A playground is a program, and Beck programs are content-addressed artefacts
([`16`](16-packages-and-ecosystem.md)): a share link is a digest; forks are new digests; embeds
(docs, blog posts, issue reports) resolve through the same CDN; the docs site's every example is a
live rung-A/B playground (docs-as-tests, [`13`](13-testing.md) §13.6, now also docs-as-demos). Bug
reports arrive as playground links — a reproduction *is* a digest.

> **Status**: half built ([`101`](101-playground-phase-3-report.md) §101.3). A link is
> content-addressed and self-certifying — the fragment carries the compressed program and names its
> BLAKE3 digest, and a link that does not match its digest is refused — and a fork is a new digest
> because a fork is different bytes. What needs the registry is everything that requires a digest to
> *resolve*: short links, embeds, and a bug report that is a digest rather than a program.

## 17.5 The playground is a Beck app (D15's first citizen)

Per [`10`](10-decisions.md) D15, built in Beck — and its own domain exercises the semantics:

- Snippets and sessions are **event-sourced** (edits, forks, shares are events; a snippet's history
  is a fold — undo and version history come from the semantics).
- Rung-C orchestration is a **saga**: `process provision_env: on RunRequested → emit_command(
  Provision); on timeout(30.min) → emit_command(Teardown)` — TTL cleanup as a typed compensation,
  not a cron script that forgets.
- Abuse control is the F3 quota machinery, live, in public.
- Presence ("3 people viewing this playground") is the D6 presence signal — and later, CRDT-valued
  `Text` (D7) makes shared editing the natural v1.x demo.

## 17.6 Roadmap fit

Rung A ships with Phase 3 as planned ([`08`](08-roadmap.md)) — it needs only the front end + WASM
backend. **Rung B lands with Mode B's WASM kernel in the same phase** — the worker-server is the
rung-0 platform compiled to WASM, so it rides work already scheduled; the `seq` scrubber and
two-client demo are small UI on top. *(Both landed in Phase 3 as forecast, and this paragraph got
one thing wrong: rung B did not ride Mode B's kernel. A kernel interprets a component's bundle; a
worker-server is a sequencer, a log and a differ, and what rung B rode was a division of the
runtime into the half that is program-shaped and the half that needs a machine —
[`98`](98-playground-report.md) §98.2, §98.9.)* Rung C follows in Phase 4 alongside the operator (it *is* an
operator workload). The playground app itself grows across those phases and, with beck.dev and the
registry, completes the D15 dogfood triad: **playground proves the language, the site proves the
web tier, the registry proves the backend and data tier** — all three in Beck, all three public.
