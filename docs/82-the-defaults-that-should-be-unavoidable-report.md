# 82 — Phase 3, part 51: the defaults that should be unavoidable

**Built.** The generated pod drops every Linux capability, refuses privilege escalation, takes the
runtime's default seccomp profile, and gets a **read-only root filesystem unless the program's own
effect row says it writes a file**.

[`06`](06-kubernetes-and-packaging.md) §6.5 has named those four since the design was written:

> Non-obvious defaults that should be *unavoidable*, because they are what separates "generated
> YAML" from "production-grade generated YAML": non-root + read-only root filesystem + dropped
> capabilities + `seccomp: RuntimeDefault` …

One of the four was emitted. This is the other three, plus the `revisionHistoryLimit` the same
sentence names — and it is here now rather than earlier because one of them was **not derivable**
until the day before.

## 82.1 Why the read-only root filesystem waited for `fs`

[`81`](81-fs-is-two-atoms-report.md) split `fs(path)` into `fs.read(path)` and `fs.write(path)`, and
§81.6 recorded what the split made possible and did not build:

> `fs.read(p)` is a `readOnly: true` mount and `fs.write(p)` is not, which is a distinction the old
> atom could not express and which is the reason not to defer the split until the derivation is
> wanted.

That is this. With one `fs(path)` atom the emitter had exactly two choices and both were wrong:
hard-code `readOnlyRootFilesystem: true` and a program that writes a file gets a container that
refuses the write, or hard-code it `false` and every program that writes nothing — which is every
program in this repository — ships a writable root for no reason. An atom that could not say which
one the program was is why the field was simply absent.

Now it is a function of the row:

```text
readOnlyRootFilesystem = not (the program performs fs.write(_))
```

Any path rather than a named one, deliberately. The flag is about the container's *root*
filesystem, and a program that writes anywhere needs it writable; matching paths against mount
points is a different question and §82.5 leaves it open.

**The default is the secure one and the row is what relaxes it**, which is the direction that fails
safe. A program that writes a file and forgets to declare it gets a container that refuses the
write — a loud failure at the point of the write — rather than a container anybody can write to.
That is the same asymmetry §3.5 uses everywhere: an undischarged effect is a compile error, and
a *missing* declaration must never be the permissive answer.

## 82.2 The three that are constants

Nothing a Beck program can do needs a Linux capability, needs to gain privileges partway through, or
needs a syscall outside the container runtime's default profile. So those three are not derived from
anything — they are applied to **every** container this emitter writes:

```yaml
securityContext:
  allowPrivilegeEscalation: false
  capabilities:
    drop:
      - "ALL"
  seccompProfile:
    type: "RuntimeDefault"
```

They are worth having anyway, and the reason is the one §6.5 gives: the difference between generated
YAML and production-grade generated YAML is not the objects, it is the fields nobody remembers to
write. A generator is the right place for them precisely because it cannot forget.

`revisionHistoryLimit: 2` comes with them. Two is enough to roll back to and enough to see what the
last rollout changed; the default is ten and unbounded histories are how a cluster ends up holding
every ReplicaSet a deploy has ever made.

## 82.3 The asymmetry, named rather than left to be noticed

The substrate's container — Postgres — gets the three constants and **not** the read-only root
filesystem. It is somebody else's image; it writes its socket and its temporary files outside the
volume, and whether it does is not a fact any Beck effect row knows.

That is a real limit on the claim, and it is asserted in both directions rather than commented:
`every_container_drops_its_capabilities_and_refuses_privilege_escalation` checks the three on both
containers and checks that the substrate's `readOnlyRootFilesystem` is **absent**. A test that
asserts the absence is what stops somebody adding it later on the strength of the app container's
example.

The general rule this is an instance of: **a derived manifest may make claims about the program's
own image and not about a dependency's.** [`06`](06-kubernetes-and-packaging.md) §6.5's promise is
about what Beck generates from a Beck program, and the Postgres image is a choice
[`07`](07-dependencies.md) §7.8.1 made rather than a consequence of anybody's effect row.

## 82.4 How it is tested, and what is *not* tested

Three levels, and the third is a gap this report will not paper over.

