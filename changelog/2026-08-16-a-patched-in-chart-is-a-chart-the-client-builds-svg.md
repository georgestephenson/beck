- **2026-08-16 — A patched-in chart is a chart: the client builds SVG in the right namespace.**
  `beck-patch.js` built every subtree with `document.createElement`, which only ever guesses HTML,
  while server-side rendering goes through the browser's own parser and gets it right. An `svg`
  built that way is not an `SVGElement`, so it lays out as nothing: a chart painted on first load
  and vanished the first time its data changed, which is the only reason to have drawn it. The
  client now uses `createElementNS`, taking the namespace **from the tag** where the tag opens one
  and **from the destination** otherwise, with `foreignObject` handing it back to HTML — and the
  second half is where the difficulty is, because a patch that adds a bar to an existing chart
  carries no `svg` tag of its own. Gated by
  `browser.rs::a_patched_in_chart_is_still_a_chart` over the new `examples/chart.beck`, the first
  program in the tree whose page is an SVG: two patches, and the assertion is the **laid-out width**
  of every `rect` rather than its namespace. Checked against three wrong versions — the original
  measures 0 on the first patch, a tag-only fix measures 0 too, and a fix with subtree inheritance
  but no destination measures `30,0` on the second. Deletes `DEFECTS.md::svg-namespace`; item 1 of
  [`docs/104`](../docs/104-styling-and-the-component-library.md) §104.11's cluster.
