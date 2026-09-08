- **2026-09-08 — An `import` has no order, and the trait table stopped having one.**
  Whether an imported `impl` could be called depended on **which `import` line was written first**.
  The checker registered each imported module's traits and impls together, one module at a time, so
  `import thing` before `import vocab` dropped `impl Labelled for Thing` — silently, with the
  failure surfacing one module later as `B0387` asking for an impl that `beck iface thing.beck`
  publishes in the file the compiler had just read. Nothing gives `import` an order:
  [`10`](../docs/10-decisions.md) D23 fixes where a name resolves *from*, and a module's contract is
  derived from its body rather than from its position. Registration is now **two passes over the
  whole import list**, traits before anything that resolves one, which closes
  `DEFECTS.md::an-imported-impl-is-visible-only-if-its-trait-was-imported-first`.
  The same defect was live one line down and worse, and nothing had noticed: a bounded `def`
  publishes its bound and the importer rebuilds the dictionary parameters the exporting module
  lowered it with by resolving the trait *by name*, so with the trait's module named second the
  rebuild found nothing, dropped them, and `total` arrived as a one-argument function. That failed
  at **run time** with `expected 2 arguments, got 1` — no diagnostic at all. Putting the imported
  names in the second pass fixes it by construction.
  What is left of the `continue` that carried both meanings is `B0388`, and it is a **warning**
  rather than a refusal: once traits are registered across every import, a trait still missing means
  the program does not import the module declaring it, and that is legitimate — a name is visible
  where its module is imported directly, so refusing it would make importing a module for one plain
  function drag its whole trait vocabulary in. It is not nothing either, because the impl is dropped
  and the later call reads `B0350: no field or function`, three modules from the fact that explains
  it. So the warning names the trait and says the impl was dropped.
  Gated in `beck-cli/tests/imports.rs`, which drives the binary against files on disk because the
  property is about a written order: both orders **run**, and two types reach their own impls, so a
  fix that registered an impl without dispatching it would not pass. Both ordering gates go red
  against the single-pass registration.
  One defect found and recorded rather than fixed:
  `DEFECTS.md::a-published-interface-naming-a-trait-cannot-be-read-back` — `beck iface` writes a
  `.becki` that `beck check` refuses with `B0383`, because `Interface::parse` checks it as a module
  with no imports and the file carries no `import` line to resolve against. Two stale code comments
  were corrected in place: `check/traits.rs` said a `.becki` publishes neither traits nor impls and
  it publishes both, and `Def::bounds` said a bounded definition is not published when it is
  [`27`](../docs/27-the-walls-come-down-report.md) §27.5's whole point that it is.
