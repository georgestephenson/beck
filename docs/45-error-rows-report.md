# 45 — Phase 3 report, part 15: failure as a row label

> **What this is**: [`08`](08-roadmap.md) §8.5.4's Wave 1, built — errors as a row label with
> `Result` reified, and row aliases. It is the head of the language queue that
> [`39`](39-bounds-report.md) vacated, and it is what §8.5.3's trap 2 says the standard library
> must not be written before.

## 45.1 The claim, and what was adopted rather than invented

[`38`](38-literature-survey.md) §38.4 is unusually prescriptive for a survey, and this report is
mostly it, built: **do not add mechanisms — add row labels and handlers.** Koka's `exn` is the
model. An error is a row label; a signature without it provably cannot fail; a handler converts the
row entry into a value, and `Result` is that *reified* form rather than a parallel channel.

Concretely:

```
union Refusal:
    NotANumber
    OverBudget(by: Int)

row Fallible = raises(Refusal)

def parse_amount(text: Str) -> Int uses Fallible:
    match str_to_int(str_trim(text)):
        case None:
            raise NotANumber
        case Some(value):
            return value

def validate(s: State, p: Proposal) -> Result[list[Event], Refusal]:
    return try:
        match p.command:
            case Spend(amount):
                [Spent(amount=check_budget(s, parse_amount(amount)))]
```

`raise e` performs `raises(E)` — an ordinary atom in the same row `durable` and `net.out` live in.
`try: block` is the handler: it discharges exactly that label and yields `Result[T, E]`.

The whole argument for the shape is that **nothing else had to be built**. Failure inherits the
effect system's existing discipline, in full and without a special case:

| Property | Was already true of every atom | Is now true of failure |
|---|---|---|
| Inferred | a caller of a `durable` function performs `durable` | a caller of a fallible function is fallible, and nobody wrote it down |
| Bounded by `uses` | B0370: "a library that starts phoning home cannot do so silently" | a signature that does not declare failure may not fail |
| Published | `.becki` carries the row | `uses raises(Refusal)` is in the published contract |
| Wire-compatible | effect widening is breaking (§4.3) | a function that starts being able to fail is a breaking change |
| Placed | the row decides the tier | `raises` is discharged everywhere, so it decides nothing |

The second row is the one that matters and the one a `Result`-returning convention cannot have: a
caller may ignore a returned value, and cannot ignore an effect.

Measured, on the module in §45.1:

```
$ beck check --wire-compat old.becki
  BREAKING   pure_caller: effects widened: +{raises(FormError)}
      a library that starts phoning home cannot do so silently — this is the supply-chain
      property, and it is only worth having if it fails the build
```

That sentence was written for `net.out`. Nothing was added to make it true of failure.

## 45.2 The handler is a form, and it catches what it can name

Two decisions inside `try:` are worth stating, because both could have gone the easy way.

**It is lexically scoped by construction.** The handler is a syntactic form, not a dynamic search
for whoever most recently installed one. Zhang & Myers (POPL 2019) give the general argument —
dynamically scoped handler search breaks abstraction — and §38.4 gives Beck's specific one, which
is sharper: *in a language where effects decide placement, an accidentally intercepted effect would
be an accidental re-placement.* A `try:` cannot intercept a failure raised by a caller's function
value, because a `try:` is a piece of syntax with a body and that body is all it covers.

**It catches the type it names, and lets anything else keep travelling.** The primitive takes the
error type as an argument, which looks redundant — the checker has just proved which label the
block carries. It is not: a block's row may carry a row *variable* standing for a caller's effects,
and that variable can hide a failure this handler has no type for. Catching it would put a value of
an unrelated type inside a `Result[T, E]`. So a raise unwinds carrying its type name, and a handler
compares.

The consequence is a diagnostic rather than a silent widening when a block can fail two ways
(`B0393`): a `Result` has one error type, and the answer is a union over the failures, named. The
diagnostic says both of them.

## 45.3 Row aliases, taken before they hurt

§38.4 records the ergonomic warning from Koka's own community — five- and six-label rows are
ordinary — and says row aliases "belong in the design from day one". So `row Fallible =
raises(Refusal), log` is a top-level declaration, expanded wherever it is used.

Three small decisions, each a refusal:

- **An alias cannot shadow an atom.** The atom is tried first, so `row durable = log` does not
  silently change what every signature in the module means.
