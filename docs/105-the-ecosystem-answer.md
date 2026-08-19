# 105 — The ecosystem answer

> **Design, not a report. Nothing here is built.** [`09`](09-risks-and-open-questions.md) §9.2 calls
> ecosystem access "the strategically important one" and [`01`](01-vision-and-premise.md) §1.5 item 7
> states it harder — "a language that cannot call the Python and npm ecosystems is a research
> project". Both are correct instinct with no artefact behind them, and neither answers the question
> a developer actually asks, which is not "can you call Python" but **"what do I do about NumPy"**.
>
> This document answers it per library, because the answers differ and averaging them is how the
> question stayed open. §105.4 weighs two instruments and discards one: download rank measures
> **fan-in**, and the Stack Overflow survey — the source [`08`](08-roadmap.md) §8.6's ≥1% rule
> already runs on — measures **use**, putting NumPy at 21.2% and pandas at 20.7%, second and third
> among all libraries in every language. §105.5 supplies the test that explains why: **does the
> package change what the language is used for?** NumPy and pandas pass it harder than almost
> anything, which is the opposite of a reason to wave them through — §105.5 argues they are the
> *hardest* category to answer, and §105.7–105.8 answer them.

## 105.1 The premise, taken seriously

The premise is that people choose Python because `import numpy` is one line, and that a new language
starts at zero against twenty years of accumulated libraries. Both halves are true. The second is
true of every language that has ever succeeded, and each won a *territory* rather than the whole
board — so it is a reason to choose the territory deliberately, not a reason to despair.

The bar this document holds Beck to is narrower and harsher: **a language that has no answer for the
functionality its target users need every day is unusable regardless of its guarantees.** Not "has
fewer libraries" — has *no answer*. §105.11 is where Beck fails to clear it, in four places.

[`01`](01-vision-and-premise.md) §1.7 conceded the scientific-computing territory explicitly:
"ML/numeric work, systems programming, and ecosystem breadth are conceded and bridged (FFI,
sidecar), not contested." **That concession is narrower than it reads and is routinely
over-applied.** Conceding *SciPy and PyTorch* — GPU kernels, solvers, a decade of numerical
methods — is sound. Conceding *arrays and dataframes* is not the same concession, because those are
not a scientific-computing speciality; they are how ordinary application code expresses aggregation
over collections, which is [`99`](99-the-data-tier-means-of-combination.md)'s territory and Beck's
own. §105.7 separates them.

## 105.2 The constraint that decides every case

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

> **A compatibility layer and a library are not two ways to get the same functionality. They land in
> different tiers, and the tier is decided by the effect row rather than by preference.**

§105.5 reaches the same conclusion by a completely different route, which is why this document
treats it as settled rather than as a recommendation.

## 105.3 Four answers, and the rule that picks one

| Answer | When | Cost | Where it lands |
|---|---|---|---|
| **Dissolve** | The library exists to patch a problem Beck's semantics do not have | Zero, plus a sentence saying so | Nowhere — the need is gone |
| **In the language** | The functionality is a means of combination or a notation, not a bag of functions | A language feature; expensive and compounding | Any tier, including folds and views |
| **Link** | Somebody's C or Rust artefact is the state of the art and always will be | A primitive in `prelude.rs` plus a dependency | Any tier — a linked primitive is pure if the function is |
| **Bridge** | Genuinely foreign, genuinely large, genuinely somebody else's | The sidecar ([`09`](09-risks-and-open-questions.md) §9.2), and an effect row that says so | **Merge points only** — §105.2 |

The rule that picks between **link** and **bridge** is the one [`AGENTS.md`](../AGENTS.md) already
states for performance: *ask what the operation should cost*. If the answer is "what a hand-tuned
kernel costs", link it. If the answer is "whatever the ecosystem's own implementation costs, because
nobody will beat it and it is not on our critical path", bridge it.

The rule that picks **dissolve** is [`29`](29-domain-driven-design.md) §29.1's, already exercised on
the four DDD patterns it marks dissolved: ask what problem the library patches, and check whether
the problem exists here.

The rule that picks **in the language** is §105.5's, and it is the one that matters most.

## 105.4 Two instruments, and the one to trust

There are two kinds of evidence about what a package is worth, and they disagree. Both were gathered
**2026-08-16**.

### The weaker instrument: download counts

Downloads are the only ecosystem-wide numbers that exist, which is why they get quoted. They measure
the wrong thing.

