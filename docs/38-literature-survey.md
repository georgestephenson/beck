# 38 — The research literature, August 2026

> **The question**: what does the academic literature — the classics Beck already stands on, and
> the 2023–2026 work published since the design documents were written — say about what this
> project needs next?
>
> **This is a dated survey, and it keeps its date.** Its verdicts are cashed elsewhere — §38.2's
> reader-frontier discipline in [`23`](23-incremental-views-report.md) §23.11, §38.4's error rows
> and scope-as-handler in [`27`](27-the-walls-come-down-report.md) and
> [`80`](80-structured-concurrency-report.md), §38.1's dictionaries-are-the-semantics in
> [`93`](93-the-native-backends-report.md) §93.10 — and each of those chapters says which half of the
> forecast held. Folding the surviving verdicts in here and deleting this would keep the conclusions
> and lose the reading that produced them.


This is a survey, not a charter change, in the sense [`35`](35-standards-landscape.md) established:
nothing below is adopted by being written here. Where [`35`](35-standards-landscape.md) surveyed
standards bodies, this document surveys research — against the unbuilt list
([`23`](23-incremental-views-report.md) §23.19, [`27`](27-the-walls-come-down-report.md) §27.10), the open
questions ([`09`](09-risks-and-open-questions.md) §9.6), and the design's own citations
([`00`](00-original-idea.md)'s provenance table), which date from 2022-era literature and had not
been rechecked since. Every citation below was confirmed against the published record (publisher
pages, arXiv listings, dblp) by web search in August 2026; anything that could not be confirmed is
either excluded or explicitly flagged, and engineering writeups are labelled as such rather than
passed off as papers. These are dated claims — a field, unlike a design decision, moves under us.

The verdicts reuse [`35`](35-standards-landscape.md)'s four words: **adopt** (take the technique
into a named piece of work), **borrow** (take a concept or a result, cite the source, build
nothing yet), **watch** (a dated pin with a named trigger), **decline** (with the reason stated).

## 38.1 Bounds and dictionary passing — the next feature, checked first

[`27`](27-the-walls-come-down-report.md) §27.1 says bounds are the single item everything else waits behind.
The literature turns out to have a near-complete instruction sheet for exactly that feature.

**The core is settled and old.** A bound is a Wadler–Blott qualified type (Wadler & Blott, *How to
make ad-hoc polymorphism less ad hoc*, POPL 1989): `def sort[T: Ord]` elaborates to an extra
dictionary parameter, and the call site resolves the unique impl and passes it. That is the same
"desugar into ordinary definitions" move [`27`](27-the-walls-come-down-report.md) §27.5 already made for
non-generic impls, extended with one hidden argument. **Adopt.** The design-space checklist to
refuse from is Peyton Jones, Jones & Meijer, *Type classes: an exploration of the design space*
(Haskell Workshop 1997) — most extensions are individually easy and jointly treacherous, and
Beck's current conservative core (single-parameter traits, global coherence, orphan rule) is that
paper's recommended starting point.

**Coherence is inherited, not re-proved.** Bottu, Xie, Marntirosian & Schrijvers (*Coherence of
type class resolution*, ICFP 2019) prove dictionary elaboration coherent — any two elaborations
contextually equivalent — including superclasses, *on the hypothesis of global instance
uniqueness*. Beck's orphan rule is that hypothesis. **Borrow** the theorem; keep the rule that
makes it apply. The 2025 cross-language survey (Racordon, Flesselle & Pham, *On the state of
coherence in the land of type classes*, ‹Programming› 2025, arXiv:2502.20546) finds Haskell, Rust
and Swift converged on near-unique instances plus a sanctioned escape hatch (newtype wrappers,
explicit witnesses); plan the desugaring so an escape hatch is possible later, and take
Winant & Devriese (*Coherent explicit dictionary application*, Haskell Symposium 2018) as the
proof that an explicit-dictionary form can coexist with coherence. That is also the answer to
§27.10's "a trait method cannot be passed as a value": a bare `show` elaborates from the in-scope
bound, and with no bound in scope it is an ambiguity error or an explicit application — the
consensus across Haskell, OCaml and Coq lineages.

**One resolver.** Rust's decade with two trait solvers giving subtly different answers — the
next-generation solver reached coherence checking in Rust 1.84 (2025) with full stabilisation a
2026 project goal; no peer-reviewed paper, the record is the Types Team posts and a-mir-formality
— is a warning with a positive form: specify resolution once, as proof search over impl clauses
(the chalk framing), and have type checking, coherence and desugaring all call it. **Adopt** as an
engineering rule. OCaml's parallel decade says the same from the other side: modular *implicits*
(White, Bour & Yallop, 2014) are still unmerged, and the team now lands modular *explicits* first
(Vivien & Rémy, OCaml Workshop 2024; ML Workshop 2025) — the explicit dictionary core is the
tractable part, implicit resolution an elaboration on top. Beck's desugared-impl design is already
on that path.

