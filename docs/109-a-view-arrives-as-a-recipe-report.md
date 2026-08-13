# 109 — A view arrives, as a recipe

**Built.** A definition that returns `Html` compiles, to **both** code generators.
[`08`](08-roadmap.md) §8.5.5's Lane E row read "the effects, `Html` and growing a collection" after
[`108`](108-closures-arrive-report.md); this is the second of those three, and it is the one
[`105`](105-text-on-the-heap-report.md) §105.10 corrected the classification of — `Html` is a tree of
children, so it follows the **collection** row rather than the text one.

It does not follow it in the way that sentence predicted. The thing a compiled `view` puts in the
arena is **not the tree**: it is the *call* `html_el(tag, attrs, children)` would have been given,
and the host builds the tree out of it on the way back — with
[`beck_core::html::element`](../compiler/crates/beck-core/src/html.rs), which is the same function
the evaluator has always built one with. §109.1 is the argument for that, and it is short: a page's
leaves are **renderings** — `html_text(x)` is `x` displayed, an attribute's value is displayed, and a
handler's command is *JSON* — so a compiled `view` that built the tree would need `Value::display`
and `Value::to_json` generated per repr, in two emitters, agreeing with the host's about every shape
a value has. Deferring them costs two words and no generated code at all.

**Across the tree, 650 → 688 definitions compile and refusals go 768 → 730.** Of the **42 refusals
that blamed a view**, **38 now compile** and **4 are re-refused for a deeper reason that had always
been true of them**. **Twenty-one of the thirty-two corpus programs have a `view` that compiles**,
including `examples/todo.beck`'s, which is the `ui:` block a reader of
[`00`](00-original-idea.md) meets first.

**What it is not is faster** (§109.6). A compiled page is 0.80×–1.33× the tree-walker at two sizes,
and the reason is the design rather than a constant to tune: the rendering is the host's either way
and the pipe is additional. What moved is what *compiles* — which is the number
[`101`](101-the-heap-report.md) §101.2 said matters — and the row Mode B's codegen has been behind
since [`94`](94-mode-b-report.md) §94.8.

---

## 109.1 A page's leaves are renderings, and rendering is the host's

Five primitives build a page, and the type of every one of them says what the problem is:

```
html_el(tag: Str, attrs: list[Attr], children: list[Html]) -> Html
html_text(x: A) -> Html
html_attr(name: Str, value: A) -> Attr
html_on(event: Str, command: A) -> Attr
html_key(k: A) -> Attr
```

Four of the five take an `A`. The evaluator renders it — `x.display()` for the first three, and
`cmd.to_json()` for the handler, whose command becomes the `data-b-click` attribute the thin client
posts back. So compiling them the obvious way means generating a `display` per repr and a `to_json`
per repr, in both emitters, and holding them to `beck_core`'s: a record renders as
`Point(x=1, y=2)`, a real renders as its shortest round-trip form — which
[`93`](93-llvm-backend-report.md) already refuses to generate, and which is why `str` of a `Float` is
still a refusal one directory over.

The alternative is to write down *what was asked* and let the host answer it. A node in the arena is
four words:

| Word | `Html` element | `Html` text | `Attr` plain | `Attr` handler | `Attr` key |
|---|---|---|---|---|---|
| 0 | tag `0` | tag `1` | tag `0` | tag `1` | tag `2` |
| 1 | the tag name, a `Str` | — | the name, a `Str` | the event, a `Str` | — |
| 2 | the attributes, a `list[Attr]` | the **shape** of the value | the shape of the value | the shape of the command | the shape of the key |
| 3 | the children, a `list[Html]` | the value | the value | the command | the key |

Rows 2 and 3 are the whole feature. A **deferred value** is a pair: the value's word, and the index
of its repr in the module's word table. That index is the only place in this backend where a repr is
a **datum** rather than a fact fixed when the module was emitted, and it is what lets `html_text(x)`
compile for every `x` that has a shape at all — an `Int`, a `Float` the emitters cannot render, a
record, a list, a map. The host reads the index, decodes the word with it, and calls the function it
already has.

This is [`adr/0026`](adr/0026-the-native-heap-is-an-arena-of-offsets.md)'s decision spent a third
time. That ADR's claim is that marshalling needs **no generated code**, because a value is an offset
and the arena crosses a pipe as bytes; a recipe is that claim applied to something that is not a
value at all. Neither emitter contains a string of markup, an escape, a hash or a byte of JSON.

