# 02 — Syntax: Python surface, Lisp core

> **Your question:** *"this lisp type syntax illustrates my idea in the best way, but Python has the
> biggest mass appeal. Could we make the language more like Python without losing any of its power?"*

**Yes.** And the reason is precise rather than hopeful: the power you are attached to does not come
from parentheses. It comes from **homoiconicity** — the property that a program's source is directly
representable as ordinary data structures of the language itself, so that programs can construct and
transform programs. That is a property of the **AST representation**, not of the **notation**.

## 2.1 The existence proof

Three production languages have non-Lisp surface syntax and lose nothing:

| Language | Surface | Homoiconic core | Macro power |
|---|---|---|---|
| **Elixir** | Ruby-ish, `do`-blocks | Every form is `{atom \| tuple, meta, args}` — an Elixir term | Full hygienic `quote`/`unquote`. `defmodule`, `if`, `defstruct`, Ecto's query DSL and Phoenix's router are *all* macros, not syntax |
| **Julia** | Familiar mathematical/imperative | `Expr(:call, :+, 1, 2)` — an ordinary Julia object | `:()`/`quote`, `@macro`, `esc()`, generated functions |
| **Nim** | **Indentation-based, Python-like** | `NimNode` tree | `macro`/`quote do:`, full compile-time AST rewriting and typed macros |

**Nim is the direct answer to your question**: it looks like Python, and it has a macro system
powerful enough to implement `async`/`await` as a library. Elixir is the answer for *ergonomics* —
its `do`-block convention is the key design move that makes an indentation-sensitive language
comfortable for macro authors, and we adopt it below.

Conversely, Python itself *fails* at this despite having `ast` in the standard library — not because
of its syntax, but because it lacks quoting, hygiene, and a compile-time expansion phase, and because
its AST is a separate object universe rather than plain values. Those are the three things to fix.

## 2.2 The design: two surfaces, one AST

```
   surface/py.beck ──┐                                        ┌── printer.py    ──▶ .beck
                     ├──▶  Reader  ──▶  Node (canonical AST) ─┤
 surface/sx.beck  ───┘        ▲                               └── printer.sexpr ──▶ .sx
                              │
                    both readers produce identical Node trees
```

`Node` is an ordinary Beck value:

```python
model Node:
    head: Sym | Lit             # what is being applied / a literal
    args: list[Node]
    meta: Meta                  # span, hygiene scope, doc, inferred type slot
```

Everything else is derived. Consequences:

- `beck fmt --surface sexpr orders.beck` emits the canonical Lisp form; `beck fmt --surface python
  orders.sx` emits the Python form; `beck ast` dumps the tree itself. Round-trip is lossless
  **modulo formatting** for the program, its `##` doc comments and its ordinary `#` comments — all
  of which ride in `meta`. The lexer still discards comments; they are collected from the source
  text by position in the same pass that collects documentation, which is what keeps a comment at
  column zero from closing an indented block. `beck fmt` therefore keeps what somebody wrote, and
  the LSP offers `textDocument/formatting` because of it.
- The reference manual can present semantics in S-expressions — where they are unambiguous and where
  your idea reads best — while the tutorial presents Python. Same language, no dialect split.
- Macros always manipulate `Node`, never text. So indentation-sensitivity is a *printing* concern,
  handled once, in the printer. This is the whole trick: **significant whitespace is only hard if
  your macros do string concatenation.** Ours cannot.

Recommendation: ship the Python surface as the default and the *only* one taught, and keep the
S-expression surface documented and supported (it is invaluable for macro debugging, for the spec,
and for generated code). Do not let two idiomatic communities form: `beck fmt` on commit normalises
to `.beck`.

### Side-by-side: the running example