**Dictionaries are the semantics; monomorphisation is a backend choice.** Ellis, Zhu, Yoshida &
Song (*Generic Go to Go*, OOPSLA 2022) measured the three strategies on Go: dictionaries always
work (including polymorphic recursion and separate compilation), monomorphisation is faster and
bloats, hybrids trade between them; Go 1.18 ships a hybrid, and Swift's witness-table ABI (Pestov,
*Compiling Swift generics*, swift.org, rev. 2025) shows separate compilation of polymorphic code
against dictionaries in production. For a language whose one definition compiles into several
tiers, the split is exactly right: dictionary passing as the ground truth in the IR, each backend
free to monomorphise. **Adopt** the framing; the evaluator needs only the dictionary half.

**Diagnostics have a literature.** Report the unresolved *constraint* with the call-site chain
that demanded it — which bound, at which call, needed which instance — not the unification
failure it decays into (Heeren & Hage, *Type class directives*, PADL 2005; Zhang, Myers,
Vytiniotis & Peyton Jones, *Diagnosing type errors with class*, PLDI 2015). `B0386` already
distinguishes "no bounds" from "no impl" ([`27`](27-the-walls-come-down-report.md) §27.10); the bounds feature
should keep that discipline and add provenance. **Adopt.**

**Two Beck-specific flags before the syntax freezes.** First, the trait's effect row: Beck holds
every impl to one row declared on the trait ([`27`](27-the-walls-come-down-report.md) §27.5). Lutze & Madsen
(*Associated effects*, PLDI 2024, in Flix) argue each impl should *instantiate* an effect
component of its own — a partial-function impl adds an error effect, a stateful impl a heap
effect — with the caller's row picking it up through the bound. Refactoring Flix's stdlib needed
this in 11 classes. Beck's single bound is the restrictive special case; the moment a
`Backend`-style trait wants impls with different rows, associated effects are the shape.
**Borrow now, decide before bounds ship** — this bears directly on
[`09`](09-risks-and-open-questions.md) §9.6 item 1's granularity question. Second, tier crossings:
a dictionary resolved on one tier and used on another is cross-stage persistence, and Xie,
Pickering, Löh, Wu, Yallop & Wang (*Staging with class*, POPL 2022) show naive persistence of
dictionaries across stages is unsound — the staged-constraint discipline is the fix. Beck's
splitter must treat "which tier holds this dictionary" as a checked question, not an accident.
**Borrow.**

**Deferred, with names.** Multi-parameter traits without associated types are a known trap (Jones,
*Type classes with functional dependencies*, ESOP 2000); associated types as an extra type
component of the dictionary (Chakravarty, Keller, Peyton Jones & Marlow, POPL/ICFP 2005) compose
with the elaboration and should come first — **watch**, trigger: the first trait that wants two
types. Supertraits are cheap in the dictionary (a field) and expensive in the solver (diamonds);
Lean's tabled resolution (Selsam, Ullrich & de Moura, *Tabled typeclass resolution*, 2020) is the
known cure — **watch**, trigger: supertraits. `@derive` has two published shapes — a
compiler-derived structural impl with libraries deriving the rest (Magalhães et al., *A generic
deriving mechanism*, Haskell 2010) or deriving-via's named impl-patterns (Blöndal, Löh & Scott,
Haskell 2018) — **borrow**, decision deferred to the macro work [`27`](27-the-walls-come-down-report.md) §27.10
already assigns it to. Zig-style comptime duck typing: **decline** — it surrenders
definition-site checking, which is the entire point of bounds. Scala 3's scoped givens:
**decline** for the same reason [`27`](27-the-walls-come-down-report.md) chose coherence — the 2025 survey
places Scala alone on that branch.

## 38.2 The incremental view engine: fusion, lifecycle, and the page as deltas

