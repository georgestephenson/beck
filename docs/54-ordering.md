# 54 — Ordering: what `Ord` would be, and what it would cost

**Options, not a decision.** [`50`](50-collections-and-dates-report.md) §50.5 ended by naming `Ord`
as a trait — "the way `Num` is one, so a type says what its own order is instead of inheriting the
one its representation happens to have" — and said explicitly that the paragraph was not a proposal
but a place for the next person to start. This is that person's write-up: what the order actually
is today, everything that depends on it, four options with their costs, and a recommendation that is
**not** the one §50.5's sentence points at.

Nothing here is built and nothing here is settled. It exists so the decision is taken deliberately
rather than under the time pressure of the change that needs it — which is
[`39`](39-bounds-report.md) §39.7's precedent, and that decision came out better for having been
written down first.

## 54.1 What the order is today

`Value` derives `Ord` (`core.rs`). That gives, in order: variant order across the enum, then the
structural comparison of the payload. Two consequences are worth having in front of you.

**A real is stored as an order-preserving key rather than as `f64::to_bits`**, so the derived order
*is* the numeric one — `docs/32` §32.2, which found that `to_bits` made `<` answer backwards for
every negative number. That is the precedent this whole document is about: the project has met this
question once already and answered it "**one order, and make it the right one**" rather than "two
orders, and let the program pick".

**A record's fields are a `BTreeMap<Arc<str>, Value>`, so a record orders by field _name_.**
`Key(score=…, name=…)` sorts by `name`, because `n` precedes `s`. That is
[`50`](50-collections-and-dates-report.md) §50.5's finding, and it is pinned by
`stdlib.rs::a_record_orders_by_field_name_and_not_by_declaration_order`.

## 54.2 Everything that depends on it

This is the part §50.5 could only gesture at, and it is what decides the question.

| | Where | What it means |
|---|---|---|
| `<`, `<=`, `>`, `>=` | `interp.rs` — `a.cmp(&b)`, for **any** two values of the same type | the user-visible one |
| `sort_by` | sorts by the key's value order | the user-visible one |
| `Map` keys | `PMap<Value, Value>` is a persistent `BTreeMap` | iteration order, `map_keys`, and `collections.beck`'s `elements` |
| The incremental engine's arrangements | keyed by `Value` | which deltas collate with which |
| **The state digest** | `core.rs`'s own comment: a total order is "not optional" for it | two runs that disagree about order have different digests |
| **The patch stream** | same comment: "iteration order is part of the rendered view, and replay must reproduce the *patch stream* bit for bit" | two runs that disagree about order are different programs |

The last two rows are the constraint. The value order is not a library convenience — it is part of
what makes a replay a replay. Any option that lets two parts of a running program disagree about the
order of the same two values has to answer for the digest and for the wire, and none of the options
below is free of that question.

## 54.3 Option A — leave it

One order, everywhere, and `Key(score=…, name=…)` sorts by the name.

**For:** nothing can diverge, because there is nothing to diverge from. The digest, the patch stream,
the arrangements and `sorted` all agree by construction, and they agree today.

**Against:** the reader's intent is silently inverted in the one case §50.5 found, and the workaround
— name the fields so that alphabetical order is the intended order — is the kind of advice that is
correct and embarrassing.

**Cost to adopt:** none. It is what is there.

## 54.4 Option B — `Ord` as a trait, the `Num` shape

A prelude `trait Ord` with a comparison method; `<` resolves through it when the operand type has an
`impl`, exactly as `+` resolves through `Num` ([`41`](41-generic-arithmetic-report.md)). All the
machinery exists: [`39`](39-bounds-report.md)'s bounds, [`47`](47-effect-polymorphic-traits-report.md)'s
per-impl rows, [`40`](40-traits-across-modules-report.md)'s module crossing.

This is the option §50.5's closing sentence points at, and it is the one to **reject**, for a reason
that only becomes visible once §54.2's table is written out.

**What it buys:** a type may override its own order. That is the whole list. It is worth being exact
about the things it does *not* buy, because they are the things people assume a `Ord` trait is for:

- *Generic code that compares.* Already possible — `<` is `(a, a) -> Bool` for every `a`, so
  `def largest[T](xs: list[T])` compiles today with no bound at all. A bound would be ceremony.
- *Sorting by something other than the natural order.* Already possible — `sort_by` takes a **key**,
  which [`50`](50-collections-and-dates-report.md) chose over a comparator on purpose.

