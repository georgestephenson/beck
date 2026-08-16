# 102 — The ecosystem answer

> **Design, not a report. Nothing here is built.** [`09`](09-risks-and-open-questions.md) §9.2 calls
> ecosystem access "the strategically important one" and [`01`](01-vision-and-premise.md) §1.5 item 7
> states it harder — "a language that cannot call the Python and npm ecosystems is a research
> project". Both are seven years of correct instinct with no artefact behind them, and neither
> answers the question a developer actually asks, which is not "can you call Python" but **"what do I
> do about NumPy"**.
>
> This document answers it per library rather than per ecosystem, because the answers differ and
> averaging them is how the question stayed open. §102.4 is a measurement — the most-downloaded
> packages of four ecosystems, fetched rather than recalled — and it says something the premise of
> the question does not predict.

## 102.1 The premise, taken seriously and then narrowed

The premise is that people choose Python because `import numpy` is one line, and that a new language
starts at zero against twenty years of accumulated libraries. The first half is true. The second
half is true and is **not** the reason to worry, because it is true of every language that has ever
succeeded, and each of them won a *territory* rather than the whole board.

What is worth taking seriously is narrower and sharper: **a language that has no answer for the
functionality its target users need every day is unusable regardless of its guarantees.** Not "has
fewer libraries" — has *no answer*. That is the bar this document holds Beck to, and §102.9 is where
it fails to clear it.

[`01`](01-vision-and-premise.md) §1.7 already conceded the scientific-computing territory
explicitly: "ML/numeric work, systems programming, and ecosystem breadth are conceded and bridged
(FFI, sidecar), not contested." That concession stands and this document does not reopen it. What it
does is stop the concession from being load-bearing for cases it was never about.

## 102.2 The constraint that decides every case

Every answer below is forced by one property, so it is stated once rather than argued four times.

A bridged call carries an effect — `net.out(host)` or `external.read/write(store)`
([`03`](03-type-and-effect-system.md) §3.6, §3.8). And [`03`](03-type-and-effect-system.md) §3.7
requires a fold's function to be **replay-pure, effect row ⊆ {}**; a view is a pure function of
signal values. That is not a convention. It is enforced:

```
compiler/crates/beck-core/src/place.rs:760
    "`{name}` is a fold function, so it must be replay-pure"
```

and gated by [`replay.rs`](../compiler/crates/beck-cli/tests/replay.rs), whose header says why: §3.7
is what makes replay determinism true, and determinism is what [`10`](10-decisions.md) D3 rests the
whole data tier on.

**So a bridge cannot reach the data tier.** It can sit at a merge point — command → ingress →
bridged call → event — and that is a real and sufficient place for a large class of work. It is not
the place anybody means when they say "I use pandas", which is *inside a view*.

This has a consequence worth stating in one line, because it is the finding that organises the rest:

> **A compatibility layer and a library are not two ways to get the same functionality. They land in
> different tiers, and the tier is decided by the effect row rather than by preference.**

## 102.3 Four answers, and the rule that picks one

| Answer | When | Cost | Where it lands |
|---|---|---|---|
| **Dissolve** | The library exists to patch a problem Beck's semantics do not have | Zero, plus a sentence saying so | Nowhere — the need is gone |
| **In the language** | The functionality is a means of combination, not a library | A language feature; expensive and compounding | Any tier, including folds and views |
| **Link** | Somebody's C or Rust artefact is the state of the art and always will be | A primitive in `prelude.rs` plus a dependency | Any tier — a linked primitive is pure if the function is |
| **Bridge** | Genuinely foreign, genuinely large, genuinely somebody else's | The sidecar ([`09`](09-risks-and-open-questions.md) §9.2), and an effect row that says so | **Merge points only** — §102.2 |