The engine [`23`](23-incremental-views-report.md) built by hand now has a formal twin. **DBSP**
(Budiu, Chajed, McSherry, Ryzhyk & Tannen, VLDB 2023 best paper; journal version VLDBJ 2025; SIGMOD
Research Highlight 2024) gives a small algebra over Z-sets — weighted collections with
differentiation, integration, delay — and a *mechanical* incrementalization theorem covering
joins, aggregation, and recursion, machine-checked in Lean (Chajed's
`database-stream-processing-theory`) and carried to production in Feldera. Beck's differential
suite tests per program exactly what DBSP proves per operator. **Borrow** the vocabulary: stating
which fragment of the plan language the recompute oracle covers in Z-set terms turns "the oracle
passed" into a claim with edges. The Lean formalization marks the ceiling a future verified claim
would aim at — **watch**.

**Query fusion has a substrate waiting.** The unbuilt fusion pass
([`23`](23-incremental-views-report.md) §23.19) is, per the literature, an equality-saturation
job: egg (Willsey et al., POPL 2021) and egglog (Zhang et al., PLDI 2023) make rewrite-based
optimization practical without phase-ordering problems; Laddad et al. (*Optimizing Stateful
Dataflow with Local Rewrites*, EGRAPHS @ PLDI 2023) demonstrate it on stateful streaming plans
specifically, and Chu et al. (*Optimizing Distributed Protocols with Query Rewrites*, SIGMOD
2024) show the same machinery subsuming hand-done distributed-systems optimization. The hard part
is not the e-graph but proving each rewrite preserves incremental semantics — which is what
DBSP's algebra is *for*. **Adopt** the shape when fusion is built: small local rewrites, each
sound against the change semantics, extracted by the cost model
[`20`](20-phase-2-report.md) already has.

**The arrangement lifecycle questions have published answers.** §23.19's "the shared dataflow is
never released" and "the history is a constant, not a policy" are both answered by the
reader-frontier discipline of **Shared Arrangements** (McSherry, Lattuada, Schwarzkopf & Roscoe,
VLDB 2020): each subscriber holds a frontier; the trace is compactable up to the minimum
subscriber frontier and droppable when the reader set is empty. Beck's 64-version constant is a
placeholder for that policy, and the paper is the direct ancestor of
[`23`](23-incremental-views-report.md)'s design, so the fit is not speculative. **Adopt** —
this is the cheapest item in this survey: the engine already has versions and subscribers; it
lacks only the rule connecting them. For subscribers that outrun any retained history, **Noria**
(Gjengset et al., OSDI 2018) is the model: partially-stateful dataflow where evicted state refills
on demand via upqueries — **borrow**, since [`23`](23-incremental-views-report.md) §23.18's
rebuild path is the degenerate form already.

**The property the engine must state.** Jamie Brandon's *Internal consistency in streaming
systems* (2021, technical essay — the reference point the community cites) names what
differential-lineage systems guarantee and Flink-lineage systems do not: every output corresponds
to exactly one prefix of the input. Beck's engine has this by construction — a diffed page
corresponds to one version of the fold — and the recompute oracle checks it; the essay supplies
the name and the failure mode to test against. **Borrow.** Flo (Laddad et al., POPL 2025) is the
first umbrella formalism over Flink, LVars and DBSP — deterministic progressive streaming with
bounded/unbounded distinguished by types — and the frame in which "everything downstream of the
merge point is deterministic" could be positioned formally. **Watch.** Watermarks (Akidau et al.,
PVLDB 2021) answer [`09`](09-risks-and-open-questions.md) §9.6 item 3's clock question the day
ingestion becomes multi-source; today's single ordered log makes them unnecessary. **Watch.**

**SQL read models and pgwire are a compatibility layer, not a rearchitecture.** Materialize
(engineering record, no confirmed paper) and RisingWave (Wu et al., 2022) both expose
arrangement-based engines over pgwire; S-QUERY (Verheijde et al., ICDE 2022) frames the design
questions as isolation level and snapshot choice — and Beck's versioned change history *is* the
snapshots. pg_ivm's trigger-based immediate maintenance is the warning: it couples view cost to
write latency, which Beck's asynchronous dataflow avoids by construction. **Borrow** the framing;
the bullet stays unbuilt but stops being unshaped. *One SQL to Rule Them All* (Begoli et al.,
SIGMOD 2019) is the vocabulary bridge — a Beck signal is a time-varying relation — when read
models arrive.

**The page as deltas is the genuinely open one.** [`23`](23-incremental-views-report.md) §23.8's
assembled-and-diffed page has no published system to copy: TreeToaster (Balakrishnan et al.,
SIGMOD 2021) shows tree-native incremental structures beating relational encodings for tree-shaped
state, Adapton (Hammer et al., PLDI 2014) legitimises demand-driven maintenance, and the
incremental λ-calculus (Cai, Giarrusso, Rendel & Ostermann, PLDI 2014) gives derivatives of pure
functions — Beck's `view` is one — but no system streams UI-tree deltas end to end. A change
algebra over the page type is a research-shaped opening, not a catalogue item. Flagged but
unverified in this pass: DeCo (arXiv:2602.20866, 2026), incrementality for generic algebraic data
types, which if it holds up is the missing theory piece. **Watch**, and say plainly: the 3–5×
constant factor stands until someone does research, possibly us.

