- **2026-08-16 — Awareness, scoped against the tree, and the scoping splits it in two.**
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.8 now specifies the construct:
  `awareness(f) : Signal[Map[ActorId, T]]`, a signal operation rather than a command, inheriting
  `presence()`'s three rules unchanged — non-log input to a view, capacity-bounded for §82.5's
  reason since the key is again a name the client chooses, and forbidden from the chokepoint, which
  is `B0515`'s reasoning with one noun changed. What the scoping found is that **what `f` may read
  splits the feature**. With `f : Session -> T` it is buildable today and needs **no wire change at
  all**, because the server already holds every subscriber's route — it arrives on `hello` and on
  every `Nav` — so *who is looking at what* costs a source, a role and an aggregation. With `f` over
  a client-local value — a cursor, a selection — it is not, and not for a protocol reason: the
  client has nothing to derive one from, since it listens for five events and `mousemove` is not
  among them. So arbitrary awareness has the **same prerequisite as the client-local fold**, and the
  two are one piece of work rather than two independent ones.