The rule that picks between **link** and **bridge** is the one [`AGENTS.md`](../AGENTS.md) already
states for performance: *ask what the operation should cost*. If the answer is "what a hand-tuned
kernel costs", link it — a reimplementation would be a performance defect in the semantics, which
survives into every backend. If the answer is "whatever the ecosystem's own implementation costs
because nobody will ever beat it and it is not on our critical path", bridge it.

The rule that picks **dissolve** is [`29`](29-domain-driven-design.md)'s, already exercised on the
four DDD patterns it marks dissolved: ask what problem the library patches, and check whether the
problem exists here.

## 102.4 The survey, measured

The premise of this question deserves a measurement rather than a recollection. Below are the
most-downloaded packages of four ecosystems, fetched on **2026-08-16**.

| Ecosystem | Source | Method |
|---|---|---|
| PyPI | `hugovk/top-pypi-packages` (ClickHouse over the PyPI download log), `last_update: 2026-08-01` | `curl -sSL https://raw.githubusercontent.com/hugovk/top-pypi-packages/main/top-pypi-packages.min.json` |
| crates.io | crates.io API, all-time downloads | `curl -sS 'https://crates.io/api/v1/crates?sort=downloads&per_page=15'` |
| NuGet | NuGet search API, total downloads | `curl -sS 'https://azuresearch-usnc.nuget.org/query?q=&take=20&prerelease=false'` |
| npm | npm downloads API, last month | **Not a true ranking** — npm publishes no ordered index, so this is the top of a 40-package candidate set of known-high names, measured individually via `https://api.npmjs.org/downloads/point/last-month/<pkg>`. Read it as "these are all very large", not as "these are the ten largest" |

**Top 10, each ecosystem:**

| # | PyPI | npm (see caveat) | NuGet | crates.io |
|---|---|---|---|---|
| 1 | boto3 | semver | Newtonsoft.Json | hashbrown |
| 2 | packaging | minimatch | Extensions.DependencyInjection | syn |
| 3 | typing-extensions | ansi-styles | Extensions.Logging | getrandom |
| 4 | certifi | debug | System.Text.Json | bitflags |
| 5 | urllib3 | brace-expansion | Bcl.AsyncInterfaces | rand_core |
| 6 | idna | ms | Azure.Core | rand |
| 7 | requests | strip-ansi | Serilog | libc |
| 8 | charset-normalizer | chalk | IdentityModel.Abstractions | quote |
| 9 | setuptools | commander | System.Drawing.Common | proc-macro2 |
| 10 | botocore | supports-color | Microsoft.Identity.Client | base64 |

**Not one of these forty is NumPy or pandas.** NumPy is PyPI **#19**; pandas is outside the top 20
entirely. That is not an argument that they do not matter — they matter enormously to the people who
use them — but it falsifies the premise as stated. What the top of every ecosystem actually holds is
four things:

1. **HTTP, TLS and text encoding** — `urllib3`, `requests`, `certifi`, `idna`,
   `charset-normalizer`.
2. **Serialisation** — `Newtonsoft.Json`, `System.Text.Json`, `pyyaml`.
3. **Build-time, packaging and compatibility plumbing** — `packaging`, `setuptools`,
   `typing-extensions`, `six`, `semver`, `minimatch`, `glob`, `brace-expansion`, `tslib`,
   `undici-types`, `syn`, `quote`, `proc-macro2`.
4. **Terminal colour** — `ansi-styles`, `chalk`, `strip-ansi`, `supports-color`, `has-flag`,
   `color-convert`, `color-name`. Seven of npm's largest packages exist to make text orange.

The npm column is the most instructive and the least flattering to its ecosystem: **every entry is
build tooling or terminal formatting, and not one is application functionality.** That is a fact
about npm's granularity — a language whose standard library omits `padStart` gets a `left-pad` — and
it is the strongest available argument that *package count is not the metric*. Beck starts at zero
against a number that is mostly measuring somebody else's missing standard library.

### The verdict table

