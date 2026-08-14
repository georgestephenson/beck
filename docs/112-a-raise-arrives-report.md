# 112 — A raise arrives, and a handler catches it

**Built.** `raise` and `try:` compile, to **both** code generators. A failure the program's own type
declares — [`27`](27-the-walls-come-down-report.md)'s Wave 1, an error as a row label with `Result`
as its reified form — now happens in compiled code, travels out of it, and is caught in it.

[`08`](08-roadmap.md) §8.5.5's Lane E row reads "the effects and growing a collection" after
[`111`](111-a-view-arrives-as-a-recipe-report.md). This is the first piece of the effects, and it is
the piece that needed no callback to the host at all: **a raise is not a call out, it is a way of
returning**. The mechanism it wanted was already there — every compiled function takes an error cell,
stores into it and returns, and every caller checks it and returns in turn
([`93`](93-llvm-backend-report.md)) — so what this change adds is a fourteenth code, two words of
arena, and a **handler**: a label the checks branch to instead of the function's exit.

**Across the tree, 688 → 711 definitions compile and refusals go 730 → 707.** Of the **38 refusals
that blamed `raise`**, **18 now compile** and **20 are re-refused for a deeper reason** — almost all
of them a callee that still does not compile. `raise` no longer appears in a refusal anywhere in the
tree.

§112.6 is the honest column and for once it is not a caution: a raise caught 3,000 frames up is
**17.0× the tree-walker**, against **20.0×** for the same recursion that does not fail — so the
failure costs about a sixth more than the frames it unwinds, and **nothing per frame** (§112.7's
clockless gate says so at two depths).

§112.8 is the finding, and it is about the *protocol* rather than the feature: clearing the trap code
is not clearing the cell.

---

## 112.1 The mechanism was already there

[`93`](93-llvm-backend-report.md) §93.2 gave every compiled function a first parameter — a pointer to
a 24-byte cell holding a code, a span index and a payload — because the host is a different process
and a `SIGFPE` would tell it nothing about which span was at fault. A computation that cannot produce
a value stores into the cell and returns; its caller checks and returns in turn.

That is an unwinder. It was built for faults — an overflow, an exhausted arena, a `match` that a
wrong exhaustiveness check let through — and a raise is the same motion with three differences:

| | a fault | a raise |
|---|---|---|
| what it means | the machine could not | the program said it might |
| what it carries | a number, for a message | a **value**, of a declared type |
| who may stop it | nobody | a `try:` naming that type |

So the work is those three rows. A fourteenth code (`Trap::Raised`), a pair of words in the arena for
the value, and a handler that a check can branch to instead of the exit.

## 112.2 What a raise carries, and the one failure whose arena travels

A raise allocates two words — the raised value's **shape** and its **word** — which is
[`111`](111-a-view-arrives-as-a-recipe-report.md)'s deferred value one subsystem over, and for the
same reason: the signature says nothing about what was raised, so the reply cannot be decoded without
being told. The cell's payload is that pair's offset.

The type **name** goes in the cell's third word, as the offset of that name in the module's literal
pool. Two things about that are worth stating rather than leaving:

- It is a **name** and not the shape, because two instantiations of one generic type are two layouts
  and one name, and the name is what the language compares: the atom the checker performs is
  `raises(T)` and `try:` is given `T` as a string. Comparing shapes would catch `Tree[Int]` and let
  `Tree[Str]` past, which is a divergence on a program nobody would write and a wrong answer all the
  same.
- It costs no table. The literal pool already interns strings and gives equal strings one offset, so
  the comparison is `icmp eq` on two constants and the "name id" is a byte offset that already
  existed.

Then the protocol change, and it is one line in each emitter's worker loop: **the arena travels with
a raise**. Every other failure sends nothing back — a trap's answer is its message — and this one
sends the used arena, because the host has to decode the value to *make* the message.
`beck-eval`'s `EvalError::raise` renders `raised \`TooBig{n: 101}\``, and a compiled program that said
"something was raised" would be a divergence the differential shows.

## 112.3 The handler is a label, and two decisions hold it up

`try:` is a form, not a function — [`38`](38-literature-survey.md) §38.4's lexical handler, with no
dynamic search for who handles what. Compiled, it is a label:

```
  ; the block, emitted with the handler pushed
  %v = <block>
  %ok = Ok(value = %v)
  br label %try.join
try.handler:
  %code = load i32, ptr %err
  br i1 (%code == RAISED), label %try.named, label %<outer>
try.named:
  %got = load i64, ptr %err+16          ; the raised type's name
  br i1 (%got == <this try's name>), label %try.caught, label %<outer>
try.caught:
  store i64 0, ptr %err                 ; handled
  %held = <the pair's second word, read as E>
  %bad = Err(error = %held)
  br label %try.join
```