## 109.2 What the host does with one, and why it is not a second builder

Decoding a node calls `beck_core::html::element(tag, attrs, children)`. That function did not exist
before this change: it is the body of the evaluator's `Prim::HtmlEl`, lifted out and called from
both. Three rules live in it, and each is a decision the differ downstream depends on:

- an attribute whose value is **empty is dropped** rather than emitted — Phase 0's `attr_if`, and
  what every conditional class in this repository produces on its false branch;
- a handler becomes `data-b-<event>` carrying the command as JSON;
- a **key** is not an attribute: it sets the node's key, and it folds into a different hash
  accumulator than the attributes do.

A second spelling of any of them would be a compiled page that differs from an interpreted one in a
way no type can catch and no rendering can show — two identical-looking trees whose structural hashes
differ, so the differ skips a subtree that did change ([`beck_core::html`](../compiler/crates/beck-core/src/html.rs)'s
own warning about a stale hash). Lifting the function is not a convenience; it is the reason the
differential below can be believed.

`html_text` is the same lift for the same reason
([`beck_core::html::text_of`](../compiler/crates/beck-core/src/html.rs)): a child that is *already* a
tree is **spliced** rather than rendered, which is what makes a view composable out of functions
([`94`](94-mode-b-report.md) §94.4 is what happens when it is not). The compiled `html_text` has that
arm too and answers with its argument, so a spliced child costs no node at all.

## 109.3 Going the other way, and why the round trip is exact

A compiled definition may **take** an `Html` — `def again(h: Html) -> Html` — and what the host holds
by then is a baked tree with its hashes computed. So the encoder writes the recipe back, and every
leaf of it is text, because that is what a built tree holds: a text node's rendering, an attribute's
value, a key. A handler is written as **the plain attribute it would become**, since an `On` in the
arena holds the command as a value and the host cannot name a repr for a `Value` it was handed —
which costs nothing, because `element` turns an `On` into exactly that pair anyway.

Replaying the builder over the same strings in the same order gives the same tree, hashes included:
the attributes arrive in their order, the key is written after them and sets the key rather than an
attribute, and an empty value cannot appear because it was dropped when the tree was first built.
The differential asserts it rather than the paragraph — `the_two_backends_agree_on_views` builds
twenty-one trees *on the evaluator* and hands each one to a compiled definition.

That argument holds for a **tree** and not for a bare `Attr`, and the difference is the only
directional rule on this boundary. A tree's attributes are already pairs of strings — an
`AttrValue::On` stops existing the moment `html_el` consumes it — so writing a handler back as its
plain form loses nothing. A `Value::Attr` the evaluator is still holding *is* an `On`, and encoding
it would answer a `Plain` where the evaluator answers an `On`: a divergence with no type to catch
it. So `Heap::inbound` refuses an `Attr` as a **parameter**, recursively — a `list[Attr]` parameter
is the same problem one collection out — and a definition may answer with one and may not take one.
An `Html` parameter is unaffected, which is the case a program would actually write.

This is the second rule on this boundary that exists because of what the *host* would have to do,
after [`108`](108-closures-arrive-report.md)'s closure — and it is worth noticing that the two are
different shapes. A closure is refused in **both** directions and in every nested position; an `Attr`
is refused in one. `Heap::crossing` could not have expressed that, which is why it is a second
function rather than a case added to the first.

## 109.4 A view has no order, and that is a refusal rather than an omission

An `Html` in the arena is a recipe, so two nodes that render the same page can be different objects:
`html_text(3)` and `html_text("3")` are two words apart and one tree. `beck_core::Html`'s derived
`Ord` compares the *pages*. A compiled comparison that answered from the recipe would therefore
disagree with the evaluator on exactly the programs nobody writes, which is worse than refusing.

So `Repr::order` grew a fourth case — `Order::Absent`, with the reason — and the demand side grew
`Heap::ordered`, which walks: a `model Card { body: Html }` has no order because its field has none,
a `list[Html]` has none because its element has none, and a `Map[Str, Html]` has none because its
value has none. Every demand for a comparison asks it *first* — an `==`, a `list_contains`, a
`sort_by` key, a map's search — so the refusal names the definition that wanted one. The same
question asked while the module was being assembled would have named nothing, and both emitters now
write **no comparison at all** for such a repr, so a bug in that rule is a missing symbol at link
time rather than a page that sorts by where it was allocated.

