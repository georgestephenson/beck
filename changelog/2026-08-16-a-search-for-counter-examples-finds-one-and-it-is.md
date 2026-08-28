- **2026-08-16 — A search for counter-examples finds one, and it is D1's own: awareness.**
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.8's recommendation said to build
  nothing server-side, on the reasoning that presence, quotas and caches already answer every
  server-side ephemeral need. A search for counter-examples returned one it missed — **awareness**:
  a cursor, a selection, a typing indicator, which a *second person* must see and which therefore no
  client-local anything can hold. [Yjs](https://docs.yjs.dev/getting-started/adding-awareness) keeps
  it in a protocol of its own because it "isn't stored in the Yjs document, as it doesn't need to be
  persisted across sessions", and its shape is a `Map<client, state>` that is broadcast, expires
  after thirty seconds of silence and is deleted on disconnect. Two things follow. **It is not a
  fold** — it is a keyed map of each client's latest value — so it is still true that nothing found
  needs a server-side ephemeral *fold*. And **Beck has nine-tenths of it**: `presence()` is that map
  with no payload, already a non-log input to a view, already capacity-bounded (§82.5), already
  forbidden from the chokepoint (`B0515`). The homes go from four to five, ordered, and the
  correction underneath them is that ephemerality comes from the stream and the audience, never from
  the absence of a `durable` wrapper — which is what D1's sentence gets wrong.