| Ecosystem | Source | Command |
|---|---|---|
| PyPI | `hugovk/top-pypi-packages` (ClickHouse over the PyPI download log), `last_update: 2026-08-01` | `curl -sSL https://raw.githubusercontent.com/hugovk/top-pypi-packages/main/top-pypi-packages.min.json` |
| crates.io | crates.io API, all-time downloads | `curl -sS 'https://crates.io/api/v1/crates?sort=downloads&per_page=100'` |
| NuGet | NuGet search API, total downloads | `curl -sS 'https://azuresearch-usnc.nuget.org/query?q=&take=100&prerelease=false'` |
| npm | npm downloads API, last month, per package | npm publishes **no ordered index**, so no ranking is quoted here at all |

**Rank measures fan-in, not choice**, and the top 100 of each index demonstrates it plainly enough
that no argument is needed:

- On PyPI, `requests` is #7 — and `certifi` (#4), `idna` (#6) and `charset-normalizer` (#8) are
  three of its four dependencies. They outrank it because they are installed *by* it. Nobody
  chose them.
- On crates.io, **seven** of the top 100 are `windows_x86_64_msvc`, `windows_x86_64_gnu`,
  `windows_aarch64_msvc`, `windows_i686_msvc`, `windows_i686_gnu`, `windows_x86_64_gnullvm` and
  `windows_aarch64_gnullvm` — per-target import-library shims pulled in by `windows-targets`.
- On NuGet, **eight** of the top 100 are `Microsoft.NET.Workload.*.Manifest-8.0.100` and
  `Microsoft.NET.Sdk.*.Manifest-8.0.100` — SDK workload manifests the toolchain downloads on its
  own. No developer has ever typed one.
- On npm, the largest packages are `semver`, `minimatch`, `ansi-styles`, `debug`, `ms` and
  `brace-expansion`, none of which any application picks up deliberately.

A count therefore ranks a package by how many *other packages* depend on it, which is close to
inversely related to how much it changes what you can do: the most-depended-upon code is the code
that does the least. A data file of CA certificates outranks the library that made HTTP pleasant.

**So a rank is not evidence about a library.** For the record, NumPy is PyPI **#19**, pandas **#38**,
SciPy **#89**, Pillow **#68** and — a datum §105.10 uses — **PyArrow is #96**. Those numbers say
these are chosen directly rather than dragged in transitively, which if anything is the harder
achievement. They are not the argument and nothing below rests on them.

### The better instrument: developers asked what they use

A survey asks the question downloads cannot: *do you use this*. This project already treats one as
authoritative — [`08`](08-roadmap.md) §8.6's **≥1% rule** ("a technology that ≥1% of developers
report using in a major annual survey is a **reality**, not a preference, and earns an explicit
verdict here rather than silence") is built on the Stack Overflow survey's cloud section. The same
survey's **Other frameworks and libraries** section, all respondents:

| | | | |
|---|---|---|---|
| .NET | 25.2% | Apache Kafka | 9.4% |
| **NumPy** | **21.2%** | Flutter | 9.4% |
| **pandas** | **20.7%** | OpenCV | 8.6% |
| .NET Framework | 16.4% | React Native | 8.4% |
| Spring Framework | 11.1% | Qt | 7.3% |
| RabbitMQ | 10.9% | Electron | 6.5% |
| scikit-learn | 10.6% | CUDA | 5.8% |
| Torch/PyTorch | 10.6% | Hugging Face Transformers | 4.5% |
| TensorFlow | 10.1% | Apache Spark | 4.4% |

