## `a-stub-in-a-library-test-is-accepted-and-can-never-fire`

**What is wrong.** A `stub` clause in a **library** module's test is accepted, reported, and
unreachable. There is no expression a library test can write that would reach it, so the clause is
dead the moment it is written and nothing says so:

```beck
import http

def catalogue(isbn: Str) -> Str:
    return require_ok(http_fetch("catalogue.example.com", get("/isbn/" + isbn)))

def pure(x: Str) -> Str:
    return str_upper(x)

test "the stub below can never fire":
    stub net.out(catalogue.example.com): "SICP"
    expect pure("a") == "A"
```

```console
$ beck test -v lookup.beck
test "the stub below can never fire" … ok
  stubbed:
    net.out(catalogue.example.com) by `catalogue`  → SICP   called 0× (named)
```

Writing the expectation that would reach it — `expect catalogue("1") == "SICP"` — is `B0700`,
"a test block's own row must be empty", because the call puts `net.out` and `raises(HttpError)` in
the test's row. In an application the stub is reached through `when`, which drives `validate`; a
library has no `when` (`needs_an_application` refuses the clause), so the two halves of §21.3 cannot
both be present in one module.

**Why it is a defect rather than an absence.** `beck-rt/src/testing.rs::needs_an_application` says
in as many words that they can:

> Everything else works: `expect <Bool>` over the module's own definitions, `property` blocks and
> their generated inputs, `stub`, and the static expectations. That is the whole of a unit test for
> a domain module.

And `B0700`'s own note sends the reader to the mechanism that cannot help them — "effects belong to
the *subject*, and §21.3 stubs those" — when in this module there is no subject a clause can name.
The silent half is the worse half: `called 0× (named)` is printed under `-v` and the test passes, so
a domain module whose author believes it is exercising its HTTP client is exercising nothing.

**The gate a fix owes**, whichever shape it takes — the row of an atom a `stub` clause names being
discharged by that clause, so a library test can call the stubbed definition; or the clause being
refused where it cannot fire, so the module is told. Positive, for the first shape: the program
above with `expect catalogue("1") == "SICP"` compiles and passes, and `-v` reports the stub
`called 1×`. Positive, for the second: the program above fails to compile, naming the clause.
Negative, and it is the half that would be forgotten under either: a library test that calls
`catalogue` **without** a stub still fails with `B0700`, because a fix that discharged the row
whether or not a stub named it would have deleted the rule rather than completed it.

Not to be confused with [`08`](../docs/08-roadmap.md) §8.5.4's "a stub that fails", which is the
absent feature next to this one: there, a stub body that *raises* is refused in an application too.
