- **2026-08-23 — A list of numbers is a column, and no program can tell.**
  [`docs/105`](../docs/105-the-ecosystem-answer.md) §105.10, [`docs/08`](../docs/08-roadmap.md) §8.5.4's
  item after the aggregates. `Value::List` was `Arc<Vec<Value>>` — 16 bytes an element, right for a
  keyed arrangement and wrong for a million doubles — and `Value::Float` holds an order-preserving
  key rather than `f64` bits, which is right for a map key and wrong for a kernel. So there was
  nothing in this language a BLAS routine or an Arrow reader could be handed a pointer to.
  `beck-core/src/seq.rs` is the second representation: a list of `Int` or of `Float` is a dense
  buffer, **8,000 bytes against 16,000** for a thousand integers and 64,000 against 128,000 for
  eight thousand, with `Seq::floats()` the `&[f64]` that did not exist. `Value` is **still 16
  bytes** — the layout enum sits behind the `Arc`, so nothing that is not a list pays for it, which
  is the trade `Record`'s own doc comment already refused once.
  **The layout was the easy half.** Two lists holding the same elements are one value, and four
  mechanisms have to agree: order and equality — written by hand, because a *derived* `Ord` compares
  the layout's discriminant first and would sort every column before every list, in the order that
  reaches the rendered page and the replay digest — the state digest, and the wire format. Both
  float columns go back through `Value::float` to compare, because `-0.0` and `NaN` are exactly the
  two IEEE values that constructor exists to canonicalise and a raw `f64` buffer holds them as
  themselves. `beck-cli/tests/columns.rs` folds every corpus program's generated log with the layout
  switched on and again with it off and holds the two runs to the same digest after every event and
  the same page for every subscriber — which is also [`docs/08`](../docs/08-roadmap.md) §8.3 item 8's
  off switch proved rather than promised (`AppConfig::columns`, `beck_core::seq::set_columns`).
  **A layout with no producer would have made that gate compare one run with itself**, so where a
  column is *born* is part of the change: `Seq::pack` reads the elements a primitive produced, and
  `Seq::push` promotes an **empty** list on its first element — which is what gives the accumulator
  idiom `go(i + 1, list_append(done, x))` a column with no program changing a line. `map_list`,
  `filter_list` and `concat_lists` build straight into a `Seq` rather than into a `Vec<Value>` that
  is then packed, so a mapped list of numbers costs one allocation and one pass rather than two of
  each; `list_min`, `list_max` and `list_sum` read the dense buffer. The sweep reports **6 of 40
  programs building a column while folding and rendering, 462 columns in all** — and the first
  instrument said zero, truthfully and about the wrong thing: it walked the accumulator, and
  `corpus/26-sensors.beck` builds its `list[Float]` inside its *view*, where it lives for as long as
  it takes to render.
  **What it costs and what it buys, measured with the switch as the control.** Memory is exact and
  gated: 8,000 bytes against 16,000 at a thousand integers and 64,000 against 128,000 at eight
  thousand. Time is a report rather than a gate ([`docs/13`](../docs/13-testing.md) §13.7) and it says
  two things. On a 200,000-element numeric workload — built by accumulation, then mapped, filtered,
  summed and counted — the release binary takes **202–212 ms with the layout on against 218–242 ms
  with it off**, three runs, the same direction each time. On Are We Fast Yet there is **no
  measurable change**: `havlak` 2,519 ms against 2,556 and `richards` 1,387 against 1,396, the only
  two benchmarks long enough to read over ~24 ms of process startup — and `awfy/list.beck` says in
  its own header why, which is that it deliberately holds no `list[Int]`. That is the honest shape
  of the result: the layout moves numeric work and leaves everything else alone.
  **The by-value iterator was the one real hazard and it is not on the hot paths.** A column has no
  `Value` to lend, so `Seq::iter` yields by value — which would have put an atomic increment per
  element on the digest, the wire format and `to_json`, all of which walk every element and keep
  none. `Seq::for_each` and `try_for_each` lend where the layout has something to lend, so a boxed
  list pays exactly what it paid before this module existed.
  **Arrow is not built**, and the reason is a gate rather than an effort: nothing in this workspace
  reads Arrow, so an encoder written here would be a writer checked by its own reader — the
  objection [`docs/07`](../docs/07-dependencies.md) §7.4 makes about hand-written formats, and the
  reason [`adr/0030`](../docs/adr/0030-the-webassembly-emitter-writes-its-own-bytes.md) made the
  WebAssembly emitter wait for a JavaScript engine. The `arrow` dependency lands with the Parquet
  archive that needs a reader for what it writes ([`docs/08`](../docs/08-roadmap.md) §8.5.4's G item),
  whose named predecessor this was.