Source: [Stack Overflow Developer Survey 2024, Technology](https://survey.stackoverflow.co/2024/technology).
This is the whole section rather than an excerpt — every entry it lists appears above or in
[`08`](08-roadmap.md) §8.6.2, because all 39 of them clear §8.6's 1% bar and the rule admits no
shortlist. **One caveat, checkable.** The 2025 survey
([Technology](https://survey.stackoverflow.co/2025/technology)) **dropped this section** — its
technology page carries languages, databases, cloud, web frameworks, IDEs, tags, community, LLMs and
collaboration tools, and no general library section — so 2024 is the most recent reading and it is a
year stale.

### The third instrument, tested and discarded: GitHub stars

Stars are the obvious "highest rated" cross-check, so they were gathered rather than assumed.
Fetched from each repository's page on **2026-08-16**, beside the survey's usage figure:

| Repository | Stars | Survey use | |
|---|---|---|---|
| tensorflow/tensorflow | 197.1k | 10.1% | |
| pytorch/pytorch | 102.4k | 10.6% | more used than TensorFlow, **half the stars** |
| pandas-dev/pandas | 49.5k | 20.7% | |
| numpy/numpy | 32.5k | **21.2%** | **most-used library in the survey, 6× fewer stars than TensorFlow** |
| matplotlib/matplotlib | 23.1k | — | |
| serde-rs/serde | 10.8k | — | |

**Stars rank TensorFlow six times above NumPy and measure it at half NumPy's use.** They disagree
with the survey by an order of magnitude, and the direction of the error is diagnostic: a star is a
one-time vote that never decays, cast at the moment of maximum enthusiasm. TensorFlow collected its
during the deep-learning boom and has kept every one of them while PyTorch overtook it in actual
use — visible in the same table, where the more-used framework has half the stars. Stars record a
library's most exciting year; the survey records this one.

So of three available instruments, **two measure something other than use** — downloads measure
fan-in, stars measure accumulated novelty — and one asks the question directly. The rest of this
document uses the third.

### What the survey says

**NumPy and pandas** are
are the **second and third most-used libraries among all developers, in every language and every
domain** — ahead of the Spring Framework, ahead of every ML framework, ahead of Kafka, at roughly one
developer in five. They are not a scientific-computing speciality that a general-purpose language may
politely decline. They are general-purpose developer vocabulary that happens to have been born in a
numerical library.

Two consequences follow, and the second is a roadmap defect:

1. **The utility test (§105.5) and the survey agree**, where the survey and the download ranks do
   not. That is the reason this document is organised around the former.
2. **§8.6's ≥1% rule is scoped to cloud and infrastructure only, and nothing applies it to
   libraries.** Applied here it is unambiguous: at 21.2% and 20.7%, NumPy and pandas clear a bar set
   at 1% by twentyfold, and neither has ever had a verdict recorded anywhere in `docs/`. Charting is
   the same story one level down — matplotlib is absent from the survey's list, but the *category*
   is not optional for any of the three ecosystems §105.6 surveys. That gap is now
   [`08`](08-roadmap.md) §8.6's, and the rule's own words are what convicts it: silence is not a
   verdict.

### Since the survey: four movements, and a warning about looking them up

The survey is from 2024 and this is 2026, so what has moved matters. Each item below is either a
number gathered here or a primary source; the warning at the end says why that discipline was
necessary rather than fastidious.

**1. The dataframe world has converged on Arrow, without being asked to.** pandas 3.0 makes
PyArrow-backed strings the **default** dtype where PyArrow is installed
([PDEP-10](https://pandas.pydata.org/pdeps/0010-required-pyarrow-dependency.html); the *hard*
dependency was postponed after feedback, the default was not). It shows in the download data:
PyArrow is PyPI **#95** at 433M, against pandas' 769M — the interchange format is being pulled in by
more than half of pandas' own installs. §105.10 argued Arrow is the boundary worth building; the
ecosystem has since argued the same thing with its defaults, which is stronger corroboration than
agreement would have been.

**2. Polars is real and is not displacing pandas — and it makes the algebra argument harder to
dodge.** By downloads, polars is **#466 at 71M against pandas' 769M**, roughly one to eleven. What
matters for §105.7 is not the share but that Polars is a **fifth** independent implementation of the
same dozen verbs (with pandas, LINQ, `dplyr` and SQL), written in Rust, whose whole pitch is that the
verbs are worth keeping and the engine underneath them is worth replacing. That is
[`10`](10-decisions.md) D9's "our own front door onto the best engines in the world" arrived at
independently by somebody else, and it is the closest thing to a proof that the verbs are the
durable part.

**3. Tooling speed is now an adoption driver on its own.** `uv` is PyPI **#219 at 187M against
Poetry's #397 at 85M** — the newer tool at more than twice the incumbent's downloads — and `ruff` is
**#132 at 316M**. Neither is faster at anything a user asked for; both are faster at things a user
waits for. [`64`](64-compile-speed-report.md)'s budgets are the same bet and this is external
evidence for it.

**4. A library category exists that did not when the survey ran.** `litellm` is PyPI **#46 at 683M —
above `pip` itself at #51** — with `openai` at #107, `langchain` at #133 and `huggingface-hub` at
#94. Nothing in the 2024 survey's list covers this. It is worth a verdict rather than a shrug, and
Beck's is unusually good: **an LLM call is nondeterministic, so it cannot live in a fold or a view**
(§105.2) and must arrive at a merge point as a command whose *response becomes an event*. That is
the shape LLM applications need anyway and mostly do not get — the prompt, the response and the
model version land in the log, so a session replays exactly ([`03`](03-type-and-effect-system.md)
§3.7) without re-calling the model. The architecture forces the discipline that these applications
otherwise have to remember. It is a **capability** and it is bridged, so nothing new is owed beyond
the sidecar; the row is added to [`08`](08-roadmap.md) §8.6.2.

> **A warning to whoever re-runs this.** Searching for 2026 ecosystem figures returns, at the top,
> confident posts citing "the 2026 Stack Overflow Developer Survey" for precise numbers — pandas at
> 42% of professional Python developers, Polars at 11%, salary premiums to the dollar. **That survey
> has not published.** It opened for responses in
> [June 2026](https://stackoverflow.blog/2026/06/23/the-2026-developer-survey-is-now-open-for-human-developers-only/)
> and the most recent published edition is 2025, which is the one that dropped the library section.
> Those figures are unverifiable and are not used here; every number in this section was either
> fetched from the download data or read from a primary source. The failure mode this document was
> written to avoid — a confident number nobody checked — is now being mass-produced, and
> [`AGENTS.md`](../AGENTS.md)'s rule that a claim must name the command that produces it is the
> defence.

## 105.5 The test that matters, and why notations cannot be bridged

The right question is the one rank cannot answer: **did the package change what the language is used
for?** Python was a scripting language and NumPy made it the language of scientific computing;
pandas made it the language of data analysis. Rust was a systems language and `serde` made it a
data-interchange language; `tokio` made async Rust exist. That is a category no download count
distinguishes — and §105.4's survey ranks its members at the top, which is the corroboration that
makes this a test rather than a preference.

Applying it sorts the passes into exactly two kinds, and the split turns out to decide Beck's answer:

| Kind | What it does | Examples | Can it be bridged? |
|---|---|---|---|
| **A notation** | Adds a way of *saying* things, which composes with your own code at the expression level | NumPy's broadcasting and slicing (`a[mask] * 2`), pandas' `groupby().agg()` chains, LINQ, `serde`'s derive, JSX, Rails' ActiveRecord | **No** |
| **A capability** | Reaches something the language could not reach | PyTorch (GPUs), `cryptography` (primitives), `boto3` (a cloud API), Pillow (image codecs), `psycopg` (a wire protocol) | **Yes** |

**A notation cannot be bridged, and this is the document's central structural claim.** Its entire
value is that it composes with the code around it — you write `df[df.x > 3].groupby('y').mean()` and
every piece is an expression in the host language, type-checked (or not) by the host, closed over by
host variables, and inlined into host control flow. Put it behind an RPC boundary and the
composition is what you lose: each call is a round trip, intermediate results must be materialised
and shipped, and the fluent chain that *was* the value becomes a batch job. A sidecar can host
PyTorch perfectly well. It cannot host pandas in any sense a pandas user would recognise.

Two independent arguments therefore reach the same conclusion — §105.2 from the effect row and
determinism, §105.5 from composition — and they are worth keeping separate, because §105.2's is
about *legality* and §105.5's is about *value*. Even if folds were impure tomorrow, bridging a
notation would still be the wrong shape.

The consequence for Beck is the one the roadmap has to absorb:

> **Every library that most expands a language's utility is a notation, and every notation must be
> in the language. So the ecosystem question is mostly not an interop question — it is a question
> about how good Beck's own means of combination are.**

Which is [`99`](99-the-data-tier-means-of-combination.md)'s subject, and [`02`](02-syntax.md) §2.4's.
It also says which half of that is now the constraint: **the machinery for building notations is
built** — a macro body runs Beck at compile time ([`102`](102-the-macro-interpreter-report.md)) — and
the notations themselves are not. The view algebra is the largest of them and §8.5.4's largest
remaining item.

## 105.6 The verdict, against the packages that pass the test

Not a ranking, and not derived from one. This is the subjective list — the packages a working
developer in each ecosystem would refuse a job without — with Beck's answer and its honest status.
Where §105.4's survey covers a row it agrees, which is the only external check available; the rows
it does not cover (charting, ORMs, testing) are judgement and are marked as such in §105.13.

| What it does | Python | JS/TS | .NET | Rust | Beck's answer | Status |
|---|---|---|---|---|---|---|
| **Arrays and numerics** | numpy | — | — | ndarray | **Notation in the language, kernels linked** — §105.8 | **Nothing** |
| **DataFrames / tabular algebra** | pandas | — | LINQ | polars | **In the language** — it is [`99`](99-the-data-tier-means-of-combination.md)'s algebra, §105.7 | Designed, unbuilt |
| **Charting** | matplotlib | Chart.js, D3, Recharts | — | — | **In the language** as an `svg:` vocabulary — §105.9 | **Nothing, and one line blocks it** |
| **Web framework** | django, fastapi | express, next | ASP.NET Core | axum | **Dissolve** — the language *is* the server; a route is a field of `Session` ([`94`](94-the-client-report.md)) | **Built** |
| **ORM / data access** | sqlalchemy | prisma | EF Core, Dapper | sqlx, diesel | **Dissolve** — [`29`](29-domain-driven-design.md) §29.1: state is a fold, "there is nothing to load, save, or mock" | **Built** |
| **Serialisation** | pydantic | zod | System.Text.Json | **serde** | **Dissolve into the type system** — the wire format is derived from types, and effect-typed at the boundary ([`04`](04-compiler-architecture.md) §4.4) | **Built** |
| **HTTP client** | requests, httpx | axios | HttpClient | reqwest | **In the language** | **Built** — `lib/http.beck` |
| **Testing** | pytest | jest, vitest | xunit, Moq | — | **In the language** — `test` and `property` blocks, mocks inferred ([`21`](21-tests-in-beck-and-proof.md)) | **Built** |
| **Async runtime** | asyncio | — | Task | **tokio** | **Dissolve** — structured concurrency is a language construct ([`80`](80-structured-concurrency-report.md)) | **Built** |
| **Logging / telemetry** | logging | debug | Serilog | tracing | **Dissolve** — OTLP, D17 | **Built**, two exporters pending |
| **CLI parsing** | click | commander | — | clap | Out of scope — Beck programs are services and pages | — |
| **Validation** | pydantic | zod | FluentValidation | — | **Dissolve** — types plus `validate` blocks | **Built** |
| **Resilience / retry** | tenacity | — | Polly | — | **Dissolve** — `process` sagas, at-least-once by construction | Phase 4 |
| **Cloud SDK** | **boto3** | aws-sdk | AWSSDK | aws-sdk | **Link or generate** — capability, not notation | **Nothing** |
| **Image handling** | pillow | sharp | ImageSharp | image | **Link** — capability | **Nothing** |
| **ML / inference** | torch, transformers | — | — | candle | **Bridge**, at a merge point — capability, and the concession §1.7 actually made | Designed, unbuilt |
| **LLM clients** | litellm, openai | ai-sdk | Semantic Kernel | async-openai | **Bridge**, at a merge point — and the response becomes an event, so replay is exact (§105.4) | Designed, unbuilt |
| **Scientific methods** | scipy | — | — | — | **Bridge** — capability | Designed, unbuilt |
| **Columnar interchange** | **pyarrow** | — | — | arrow-rs | **In the language** — §105.10 | **Nothing** |

Read down the "status" column: Beck's answers are strong precisely where the *dissolve* verdict
applies, which is most of web application development, and that is the territory
[`01`](01-vision-and-premise.md) §1.9 aims at. They are absent in five places, four of which are the
same place — arrays, dataframes, charts and columnar interchange are one gap seen from four angles,
and §105.10 is the change that closes them together.

## 105.7 pandas is an algebra, not a library

The pandas answer is the one the utility test makes urgent rather than optional, because pandas
passes that test about as hard as any library in the survey — and it is a **notation**, so §105.5
says it cannot be rented.

[`99`](99-the-data-tier-means-of-combination.md) established that Beck's view algebra had **unary
means of combination only** — every operator took one collection — and that join, `group by`,
aggregates other than a whole-collection count, `distinct` and difference were all missing. §99.2
establishes this was an oversight rather than a decision: "joins have no argument anywhere in the
document set." The join, both of its indexes and three of the four per-group aggregates have since
landed; `sum`, `distinct` and difference have not.

Set §99.9's order of work beside the pandas core API:

| §99.9 item | pandas | polars | LINQ |
|---|---|---|---|
| 3. `arrange_by` — **built** | `set_index` | index | — |
| 4. `join` — **built** | `merge` | `.join()` | `Join` |
| 5. recognise loop-plus-lookup — **built** | — | — | — |
| 6. `count`/`min`/`max` per group — **built**; `sum` and a `group by` over the groups themselves | `groupby().agg()` | `.group_by().agg()` | `GroupBy`, `Sum` |
| 7. `distinct`, difference | `drop_duplicates` | `.unique()` | `Distinct`, `Except` |
| built | `apply`, masks, `sort_values` | `.select()`, `.filter()`, `.sort()` | `Select`, `Where`, `OrderBy` |

Three ecosystems independently converged on the same dozen verbs, which is a strong hint that they
are the actual means of combination for tabular data rather than one library's taste. Beck has the
unary ones, the join, both of its indexes, and per-group `count`, `min` and `max` — each answered
without the group being built. What it does not have is a `group by` that hands over the *groups*,
`sum`, and `distinct`.

So **finishing the algebra is the shortest path to pandas' functionality, not a detour around it** —
and the result is better than a port for a structural reason: these operators are *maintained
incrementally* ([`23`](23-incremental-views-report.md)), where every dataframe library is batch by
construction. A pandas-shaped Beck package would put whole-table recompute semantics on top of an
incremental engine, which is the defect class [`AGENTS.md`](../AGENTS.md) names first.

§99.3 showed the cost was already being paid: `corpus/27-review.beck` contained a nested-loop join,
reapplied to every element on every event, and `beck explain cost` printed the defect without
counting it. **The join half is built, over both of its indexes**
([`99`](99-the-data-tier-means-of-combination.md) §99.6): that program and three others compile to a
maintained equi-join with no edit to any of them, and the tally counts what it prints. Three of the
four per-group aggregates followed — `count`, `min`, `max`, each answered from state the join or the
`group_by` keeps rather than from a group. What is not built is where the rest of the dataframe verbs
live: a `group by` that hands over the groups, `sum`, and `distinct`.

**Verdict: build, as the missing half of the language, in §99.9's order.** What is left of that order
is `sum` — which owes a decision per numeric type before an operator — `distinct` and difference;
items 3, 4, 5 and most of 6 have landed.

## 105.8 NumPy is a notation over a link

NumPy is two things wearing one name, and the answers differ:

| Half | What it is | Answer |
|---|---|---|
| **The kernels** — BLAS, LAPACK, FFT | Decades of hand-tuned per-microarchitecture assembly | **Link.** `AGENTS.md`'s rule settles it before any measurement: a `dgemm` should cost what a tuned `dgemm` costs, so a Beck reimplementation is a design error rather than a slow right answer |
| **The notation** — broadcasting, slicing, `a[mask] * 2`, ufuncs | A way of saying "elementwise over this whole thing" without writing the loop | **In the language**, per §105.5 — and it is the half that made Python scientific, not the BLAS binding |

Splitting them this way is what stops the answer being either "rewrite NumPy" (absurd) or "concede
numerics" (over-broad). What has to be Beck's is the notation; what must never be Beck's is the
kernel.

The representation stands in the way of both. `Value` is 16 bytes and a list is
`List(Arc<Vec<Value>>)` ([`core.rs:790`](../compiler/crates/beck-core/src/core.rs)), so a million
doubles is a boxed, pointer-chased 16 MB; and `Float(u64)` is stored as an **order-preserving key**
rather than as `f64` bits, which is exactly right for the reason its doc comment gives — a map key
and the state digest need a total order agreeing with arithmetic — and exactly wrong for a dense
kernel, which pays a bit transform per operation.

That is not a defect to fix in `Value`. It is a second representation to add, and §105.10 is it.

## 105.9 Charting: why it belongs here, and where it is answered

**The mechanism and the schedule are [`104`](104-styling-and-the-component-library.md)'s**, which
audited the component library independently and found the same defect from the other side: the patch
applier builds subtrees with `document.createElement` and no namespace, so a patched-in `rect`
renders at width 0 against server-side rendering's 50 — a chart paints once and vanishes when its
data changes. It is `DEFECTS.md::svg-namespace`, it is item 1 of [`08`](08-roadmap.md) §8.5.4's
styling cluster, and §104.10 owns the question of what a chart component library is. Nothing here
duplicates that.

What this document adds is the **reason it ranks**, which an audit of the UI could not supply.
Charting is the one row in §105.6 that no ecosystem in the survey does without and that Beck had no
verdict for anywhere — and it passes §105.5's utility test cleanly: matplotlib is a large part of why
analysis happens in Python at all, and every dashboard, admin page and report has a chart in it. A
framework that renders HTML and cannot draw a line chart sends its users to a JS library, which for
Mode A means a merge point and a client that is no longer a patch interpreter. That is squarely a
"week two wall" ([`09`](09-risks-and-open-questions.md) §9.3) and it was not on the list of four.

It also belongs in *this* document rather than only in an accessibility one, because of what the
answer is worth once it works. A chart is an ordinary `component` returning `svg:` elements from a
pure function of a view, so charts are **incrementally maintained**, WCAG-checkable by
[`12`](12-standards-and-conformance.md) §12.4's machinery like any other component, server-rendered,
and patched by delta rather than redrawn. No other ecosystem's chart gets that, because no other
ecosystem's chart is a pure function of an incrementally maintained collection. That is the same
argument §105.7 makes for the dataframe verbs, one tier up.

## 105.10 Arrow is the boundary, and it discharges four commitments at once

§105.8 needs a dense typed column. §105.7's aggregates want one. The archive needs Parquet. And the
numeric ecosystem has already standardised on the answer: **PyArrow is PyPI #96**, and NumPy, pandas,
Polars, DuckDB, Spark and R all speak Arrow natively. The interchange problem is solved and the
solution is not ours to design.

[`07`](07-dependencies.md) §7.4 already pins **Apache Arrow** as the columnar interchange format and
**DataFusion** as the analytical engine, with alternatives argued.
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
| §105.8's kernel half | A dense typed column is what BLAS takes, and is zero-copy to NumPy, Polars, DuckDB and R |
| §105.7's aggregates | An arrangement over columns is what a maintained aggregate wants anyway |

So the sentence that replaces "we don't have NumPy" is:

> **Beck does not bridge to Python. It bridges to Arrow, and Python is one of the things standing on
> the other side.**

That is [`10`](10-decisions.md) D9's own framing — "our own front door onto the best engines in the
world" — applied to the ecosystem question instead of to the runtime.

## 105.11 What has no answer, and what this schedules

Per [`AGENTS.md`](../AGENTS.md): **"built", "runs" and "measured" are three different claims**, and
everything in this section is at zero.

| Gap | Class | Position |
|---|---|---|
| **Charting / `svg:`** (§105.9) | **S**, small, most users per unit of effort | **Scheduled** — [`08`](08-roadmap.md) §8.5.4's styling cluster item 1, and `DEFECTS.md::svg-namespace`. [`104`](104-styling-and-the-component-library.md) owns it; this document supplied the ranking, not the fix |
| **The data tier's algebra** (§105.7) | **F** — §99.7 lists five written-down items it closes | §8.5.4, in Lane B, and now the **largest item left** — it contends with nothing in Lane A, so it runs beside the macro interpreter's successors |
| **A columnar value and Arrow** (§105.10) | **F** | §8.5.4, after the algebra — an aggregate is what makes a column worth having |
| **The array notation and BLAS** (§105.8) | **S** for the kernels, **Lane A** for the notation | Phase 4, after Arrow. **The prerequisite has landed**: the macro interpreter is built ([`102`](102-the-macro-interpreter-report.md)), so the notation half is unblocked rather than queued — though a *typed* macro, which is what an array notation wants, is itself the first of §8.5.4's successor list |
| **Cloud SDKs** (boto3, and the same shape in three other ecosystems) | **S** | Phase 4, beside the managed-cloud path. `external store` and `net.out` already type it; what is missing is that nobody wants to hand-write S3's signature algorithm |
| **Image handling** (Pillow, sharp, ImageSharp) | **S** | Phase 4. A capability, so linking is the whole answer |
| **The Python sidecar** ([`09`](09-risks-and-open-questions.md) §9.2) | **S** | Phase 4, unchanged — with §105.2's placement restriction as a **diagnostic** rather than a discovery |
| **Regex** (`regex"…"`) | **S** | **Unblocked** — §2.5's typed literal macros are named in §8.5.4's successor list, and §104 confirms that half is free of Lane A |

The sidecar's diagnostic is worth naming as its own obligation. A `python_service` call inside a fold
is refused today by `place.rs:760`'s replay-purity check, with a message about fold purity that does
not mention the bridge. Someone reaching for the bridge in a view should be told *why* it cannot go
there and *where* it can. Per [`82`](82-the-edge-report.md) §82.10 the gate is written against the
shape of the gap: a program that calls a bridged service from inside a `durable` fold, refused with a
diagnostic naming the merge point as the alternative.

## 105.12 What this refuses

- **In-process CPython.** The GIL, plus a Python runtime inside images whose bit-for-bit
  reproducibility is the security story ([`06`](06-kubernetes-and-packaging.md) §6.2). Already
  refused in [`09`](09-risks-and-open-questions.md) §9.2; refused again with a second reason.
- **Compiling a Python subset.** §9.2 calls it a tar pit. Unchanged.
- **A pandas-shaped Beck package.** §105.7 — batch semantics on an incremental engine, and it would
  make a language problem look like a library problem.
- **Bridging a notation.** §105.5. A sidecar can host PyTorch; it cannot host pandas in any sense a
  pandas user would recognise, and shipping one that claims to would be the over-promise
  [`09`](09-risks-and-open-questions.md) §9.2 warns burns the audience it courts.
- **`pip install` compatibility as a goal.** [`01`](01-vision-and-premise.md) §1.7's "no `pip
  install` promises" stands. `import numpy` is easy because of twenty years of wheels against one
  ABI-stable interpreter — not a property of Python the language, and not one a new language
  acquires by wanting to.
- **Competing with SciPy and PyTorch.** §1.7 conceded these deliberately and the concession is
  sound. §105.1 is the correction to how widely it has been read.

## 105.13 What this document does not claim

- **Nothing here is built.** Not the algebra, not a column, not a chart, not the sidecar. The four
  code facts asserted — `place.rs:760`, `core.rs:790`, `html.rs`'s open vocabulary,
  `beck-patch.js:10` — were read from the tree on 2026-08-16 and are the only claims about the
  implementation this document makes.
- **§105.6 is a judgement, not a measurement**, and is labelled as one. §105.4's survey corroborates
  the rows it happens to cover; it does not cover charting, ORMs, testing, validation or CLI
  parsing, and those rows are argument alone.
- **The download data measures downloads**, and §105.4 argues it measures fan-in more than value. It
  is quoted to make that argument and is not evidence for any verdict in §105.6.
- **The survey is a year old and its section is gone.** The Stack Overflow 2025 technology page no
  longer carries a general library section, so 2024 is the latest reading available and the figures
  should be re-read against whatever replaces it. A one-year-stale 21.2% does not become 1%, so the
  §8.6 finding survives the staleness even though the exact numbers may not.
- **Stars are quoted only to discard them.** §105.4's star table is evidence about the instrument,
  not about the libraries: no verdict anywhere rests on a star count. The six repositories were
  chosen to test the instrument against the survey, not sampled to represent anything.
- **No "most discussed" metric is used.** The 2025 survey's tag section measures *emerging*
  technology (Gemini 29.2%, Pydantic 10.1%, `uv` 9.5%) and answers a different question than this
  document asks; Stack Overflow question volume now moves with LLM adoption rather than with library
  use, which makes it unusable for this purpose in exactly the years it would matter.
- **The notation/capability split is a design heuristic, not a taxonomy.** Real libraries are both:
  `requests` is a capability with a pleasant notation, and PyTorch is a capability whose autograd
  tape is a notation. The split is applied to the part that carries the value, and where a library
  is genuinely both, the notation half is the half that constrains the answer.
- **It does not price the sidecar.** No one has built one, so nothing here says what a call costs.
- **It does not own charting.** §105.9 supplies the ranking;
  [`104`](104-styling-and-the-component-library.md) owns the defect, the fix and the component
  question, and found the same `createElement` hole independently from the UI side.
- **It does not design the array notation.** §105.8 says broadcasting has to be in the language.
  What the surface syntax is, and whether broadcasting is a trait or a macro, is not decided here —
  and now that the macro interpreter is built, that is the only thing standing between the argument
  and an implementation.

## 105.14 What this corrects, elsewhere

| Document | Correction |
|---|---|
| [`09`](09-risks-and-open-questions.md) §9.2 | The sidecar recommendation stands and was **missing its most important property**: the effect row keeps it out of folds and views, so it answers the merge-point half of the ecosystem question and cannot answer the data-tier half. §9.2 now says so and points here |
| [`16`](16-packages-and-ecosystem.md) §16.8 | "Tarns extend the language; bridges rent from neighbours" is right and incomplete — the two largest categories are neither extended nor rented but **dissolved** or **absorbed into the language**. §16.8 now names four |
| [`01`](01-vision-and-premise.md) §1.5 item 7 | "Interop or die" is a true instinct pointed at the wrong noun. The libraries that most expand a language's utility are **notations**, and a notation cannot be called across a boundary at all — so most of the ecosystem question is about Beck's own means of combination rather than about interop |
| [`01`](01-vision-and-premise.md) §1.7 | The ML/numeric concession is sound for SciPy and PyTorch and has been read to cover arrays and dataframes, which are ordinary application vocabulary rather than a scientific speciality. §1.7 now says which is conceded |
| [`08`](08-roadmap.md) §8.5.4 | Three items acquire a position: the data tier's algebra (a Phase 4 bullet never in the ordered list), the columnar value, and the array notation. Charting acquired one too and then acquired a better one — [`104`](104-styling-and-the-component-library.md)'s styling cluster owns it, with the `DEFECTS.md` entry this document did not write |
| [`08`](08-roadmap.md) §8.6 | The ≥1% rule had always been scoped to cloud and infrastructure and the scoping was never argued. §8.6.2 applies it to libraries and gives all 39 entries of the survey's section a verdict |
