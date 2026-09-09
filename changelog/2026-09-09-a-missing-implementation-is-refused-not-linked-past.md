- **2026-09-09 — A dependency with no implementation is refused where the program would run.**
  A module reached through an `import` with a `.becki` and **no** `.beck` contributed no bodies, so
  it was simply absent from the link: every call into it arrived as `no such definition`, an
  absence with nothing naming the module or saying why. `B0604` — "has an interface but no
  implementation", with the note "an interface is enough to compile against and never enough to
  run" — already says the right thing and was reachable only when such a module was the **root**.
  That closes `DEFECTS.md::an-interface-only-dependency-links-to-nothing-and-says-nothing`, and the
  register is **empty**.
  Where it is asked is the whole of the design. Compiling against a `.becki` with no `.beck` beside
  it *is* §3.6's separate compilation, so refusing it while reading would delete the feature:
  `check_project` records the modules rather than refusing them, and `require_implementations` is
  the question a caller that means to execute asks. `beck check` and `beck iface` do not ask it and
  still answer; `beck build`, `beck run`, `beck image`, `beck explain` and `beck test` do, because
  each produces or describes a runnable artefact and there is none.
  Gated in `beck-cli/tests/imports.rs` in both directions, each checked against the corresponding
  mistake: `beck test` on such a project reports `B0604` naming the module and **not** `no such
  definition`, which goes red with the call removed; and `beck check` on the same project still
  answers `ok:`, which goes red if the refusal is moved into `check_project` — the over-broad fix
  that would pass the first gate and delete §3.6. It has nothing to do with traits: the fixture is
  a plain `def`, which is how the defect was found beside one that was about traits.
