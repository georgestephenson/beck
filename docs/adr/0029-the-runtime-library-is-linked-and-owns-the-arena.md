# ADR 0029 — A primitive that is somebody else's table is linked, and the library owns the arena

**Status:** accepted
**Date:** 2026-08-14
**Context:** [`93`](../93-the-native-backends-report.md) §93.12,
[`43`](../43-threat-model.md) §43.4,
[`0026`](0026-the-native-heap-is-an-arena-of-offsets.md),
[`0021`](0021-the-native-backend-writes-ir-and-runs-a-process.md),
[`0015`](0015-blake3-for-the-standard-librarys-digests.md)

## The decision

A primitive whose correctness is **somebody else's artefact** — a Unicode case table, BLAKE3's round
function, RFC 4648's alphabet, Rust's `i64` parser, the civil calendar — is neither emitted as
instructions nor asked of the host. It is a **call into a static library the compiled program is
linked against**, and that library is the same crate the evaluator calls.

Two things follow, and the second is the one worth a record.

1. **`beck-prim` is the one implementation**, linked into the compiler as an rlib and into a
   compiled program as an archive embedded in the `beck` binary.
2. **The library owns the compiled program's arena**, so every call carries **offsets** rather than
   pointers.

[`93`](../93-the-native-backends-report.md) §93.12 is what that is and what it measures. This record
is why, and what was refused.

## Why linked, rather than asked or emitted

**Asking** is right for a *question* — a clock, an id, the network — where the answer is not a
function of the arguments and the round trip buys something no local code could. It is wrong for a
digest, which *is* a function of its argument: a question costs 5.2 µs where the digest costs 274 ns
and a `str_to_int` 61 ns.

**Emitting** is right for everything whose specification is the language's own — arithmetic, a field
read, a comparison, `str_slice`. It is wrong here for the reason the refusals gave before this
existed: a compiled ASCII case fold disagrees with the evaluator on the first letter that is not
ASCII, and a second BLAKE3 agrees with the first only as far as somebody has tested it. The
differential would then be a test of two implementations rather than of the compiler.

**Linking** is what a compiler normally does — `rustc` ships `libstd`, `clang` ships `compiler-rt` —
and it makes the agreement a property of there being one function rather than a claim a test
supports.

## Why the library owns the arena

This is the part that was a decision rather than a consequence.

The workspace forbids `unsafe_code` and [`43`](../43-threat-model.md) §43.4 claims that
structurally. The obvious ABI — `(*const u8, usize)` in, a pointer out — needs
`slice::from_raw_parts` in the first line of every primitive, so the claim would have had to be
rewritten around fifteen `unsafe` blocks in the crate most exposed to a compiler bug.

Turning the arena around removes the pointer from the ABI instead: the library allocates the heap,
hands the program its base once, and is thereafter called with `i64` offsets into a `Vec<u8>` it
holds, where reading one is an index and a bad one is a bounds check.

This is only available because [`0026`](0026-the-native-heap-is-an-arena-of-offsets.md) had already
made a value **an offset and not a pointer**, so that a heap could cross a pipe as bytes. The same
property lets a heap cross a C ABI as a number.

**What the design rests on**, stated plainly because it is asked for rather than proved: the
compiled program writes into a buffer this crate owns, through a base address handed to it once.
That is the ordinary shape of a buffer handed to foreign code — `read(2)` into a `Vec` is the same
shape — and it holds because the buffer is allocated once and never grown, no Rust reference into it
is live while compiled code runs (the only way compiled code runs is *between* calls into the
library), and a compiled program is one thread asking one question at a time. A bad offset panics,
which aborts at the `extern "C"` boundary: the price of having no `unsafe`, and the right price,
because the only caller is a code generator in this workspace and a bad offset is therefore a
compiler bug.

## What it costs

- **The `beck` binary carries the archive** — 21.4 MiB of `staticlib`, 6.1 MiB compressed. Most of
  it is Rust's standard library, which a `staticlib` carries whether a primitive reaches it or not.
- **A program that calls one grows from 16 KiB to 4.9 MiB**, and only such a program: both emitters
  record whether any definition reached one, and a module that did not names none of the symbols.
- **A second `cargo` invocation in `beck-llvm`'s build script**, because the archive is built for
  the program rather than for the compiler.
- **The archive is built for the host triple.** A `beck native --target` that cross-compiled would
  need one per target, and nothing here answers that.

## What was considered and refused

**A C runtime library, compiled at link time from source embedded in `beck`.** No second `cargo`
invocation, no archive in the binary, and no `unsafe` question at all — C is not Rust. It also
cannot be the implementation the evaluator calls, so BLAKE3, Unicode's case table and the number
grammar would each be reimplemented and the differential would be testing two of everything. This is
the option the refusals themselves argued against.

**Shipping the archive beside the binary** rather than inside it — the layout `rustc` uses for
`libstd`. Smaller, and what a package manager expects. It also means a `beck` that works from
`target/release` and fails after being moved, and one more thing for `install.sh` to get right; a
release here is one executable per platform ([`28`](../28-releases-and-deployment.md) §28.2), and
keeping that is worth 6 MiB.

**Passing a pointer and using one `unsafe` block.** Honest, small, and a change to §43.4's claim.
The arena-owning design costs a lock and a bounds check per call — against a hash or a case fold,
neither is measurable — so there was nothing to trade for the claim.

**Letting the library build the values it answers with.** It would close `json_parse`, the one
primitive of this shape still refused. It needs the library to know a *layout*, which belongs to
whichever code generator asked; the division here is that the library produces text and numbers and
the emitter builds anything with a declared type around them, including every raised error.
[`93`](../93-the-native-backends-report.md) §93.15 says what closing `json_parse` would take
instead.