One consequence is stated here rather than left to be met: a lookup into a `Map[Str, Html]` is
refused, because `wants` demands a *map's* comparison and a map compares its values as well as its
keys — even though the search itself only ever compares keys. It has not bitten (no program in the
tree holds a page in a map), and the fix if it does is to split the demand rather than to weaken the
rule.

That is [`107`](107-a-map-arrives-read-only-report.md) §107.5's accessor doing its job a second time.
It was built after the same defect had been found three times — a `_` arm swallowing whichever
reference kind had just been added, so two equal values compared unequal because their offsets
differed — and the shape of this change is exactly what it was built for: a new repr, a new case in
one enum, and a compile error at every site that has to say what it means.

## 109.5 What compiles now

| | before | after |
|---|---|---|
| definitions compiled across the tree | 650 | **688** |
| definitions refused | 768 | **730** |
| refusals blaming a view | 42 | **0** |
| corpus programs whose `view` compiles | 0 | **21 of 32** |

Both columns are totals of `beck native <file>`'s own two headline lines, over
`corpus/ awfy/ clbg/ sicp/ examples/ lib/`, taken at the commit before this change and at this one.
They differ by four from [`108`](108-closures-arrive-report.md)'s 646 at the same commit, and the
difference is the *method* rather than the tree: that report's number came from counting inside the
compiler and this one from the command a reader can run. Neither is wrong; the delta is what this
section is claiming, and it is a delta between two runs of one command.

The four that blamed a view and are refused anyway are the useful column, because each names
something else that was already true:

| | now refused for |
|---|---|
| `corpus/13-reservations.beck` `view` | `str` of a `SlotId` |
| `corpus/26-sensors.beck` `render` | `str` of a `Float` |
| `corpus/27-review.beck` `view` | a callee that does not compile |
| `corpus/28-catalogue.beck` `view` | a trait method that does not compile |

The first two are worth one sentence, because they look like a contradiction: `html_text(x)` renders
any `x` and `str(x)` renders three. It is not a contradiction, it is the whole design in one line —
**a deferred value works because nothing in compiled code looks at the rendering**. `str(x)` answers
a `Str` that the program then slices, compares and concatenates, so it has to exist at the machine;
`html_text(x)` answers a node nobody reads. The two will meet the day a `Float` can be rendered
here, and not before.

`beck native examples/todo.beck` now says:

```
6 compiled to native code:
  if_owned … mine … remaining … view … render … done_class

3 left to the evaluator:
  apply_event   `map_insert` grows a map …
  toggled       `map_insert` grows a map …
  validate      `str_trim` trims Unicode whitespace …
```

## 109.6 What it costs, and it is not a win

`measure_native.rs::what_a_page_costs_against_the_tree_walker`, release build, two sizes eight times
apart:

| page | rows | evaluator | native | ratio |
|---|---|---|---|---|
| the `ui:` block, keys and handlers | 200 | 817.8 µs | 1.023 ms | **0.80×** |
| | 1,600 | 11.84 ms | 8.87 ms | **1.33×** |
| the same page, text only | 200 | 350.0 µs | 265.1 µs | **1.32×** |
| | 1,600 | 1.891 ms | 2.081 ms | **0.91×** |

Nothing there is asserted to be faster, and the report says so before it says anything else. A
compiled `view` builds the call and the host bakes the tree, so **the rendering is the same
`Value::display` either way** and the pipe is additional — what is compiled is the program's own
logic, the loops and the conditionals and the field reads, which on these pages is not where the time
goes. [`94`](94-mode-b-report.md) §94.14 measured the same thing from the other end and said it
first: 97% of an interaction is `view`, and what grows is `view` being a pure function of the whole
state.

What *is* asserted is a shape, twice, because a shape is what a wrong design shows in:

- **the ratio does not collapse over eight times the rows** (the table above, gated), which is what a
  recipe that copied the children built so far would do;
