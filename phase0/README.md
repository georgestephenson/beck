# Phase 0 — prove the premise

> "No compiler. Hand-write, in Rust, the *output* the compiler will eventually generate for the
> todo sketch: ingress + envelope stamping, a durable fold over a Postgres log (and redb embedded),
> server-side `view` + structural diff, the thin patch-interpreter client, `(subscription, seq)`
> resumption, an apko image, k8s manifests, a kube-rs operator stub, deployed to k3d. Then kill the
> process mid-stream and replay the log."
> — [`docs/08-roadmap.md`](../docs/08-roadmap.md)

This directory is that program. It is **not** the beginning of the compiler; it is the compiler's
*output*, written by hand so that the architecture can be measured before anything is built on top
of it. Results and the honest list of what turned out harder than expected are in
[`docs/18-phase-0-report.md`](../docs/18-phase-0-report.md).

The source it stands in for is the 40-line sketch in
[`docs/00-original-idea.md`](../docs/00-original-idea.md). Every file below says which part of the
design it implements, and where the compiler will one day generate it.

## Run it

```console
$ cargo run -p beck-p0-server -- run --store memory --addr 127.0.0.1:8080
$ open http://127.0.0.1:8080/?actor=alice
```

Two tabs with different `?actor=` values show the multi-client behaviour; `?scope=mine` switches to
the per-session view (§3.8). Kill the process and start it again: the state comes back, because the
state *is* the log.

```console
$ cargo run -p beck-p0-server -- seed    --store redb --events 20000   # traffic through real ingress
$ cargo run -p beck-p0-server -- replay  --store redb --genesis        # beck replay
$ cargo run -p beck-p0-server -- verify  --store redb                  # replay is exact
$ cargo run -p beck-p0-operator -- emit --out deploy/k8s               # infra as a function of effects
```

## What is here

| Path | What it is | Design reference |
|---|---|---|
| `crates/beck-p0-core` | The pure tiers: domain, envelopes, `validate`, the replay-pure fold, `Html`, the structural differ, the patch protocol | [`02`](../docs/02-syntax.md), [`03`](../docs/03-type-and-effect-system.md), [`04`](../docs/04-compiler-architecture.md) §4.4 |
| `crates/beck-p0-log` | The log engine over Postgres, redb and memory — one total order, three substrates | [`05`](../docs/05-tier-lowering.md) §5.3, [`07`](../docs/07-dependencies.md) §7.4 |
| `crates/beck-p0-server` | The runtime: ingress, the sequencer, the durable fold, per-session views, patch fanout, SSR, metrics, drain | [`05`](../docs/05-tier-lowering.md) §5.2, [`04`](../docs/04-compiler-architecture.md) §4.3 |
| `crates/beck-p0-operator` | The `InfraGraph` derived from the effect row, and the kube-rs operator | [`06`](../docs/06-kubernetes-and-packaging.md) |
| `crates/beck-p0-bench` | The exit-criteria measurement harness | [`08`](../docs/08-roadmap.md), [`13`](../docs/13-testing.md) §13.7 |
| `client/beck-thin.js` | The patch interpreter — the only JavaScript in the system, and it is compiler residue | [`05`](../docs/05-tier-lowering.md) §5.1 |
| `deploy/` | apko image config, generated Kubernetes objects, effect-derived database grants, k3d bring-up | [`06`](../docs/06-kubernetes-and-packaging.md) |
| `tests/` | The browser end-to-end suite and `measure.sh` | [`04`](../docs/04-compiler-architecture.md) §4.8 |

## The tests worth knowing about

```console
$ cargo test                                              # unit + substrate + kill/replay
$ BECK_PG=postgres://…/beck_p0 cargo test                 # ...including Postgres
$ NODE_PATH=$(npm root -g) node tests/browser.mjs         # the real thing, in Chromium
$ ./tests/measure.sh                                      # every number in the report
```

- `crates/beck-p0-server/tests/kill_and_replay.rs` — SIGKILL mid-stream; assert that everything
  acknowledged survived, that replay is bit-identical, and that a subscriber resumes across the
  death of the process it was talking to.
- `crates/beck-p0-log/tests/substrates.rs` — the same contract asserted against all three
  substrates, and against the pure fold as oracle.
- `crates/beck-p0-operator/tests/manifests.rs` — the committed manifests must be what the effect
  row implies; hand-edited YAML fails the build.
- `crates/beck-p0-core/src/diff.rs` — `apply(old, diff(old, new)) == new` over a long random walk.

## Deliberate limits

Phase 0 is a measurement instrument, not a product. It has no compiler, no type checker, no effect
inference, no incremental views, no Mode B client, no identity beyond a dev-mode actor name, and no
authorisation beyond first-writer-wins and ownership. Those are Phases 1–3, in that order. What it
does have is every tier of the eventual system, end to end, with numbers attached.