Every trap and every call-check inside the block branches to `%try.handler` rather than to the
function's exit, because `escape()` answers the innermost handler and *all three* sites that leave a
block go through it. A handler only some of them honoured would catch a raise and miss an overflow.

Two decisions are load-bearing and neither is a style:

**The block is emitted inline.** The checker wraps a `try:`'s body in a `lam` of no parameters so the
evaluator can delay it. Here there is nothing to delay, and inlining is what puts the block's own
calls under the handler — a `beck.lam.N` applied through the closure machinery would check the cell
inside *its* frame and leave through its own exit, and the handler would never see it.

**It is emitted for a value.** A call in tail position is a `musttail` that deliberately does *not*
check the error cell — there is no frame left to check in — which is correct at the top of a function
and walks straight through a handler. `Dest::Value` is what guarantees no such call is emitted inside
a protected block, and `caught` in the fixture is written in tail position precisely so that a
regression there is a failing test rather than a subtlety.

## 112.4 What it must not catch

The evaluator's `Prim::Try` catches one type and lets everything else travel: *a fault is not a
failure, and a different error type belongs to a handler further out*. Both halves are what the two
tests in the handler are, and both are in the differential as programs written to make them bite:

- `overflows` multiplies by `i64::MAX` inside a `try:`. The code is `MulOverflow`, not `Raised`, so
  the first test forwards it — and a handler that caught by *code* would answer an `Err` where the
  evaluator fails.
- `wrong_type` calls something that raises `Other` inside a `try:` for `Bad`. The code is `Raised`
  and the name is not, so the second test forwards it.
- `nested` puts one handler inside another, catching two different types, so the handler stack has to
  be a stack.

Forwarding means branching to the *enclosing* handler with the cell **untouched**, which is what
makes the outer one see the same failure the inner one declined.

## 112.5 What compiles now

| | before | after |
|---|---|---|
| definitions compiled across the tree | 688 | **711** |
| definitions refused | 730 | **707** |
| refusals blaming `raise` | 38 | **0** |

Counted the way [`111`](111-a-view-arrives-as-a-recipe-report.md) §109.5 counts: the totals of
`beck native <file>`'s own two headline lines over `corpus/ awfy/ clbg/ sicp/ examples/ lib/`, at the
commit before this change and at this one.

