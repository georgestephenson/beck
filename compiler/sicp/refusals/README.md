# The refusals

One file per wall still standing between this suite and the rest of SICP, each the smallest program
that hits it, with the diagnostic in its header comment. The harness asserts every one is **still
refused**, so a wall coming down is a test that starts failing rather than a fact somebody notices.

**It is empty.** That is a state worth writing down rather than a directory worth deleting.

[`docs/25-benchmarks-and-expressiveness.md`](../../../docs/25-benchmarks-and-expressiveness.md)
§25.6 measured six walls and all six came down
([`27`](../../../docs/27-the-walls-come-down-report.md), [`27`](../../../docs/27-the-walls-come-down-report.md),
[`27`](../../../docs/27-the-walls-come-down-report.md)). Removing them wrote three more,
and those came down too: a `list[T]` that could not be taken apart
([`27`](../../../docs/27-the-walls-come-down-report.md)), a type that could not
take a parameter ([`27`](../../../docs/27-the-walls-come-down-report.md)), and exact rationals,
which needed `+` to reach a type the compiler does not know about
([`27`](../../../docs/27-the-walls-come-down-report.md)).

So: **nothing measured is refused today.** That is not the same claim as "Beck can express SICP".
It is the narrower and checkable one — that every wall this project has *found* has been removed,
and finding the next one needs somebody to write more of the book. Chapter 3 has been written since
([`87`](../../../docs/87-the-chapter-that-argues-back-report.md)) and put no file here: every
section of it is expressible. Two of its refusals are *decisions* rather than walls and live in
`sicp.rs` — §3.4's interleaving is `B0399` and exercise 3.8's is `B0398`
([`docs/80`](../../../docs/80-structured-concurrency-report.md)) — and the one thing it genuinely
cannot express, §3.5.1's **memoised** `delay`, is the rule at the bottom of this page working: it
compiles and it is slow, so it is a cost rather than a refusal (§87.7). Chapters 4 and 5 are still
unattempted, and §2.3.4's Huffman decoder wants a pattern more than one level deep
([`27`](../../../docs/27-the-walls-come-down-report.md) §27.10), which is the
nearest thing to a wall with a name on it.

## What puts a file back here

An exercise that cannot be written, reduced to the smallest program that fails, with the diagnostic
quoted in its header and a test asserting it. Not a slow one, not an ugly one — a refused one. The
register an exercise lands in is [`25`](../../../docs/25-benchmarks-and-expressiveness.md) §25.5's:
**translated**, **re-expressed**, or **refused**, and only the third belongs here.