Python surface (a clause of the canonical example's fold):

```python
def toggle(todos: Map[Id, Todo], e: Toggled) -> Map[Id, Todo]:
    return todos.update(e.id, lambda t: t.with(done=not t.done))
```

Canonical core, printed as S-expressions:

```clojure
(def toggle
  (params (: todos (Map Id Todo))
          (: e Toggled))
  (returns (Map Id Todo))
  (return (. todos update (. e id)
             (fn ((: t Todo))
               (. t with :done (not (. t done)))))))
```

Identical `Node` tree — and within notational distance of the original sketch's
`(update-at todos id (fn [t] (set t :done (not t.done))))`. `beck fmt` moves between the surfaces
mechanically.

## 2.3 The one thing Python is missing: block-passing calls

This is the crux. In Lisp, `(with-transaction (do-a) (do-b))` passes unevaluated code to a macro
trivially. In Python, `with transaction():` is a *fixed statement* — you cannot define a new one, and
`lambda` cannot hold statements. So we add a single, uniform rule:

> **Block rule.** A call written `f(args):` **in final position** — as a statement, after `return`,
> or on the right of a binding — followed by an indented block, desugars to
> `f(args, do=<block as quoted Node>)`. If the callee is a macro, it receives the AST. If it is a
> function, it receives a thunk.
>
> **What the block may hold depends on where the call is.** Written as a module item it holds
> declarations — which is what §2.4's `derive` is, a macro whose block is a `model`. Written
> anywhere else it holds statements, because a `model` inside a function body is not a thing this
> language has and reading one there would turn a mistake into a mystery.

The "in final position" clause is part of the rule, not a caveat on it, and Phase 1 found out why
([`19`](19-phase-1-report.md) §19.4 item 2): applied to *every* `:`, the rule reads `for t in todos:`
as a block-form call on `todos` and swallows the loop body, and does the same to `if ready:`. §2.7
records the same restriction as a mitigation for a different problem; it belongs here, because
without it the rule does not describe a language anyone can parse.

That one rule buys the entire Lisp special-form vocabulary with Python punctuation:

```python
atomically:                                  # user-defined macro, not built-in syntax:
    emit(OrderPlaced(...))                   # emit both events at one log position
    emit(StockReserved(...))                 # (an atomic multi-event append)

retry(times=3, backoff=exponential):
    charge(card, total)

ui:                                          # `ui` is a macro; children are AST, not values
    table:
        for o in rows:
            tr: o.id
```

Additional block forms, all sugar over the same rule:

| Surface | Desugars to |
|---|---|
| `f(x):` + block | `f(x, do=quote(block))` |
| `f(x): expr` (single line) | `f(x, do=quote(expr))` |
| `else:` / `catch e:` clauses after a block | extra keyword args: `else_=quote(...)`, `catch=quote(...)` |
| `@deco` before `def` | `deco(quote(def ...))` — a real AST transform |
| `q"..."` typed literal | `q_sigil(raw="...", span=...)` expanded at compile time |

`@decorator` deserves emphasis: Python programmers already know and love decorator syntax, but in
Python a decorator only sees a *function object*. In Beck, `@server`, `@memo`, `@component`,
`@derive(Eq, Json)` receive the **definition's AST** and may rewrite it arbitrarily. Familiar
notation, Lisp semantics. This is the highest-leverage familiarity/power trade in the design.

## 2.4 Macros

```python
macro unless(cond, do):
    return quote:
        if not $cond:
            $do

macro derive(*traits, do):
    ty = do.as_model()
    impls = [gen_impl(t, ty) for t in traits]     # ordinary Beck code, compile time
    return splice([do, *impls])
```

Design decisions:

- **`quote:` / `$expr`.** `quote` blocks build `Node`s; `$` unquotes, `$*` splices (Elixir's
  `unquote`/`unquote_splicing`, Julia's `$`, with Python punctuation). Nested quoting is supported and
  specified in terms of the core form `(quote depth node)`.
- **Hygiene by default.** Identifiers introduced inside a `quote` get a fresh hygiene scope in
  `Node.meta`; capture is possible but must be explicit (`inject(name)`), and is a lint warning
  outside `unsafe_macro`. Getting hygiene right at the `Node` level from day one is far cheaper than
  retrofitting it (see: Scheme's 20-year history here).
- **Phase separation.** Macro bodies run at compile time in the compiler's own Beck interpreter, with
  a *capability-restricted* environment: pure computation and reads of the declared module graph, no
  ambient filesystem or network. Non-negotiable — build reproducibility and the "compile once, deploy
  many" model depend on it, and it closes a real supply-chain hole that Rust `build.rs` and npm
  `postinstall` leave open.
- **Typed macros.** Two flavours, as in Nim: `macro` (untyped AST in, AST out, expands before type
  checking) and `typed macro` (receives the AST *with* inferred types attached — needed for
  `derive`, ORM-style query building, and exhaustiveness-aware codegen).
- **Expansion is incremental and cached.** Keyed by the macro's own content hash plus input `Node`
  hash, memoised in Salsa (§4.6). Macro-heavy code must not destroy IDE latency.

**Status, said plainly — this section is half description and half design, and the halves are
these.** Built: hygiene by sets of scopes from the first commit, `$x` unquotes and `$*xs` splices
expanded to a fixpoint, four independent resource bounds (`B0201` expansion depth, `B0213`
nesting, `B0214` production budget, `B0215` the interpreter's step budget), and **a macro body
that is ordinary Beck, evaluated at compile time** ([`102`](102-the-macro-interpreter-report.md)):
bindings, `if`, `for`, `while`, lambdas, calls to the module's own `def`s and to the pure part of
the prelude, `node_*` reflection over syntax, and `splice([…])` returning several definitions where
one was written. `$e` is an expression whose *value* is reflected into the template, so `$x` is the
caller's code and `$(n * 2)` is a literal. The environment is capability-restricted as this section
demands, and `macro_sandbox.rs` is what says so: a whitelist, the effectful primitives refused by
name (`B0207`), and an enumeration over the prelude that fails when a new one appears.

**And a macro can decorate a declaration**, which is what the `derive` sketch above is: a block
passed to a macro *in item position* holds declarations, a `quote:` holds them too, `$` unquotes
where a **type** and where a **field name** go, and a `do` at module level is flattened all the way
down — so a macro takes a `model`, reads its fields out of the syntax, and emits an `impl` that
names each one. `examples/derive.beck` is the program, and it closes the row
[`46`](46-standard-library-report.md) §46.16 and `prelude.rs` have both carried since the standard
library was written: turning a `model` into a `Json` is generated rather than written, with **no
reflection in the running program**. The `$`-in-a-type rule is what hygiene makes necessary rather
than convenient — a type name *written* in a template gets a fresh scope and refers to nothing, so
the generated `impl` has to unquote the caller's own name.

Not built: **the half that wants the checker's answers, and the half that wants a run time** —
typed macros, `derive`'s `.as_model()` sugar, `inject`/`unsafe_macro`, Salsa-memoised expansion,
nested quoting's `(quote depth node)`, and §2.5's typed literal macros. The `derive` sketch above is
written in two spellings the language does not have: a list comprehension (`for` inside `[…]` does
not parse, in a macro body or anywhere else — a `for` loop that appends is how that is written) and
`*traits`, since a parameter list has no rest form. `.as_model()` is a third: `node_args` reads the
declaration, which is the same information with more punctuation.

**And a macro crosses a module boundary**, so a library can ship one — which is what turns every
mechanism above into a facility. `lib/json.beck` is the first: `import json` and `derive_json:` over
a `model` generates its JSON encoder, closing [`46`](46-standard-library-report.md) §46.16's
`@derive` row. Two things about the crossing are worth knowing before using it:

- **A macro is published by a module's *source*, not by its interface.** A macro has no signature,
  so there is nothing for a `.becki` to carry; an import that resolves to an interface alone does
  not bring one, and `B0307`'s note says so where somebody will hit it.
- **The namespace is flat here as everywhere.** Two macros of one name cannot both be in scope,
  wherever they came from, and `B0200` — which has always refused a module that declared one twice
  — is what refuses that too. The crossing added no second rule about names.

Until this, expansion ran per module *before* any import was resolved, so a macro was usable in the
file that declared it and nowhere else. Nothing refused it — the name simply was not there — which
is why it went unwritten in this section for as long as it did.

The `ui:` block is still a
compiler-provided macro standing in for a user-written one (D22), and a `quote` that survives
expansion is still `B0332` — a `Node` is a compile-time value, not one a running program holds.
[`08`](08-roadmap.md) §8.5.4 carries what is left and
[`12`](12-standards-and-conformance.md) §12.10 what the interpreter cashed.

## 2.5 Typed literal macros (the DSL escape hatch)

Lisp reader macros let you change tokenisation arbitrarily. We deliberately do **not** allow that
(see §2.7), and instead confine foreign notation to delimited, typed literals — Elixir sigils / Rust
proc-macros in spirit:

```python
q  = sql"select * from orders where total > {floor}"   # parsed & checked at compile time
h  = html"<p>{escaped_name}</p>"                       # XSS-impossible by construction
k  = k8s_patch"""spec: {containers: [{name: app, ...}]}"""
re = regex"^\d{4}-\d{2}$"                              # compiled, groups typed
```

Each is a compile-time macro that parses its own body, reports errors *at the right source offsets
inside the literal*, and returns typed `Node`s. `sql"..."` returns a `Query[Order]` whose columns are
checked against the `store` declarations, so a typo is a compile error and interpolation is
parameter-bound (injection is unrepresentable, as in Ur/Web).

**Status: none of these exist** — not in the lexer, not in the expander. A typed literal macro is
a compile-time macro that parses its own body, so the whole section arrives with §2.4's
interpreter ([`08`](08-roadmap.md) §8.5.4's first item); the `sql`/`html` rows are the mechanism
the security suite already points at for injection and XSS.

## 2.6 Other surface decisions, and their reasons

| Decision | Choice | Reason |
|---|---|---|
| Indentation | Significant, spaces only, 4 | Mass appeal is the whole point; also forces the AST-printer discipline we want |
| Statements vs expressions | **Everything is an expression**; `if`/`for`/`match`/blocks all have values | Python's statement/expression split is its worst property for macros. `x = if c: 1 else: 2` must work |
| Call parens | Required for calls | Removes the Ruby/Nim ambiguity; keeps macro args unambiguous |
| Types | Mandatory on all *public* signatures, inferred inside bodies | Python devs read annotations as familiar; we get soundness. Never a `dict[str, Any]` culture |
| Mutability | Immutable by default, `var x` for mutable bindings | Required for the placement solver to move code between tiers safely |
| Nil | No `None` in the type system; `Option[T]` + `?.`/`or` sugar | Eliminates the single largest bug class; sugar keeps it Pythonic |
| Errors | `Result[T,E]` + `?` propagation + `try:`/`catch e:` sugar over it | Typed error rows compose with the effect rows in §3 |
| Concurrency | Structured concurrency (`spawn` inside a nursery block), async is *not* in the surface type | `async`/`await` colouring is a disaster across tier boundaries; the compiler inserts awaits |
| Pattern matching | `match` with exhaustiveness checking | Needed for `Result`, ADTs, and it is already in Python 3.10+ syntax |
| Operators | Fixed precedence table; user-defined operators allowed but only at existing precedence levels | Full precedence declarations make tooling and error recovery miserable for marginal gain |
| Modules | Explicit `import`, no wildcard, module = file, package = directory + `beck.toml` | Separate compilation depends on this (§1.6) |
| Naming | `snake_case` values, `PascalCase` types, `SCREAMING` consts, enforced by `beck fmt` | One community, one style, zero bikeshedding — and `snake_case` rather than kebab-case because the Python surface has infix `-` ([ADR 0011](adr/0011-identifiers-are-snake-case-in-the-python-surface.md)). The S-expression surface, which has no infix operators, uses kebab-case for its own forms |

## 2.7 What you genuinely give up (be honest about this)

Four real losses versus a parenthesised language. All four are, in my judgement, worth it:

1. **Arbitrary reader macros.** You cannot change tokenisation mid-file. Mitigation: typed literal
   macros (§2.5) cover ~95% of real uses (embedded SQL, HTML, regex, JSON, YAML, GraphQL).
2. **Uniformity of "everything looks like a call."** Python surface has `for`, `if`, `.`, operators,
   indentation — so a macro author must know that `a.b(c)` is `(. a b c)`. Mitigation: `beck fmt
   --sexpr` and a `beck ast <expr>` command; macro authors learn the core notation, which is exactly
   the notation your original sketch used. **Your Lisp syntax becomes the macro-author's dialect** —
   that is a feature, not a compromise.
3. **Some macro shapes get awkward.** Notably macros that want to introduce *new binding forms in
   arbitrary positions*, e.g. Lisp-style `let`-over-`lambda` chains. Mitigation: the block rule plus
   `with pattern = expr:` covers the common cases; the rest are expressible, just less pretty.
4. **A trailing-block ambiguity** when a call with a block is itself an argument to another call.
   Mitigation: a hard syntax rule — a block-form call may not appear as a non-final argument;
   `beck fmt` inserts an explicit `do=quote(...)` when it must.

Nothing here touches the *expressive* power (what programs are writable), only *notational*
convenience. There is no expressible Lisp program shape that becomes inexpressible.

## 2.8 Implementation notes

- **Lexer**: [`logos`](https://github.com/maciejhirsz/logos) for tokens; a hand-written layout
  algorithm producing explicit `INDENT`/`DEDENT`/`NEWLINE` tokens (Python's approach), with brackets
  suppressing layout so multi-line calls work.
- **Parser**: hand-written recursive descent + Pratt for expressions. **Not** a parser generator.
  Rationale: error messages and error *recovery* are the top-two UX properties of a new language, and
  they are exactly what generated parsers are worst at. Every serious modern language front end
  (rustc, Roslyn, TypeScript, Zig) is hand-written for this reason.
- **A separate [`tree-sitter`](https://tree-sitter.github.io/) grammar** for editors — deliberately
  duplicated, since editor grammars want error tolerance and speed rather than exactness. Keep them
  honest with a shared corpus test (`tests/corpus/*.beck` parsed by both, ASTs compared modulo
  fidelity). **Not built**: `beck lsp`'s semantic tokens serve editors today, and the grammar
  remains wanted for the editors LSP does not reach.
- **The S-expression reader is ~300 lines** and should exist from week one: it lets you write compiler
  tests against canonical ASTs without depending on the Python surface being finished, and it is how
  you'll dump intermediate state for the rest of the project's life.
- **Printer**: a Wadler/Prettier-style pretty-printer over `Node`, shared by `beck fmt`, macro
  expansion dumps, and error messages. Build it early; every later phase uses it.

## 2.9 The two syntax decisions, settled

Both were held open here as "expensive later" and are now taken, in
[`10`](10-decisions.md) D21 and D22. They did not resolve the way a single recommendation would
have: the first splits.

- **Effect and capability annotations are clauses in the signature** — `def f(x) -> T uses
  durable(orders)`, never `@uses(...)` — because an effect row is part of the *type*: it unifies, it
  is inferred, it is generalised over, and §3.6 publishes it. **Placement is a decorator** —
  `@on(server)` — because Phase 2 made placement *inferred*, so the annotation is an override handed
  to the solver rather than a fact about the definition. The measurement is the argument: one of 28
  corpus programs carries `@on(...)`, and it exists to test that pinning still works.
- **UI blocks are a `ui:` macro producing a typed DOM tree**, not a JSX-like literal syntax. The
  macro keeps the surface small — it is an ordinary call with a block under §2.3's rule, so it
  needs nothing the language does not already have — and is implementable by users for other
  targets (terminal UI, native). Its output is the Hiccup lineage the original sketch used —
  `[:main [:h1 "todos"] ...]` maps 1:1 onto the `ui:` block's `Node` tree, so the sketch's pages
  *are* these pages. What it forfeits is editor tooling for a bespoke literal, which D22 states
  rather than hides.
