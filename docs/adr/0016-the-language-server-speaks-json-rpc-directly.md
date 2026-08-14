# ADR 0016 — The language server speaks JSON-RPC directly

**Status:** accepted
**Date:** 2026-08-04
**Context:** [`65`](../65-the-editor-report.md), [`04`](../04-compiler-architecture.md) §4.6

## The decision

`beck lsp` implements the Language Server Protocol's wire format by hand — a `Content-Length`
header, a blank line, a JSON body — over `serde_json`, which the CLI already depended on. **No LSP
framework is taken.**

## The alternative, and why it was refused

`tower-lsp` is the obvious choice and is a good crate: MIT/Apache-2.0, widely used, and it hands you
typed request and response structures for the whole protocol, cancellation, and an async runtime
integration. `lsp-types` alone would give the structures without the runtime.

Three things decided against both, and only the third is about this repository in particular.

1. **The protocol surface actually used is small and closed.** `initialize`, `initialized`, four
   `textDocument` notifications, three `textDocument` requests, `shutdown`, `exit`. The framing is
   nine lines. A framework earns its place by absorbing a large surface, and this is not one — the
   whole transport, in [`lsp.rs`](../../compiler/crates/beck-cli/src/lsp.rs), is under a hundred
   lines including the parts that exist to be correct rather than to work.

2. **Typed structures for a protocol we are translating *into* buy less than they look.** Every
   answer this server gives is built from a `beck-core` value — a `Diagnostic`, an
   `iface::Item`, a `Span`. The types on the other side of that translation would be a second set of
   shapes to convert to, not a set to compute in. `serde_json::json!` builds the same object with
   one conversion instead of two.

3. **`beck-cli` is the crate a `tokio` runtime already complicates.** `tower-lsp` is async and
   assumes ownership of the loop; `beck lsp` is a blocking read of stdin, and
   [`07`](../07-dependencies.md)'s standard is that a dependency has to pay for the coupling it
   introduces. This one would introduce an async server framework so that a synchronous loop could
   read a header.

## What would reverse it

Any of these, and none is hypothetical — the first is the likeliest:

- **Incremental sync.** `textDocumentSync: Full` is what a whole-file re-check wants; the moment
  the server tracks ranges, `lsp-types`' `TextDocumentContentChangeEvent` and its position encoding
  are worth having and getting wrong by hand is easy.
- **Completion, code actions, semantic tokens, or inlay hints.** Each is a large typed structure
  with rules about what may be omitted, and three of them at once is the point where hand-rolling
  becomes the expensive choice.
- **Position encoding negotiation.** This server assumes UTF-16, which the protocol makes the
  default and every mainstream client sends. A client that negotiates UTF-8 is handled by a
  framework and would be handled here by reading the capability, which is a thing to remember rather
  than a thing that is true.

The upgrade is not costly, which is part of why the refusal is safe: the translation layer is one
file, and nothing outside it knows the protocol exists.

## What this is not

It is **not** a claim that hand-rolling protocols is the house style. [`0015`](0015-blake3-for-the-standard-librarys-digests.md)
took a cryptographic dependency without hesitation, for the reason that applies there and not here:
a hash that is subtly wrong is a security defect and a JSON header that is subtly wrong is a client
that reports a parse error on the first message.
