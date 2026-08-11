# 44 — Phase 3 report, part 14: Wave 0

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 0, built — the set whose members were
> "overdue, a one-way door, or a gate on something already shippable". Seven items, of which two
> were code, two were code and prose, and three were prose alone.

This is the first report about a *wave* rather than about a feature, and the difference is the
point of §8.5. None of these items is interesting. Four of them had been decided, written down and
agreed for one to three phases, and had not happened — not because anybody disagreed but because a
decision with no position in an order never comes due.
[`42`](42-security-assurance.md) §42.4 named the pattern precisely, about F11: "the decision was
correct and written down twice. What it never had was a **position in an order**."

So the honest summary of this wave is not "we built seven things". It is: **a list of decisions
became a list of gates.** Every item below ends in something that goes red.

## 44.1 What was built

| Item | Class (§8.5.1) | Artefact | The gate |
|---|---|---|---|
| Front-end recursion bound | **G** | `beck_diag::depth`, [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md) | `front_end_bound.rs`; a per-crate measurement of bytes per level |
| Injected clock | **R**, overdue since Phase 1 | `beck_core::clock` | `clock.rs`: `SystemTime::now()` in exactly one place |
| Threat model | **G** | [`43`](43-threat-model.md) | §43.4's absences are `pending_security.rs`'s tests |
| Disclosure policy + memory-safety roadmap | **G** | `SECURITY.md` | none needed; its absence was the defect |
| `pending_security` suite | **G** | `pending_security.rs` | itself: each test goes red when the control is built |
| The two syntax decisions | **R**, deadline end of Phase 3 | [`10`](10-decisions.md) D21, D22 | — |
| Unicode pin + UTS #39 | **R** | `beck_syntax::security` | `identifiers.rs`, grouped by attack |
| The four moved standards rows | **S**, free | [`12`](12-standards-and-conformance.md) §12.6–§12.7 | — |

## 44.2 The recursion bound, and the number that was already false

[`42`](42-security-assurance.md) §42.2 measured `beck check` aborting on an ~7.6 KB file of nested
parentheses — `fatal runtime error: stack overflow`, no span, nothing catchable — while the same
file compiled in a release build, where the threshold was more than an order of magnitude further
out. It also measured the sharpest version of the defect: the 64 MiB
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md) declared for 4,000 evaluator
frames was exhausted by ~3,600 *parser* frames, so the declaration was not merely incomplete — in
one profile it was already false.

The fix is one count and one declared stack, and the reasoning is 0007's, applied to the front end
by [`adr/0012`](adr/0012-the-front-end-counts-its-own-recursion.md). `MAX_NESTING` is **256**,
applied at each recursion *site* rather than at one grammar production — the Scriban lesson §42.2
quotes — so the parser counts at `primary` and `block`, the S-expression reader at `list`, the
expander at its argument walk, and the checker at `expr` and `ty_from_node`. Everything downstream
walks the `Core` the checker built and is bounded by construction.

**Measured, not assumed.** The per-level cost is what decides whether a count is a bound or a
wish, so each crate that recurses measures its own and fails if the declaration has stopped
covering it — the pair `beck-eval` has had since [`27`](27-the-walls-come-down-report.md) §27.2:

```
$ cargo test -p beck-syntax nesting -- --nocapture
parser: 3653744 bytes for 200 levels (18268 per level)
$ cargo test -p beck-core nesting -- --nocapture
checker: 409856 bytes for 100 levels (4098 per level)
```

18 KiB per level in the parser, unoptimised. The ceiling therefore costs about 4.7 MiB — inside the
stack an ordinary main thread has, and a fourteenth of the 64 MiB declared. That the two numbers
now have to agree is itself a gate: `const _: () = assert!(beck_diag::depth::STACK_BYTES <=
beck_eval::STACK_BYTES)` fails the *build*, not the test run, because both sides are constants.

**One defect fell out of it that nothing else could have found.** The macro expander had a single
`MAX_DEPTH` counting structural descent and re-expansion together, so a 65-level-deep expression
*containing no macros at all* reported that "macro expansion did not terminate". It was invisible
while the parser accepted arbitrary depth: nothing reached the expander with a deep tree, and a
reader who saw that message had usually written a runaway macro. The two counters are now separate,
and B0201 keeps its meaning.

## 44.3 The clock, and what is deliberately not on the seam

[`14`](14-review-findings.md) F11's status was `FIXED (constraint recorded)`, which is all it ever
claimed; the runtime then called `SystemTime::now()` directly for three phases. `beck_core::clock`
is the cheap half of the fix and nothing more: a `Clock` trait, a `SystemClock` that is now the
only reader of the host's clock anywhere in the workspace, and a `ManualClock` a caller sets. No
scheduler, no virtual time, no ordering of events against each other — §42.4's verdict was "adopt
the injected clock now; **watch** DST proper", and this is the first of those and not the second.

Three of the four readings take their clock as a parameter. The sequencer takes it from
`AppConfig`, which is where it belongs: §3.7 says the merge point is the one place time enters, and
it now enters from a value somebody handed in — which is what makes replaying an envelope reproduce
the run rather than re-read the clock. The evaluator's `now()` and the milliseconds inside a
time-ordered id go through the process clock. Telemetry does too, and that one is genuinely a
global: a metric's timestamp belongs to no application and no evaluation, and threading a clock to
it would mean threading one through every counter.

**`Instant::now()` survives, and the module says so.** Elapsed time — `beck bench`, the append and
render histograms — is not on the seam. A duration measured for a metric does not enter the log,
does not reach a fold, and cannot change what a replay produces. It will have to move when DST
proper arrives. Saying that is cheaper than implying a coverage this does not have.

