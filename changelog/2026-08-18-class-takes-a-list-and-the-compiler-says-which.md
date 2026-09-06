- **2026-08-18 — `class=` takes a list, and the compiler says which classes a page can carry.**
  [`docs/08`](../docs/08-roadmap.md) §8.5.4's styling cluster item 3 — the **F** everything else in that
  half is behind ([`docs/104`](../docs/104-styling-and-the-component-library.md) §104.4). A list where
  HTML defines a space-separated value — `class`, `rel`, and the two ARIA id-list relationships — is
  joined in the **`ui:` lowering** rather than at the seam, so what reaches the checker is one
  `str_join` and no emitter had to learn anything; the three backends agree by construction. An
  existing `class="a b"` is untouched and renders the characters it always did.
  The surface is not decoration: `class=["btn", "primary" if hot else "plain"]` is what a program
  writes instead of `"btn " + variant`, and the difference is that a list of alternatives can be
  **enumerated** where a concatenation cannot — by Beck or by Tailwind's own scanner, which §104.3
  measured over this tree reading English prose out of comments and missing a real utility behind a
  module boundary. So `beck_core::style` enumerates every class that can reach a `class=`, following
  a call and taking both arms of an `if`, which is the shape every dynamic class in this tree is
  already written in: `examples/routed.beck` is `{done, here}`, `corpus/02-chat.beck` is
  `{mine, theirs}`, and neither program was edited. `beck explain style` prints that set and, beside
  it, every site where a class is *built* rather than named, with which of three reasons — a
  concatenation, a value, or a shape the analysis does not enter — because a reader does something
  different about each. Nothing is rejected: the report is what makes the set honest, and the escape
  hatch a stylesheet emitter will need is a decision for the item that emits one.
  `style.rs` is the harness, and it holds both directions of the lowering's table (a list in `class`
  joins, a list in `title` does not) and that one computed class does not hide the ten beside it.
  **What item 3 still owes is the `Class` type**, and it moved *behind* item 4 rather than in front:
  a class name has nothing to be checked against until the utility table exists, and a type whose
  checking is empty is a scaffold. §8.5.4 and §104.11 both say so now.