## 38.3 Slicing and placement: the choreographic turn

The multitier field Beck grew from (surveyed in Weisenburger, Wirth & Salvaneschi, ACM CSUR 2020
— still the canonical map, with Beck at its extreme no-annotations end) did not produce a new
tierless-web wave. Its energy went to **choreographic programming**, and that is where the
slicer's missing theory now lives. A choreography makes communication explicit and *projects*
per-tier programs (endpoint projection); Beck makes code primary and the slicer discovers the
communication — the same compilation problem from opposite ends, and the choreographic side has
spent 2022–2026 mechanising it: Pirouette (Hirsch & Garg, POPL 2022, Coq), Kalas (Pohjola et al.,
ITP 2022, HOL4 down to CakeML), a full formal theory (Cruz-Filipe, Montesi & Peressotti, JAR
2023), a ten-rule core (ICFP 2025), and location-set polymorphism with mechanised deadlock freedom
(λQC, OOPSLA 2025). Three results transfer directly. **Knowledge of choice**: when a server-side
conditional changes what the client must do, projection theory says exactly which selection
messages must cross the boundary — a correctness criterion the slicer currently satisfies
implicitly. **Multiply-located values** (MultiChor 2024; Bates et al., PLDI 2025): a value typed
as living at a *set* of tiers, without redundant traffic — precisely Beck's unplaced pure code
compiled twice. **Certified projection** is the proof shape for "the sliced program means what the
source means": a bisimulation between signal-graph semantics and the projected tiers. **Borrow**
all three; the trigger to **adopt** (an EPP-style correctness statement for the slicer) is the
first slicer bug that the differential suite finds late — [`23`](23-incremental-views-report.md)
§23.2's silent mis-slice was one already.

Two adjacent results give Beck's existing claims their citations. The placement pipeline —
cost-model min-cut with secrecy as a hard constraint — is the Coign (OSDI 1999) → J-Orchestra
(ECOOP 2002) → Swift (Chong et al., SOSP 2007) → Viaduct (Acay et al., PLDI 2021) lineage; Swift
specifically proved a compiler split can guarantee confidential data stays server-side, so
`secret[T]` ([`20`](20-phase-2-report.md) §20.3) has twenty years of precedent, with Cocoon
(OOPSLA 2024) as the modern zero-cost form.

**This lineage is entirely static, and the gap that leaves is a second literature this section did
not have.** Every system above partitions once, at compile time; Swift's own paper has no runtime
re-evaluation. The systems that *do* re-place from measurement are a different tradition —
MAUI (MobiSys 2010), which decides at run time which methods to offload with an LP solver over
measured connectivity; CloneCloud (EuroSys 2011), which pairs static analysis with dynamic profiling
and migrates a live thread; Wishbone (NSDI 2009), whose unit is a **dataflow operator** and whose
ILP minimises bandwidth against CPU; and Emerald (Jul, Levy, Hutchinson & Black, TOCS 6(1), 1988) as
the fine-grained mobility ancestor of all of them. None carries a static legality proof: they move
what fits, and safety is the programmer's. **Beck is the only design in view that has both halves**,
and [`100`](100-placement-at-runtime.md) is where the conjunction is worked out — legality static,
choice possibly dynamic, with deterministic replay making a cutover a comparison rather than a leap.
**Watch** rather than adopt: the offloading results are fifteen years old and their metric is
handset energy. And effect-driven tierless design is where the
survivors converged — Links is alive and grew a WasmFX backend in 2025. The standing objection is
Eliom's (Radanne, Vouillon & Balat, APLAS 2016): explicit placement buys predictability and
separate compilation. Beck's answer must be legibility — `beck explain placement` already prints
the solve; keep that the first-class artifact. Beck's enumerated tier crossings are morally a
generated multiparty session protocol; emitting an MPST global type per sliced program would
import deadlock-freedom results wholesale (**watch** — and note ECOOP 2025's mechanised
subject-reduction paper found published pen-and-paper MPST proofs wrong, so lean only on the
mechanised line). ML5's modal reading (Murphy, Crary & Harper, 2007) remains the declarative
soundness story an inferred placement can be checked against: `secret[T]` is a value at a world
the browser cannot inhabit. **Borrow.**