| What | Representative packages | Answer | Status |
|---|---|---|---|
| HTTP client, TLS, URL, encodings | requests, urllib3, certifi, idna, axios | **In the language** | **Built** — [`46`](46-standard-library-report.md), `lib/http.beck`, `net.rs`'s three implementations |
| JSON | Newtonsoft.Json, System.Text.Json | **In the language** | **Built** ([`46`](46-standard-library-report.md)) |
| Dates and times | python-dateutil | **In the language** | **Built** — `lib/dates.beck`, RFC 3339, UTC-only by choice ([`12`](12-standards-and-conformance.md)) |
| Base64, digests, UUID | base64, uuid, cryptography (part) | **In the language** | **Built** — `lib/crypto.beck`, `prelude.rs` |
| Decimal and big integers | decimal, bignum | **In the language** | **Built** ([`46`](46-standard-library-report.md)) |
| Logging | Serilog, Extensions.Logging, debug | **Dissolve** | D17's telemetry — OTLP, one vendor-neutral format ([`08`](08-roadmap.md) §8.6). A log line is a span attribute |
| Dependency injection | Extensions.DependencyInjection | **Dissolve** | There is no container because there is no wiring: placement is inferred and effects are the contract. A DI framework patches a language that cannot express where code runs |
| ORM, migrations, schema | EntityFrameworkCore, SQLAlchemy, Alembic | **Dissolve** | [`29`](29-domain-driven-design.md) §29.1 — the repository pattern "does not exist. State is a fold over the log; there is nothing to load, save, or mock"; migration is `migrate`/`upcast` demanded at deploy |
| Validation | pydantic, zod | **Dissolve** | The type checker and `validate` blocks. A runtime schema validator patches a language whose boundary is untyped |
| State management, caching | redux, Extensions.Caching | **Dissolve** | [`15`](15-scale-and-distribution.md) §15.6 — "cache does not exist as a concept; incrementally-maintained views *are* the cache, invalidated by construction" |
| Terminal colour | chalk, ansi-styles, ×5 more | **Dissolve** — for a *program*; a CLI concern the compiler itself has and a program does not | Beck programs are services, pages and folds |
| Build plumbing | packaging, setuptools, semver, minimatch, tslib | **Dissolve** | One compiler, one lockfile, one build. §102.4's point 3 is the category that a language absorbs by existing |
| Macro machinery | syn, quote, proc-macro2 | **In the language** | Macros are a language feature ([`02`](02-syntax.md) §2.4). The **macro interpreter** is [`08`](08-roadmap.md) §8.5.4's first item |
| Hash maps, ordered maps, RNG | hashbrown, indexmap, rand, getrandom | **In the language** | `Map` is a `PMap`; randomness is an effect, per §3.7 |
| Regex | regex, re | **In the language**, and **not built** — §2.5's `regex"…"` typed literal waits on the macro interpreter | Named in §102.9 |
| YAML | pyyaml | **Link** — a Rust crate behind a primitive | **Not built**, small, named in §102.9 |
| Compression | gzip, brotli | **Link** | Partially — `brotli` is already in the Mode B budget path |
| Crypto primitives | cryptography, cffi, Microsoft.Identity | **Link**, by delegation — [`16`](16-packages-and-ecosystem.md) §16.7 says the standard library delegates rather than implements | **Built** for digests and signatures; identity is [`48`](48-identity-report.md) |
| Cloud SDKs | boto3, botocore, aiobotocore, Azure.Core, AWSSDK.Core | **Bridge or generate** — and PyPI's **#1, #10 and #18** | **No answer exists.** §102.9 |
| Dense numerics, linear algebra | numpy, scipy | **Link** — §102.6 | **No answer exists.** §102.9 |
| DataFrames | pandas, polars | **In the language** — §102.5 | Designed ([`99`](99-the-data-tier-means-of-combination.md)), unbuilt |
| Charting | matplotlib, Chart.js, Recharts, System.Drawing.Common | **In the language** (an `svg:` vocabulary) — §102.7 | **No answer exists, and one line blocks it.** §102.9 |
| ML inference, PDF, scientific | torch, transformers, reportlab | **Bridge**, at a merge point | Designed ([`09`](09-risks-and-open-questions.md) §9.2), unbuilt |