Of the 38, **18 compile** and **20 are refused for a deeper reason** — sixteen of those for a callee
that still does not compile (`lib/bignum.beck`'s `divide` needs `divmod_limbs`, which grows a list),
two for a map that grows, and two for a callee's callee. That ratio is the same shape
[`108`](108-closures-arrive-report.md) §108's was, and it says the same thing: in a library, a
refusal is usually inherited, so removing a cause moves more than the definitions that named it.
`corpus/29-fallible.beck` — the program written to be about this feature — compiles `check_budget`,
which is a `raise` at the point the decision is made, and refuses the other two for a reason that has
nothing to do with failure: `parse_amount` calls `str_trim`, and `validate` calls `parse_amount`. So
the program whose subject is this feature is also an example of §112.5's ratio, and of what is
actually holding the tree back.

## 112.6 What it costs

`measure_native.rs::what_a_raise_costs_against_the_tree_walker`, release build. `deeply` raises at the
bottom of `n` frames and is deliberately **not** tail-recursive, so every frame on the way out reads
the cell and returns; `down` is the same recursion without a failure, as the control:

| benchmark | frames | evaluator | native | ratio |
|---|---|---|---|---|
| `caught` — raised, unwound, caught | 500 | 189.0 µs | 46.2 µs | **4.09×** |
| | 3,000 | 1.576 ms | 92.7 µs | **17.01×** |
| `down` — the same frames, no failure | 500 | 189.0 µs | 41.5 µs | **4.56×** |
| | 3,000 | 1.296 ms | 64.7 µs | **20.03×** |

The number to read is the pair, not either row: **failing costs about a sixth more than not failing**,
at both depths, on both implementations. The sizes are six times apart rather than eight because the
*evaluator* is what bounds the larger one —
[`adr/0007`](adr/0007-evaluator-stack-is-declared-not-discovered.md)'s declared nesting ceiling is
4,000 and this recursion is not in tail position, which is itself a difference worth knowing: the
compiled side has no such ceiling and the tree-walker does.

## 112.7 The gates

- **`native.rs::the_two_backends_agree_on_failure`** and
  **`cranelift.rs::the_three_backends_agree_on_failure`** — 84 calls each, over programs written to
  make each rule bite: a raise nothing catches, one a `try:` catches, a variant with no fields, a
  raise carrying text and one carrying a list, a fault inside a `try:`, a *different* error type
  inside one, nested handlers, a raise inside a generated loop, and a `try:` in tail position.
- **`native.rs::an_uncaught_raise_names_the_value_it_carried`** — the message, asserted as a string
  rather than only differentially, because a regression to "the compiled program failed" would still
  agree with itself.
- **`native.rs::unwinding_costs_nothing_per_frame`** — a clockless shape gate: a raise caught 25
  frames up and one caught 200 frames up leave the **same 168 bytes** of arena. A scheme that
  allocated per frame on the way out — a trace, a boxed error per level, a copy at each check — would
  be linear in the depth and would still answer correctly at every size a test would run.

One thing the gates found rather than asserted: **a raise inside `map_list`'s generated loop already
worked**, on both backends, with nothing written for it. A loop applies a closure and checks the cell
after it, and a raise is a code in that cell — so "leave the loop rather than run the next element"
was true the moment a raise could happen at all. That is the argument for reusing the fault path
rather than building a second one, cashed.

## 112.8 The finding: clearing the code is not clearing the cell

The first end-to-end call of a caught raise came back as *"the compiled program answered with offset
64, and its heap is 0 bytes"* — a decode failure, on a call that had answered correctly.

The handler cleared the trap code with `store i32 0`. The cell's first eight bytes are a `u32` code
**and** a `u32` span, and the worker's loop reads those eight bytes as one `i64` to decide whether the
call answered. So a caught failure came back with the raise's span sitting in the high half of a word
the protocol compares against zero: not an answer, not a raise, and therefore an arena that was never
sent.

Everything *inside* the function was right, and every later read of the code was right. What was wrong
is that two pieces of the same program disagree about what the cell **is** — a code and a span to the
emitter, one word to the loop — and "cleared" is a different act under the two readings. The fix is
one character (`i32` → `i64`) and the reason is worth more than the fix:
[`107`](107-a-map-arrives-read-only-report.md) §107.5 made the *layout* of a value one decision so two
emitters could not disagree about it, and this is the same class of defect one level down, in the one
piece of shared shape that is written as three constants rather than as a type.

Both emitters now clear the whole word, and both say why in a comment beside it.

## 112.9 What this does not establish

- ~~**Nothing about the other effects.**~~ **Built** ([`116`](116-the-host-answers-back-report.md)):
  the protocol grew the second direction this bullet forecast, and `now()`, `uuid()`, `secret_env`
  and `http_fetch` compile by asking across it. The forecast held — a raise needed none of it, which
  is why it was worth taking first.
- **Nothing about a raise crossing into the evaluator.** `ExecError` carries a message and a span and
  not a raised value, so a failure that leaves a compiled call cannot be caught by a `try:` in an
  interpreted caller. It does not arise today — a compiled definition only calls compiled definitions
  — and it is what would have to change first if execution ever mixed the two mid-expression.
- **Nothing about `raises` in a row that a *`.becki`* publishes**, or about anything else the effect
  system does with the atom. This is the run-time half only; the checker was not touched.
- **Nothing about a raised value the host cannot decode.** A raise of a type with no layout is
  refused at the raise site, by name, with the reason.
- **Nothing about `list_flat_map`, growing a collection, or the rest of Lane E.**

## 112.10 What this corrects

- [`108`](108-closures-arrive-report.md) §108.9's "nothing about the effects, which are the other
  unbuilt row" and [`111`](111-a-view-arrives-as-a-recipe-report.md) §109.8's "nothing about the
  effects" both stand as history; this is the first row of that item, and the roadmap's Lane E cell
  now says which part is left.
- **No refusal in the tree says "`raise` is not one of the scalar primitives"** any more. That
  sentence came from `refusal`'s catch-all rather than from an arm of its own, which is why it read
  as a statement about a category instead of about the thing that was missing — the pattern
  [`106`](106-lists-arrive-read-only-report.md) §106.7's gate exists for, met from a third direction:
  a refusal inherited from a *default* is a claim nobody wrote.
- `beck_llvm`'s module documentation said "every **effect** is still the tree-walker's". A raise is a
  row label rather than an effect atom in the sense that list means, and the sentence now says so.
