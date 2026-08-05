# 76 — Phase 3, part 45: the record and the read

**Built.** Three changes the profiler pointed at, worth **4% to 8% on record-heavy programs** and
nothing at all on programs without records — which is the right shape, and is why the two are
reported separately.

It is also the report where the profiler's own units had to be corrected: `callgrind` counts
**instructions**, and one of the three looked like 6% of the process and turned out to be 2%.

## 76.1 A record literal sorts once instead of searching per field

[`75`](75-what-the-profiler-said-report.md) §75.3 made a record's fields a sorted `Vec` and left
`Fields::insert` doing what the `BTreeMap` had done: a **binary search per field**, with a string
comparison at every probe. `eval_make` called it once per field of every record a program builds,
and `eval_with` once per field it updates — **13.97 million times** in one run of
`awfy/havlak.beck`, 4% of the process between the search and the `memcmp` under it.

That is the wrong shape for the job twice over. A record literal knows all its fields at once, so it
should sort them once rather than place them one at a time; and `with` never introduces a field, so
it should find one rather than order one.

- `Fields::from_pairs` takes the fields in the order they were written, evaluated in that order
  because a field expression can raise, and sorts once. For a handful of elements
  `sort_unstable_by` is an insertion sort: `n - 1` comparisons and no movement at all when the
  fields already arrive in order, which a record literal's usually do.
- `Fields::insert` scans by **equality** rather than binary-searching by order. `==` on two `str`s
  tests the lengths first and can stop; ordering cannot. A record has three to eight fields, so the
  scan makes at most as many comparisons as the search did and nearly all of them are an integer
  test. Only a genuinely new field pays for the ordered insert, and `with` never has one.

## 76.2 A variable read no longer proves what it is not about to do

`Env::read` walks the scope chain, and [`70`](70-last-use-moves-report.md) gave it a second job: on
a **last** read of a movable value it takes the binding out of the frame instead of copying it,
which is only sound if nothing else holds that frame. Establishing that costs two atomic loads per
scope level — `Arc::strong_count` and `Arc::weak_count` — plus an `Arc::get_mut`.

It was paying that on **every** read. The overwhelming majority are not last uses, and a read that
is not going to take anything out of a frame does not need to know who else holds it. One line:

```rust
if !may_move {
    return self.get(v).cloned();
}
```

`Env::read` was 5.0% of `awfy/havlak.beck`. This is worth 0–3.5% depending on how deeply a program
nests its `let`s, because the atomics were per *level*.

## 76.3 A field name is decided on its first byte

`str`'s `==` checks the length and hands the bytes to `memcmp`; `<[u8]>::cmp` calls `memcmp` over
the common prefix before it even looks at the lengths. A record's field names are three to eight
short strings, and telling `kind` from `link` should not be a library call.

`same_name` and `cmp_name` compare the length and the first byte before anything else. Two field
names that share both are rare, so almost every comparison now ends in a register.

## 76.4 What the profiler got wrong, and it is worth knowing

`callgrind` measures **instruction counts**, and §76.3 is where that stops being a proxy for time.
`memcmp` was **6.21% of the instructions** executed by `awfy/richards.beck`, over half of it inside
the sort in §76.1; removing nearly all of it is worth about **2%** of wall clock. glibc's `memcmp`
is an AVX2 routine that retires a lot of instructions per cycle, so a profile that counts them
over-weights it against branchy interpreter code that stalls.

[`75`](75-what-the-profiler-said-report.md) §75.1 said the profiler saw what four reports of
reasoning had not, and that stands. This is the other half of it: an instruction profile ranks
*candidates*, and the wall clock decides. Both of §76.1 and §76.3 came off the same profile line and
one is worth three times the other.

## 76.5 What it buys

Release, minimum of nine, the two binaries interleaved:

| | [`75`](75-what-the-profiler-said-report.md) | now | |
|---|---|---|---|
| `awfy/havlak.beck` | 2.443 s | **2.240 s** | **−8.3%** |
| `awfy/deltablue.beck` | 0.051 s | **0.048 s** | **−6.9%** |
| `awfy/richards.beck` | 1.338 s | **1.248 s** | **−6.8%** |
| `clbg/pidigits.beck` | 0.899 s | 0.905 s | **±0** |

`pidigits` builds no records and is the control: it should not move and it does not.
`awfy/json.beck` runs in 82 ms, of which most is parsing and checking, and it is below this
harness's noise floor either way — the per-change measurements have it at −2 to −3%.

## 76.6 How it is tested

Nothing new. None of the three changes an answer — §76.1 in particular keeps both the evaluation
order of a record literal's fields and the iteration order of the result, which is what the 48
suites check between the replay-determinism harness, the state digests and the checked-in page
snapshots.

## 76.7 What is not built

| | |
|---|---|
| A variable read that is an index | **still not built**, and [`75`](75-what-the-profiler-said-report.md) §75.5 remains the description. §76.2 removed the atomics from the walk; the walk itself is still one hop per scope level, and making it an indexed load needs the checker to assign a slot per binding and a function's locals to live in one frame |
| Fields placed by a compile-time permutation | **not built.** §76.1 still sorts at run time, `n - 1` comparisons per record built. The order is a function of the source alone, so it could be a permutation computed once — at the cost of a new field on `CoreKind::Make` and a pass to fill it. Worth doing when something else needs that pass |
| Interned field names | **not built**, and §76.3 is why it is now less attractive: a first-byte test recovers most of what pointer equality would, for none of the cost of a process-wide interner |