## 102.5 pandas is an algebra, not a library

The pandas question resolves into a question this project has already answered and not scheduled
into its ordered list.

[`99`](99-the-data-tier-means-of-combination.md) establishes that Beck's view algebra has **unary
means of combination only** — every operator takes one collection — and that join, `group by`,
aggregates other than a whole-collection count, `distinct` and difference are all missing. §99.2
establishes this was an oversight rather than a decision: "joins have no argument anywhere in the
document set."

Set §99.9's order of work beside the pandas core API:

| §99.9 item | pandas | polars |
|---|---|---|
| 3. `arrange_by` | `set_index` | `.sort()` / index |
| 4. `join` | `merge` | `.join()` |
| 5. recognise loop-plus-lookup | — | — |
| 6. `group by`, `count`/`sum`/`min`/`max` | `groupby().agg()` | `.group_by().agg()` |
| 7. `distinct`, difference | `drop_duplicates` | `.unique()` |
| built | `apply`, boolean masks, `sort_values` | `.select()`, `.filter()`, `.sort()` |

It is the same list. So **a pandas port is not the shortest path to pandas' functionality — finishing
the algebra is**, and the result is strictly better than a port for the workload Beck's users
actually have, for a reason that is structural rather than aspirational: these operators are
*maintained incrementally* ([`23`](23-incremental-views-report.md)), and a dataframe API is batch by
construction. A pandas-shaped Beck library would put whole-table recompute semantics on top of an
incremental engine, which is the defect class [`AGENTS.md`](../AGENTS.md) names first.

§99.3 shows the cost is already being paid: `corpus/27-review.beck` contains a nested-loop join,
reapplied to every element on every event, and `beck explain cost` prints the defect without
counting it.

**Verdict: build, as the missing half of the language, in §99.9's order.** Not as a package.

## 102.6 NumPy is a link, not a rewrite

The opposite conclusion, for reasons that do not apply to pandas.

**NumPy's speed is not Python's — it is OpenBLAS's.** A reimplementation would be a reimplementation
of decades of hand-tuned per-microarchitecture kernels, and `AGENTS.md`'s rule settles it before any
measurement: a `dgemm` should cost what a tuned `dgemm` costs, so a Beck version is a design error
rather than a slow version of a right answer.

The representation says the same thing from the other end. `Value` is 16 bytes and a list is
`List(Arc<Vec<Value>>)` ([`core.rs:790`](../compiler/crates/beck-core/src/core.rs)), so a million
doubles is a boxed, pointer-chased 16 MB; and `Float(u64)` is stored as an **order-preserving key**
rather than as `f64` bits, which is exactly right for the reason its doc comment gives — a map key
and the state digest need a total order that agrees with arithmetic — and exactly wrong for a dense
numeric kernel, which pays a bit transform per operation.

That is not a defect to fix in `Value`. It is a second representation to add, and §102.8 is the one
worth adding.

**Verdict: link.** A dense typed column plus BLAS behind primitives, reachable from pure code so it
is legal in a fold. The bridge is for the *rest* of SciPy, at merge points, where it belongs.

## 102.7 matplotlib, and the one line that blocks it

Charting is the gap this document was most surprised to find, because it is the one with a real user
in front of it and no design anywhere in `docs/`.

Every dashboard, every admin page and every report has a chart in it. A framework that renders HTML
and cannot draw a line chart sends its users to a JS library, which for Mode A means a merge point
and a client that is no longer a patch interpreter. This is squarely a "week two wall"
([`09`](09-risks-and-open-questions.md) §9.3) and it is not on the list of four.

The good news is that the answer is small and native, and the blocker is precise. Beck's element
vocabulary is **open** — `Html::Element { tag: String, .. }`
([`html.rs`](../compiler/crates/beck-core/src/html.rs)) validates no tag list — so `svg:` elements
already survive the compiler and SSR. What stops them is one call in the patch interpreter:

```
compiler/crates/beck-rt/client/beck-patch.js:10
    const el = document.createElement(h[0]);
```

`createElement` puts every node in the HTML namespace. An `<svg>` subtree built that way parses and
does not draw. The fix is `createElementNS` under a namespace determined by the tag — a few lines,
in a file that is already the thin client's core.

With that, a chart is an ordinary Beck `component` returning `svg:` elements, computed by a pure
function of a view. Which means: charts are **incrementally maintained**, WCAG-checkable by
[`12`](12-standards-and-conformance.md) §12.4's machinery like any other component, server-rendered,
and patched by delta rather than redrawn. No charting library in any other ecosystem gets that,
because no other ecosystem's chart is a pure function of an incrementally maintained collection.

**Verdict: build, in the language, small.** And it is the highest ratio of user-visible value to
effort anywhere in this document.

## 102.8 Arrow is the boundary, and it discharges four commitments at once

The move that makes the bridge strategic rather than apologetic is to stop bridging to Python.

[`07`](07-dependencies.md) §7.4 already pins **Apache Arrow** as the columnar interchange format and
**DataFusion** as the analytical engine, with the alternatives argued.
[`04`](04-compiler-architecture.md) §4.4 already routes bulk results over Arrow IPC.
[`12`](12-standards-and-conformance.md) charters Arrow/Parquet. [`08`](08-roadmap.md) §8.5.4's G item
notes that **five documents commit to this and not one gave it a position in an order**, and that no
`arrow`, `parquet` or `datafusion` dependency exists in the workspace — which this document
re-verified.

One columnar value type discharges all of it:

| Commitment | Discharged by the same change |
|---|---|
| [`07`](07-dependencies.md) §7.4's DataFusion choice | It is an Arrow engine; without Arrow values there is nothing to give it |
| [`08`](08-roadmap.md) §8.5.4's G item — Parquet archival | Parquet is Arrow written down |
| §102.6's NumPy problem | A dense typed column is zero-copy to NumPy — and to Polars, DuckDB, R and Spark |
| §102.5's operators | An arrangement over columns is what a maintained aggregate wants anyway |

So the sentence that replaces "we don't have NumPy" is:

> **Beck does not bridge to Python. It bridges to Arrow, and Python is one of the things standing on
> the other side.**

That is [`10`](10-decisions.md) D9's own framing — "our own front door onto the best engines in the
world" — applied to the ecosystem question instead of to the runtime.

## 102.9 What has no answer, and what this schedules

Stated plainly, per [`AGENTS.md`](../AGENTS.md): **"built", "runs" and "measured" are three different
claims**, and everything in this section is at zero.