The gate is a count and not a list of blessed files:

```
$ cargo test -p beck-cli --test clock
the_host_clock_is_read_in_exactly_one_place ... ok
the_seam_is_the_only_file_that_names_the_standard_librarys_clock ... ok
an_envelopes_instant_is_the_clock_the_app_was_given ... ok
```

Writing it found one thing worth recording. `SystemClock` first read the clock twice — once for
milliseconds, once for nanoseconds — which satisfies "one place" while being exactly the habit the
seam exists to end. Milliseconds are now derived from the one reading.

## 44.4 The two prose items, which were the cheapest and had been outstanding longest

[`43`](43-threat-model.md) is the document §42.10 named as missing first: four adversaries, what
each is assumed able to do, what is defended and by what *kind* of evidence, and — the section that
does the work — what is explicitly not defended. `SECURITY.md` is the ISO/IEC 29147 and 30111
policy, and it carries the memory-safety roadmap paragraph in CISA's terms: a property this
workspace has had since its first commit (`unsafe_code = "forbid"`, inherited by all nine crates)
and had never *stated* in the form an external reader expects.

Both are short on purpose. A threat model that enumerates attacks goes stale; one that names
adversaries and their assumed capabilities turns a new attack into a question with an answer.

**`pending_security.rs` is what keeps §43.4 true.** Eight tests, each asserting that a control does
*not* exist — the actor is self-asserted, no per-actor quota (F3), no subscription or connection
quota (F15), no bounded deploy buffer (F12), no origin check, no message limit of this project's
choosing, and macro expansion bounded in depth but not in work (F17). Each failure message names
the documents to correct when it goes red. It is `sicp/refusals/`'s pattern applied to security
debt, and it is worth more than the prose it protects: §43.4 is exactly the kind of list that is
accurate the day it is written and quietly wrong six months later.

One item in §42.6 was smaller than its paragraph and was simply fixed: the operator dashboard's
escaper handled `&<>` only while half its interpolations were into attributes.

## 44.5 The identifier profile, and the half it does not reach

[`35`](35-standards-landscape.md) §35.5 item 2 asked for a pinned Unicode version and "UTS #39's
security profile with conformance vectors". The pin is one line, and the reason is worth stating:
Beck's identifiers are `[A-Za-z_][A-Za-z0-9_]*` and always have been, which is UTS #39's **strictest
restriction level, ASCII-Only** — so the two attacks the report is mostly about, confusables and
mixed-script identifiers, are *unrepresentable* rather than checked. §12.7's vocabulary for that is
"unrepresentable by construction, with the test proving it", and `identifiers.rs` is the test. The
compiler needs no Unicode tables to do it, so `UNICODE = "17.0"` is a statement of which version
the rules were written against rather than a dependency — and the day Beck accepts a non-ASCII
identifier, that constant stops being a note.

What ASCII identifiers do **not** close is UTS #39 §4's other half. **Bidirectional confusion** —
Trojan Source, CVE-2021-42574 — works through comments and string literals, where no restriction on
identifiers reaches it, and it makes a file render in an editor differently from how it compiles.
Both surfaces now refuse the twelve bidi formatting characters, and `\u{...}` is the escape a
program uses when it wants one as a *value*: spelled out, which is the difference between a value
and a disguise.

The vectors are grouped by the attack each defeats, because that is the only way to tell a
conformance suite from a list of strings — and one group is asserted as deliberately *allowed*:
U+200D is how an emoji sequence is spelled, and a rule with no attack behind it is a rule somebody
is eventually forced to work around.

## 44.6 The two syntax decisions, which did not resolve the way they were posed

[`09`](09-risks-and-open-questions.md) §9.6 item 5 had held these since the design documents were
written, with its own deadline attached: "cheap now, expensive after Phase 3". They are
[`10`](10-decisions.md) D21 and D22.

The first **splits**, and that is the finding. It was posed as one question — signature clauses or
decorators — and four phases of implementation had already answered it as two, without anybody
noticing that the answer was a distinction rather than a winner. **An effect is a clause in the
signature** because an effect row is part of the *type*: it unifies, it is inferred, it is
generalised over ([`27`](27-the-walls-come-down-report.md)), it is a bound an
impl is held to ([`27`](27-the-walls-come-down-report.md)), and §3.6 publishes it. **A placement is a decorator**
because Phase 2 made placement *inferred*, so `@on(...)` is an override handed to the solver rather
than a fact about the definition.

The measurement is the argument, and it took one command: of 28 single-file corpus programs,
**one** carries `@on(...)` — and that one exists to test that pinning still works. Ten carry a
`uses` clause. An annotation almost nobody writes should not occupy space in the signature
everybody reads.

D22 keeps `ui:` as a block macro and states what that forfeits — editor tooling for a bespoke
literal syntax, which is the LSP's problem and not the grammar's.

## 44.7 What this wave did not do

- **It built no security control.** Everything in §44.4 is a *statement* of the posture, and the
  posture is unchanged: no identity, no quotas, no limits. The suite that says so is the deliverable.
- **DST is not begun.** The clock is a seam. `Instant` is not on it, the network and the disk are
  not on it, and F11's constraint named all three.
- **The recursion bound bounds depth, not breadth.** A file with a million top-level items is still
  a file with a million top-level items, and macro expansion is still unbounded in *work* (F17).
- **No grammar-aware fuzzing.** §42.9 pinned it with the trigger "the bound lands". The bound has
  landed, so that trigger has fired and the item is now due — [`08`](08-roadmap.md) §8.5.4 Wave 5.
- **The `pending_security` suite is not a threat assessment.** It records what is absent; whether
  the absences matter is [`43`](43-threat-model.md)'s question, and that document is prose, not
  evidence.