**The derivation moves.** `the_root_filesystem_is_read_only_unless_the_program_says_it_writes` runs
the derivation three times — no filesystem atom, `fs.read`, `fs.write` — and asserts `true`, `true`,
`false`. The middle one is the assertion that would have been impossible yesterday, and the one that
says the split was worth taking: **reading a file is not a reason to make the root writable.**

**The manifest set is a reviewed diff.** The golden file moved by exactly the fields above, and the
snapshot gate is what made that a decision rather than a surprise — this change was written, the
suite went red, and the diff was read before it was accepted.

**Nothing here has been applied to a cluster.** `beck-infra/tests/conformance.rs` is the rung that
would `kubectl apply` these objects and it **skipped**, because there is no cluster and no `kubectl`
in this environment:

```text
conformance: the todo sketch did not run — `kubectl` is not on PATH
    (set BECK_REQUIRE_CLUSTER=1 to make this a failure)
```

So what is established is that the emitter *writes* these fields and that the flag is a function of
the row. That the pod then starts is not established. The specific risk is the obvious one: a
read-only root filesystem breaks any process that writes to `/tmp`, and the usual remedy is an
`emptyDir` mounted there. The reason for not adding one pre-emptively is that it would be a second
unverified guess rather than a fix — and the reason to think it is not needed is checkable without a
cluster: on the deployed path the runtime writes no file. `beck run` is given `--store postgres` or
`--store memory`, both of which keep the log outside the container's filesystem, and the only
`std::fs` writes in `beck-rt` are the file-backed log store (not reachable from either of those
arguments) and `beck test --update` (not a server path). `k8s.rs` now says that next to the `--store`
argument, because the two decisions have to stay true of each other.

## 82.5 What is not built

| | |
|---|---|
| A **mount** derived from `fs.read(path)` / `fs.write(path)` | **not built.** The flag is derived; the volume is not. `fs.write(/var/lib/app)` says the root must be writable and does not say a volume should exist at that path — which needs a source (an `emptyDir`? a PVC? a ConfigMap?) that the atom does not name, and [`06`](06-kubernetes-and-packaging.md) §6.5 does not either |
| The same hardening on the **Compose** platform | **not built.** Compose has `read_only`, `cap_drop` and `security_opt`, and the rung it serves is a laptop rather than a cluster. The parity claim between the two platforms is about the objects, not about the hardening, and stretching it wants its own argument |
| Resource requests and limits | **not built**, and §6.5 says why it is not a one-liner: "a genuinely hard inference problem", with a per-construct heuristic and `beck tune` planned. Emitting a guessed number would be worse than emitting none |
| Anti-affinity across zones | **not built.** `replicas` is 1, so a spread constraint would be a field with no effect. It becomes real when replicas do |
| Any of it verified against a cluster | **not done** (§82.4) |

## 82.6 What this corrects

**[`06`](06-kubernetes-and-packaging.md) §6.5's word "unavoidable" was three-quarters aspirational.**
The document has listed four defaults since it was written and the emitter has produced one of them
for as long as it has produced objects. Nothing was wrong with the *derivation* — which is what
`tests/manifests.rs` was built to check, and it checks it well — and the gap was in a list of fields
that no test asked for, because no test asks for a field nobody has written.

That is the same shape as [`80`](80-structured-concurrency-report.md) §80.11's round-trip harness
and [`70`](70-the-evaluator-gets-fast-report.md) §70.7's gate that could not fail, and it is the
third instance in three reports: **a claim in a design document is not a claim anything checks.** The
difference this time is that the claim was checkable all along and one quarter of it was not
*expressible*, which is the more interesting failure — the missing test and the missing atom were
the same absence seen from two ends.

## 82.7 What this establishes

**That splitting the atom paid twice, and the second payment is the one that was doubted.**
[`81`](81-fs-is-two-atoms-report.md) §81.5 had to correct [`80`](80-structured-concurrency-report.md)
for claiming §6.5 already derived a mount from `fs` — it did not — and the honest version of the
argument was that the split was worth taking for one reason (a `parallel:` scope may read files) with
the second available rather than banked. It is banked now, and it took one field and one boolean,
which is what "available" was supposed to mean.

**And that a generator is the right place for the fields nobody remembers.** Every field in §82.2 is
one a reviewer would have to notice was missing from a hand-written manifest, on every service,
forever. Deriving them is not cleverness; it is the one thing a compiler backend is unambiguously
better at than a person, and it is worth the report only because the design said so years before the
emitter did it.