| Gap | Class | Position |
|---|---|---|
| **Charting / `svg:`** (§102.7) | **S**, small, with a user in front of it | [`08`](08-roadmap.md) §8.5.4. The `createElementNS` fix is a day; the component vocabulary above it is a week |
| **The data tier's algebra** (§102.5) | **F** — §99.7 lists five written-down items it closes | §8.5.4, in Lane B, **parallel to the macro interpreter** — it is the largest item that does not contend for Lane A's files |
| **A columnar value and Arrow** (§102.8) | **F** | §8.5.4, after the algebra — an aggregate is what makes a column worth having |
| **Dense numerics / BLAS** (§102.6) | **S** | Phase 4, after Arrow. It is a primitive and a dependency, not a phase |
| **Cloud SDKs** (boto3 is PyPI #1) | **S** | Phase 4, beside the managed-cloud path. `external store` and `net.out` already type it; what is missing is that nobody wants to hand-write S3's signature algorithm |
| **The Python sidecar** ([`09`](09-risks-and-open-questions.md) §9.2) | **S** | Phase 4, unchanged — with §102.2's placement restriction as a **diagnostic** rather than a discovery |
| **Regex** (`regex"…"`) | **S** | Waits on the macro interpreter, which is already §8.5.4's first item |

The sidecar's diagnostic is worth naming as its own obligation. A `python_service` call inside a fold
is refused today by `place.rs:760`'s replay-purity check, with a message about fold purity that does
not mention the bridge. Someone who reaches for the bridge in a view should be told *why* the bridge
cannot go there and *where* it can — which is §102.2 in two sentences. Per
[`82`](82-the-edge-report.md) §82.10, the gate is written against the shape of the gap: a program
that calls a bridged service from inside a `durable` fold, refused with a diagnostic that names the
merge point as the alternative.

## 102.10 What this refuses

- **In-process CPython.** The GIL, plus a Python runtime inside images whose bit-for-bit
  reproducibility is the security story ([`06`](06-kubernetes-and-packaging.md) §6.2). Already
  refused in [`09`](09-risks-and-open-questions.md) §9.2; refused again with a second reason.
- **Compiling a Python subset.** §9.2 calls it a tar pit. Unchanged.
- **A pandas-shaped Beck package.** §102.5 — batch semantics on an incremental engine, and it would
  make a language problem look like a library problem.
- **`pip install` compatibility as a goal.** [`01`](01-vision-and-premise.md) §1.7's "no `pip
  install` promises" stands. `import numpy` is easy because of twenty years of wheels against one
  ABI-stable interpreter, which is not a property of Python the language and not one a new language
  can acquire by wanting to.
- **Competing with SciPy.** §1.7 conceded it deliberately. This document makes the concession
  *excellent* rather than un-making it.

## 102.11 What this document does not claim

- **Nothing here is built.** Not the algebra, not a column, not a chart, not the sidecar. The four
  code facts asserted — `place.rs:760`, `core.rs:790`, `html.rs`'s open vocabulary,
  `beck-patch.js:10` — were read from the tree on 2026-08-16 and are the only claims about the
  implementation this document makes.
- **The npm column of §102.4 is not a ranking.** npm publishes no ordered index; it is the top of a
  candidate set, and it says "these are all very large" rather than "these are the largest".
- **The download counts measure downloads.** CI pulls the same package thousands of times a day; a
  count is a proxy for usage and a poor one. The *shape* of the result — infrastructure at the top,
  numerics at #19 — is what is being read, not the ordering within it.
- **It does not price the sidecar.** No one has built one, so nothing here says what a call costs.
- **It does not settle charting's vocabulary.** §102.7 establishes that the blocker is one call and
  that the shape is a component; which SVG elements and what a `chart:` abstraction over them looks
  like is undesigned.
- **"Dissolve" is a claim about Beck's semantics, not a claim that the work is done.** A dissolved
  row means no library is needed, not that the replacement is finished — telemetry's push exporter
  is a Phase 4 bullet, and the DI row assumes placement inference that Phase 2 built.

## 102.12 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`09`](09-risks-and-open-questions.md) §9.2 | The sidecar recommendation stands and was **missing its most important property**: the effect row keeps it out of folds and views, so it answers the merge-point half of the ecosystem question and cannot answer the data-tier half. §9.2 now says so and points here |
| [`16`](16-packages-and-ecosystem.md) §16.8 | "Tarns extend the language; bridges rent from neighbours" is right and incomplete — there is a third category, and it is the largest: functionality that is neither extended nor rented but **dissolved** or **absorbed into the language**. §16.8 now names four categories rather than two |
| [`01`](01-vision-and-premise.md) §1.5 item 7 | "Interop or die" is a true instinct pointed at the wrong noun. The ecosystems' own download data says the top of every index is HTTP, serialisation, build plumbing and terminal colour — the territory a standard library and a compiler absorb — rather than the numerics the slogan implies |
| [`08`](08-roadmap.md) §8.5.4 | Three items acquire a position: charting, the data tier's algebra (a Phase 4 bullet that was never in the ordered list), and the columnar value |
