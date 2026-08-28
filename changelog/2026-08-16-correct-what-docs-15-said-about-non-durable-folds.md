- **2026-08-16 — Correct what `docs/15` said about non-durable folds, which was wrong twice.**
  Its Redis-replacement ladder said hot ephemeral state is answered by non-durable folds, "already a
  language construct (D1)", and that "quota counters (F3) are exactly this". Neither holds. The
  construct is decided and **unbuilt** — that is `DEFECTS.md::non-durable-fold` — and F3's quota is
  not an instance of it and could not be: it is a **sharded** fixed table precisely because a
  per-actor map is unbounded memory keyed by a name the client chooses, which is the denial of
  service it exists to prevent ([`docs/82`](../docs/82-the-edge-report.md) §82.5), and a fold would be
  that map. Presence is not an instance either — it is D6's first-class non-durable `Signal`, a
  compiler-provided source moved by *connections* rather than by events, which its own module
  documentation states as the one thing that makes it unusual. So nothing in the tree is a
  non-durable fold, where two things looked like one. Found by doubting the sentence rather than by
  a gate, which is [`docs/08`](../docs/08-roadmap.md) §8.5.6's second direction of decay and the one
  nothing outside `pending_security.rs` catches.
