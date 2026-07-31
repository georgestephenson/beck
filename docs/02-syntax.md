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

- `beck fmt --sexpr orders.beck` emits the canonical Lisp form. `beck fmt --py orders.sx` emits the
  Python form. Round-trip is lossless **modulo formatting** (comments and spans ride in `meta`).
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
| Naming | `snake_case` values, `PascalCase` types, `SCREAMING` consts, enforced by `beck fmt` | One community, one style, zero bikeshedding |

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
  fidelity).
- **The S-expression reader is ~300 lines** and should exist from week one: it lets you write compiler
  tests against canonical ASTs without depending on the Python surface being finished, and it is how
  you'll dump intermediate state for the rest of the project's life.
- **Printer**: a Wadler/Prettier-style pretty-printer over `Node`, shared by `beck fmt`, macro
  expansion dumps, and error messages. Build it early; every later phase uses it.

## 2.9 Concrete syntax risk to settle now

Two open decisions that are expensive later. My recommendation in bold; see
[`09-risks-and-open-questions.md`](09-risks-and-open-questions.md) §9.6 for the full trade-off.

- Effect/placement annotations: **`requires`/`uses` clauses in the signature** (`def f(x) -> T uses
  durable(orders)`) vs. decorators (`@uses(...)`). Signature clauses read better and are part of the
  published module interface, which §3.6 requires.
- UI blocks: **a `ui:` macro producing a typed DOM tree** vs. JSX-like literal syntax. The macro
  keeps the surface small and is implementable by users for other targets (terminal UI, native).
  Its output is the Hiccup lineage the original sketch used — `[:main [:h1 "todos"] ...]` maps 1:1
  onto the `ui:` block's `Node` tree, so the sketch's pages *are* these pages.
