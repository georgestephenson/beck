# 0011 — Identifiers are `snake_case` in the Python surface and kebab-case in the S-expression one

**Context.** [`02`](../02-syntax.md) §2.6 records the naming convention in a table row —
"`snake_case` values, `PascalCase` types, `SCREAMING` consts, enforced by `beck fmt`" — with the
rationale "one community, one style, zero bikeshedding". That rationale is true and it is not the
reason. Asked directly whether the language could write `print-rat` instead of `print_rat`, a reader
of §2.6 has nothing to go on, and the honest answer is a property of the grammar rather than a
matter of taste. This record exists because the question was asked and the answer was not written
down anywhere.

Hyphens are genuinely better to type — unshifted — and this project already reaches for them. Fifty-one
distinct kebab-case words appear in its own comments and test names, including `add-rat`,
`count-leaves`, `fold-right` and `even-fibs`: the SICP suite writes the book's names in prose and
then renames them for the identifier. The friction is real and measurable, not hypothetical.

**Decision.** Keep `snake_case` in the Python surface. Keep the S-expression surface as it is, where
a hyphen is an ordinary symbol character and `print-rat` already reads today — as do the language's
own reserved forms, `fn-type`, `unquote-splicing`, `stub-arms` and `kw-arg`.

The reason is **infix `-`**. The Python surface lexes `Ident` as `[A-Za-z_][A-Za-z0-9_]*` and
subtraction needs no surrounding whitespace: `countdown(n-1)` parses today as `(- n 1)`. Admitting
hyphens into identifiers makes `total-count` ambiguous between a name and `total - count`, and the
longest match takes the name.

Most of the time that fails loudly — `n-1` is an unknown identifier and B0340 says so. The case that
does not is the one kebab-case naming makes *more* likely rather than less: a binding called
`total-count` alongside bindings called `total` and `count`, where both readings resolve and the
wrong one wins silently. Kebab names are built from short common words, so the collision space is
exactly the vocabulary the convention encourages.

Rejected: **require whitespace around infix `-`**. The corpus says the style is already universal —
forty subtractions across `corpus/`, `sicp/` and `examples/`, all written `a - b`, none written
`a-b`. The problem is not adoption, it is repair: `beck fmt` works on the parsed AST, so by the time
it sees `n-1` the token is already an identifier and there is nothing left to reformat. A rule the
formatter cannot enforce is a rule that lives only in the reader's head, and it would make
whitespace semantic in a language where it is otherwise significant only for indentation.

Rejected: **kebab in both surfaces, with `beck fmt` translating**. It gives one name two spellings,
and [`04`](../04-compiler-architecture.md)'s round-trip property — `parse(print(parse(src)))` is
structurally equal to `parse(src)` — is the thing that would have to absorb the difference.

**Consequences.** The two surfaces disagree about one character, and that is not an accident of
implementation: it is the S-expression surface having no infix operators to collide with, which is
the same property that makes it the canonical one ([`02`](../02-syntax.md) §2.3). A macro author
reading expansion dumps sees `fn-type`; a program author writes `print_rat`.

The three languages [`02`](../02-syntax.md) §2.1 cites as proof that homoiconicity does not require
S-expressions — Elixir, Julia and Nim — all use `snake_case`, and all have infix `-`. That is not a
coincidence, and it is the closest thing to external corroboration this decision has.

What would reopen it: the Python surface losing infix `-`, which nothing plausible would cause. A
future surface with no infix operators inherits the S-expression answer for free.