One gap runs the other way. [`30`](30-bounded-contexts-and-microservices.md)'s sagas-as-only-write-path
has no landmark formal treatment to cite — the formal work clusters around durable-execution
replay semantics (Burckhardt et al., 2021) — so compiler-checked saga-only writes is a claim Beck
will have to prove itself, and the design doc should say "no published equivalent found" rather
than implying support.

## 38.4 Errors and structured concurrency: labels and handlers, not mechanisms

Both unbuilt bullets ([`23`](23-incremental-views-report.md) §23.19) land the same way in the
literature: **do not add mechanisms — add row labels and handlers.**

**`Result`/error rows.** Koka's `exn` is the model: an error is a row label, a signature without
it provably cannot fail, and a handler converts the row entry into a value — `Result` is the
*reified* form a handler produces, not a parallel mechanism (Leijen, MSFP 2014; POPL 2017; Koka
remains alive, v3.1.3 2025, with two distinguished papers at ICFP/PLDI 2025 built on it). The
empirical side is unusually consistent: Rust measurements show `Result` propagation near-zero-cost
while panic paths inflate binaries (EuroSec 2026), and the TSE 2024 study of real Rust failures
finds them pooling in panics — the *untyped* escape hatch is where failures live. So: typed
channel cheap and default, panic outside the row as a genuinely unrecoverable trap. Koka's
community also supplies the ergonomic warning — five-and-six-label rows are common — so row
aliases for common bundles belong in the design from day one, which is also
[`02`](02-syntax.md) §2.9's effect-clause syntax decision coming due. **Adopt** the shape. The
honest gap: no controlled user study of error-handling ergonomics exists; that half stays
judgement.

