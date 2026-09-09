- **2026-09-09 — A published interface says what it depends on, so it reads back.**
  `beck iface` wrote a `.becki` that `beck check` then **refused**. Any module implementing a trait
  declared elsewhere, or publishing a bounded `def`, produced a contract naming `Priced` with
  nothing to resolve it against: [`Interface::parse`](../compiler/crates/beck-core/src/iface.rs)
  checked the file as a module with **no imports**, and the file carried no `import` line, so
  `B0383` refused it. Checking the `.becki` in stopped the program building, and deleting it made
  the same program compile and run — which closes
  `DEFECTS.md::a-published-interface-naming-a-trait-cannot-be-read-back`.
  Both halves moved, because either alone leaves a case broken. The **writer** emits the module's
  `import` lines first, so a contract is readable as the module it describes and an interface-only
  dependency is discoverable at all; the **reader** gains `Interface::parse_with`, and
  [`project`](../compiler/crates/beck-core/src/project.rs) hands it the same interfaces the module
  itself was checked against — it has them, because it works in dependency order, deepest first.
  The import list is deliberately **outside the digest**: it is provenance for resolving the
  published names rather than one of them, and hashing it would report an API change to every
  consumer whenever a module gained an import its contract never mentions.
  Gated in `beck-cli/tests/imports.rs`: `beck iface` then `beck test` with the `.becki` checked in,
  in both import orders, asserting the file says `import priced` **and** that the program runs —
  dispatch has to reach the impl through the interface, which a gate checking only for the absence
  of `B0383` would not show. Each half was removed in turn and the gate went red for both. The
  negative half is that a `.becki` naming a trait nothing declares is still refused, pointing into
  the `.becki` rather than at whoever imported it, so the fix did not turn `B0383` into silence.
  One defect found beside it and recorded rather than fixed:
  `DEFECTS.md::an-interface-only-dependency-links-to-nothing-and-says-nothing` — a dependency with
  a `.becki` and no `.beck` links and fails at run time with a bare `no such definition`, while
  `B0604` says exactly the right thing and is raised only when such a module is the root. It has
  nothing to do with traits: a plain `def` behaves the same way.
