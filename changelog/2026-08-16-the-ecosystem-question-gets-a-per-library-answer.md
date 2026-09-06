- **2026-08-16 — The ecosystem question gets a per-library answer, and the roadmap gets a sweep.**
  [`docs/105`](../docs/105-the-ecosystem-answer.md) answers "what about NumPy and pandas" from two
  independent constraints: a bridged call carries an effect and `place.rs:760` makes a fold
  replay-pure, so the [`09`](../docs/09-risks-and-open-questions.md) §9.2 sidecar cannot reach the data
  tier; and the libraries that most expand a language's utility are **notations**, which cannot be
  bridged at all because an RPC hop destroys the composition that was their value. §102.4 discards
  download rank as an instrument — it measures fan-in, and `requests` is outranked by three of its
  own dependencies — for the Stack Overflow survey [`08`](../docs/08-roadmap.md) §8.6's ≥1% rule
  already runs on, which puts NumPy at 21.2% and pandas at 20.7%, second and third among all
  libraries in all languages. GitHub stars were tested as a third instrument and discarded with
  evidence: they rank TensorFlow 6× above NumPy and measure it at half NumPy's use, because a star
  is a one-time vote that never decays. §8.6.2 applies the ≥1% rule to libraries for the first time
  and gives **all 39 entries** of the survey's section a verdict — four had none anywhere, including
  the Electron/Tauri adjacency (15.4% together, and Beck already emits both halves), which is
  recorded as watch rather than scheduled. §102.4 also carries what has moved since the 2024 survey:
  pandas 3.0 defaults to PyArrow-backed strings and PyArrow is PyPI #95 at 56% of pandas' own
  downloads, so the ecosystem has corroborated the Arrow argument with its defaults; Polars is a
  fifth convergence on the same dataframe verbs at a ninth of pandas' volume; and LLM clients are a
  category that post-dates the survey entirely, with `litellm` at #46 above `pip` — bridged, and the
  response becomes an event, so a session replays without re-calling the model. So pandas is
  [`99`](../docs/99-the-data-tier-means-of-combination.md)'s missing algebra, NumPy is a notation over
  a linked kernel, and charting is blocked on `beck-patch.js`'s `createElement`. A doc-versus-code
  sweep (§8.5.6) then found one document behind the code —
  [`42`](../docs/42-security-assurance.md) called macro expansion fuel "absent" when `MAX_EXPANSION`,
  `B0214` and `macro_bomb.rs` have bounded it all along — and seven items no ordered list held,
  including deterministic `sin`/`cos`, which resolve to the host libm in all three backends, so two
  machines can fold one log to two states. All now have a position in §8.5.4, and the two that are
  **defects rather than absences** — the libm divergence and `beck explain cost` excluding an
  `O(n)` operator from its own tally — are entries in [`DEFECTS.md`](../DEFECTS.md) with the gate each
  fix owes. Charting was ranked here and is fixed nowhere: [`104`](../docs/104-styling-and-the-component-library.md)
  found the same `createElement` defect from the UI side and owns it. Documents only; nothing built.