**Structured concurrency.** The cross-language consensus (Trio's nurseries, 2018; Kotlin
coroutines, Onward! 2021; Java's JEP 505 — still in preview in JDK 25/26, which says the API
shape is genuinely hard) is: a scope owns its children, and errors and cancellation join at the
scope. The effect-system literature says how Beck should express that: `spawn`/`await` as effect
operations, the scope as their handler (Leijen, *Structured asynchrony with algebraic effects*,
TyDe 2017; schedulers-as-handlers in OCaml's Eio, 1.0 in 2024) — at which point derived mocks,
slicing and placement apply to concurrency with no new machinery, the same "desugar, don't extend
the IR" trick [`27`](27-the-walls-come-down-report.md) §27.1 item 3 names. Cancellation is the error row
crossing the scope. **Adopt** the shape when the bullet is built.

Three supporting results. Handlers must be **lexically scoped**: dynamically scoped handler search
breaks abstraction (Zhang & Myers, POPL 2019), is now compilable at zero overhead (OOPSLA 2025) —
and in a language where effects decide placement, accidental interception would mean accidental
*re-placement*, so this is load-bearing for Beck specifically. **Adopt.** Generalized evidence
passing (Xie & Leijen, ICFP 2021) — evidence vectors are runtime effect rows — is the canonical
route from the evaluator to a compiled backend behind the seam; WasmFX (OOPSLA 2023) is that
technique's Wasm form, W3C stage 2 and engine-partial, so **watch**, don't depend. And the
merge-point discipline has formal ancestors worth citing: LVars' freeze (POPL 2014) — one
controlled observation point, nondeterminism-as-error made deterministic — and Lingua Franca
(TECS 2021, TACO 2023) as the running evidence that deterministic-by-default with syntactically
visible nondeterminism performs; making `merge_clients()` a distinguished *effect* would put the
one impure place in the row where everything else already is. **Borrow**, pending a design pass.
Modal effect types (OOPSLA 2025; POPL 2026) prove rows and capabilities inter-translatable — the
row choice is not a dead end, and Scala's capture checking being still unstable in 2026 confirms
rows-from-day-one was right. One publishable gap the agents' sweep found on our side of the
ledger: nothing derives OS/RBAC/NetworkPolicy from a *row-polymorphic effect signature* — Wyvern
(ECOOP 2017) does authority-from-effects statically, but `Tier::discharges` feeding derived
least-privilege manifests ([`06`](06-kubernetes-and-packaging.md)) has no published equivalent.

## 38.5 The merge point: what the collaboration literature converged on

The design's oldest admission — "the moment two users edit the same text field you need CRDTs or
operational transforms, and no type system absolves you"
([`00`](00-original-idea.md)) — reads differently in 2026, because the field converged on Beck's
architecture from three directions at once. **Eg-walker** (Gentle & Kleppmann, EuroSys 2025, best
artifact) shows optimal collaborative text editing is *deterministic replay of a causal event
graph* — OT and CRDTs are projections of it, and replay beats CRDTs on memory and OT on divergent
merges. The commercial sync-engine wave landed on the same spine: ElectricSQL pivoted off
CRDT-heavy merge to server-centric sync (2024), Rocicorp's Zero 1.0 (2026) ships
server-authoritative mutators with optimistic client application, and LiveStore — descended from
Riffle (Litt, Schiefer, Schickling & Jackson, UIST 2023) — is client-side event sourcing: a typed
mutation log deterministically folded into SQLite, the closest running relative of the sketch and
worth reading line by line. **Keep the server-ordered log and client rebase; it is the attractor,
not a compromise.**

What should grow is what the fold may do *at* the merge. The MRDT lineage (Kaki et al., OOPSLA
2019 → certified in F*, PLDI 2022 → automatic verification, PACMPL 2025) derives three-way merge
functions over ordinary functional types — the most Beck-shaped mergeable-types theory — and
VeriFx (De Porre, Ferreira & Gonzalez Boix, ECOOP 2023) SMT-verifies convergence of user-written
merge functions automatically (51 CRDTs verified), while Katara (Laddad et al., OOPSLA 2022)
synthesises them from sequential types. Together they make a `mergeable` type qualifier —
per-field merge functions the compiler *verifies* for convergence — a catalogue item rather than
a research project. **Watch**, trigger: the first collaborative-text or offline demand on a Beck
program. Supporting results to take with it: merge semantics is a specifiable property, not
folklore (Fugue's maximal non-interleaving, IEEE TPDS 2025); *move* does not fall out of
insert/delete (Kleppmann's move-operation papers, PaPoC 2020/2024); undo is a typed command with
specified semantics, not log truncation (Kleppmann, PaPoC 2024). LoRe (Haas et al., ECOOP 2023,
TOPLAS 2024) is Beck's nearest academic neighbour — reactive local-first language, invariants in,
coordination points out — the principled generalisation of one hand-declared merge point.

Two cheap actions and one named hole. Cheap: hash-chain the event log now — Kleppmann's
PaPoC 2022 result gets Byzantine-tolerant convergence almost free from a hash-DAG, and it keeps
Keyhive-style concurrent access control reachable without committing to it (**borrow**). Cheap:
the optimistic-UI reconciliation invariant has a formal statement to adopt — client state after
rebase ≡ the fold of the authoritative log — with NSDI 2009's client-speculation work supplying
the piece the design has not articulated: *dependent* speculative commands (a command predicated
on a speculated result) need predicated-write semantics. The hole: per-field consistency choice as
types (MixT, PLDI 2018; ConSysT) exists and composes with effect typing, but adopting it is a
[`10`](10-decisions.md)-grade decision, not a survey's.

## 38.6 Testing, migrations, and derived infrastructure: ratified, and twice novel

Four of the project's standing claims now have their citations. The differential suite —
evaluator against recompute oracle, [`13`](13-testing.md) — is the ShardStore pattern (Bornholt
et al., SOSP 2021): an executable reference model checked by property testing, kept alive in CI,
shown there to beat one-shot proofs for evolving systems. Determinism-by-construction is what
FoundationDB (SIGMOD 2021) retrofitted with an actor DSL, TigerBeetle engineered with VOPR, and
Antithesis sells via a deterministic hypervisor ($105M Series A, Dec 2025; no papers, and
[`13`](13-testing.md) should keep citing it as industry, not literature) — Beck gets from
semantics what they build machinery for. "Incremental = recompute" is provable, not just testable
(DBSP's Lean development, §38.2). And MongoDB's trace-checking retrospective (VLDB 2020; 2024
follow-up) shows the abstraction gap that killed spec-vs-implementation checking — a gap Beck
does not have, because the spec *is* the program. State these as citations, not analogies.

The highest-value unadopted techniques, in order. **Coverage-guided property generation**
(FuzzChick, OOPSLA 2019; the ICSE 2024 interview study naming generators as PBT's bottleneck):
the evaluator already observes execution, so feeding coverage — or event-shape/merge-order
coverage — back into `property` generation is the natural upgrade to
[`22`](22-phase-3-report.md)'s one type-directed generator. **Adopt**, trigger: the generator
work `test --update` will force anyway. **Tyche-style reporting** (UIST 2024): show what a
property run actually exercised, answering the distrust the ICSE study documents — fits
[`23`](23-incremental-views-report.md) §23.10 item 4's complaint that Beck exports numbers and
gates nothing. **Borrow.** **EMI-style metamorphic testing** (Le, Afshari & Su, PLDI 2014) for
the slicer and future optimiser: mutate unexecuted code, behaviour must not change — "slicing
must not change observable behaviour" as a generated test family, cheaper than a second backend.
**Adopt**, trigger: the fusion pass. Elle (PVLDB 2021) gets cheaper for Beck than for its targets
— version order is free in an event-sourced fold — **borrow** when the scaling ladder
([`15`](15-scale-and-distribution.md)) is built.

Migrations: the empirical literature (Overeem et al., JSS 2021 — five tactics from 19
event-sourced systems; PRISM's finding that schemas grow ~217% in 48 months) says schema change
is constant and teams reinvent upcasting ad hoc; Beck's deploy-demanded `migrate` is that
literature's recommendation made compulsory. Two things to take: Lamdera Evergreen's UX —
generate the migration skeleton from the type diff, demand hand-written code only where the diff
is ambiguous (**adopt**, when [`06`](06-kubernetes-and-packaging.md)'s deploy path is built) —
and lens round-trip laws (Boomerang, POPL 2008; edit lenses, POPL 2012) as *properties to test* on
migration/downgrade pairs (**borrow**). No verified-migration system was found: Beck
property-testing migrations over generated old-states would exceed the published record.

Infrastructure: the seven-rung ladder ([`21`](21-tests-in-beck-and-proof.md) §21.4) has no
published equivalent — IaC correctness work is policy scanning, which the ladder adopts as one
rung (§21.4 rung 6) rather than as the whole story — but the literature supplies
three rungs ready-made: Acto's state-oracle testing (SOSP 2023), Sieve's
perturb-the-controller's-view differential (OSDI 2022), and Anvil's "eventually stable
reconciliation" (OSDI 2024, best paper) as the correct top-rung *specification* for what derived
manifests must guarantee. **Borrow** all three as rung definitions; claim the ladder itself as
"no published equivalent found."

## 38.7 Backends, memory, and the standard library

The route [`07`](07-dependencies.md) sketched is confirmed and sharpened. **Memory**: Perceus
(Reinking, Xie, de Moura & Leijen, PLDI 2021) with frame-limited reuse (ICFP 2022 — the current
Koka baseline) and Lean-style borrow inference (Ullrich & de Moura, IFL 2019 — the single biggest
RC-traffic reducer) is the state of the art for strict pure languages; immutability means no
cycles, so no tracing collector. Implement it as an IR pass so every backend inherits it. FP²
(ICFP 2023; PLDI 2024) shows reuse-conscious code reaching imperative speed — which makes the
standard library a *client* of this decision: write it in the functional-but-in-place style so
reuse analysis fires. **Adopt** (it was already the plan; now it has its papers and its ordering).
Tail-recursion-modulo-cons (Leijen & Lorenzen, POPL 2023; mechanised for OCaml, POPL 2025) is the
principled answer to [`27`](27-the-walls-come-down-report.md)'s depth-bounded non-tail recursion for the
common list/tree builders — **adopt**, trigger: native codegen, where it is a real transformation
rather than an evaluator contortion. The measured 13% (§27.8) is an interpreter artifact, not
destiny: in native code intra-SCC tail calls are jumps and cross-function ones are `musttail` /
Cranelift's `tail` convention.

**Backends**: Cranelift generates code ~2% behind V8 and ~14% behind LLVM at roughly 10× the
compile speed, and is now the only production instruction selector with formal verification
behind it (Crocus, ASPLOS 2024; Arrival, OOPSLA 2025) — which flips [`07`](07-dependencies.md)'s
framing: Cranelift is not just the fast tier, it is the *audited* one. The validated pattern is
two codegen tiers over one IR with the reference interpreter kept as oracle — exactly the
`Backend` seam's shape, so the evaluator stays as tier zero and differential anchor, never
deleted. CPython's copy-and-patch JIT saga (a claimed 15% shrinking to ~5% once the baseline was
fixed) is the measurement-methodology cautionary tale that vindicates §27.8's
measured-and-not-recovered discipline. **Client tier**: Wasm 3.0 shipped (Sept 2025) with
guaranteed tail calls and WasmGC baseline across all major browsers — [`35`](35-standards-landscape.md)'s
dated pin has resolved in Beck's favour — so Mode B should target WasmGC + tail calls on the
wasm_of_ocaml model rather than shipping an RC runtime in linear memory, accepting that the
browser tier's memory model then differs from the native Perceus tier. **Adopt** as the Mode B
plan of record; WasmFX stays **watch** (§38.4).

**Standard library**: RRB vectors (Stucki et al., ICFP 2015) and CHAMP maps/sets (Steindorfer &
Vinju, OOPSLA 2015) are the settled choices — CHAMP's canonical form makes structural equality
cheap, which the view engine's diffing will feel — implemented FBIP-style so unique-owner updates
go in place (the immer, ICFP 2017 / Roc synergy: RC host + persistent structures + transients).
That also retires §27.10's standing "a `list[T]` is O(n) to take apart" the honest way: not by
optimising cons, but by giving the library a vector. Gleam's discipline — stdlib semantics
identical across both targets — is the portability constraint Beck's multitier stdlib inherits
verbatim. **Adopt** when the stdlib bullet opens. Nested patterns: the canonical route is the
Maranget pair — decision-tree compilation (ML 2008) and usefulness-based exhaustiveness (JFP
2007) off one pattern matrix — replacing [`27`](27-the-walls-come-down-report.md)'s
one-level machinery with the standard one; Lower Your Guards (ICFP 2020) only if guards arrive.
**Adopt**, trigger: the second level of nesting a program actually needs. Incremental
compilation, when it comes, is Salsa-shaped query memoization with early cutoff (Build Systems à
la Carte, ICFP 2018, names it precisely) — noting the tension the literature has not resolved:
whole-program monomorphisation fights incrementality, which is one more reason dictionaries stay
the IR's ground truth (§38.1). Roc's 2025–26 Rust-to-Zig rewrite is context worth having (35 ms
rebuilds; two memory-safety bugs in 18 months), but its lesson for Beck is about compiler
iteration speed as a budgeted cost, not about the host language decision, which stands.

## 38.8 The verdicts, in one place

| Area | Adopt | Borrow | Watch (trigger) | Decline |
|---|---|---|---|---|
| Bounds (§38.1) | Wadler–Blott elaboration; one resolver; dictionaries-as-IR-truth; constraint-provenance diagnostics | Bottu coherence via orphan rule; staged constraints for tier-crossing dictionaries; associated effects; deriving designs | Associated types (first two-type trait); tabled resolution (supertraits) | Zig comptime; Scala scoped givens |
| Views (§38.2) | Reader-frontier arrangement lifecycle; e-graph fusion when built | DBSP/Z-set vocabulary; Noria partial state; internal consistency as named property; pgwire framing | Lean-verified operator claims; Flo; watermarks (multi-source); tree-delta theory for the page | — |
| Placement (§38.3) | — | Knowledge-of-choice; multiply-located values; certified-EPP proof shape; ML5 modal reading; Swift/Viaduct as precedent | MPST global type per slice (mechanised line only); EPP correctness statement (next slicer bug) | — |
| Effects (§38.4) | Errors as row label + `Result` reified; row aliases; lexical handlers; scope-as-handler concurrency | Merge point as marked effect; evidence passing for compiled backends | WasmFX (W3C stage 3+) | New error/concurrency mechanisms outside the row |
| Merge (§38.5) | Server-ordered log + rebase stays the spine | Hash-chained log; rebase invariant + predicated writes; undo-as-typed-command | `mergeable` verified merge functions (first collaborative demand) | Materialised-CRDT-in-state designs |
| Testing (§38.6) | Coverage-guided generation; EMI for the slicer; Evergreen-style migration skeletons | Tyche reporting; lens laws on migrations; Acto/Sieve/Anvil as rung definitions; Elle | VerIso-style verified isolation | — |
| Backends (§38.7) | Perceus+reuse+borrows as IR pass; TRMC at codegen; WasmGC+tail-calls Mode B; RRB+CHAMP FBIP stdlib; Maranget patterns | Cranelift-as-audited-tier framing; Salsa taxonomy | Component model/WASI 1.0 (server packaging) | Trampolines/full CPS for tail calls; finger trees |

## 38.9 What this survey is not

It adopts nothing — every **adopt** above is a verdict awaiting its named piece of work, and the
ones that touch design decisions ([`10`](10-decisions.md)) need D-numbers before they are real.
It contains no measurements; where a number appears it is the cited work's, not ours. Its
confirmations are search-level — titles, venues, authors checked against publisher pages in
August 2026 — not full-text review; five surfaced items are named in §38.2 and the agents' notes
as title-confirmed only, and nothing rests on them. And it found two places where the project is
ahead of the record rather than behind it — the manifest ladder and compiler-checked saga-only
writes — which is not a compliment but an obligation: a claim with no literature behind it has
only our own harnesses under it, and [`21`](21-tests-in-beck-and-proof.md)'s ladder is what
holding that weight looks like.