- **An alias is module-local and does not cross a `.becki`.** A published contract renders the
  expanded atoms, because a contract that refers to a name the reader has to look up elsewhere is
  not a contract.
- **No forward references, no cycles.** A row is a set being built, not a declaration being
  resolved later, and this is the one place that differs from types — which may mention anything in
  any order ([`27`](27-walls-report.md) §27.3).

## 45.4 What it cost, and the bug it exposed

The IR did not change. `raise` and `try` are **primitives**, not `Core` nodes, so not one line of
the splitter, the placement solver, the plan, the incremental engine or the runtime knew this
feature was happening — the third feature running for which that is true
([`37`](37-traits-report.md) §37.8, [`39`](39-bounds-report.md)). The evaluator gained one field on
its error type, distinguishing a failure a program *chose* from a fault, because both unwind the
same way and everything between the raise and its handler has to pass both along unchanged.

The printer did change, and that is the harness working: `parse(print(parse(src))) == parse(src)`
is asserted over the corpus, and three new forms meant three new cases. The failure was immediate
and specific.

**One latent defect fell out.** Getting the tier table wrong first made every fallible definition
unplaceable, which is what led to reading the `@on(any)` branch of placement verification. It
reported the first *visible* effect as undischargeable rather than the first effect the tier cannot
discharge — so it was already wrong for `partial`, and had been since Phase 2. Nothing had hit it
because no corpus program declared `partial` without also doing something that forced a tier.

The right answer for `raises`, stated so it does not get re-litigated: **failing is control flow,
not a resource.** Every tier discharges it, so it moves nothing, and it does not break replay — a
raise is deterministic in its input, which is the same reasoning `partial` already carried.

## 45.5 The corpus program, which is the honest measure

`corpus/29-fallible.beck` is the whole-program version, and comparing it with the other 28 is the
argument in one file. Every other program builds its `Result` by hand: an `Err(...)` written out at
each refusal and threaded back through every helper that might refuse, so the plumbing is in the
return type of everything on the path whether or not that function is where the decision is.

Here `parse_amount` and `check_budget` say `raises(Refusal)` and `raise` where they decide;
`validate` writes `try:` once, at the boundary where the runtime wants a `Result`. Nothing in
between mentions failure. `beck iface` is where the property shows:

```
@on(any)
def parse_amount(text: Str) -> Int uses raises(Refusal)
@on(any)
def check_budget(s: State, amount: Int) -> Int uses raises(Refusal)
```

— and `validate` publishes no row at all, because the handler discharged it.

It places itself with no annotations, like the other 28, and its six `test` blocks run under
`beck test`, including the two that assert *which* refusal a bad input produces.

`str_to_int` was added to the prelude for it. A parse is the canonical fallible operation in every
language, and the program needed one thing that can genuinely fail on its input. It is one
primitive, not a standard library.

## 45.6 What is **not** built

- **Structured concurrency.** §38.4 treats it as the same piece of work — `spawn`/`await` as effect
  operations with the scope as their handler — and [`08`](08-roadmap.md) §8.5.4 lists it inside
  Wave 1. It is not here. What is here is the half that gates the standard library; the scope-as-
  handler half gates nothing yet and would have meant designing a concurrency model in the same
  change as an error model.
- **General algebraic effect handlers.** `try:` is a handler for one label with one behaviour
  (reify and return). There is no `handle … with`, no resumption, no user-defined effect. §38.4's
  adopt verdict is for the *shape*, and the shape is now demonstrated once.
- **Propagation sugar.** There is no `?`. A fallible call is an ordinary call, which is the point —
  but a caller that wants to *convert* one error type into another writes a `try:` and a `match`,
  and that is more ceremony than the equivalent in a language with `?`.
- **`raises` in a trait's declared row across an impl** is untested. It should work — the bound is
  the ordinary row check — but nothing asserts it, and "should work" is not a claim this project
  makes.
- **Nothing about panics.** §38.4's adopted position is "typed channel cheap and default, panic
  outside the row as a genuinely unrecoverable trap", and `partial` is still what a Beck program
  says when it may abort. The two are separate atoms and always were; making the second rarer is
  not part of this.
- **No measurement.** Nothing here has a number. `Result` propagation being near-zero-cost is a
  result from the Rust literature (§38.4), not from this evaluator, and the tree-walker is 33×
  CPython on `fib(30)` anyway ([`25`](25-benchmarks-and-expressiveness.md) §25.3) — a cost claim
  from it would measure the wrong thing.