**What it costs:** the type now has **two** orders. The runtime holds no dictionary, so `Map` keys,
arrangements, the digest and the patch stream keep the representation order while `<` and `sorted`
use the impl. `sorted(xs)` stops agreeing with `elements(set_of(xs))` over the same records — which
§50.5 called "exactly the bug a library that promises determinism must not have", about a *different*
mechanism, and the objection transfers intact.

So the one thing it buys is the one thing that costs. That is not a trade-off to weigh; it is the
same fact stated twice.

## 54.5 Option C — Option B, and the containers carry the dictionary

The only way to make a user-defined order total: `Map[K, V]` requires `K: Ord`, every map operation
takes `Ord::cmp@K`, and `PMap` is parameterised by a comparator.

**For:** one order again, and it is the program's.

**Against, and this is the load-bearing part:** the comparator becomes a **runtime** value, and
§54.2's last two rows are about a *stored* artefact. A digest computed under one comparator and a
replay under another are a silent divergence of exactly the kind [`04`](04-compiler-architecture.md)
§4.8's replay determinism suite exists to catch — so the comparator has to become part of the
program's identity, which means `--wire-compat` has to have an opinion about a change to an `impl
Ord` body. That is a real design question and not a hard one, but it is a *deploy-time* question,
which puts this option in a different weight class from the other three.

**Cost to adopt:** large, and it touches the engine, the arrangements, the digest and the wire — the
four things this project is most careful with. Worth doing only if something needs user-defined
order in the *database*, which nothing does yet.

## 54.6 Option D — a record's value carries its declaration order

Fix the finding rather than the abstraction. `Data.fields` is an
`Arc<BTreeMap<Arc<str>, Value>>`; if it were an ordered sequence in **declaration** order, then
`Key(score=…, name=…)` compares by `score` first — in `<`, in `sorted`, as a `Map` key, in the
arrangements, in the digest and in the patch stream, because there is still exactly **one** order.

§50.5 dismissed this in one sentence — "a value carries its fields and not its declaration" — and
that sentence is true of the representation as it stands rather than of representations in general.
The checker knows the declaration at every `Make`, so it can emit fields in declaration order and
the runtime can preserve it; `With` updates in place and preserves it too.

**For:** the intuitive answer, one order, and no trait. It is the same answer
[`32`](32-numeric-tower-and-polymorphism-report.md) §32.2 gave for reals — change the representation
so the one order is the right one — which is the strongest precedent in the tree.

**Against, and unverified:** three things would have to be established rather than assumed, and this
document has established none of them.

1. **Field lookup becomes a scan.** `CoreKind::Field` is a `BTreeMap` lookup today. Records are
   small and this is almost certainly noise, but "almost certainly" is not a measurement.
2. **The digest and the wire encoding of every record change.** That is a `--wire-compat` breaking
   change for every published `model`, and it is a one-time cost paid by everyone.
3. **A union variant and a record must agree.** `Data` carries both, and only three places in the
   tree pattern-match its fields — which suggests the change is small, and a count of match sites is
   not a proof that it is.

**Cost to adopt:** medium, and concentrated in one type.

## 54.7 The recommendation

**Take D if the finding is worth fixing; otherwise take A. Do not take B.**

B is the option the earlier report's closing sentence points at, and writing §54.2 out is what
changed the answer: `Ord` as a trait is a mechanism for letting one type have two orders, in a system
where the order is part of the replay. The feature that sounds like the general solution is the one
that breaks the invariant, and the two options that keep the invariant do not need a trait at all.

C stays on the shelf with a named trigger: **a program that needs a user-defined order inside the
database** — a `Map` whose iteration order is domain order rather than value order. Nothing asks for
that today. If something does, C is the answer and B is still not.

What would change this: a use for `Ord` as a **bound** that is not about sorting — a trait with a
default method body, say, where `Ord` is a supertrait — since that is a use of the trait that does
not put a second order into a running program. Beck has neither supertraits nor default bodies, so
that is not a live argument, only an honest one.

## 54.8 What this is not

- **Not a decision.** Nothing here is in [`10`](10-decisions.md), nothing is scheduled in
  [`08`](08-roadmap.md), and no code changed.
- **Not a measurement.** §54.6's three "against" items are the ones that need evidence, and none of
  them has any. In particular there is no number for the field-lookup change and no count of what a
  wire-format change to records would break.
- **Not a survey of the literature.** [`38`](38-literature-survey.md) is where that would go, and
  ordering as a type class is well-trodden ground that says little about a system where the order is
  also a wire format.