- **a page costs 96 bytes a row and 504 bytes a page, at 100 rows and at 800**
  (`native.rs::a_page_costs_its_own_nodes_and_nothing_per_page`, a gate with no clock in it). Twelve
  words a row: an `li`, its empty attribute list, its child list, its text node, and the word it
  occupies in the page's own child list. A builder that reallocated a list per child would be
  quadratic in the arena and would still answer correctly at every size a test would run.

## 109.7 The gates

- **`native.rs::the_two_backends_agree_on_views`** — 253 calls, LLVM against the tree-walker, over
  programs written to make each rule bite: an attribute whose value is empty (dropped), a key (not an
  attribute), a handler (JSON), two attributes in a fixed order (the hash is folded in order), a text
  node of every shape a value has here, a child that is already a tree, children built by a loop, the
  empty element, a view held in a record's field, and twenty-one trees built by the evaluator and
  handed *back in*. `Value`'s equality on an `Html` includes every structural hash, so a node
  assembled in a different order fails even when it renders the same.
- **`cranelift.rs::the_three_backends_agree_on_views`** — 127 calls over all three, plus
  `the_two_emitters_agree_on_which_views_compile`, which is [`97`](97-cranelift-report.md) §97.3's
  assertion one type over. A view is the case where the two emitters could most easily drift, because
  neither generates a runtime function for it — there is nothing to link against that would notice.
- **`native.rs::a_ui_block_compiles_and_agrees`** and its Cranelift twin — the `ui:` macro's own
  lowering rather than the five primitives written by hand, which is what a program actually
  contains.
- **`native.rs::a_view_has_no_order_and_the_refusal_says_why`** — the three ways an ordering can be
  demanded (a search over a list of views, a record holding one being compared, `==` between two),
  each refused with `Repr::order`'s sentence; and §109.3's directional rule, both ways — an `Attr`
  parameter and a `list[Attr]` parameter refused, against the control that answering with one
  compiles and so does taking a whole tree.
- **`native.rs::a_corpus_fold_compiles`** — `view` moved from that test's refusal side to its control
  side, which is what those lists are for. The other side is now "something in the corpus is still
  refused for growing a map", so it cannot pass by everything compiling.

## 109.8 What this does not establish

- **Nothing about speed.** §109.6 is the whole of it, and the two rows that beat the tree-walker are
  a fixed pipe cost being amortised rather than a code generator winning.
- **Nothing about a page that is *rendered* by compiled code.** SSR, the wire encoding and the diff
  are all `beck_core`'s and are reached after the value comes back. A compiled `view` shortens no
  path through them.
- **Nothing about Mode B.** The browser needs a wasm emitter as well as a heap
  ([`94`](94-mode-b-report.md) §94.8, [`adr/0022`](adr/0022-mode-b-ships-the-backend-it-has.md)), and
  a code generator that compiles to machine code is the opposite direction
  ([`97`](97-cranelift-report.md) §97.7). What this removes is the *prerequisite*, not the work.
- **Nothing about the effects**, which are the last row of Lane E and are not a view's problem.
- **Nothing about growing a collection.** A page is built from lists the program already has —
  `map_list` over the todos — and `list_append` is still refused
  ([`106`](106-lists-arrive-read-only-report.md) §106.5). A `ui:` block that accumulated its children
  would not compile.

## 109.9 What this corrects

- [`105`](105-text-on-the-heap-report.md) §105.10, [`106`](106-lists-arrive-read-only-report.md)
  §106.8, [`107`](107-a-map-arrives-read-only-report.md) §107.7 and
  [`108`](108-closures-arrive-report.md) §108.9 each carry `Html` in the list of what is left. All
  four stand as history; this is where the correction lives.
- **`Heap::repr` no longer refuses `Html` or `Attr`**, and the reason it used to give — "a tree of
  children, which follows the collections rather than text" — was a true sentence about a design that
  turned out not to be the one taken. A view follows neither: it is the first thing in this arena
  whose contents are a *call*.
- **`beck_llvm`'s own module documentation was stale in a second place.** It said "text, collections,
  closures and every effect are still the tree-walker's", which stopped being true at
  [`105`](105-text-on-the-heap-report.md) and was not edited then. What is the tree-walker's now is
  the effects and every operation that *grows* a collection, and it says so.
- [`08`](08-roadmap.md) §8.5.4's Wave 4 paragraph and §8.5.5's Lane E row both list three things;
  they list two now.
