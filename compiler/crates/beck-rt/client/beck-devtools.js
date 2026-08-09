// The devtools panel: the signal graph, the patch traffic and the pending state.
//
// docs/08 §8.6 asks for "a devtools extension showing signal graph, patch traffic and pending
// state". This is the three things and not the extension, and the difference is deliberate: an
// extension is a second artefact with its own distribution, its own permissions and its own release
// pipeline, and nothing in this repository could run one — the browser gate here drives a page.
// A panel the server serves is testable by the same harness that tests the client, and it is the
// same residue: no framework, no CDN, nothing the network policy this program derives would refuse.
//
// It is loaded only when it is asked for (`beck.devtools()` in each mode's shim), so a page that
// does not want it pays nothing, and it is appended to `body` rather than into `#b-root` — a patch
// path is a child index from the frame root, and a panel inside it would be counted.
(() => {
  if (window.__beckDevtools) return;
  window.__beckDevtools = true;

  const root = document.getElementById("b-root");
  if (!root) return;

  const panel = document.createElement("aside");
  panel.id = "beck-devtools";
  panel.setAttribute("aria-label", "Beck devtools");
  const style = document.createElement("style");
  style.textContent = `
    #beck-devtools {
      position: fixed; right: 0; bottom: 0; z-index: 2147483647; width: min(26rem, 100vw);
      max-height: 70vh; overflow: auto; box-sizing: border-box;
      font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
      background: #0f1115; color: #d7dce5; border-top: 1px solid #262c36;
      border-left: 1px solid #262c36;
    }
    #beck-devtools header {
      display: flex; gap: .5rem; align-items: baseline; justify-content: space-between;
      padding: .4rem .6rem; border-bottom: 1px solid #262c36; position: sticky; top: 0;
      background: #161a21;
    }
    #beck-devtools h1 { margin: 0; font-size: 11px; letter-spacing: .12em; text-transform: uppercase; }
    #beck-devtools h2 {
      margin: .6rem 0 .2rem; font-size: 10px; letter-spacing: .12em; text-transform: uppercase;
      color: #8b95a5;
    }
    #beck-devtools section { padding: 0 .6rem .6rem; }
    #beck-devtools table { border-collapse: collapse; width: 100%; }
    #beck-devtools td { padding: 1px 6px 1px 0; vertical-align: top; }
    #beck-devtools td.k { color: #8b95a5; white-space: nowrap; }
    #beck-devtools td.n { text-align: right; font-variant-numeric: tabular-nums; }
    #beck-devtools button {
      font: inherit; background: none; color: #8b95a5; border: 1px solid #262c36;
      border-radius: 3px; cursor: pointer; padding: 0 .4rem;
    }
    #beck-devtools .per-session { color: #6aa9ff; }
    #beck-devtools .recompute { color: #f0a35e; }
    #beck-devtools .incremental { color: #7fd18a; }
  `;

  const head = document.createElement("header");
  const title = document.createElement("h1");
  title.textContent = "beck";
  const close = document.createElement("button");
  close.textContent = "close";
  close.addEventListener("click", () => {
    try {
      localStorage.setItem("beck:devtools", "");
    } catch (e) {
      // A context with no storage. Closing still works; it just will not be remembered.
    }
    panel.remove();
  });
  head.append(title, close);

  const traffic = document.createElement("section");
  const client = document.createElement("section");
  const graph = document.createElement("section");
  panel.append(style, head, traffic, client, graph);
  document.body.appendChild(panel);

  const rows = (into, heading, pairs) => {
    into.textContent = "";
    const h = document.createElement("h2");
    h.textContent = heading;
    const table = document.createElement("table");
    for (const [k, v, cls] of pairs) {
      const tr = document.createElement("tr");
      const kd = document.createElement("td");
      kd.className = "k";
      kd.textContent = k;
      const vd = document.createElement("td");
      vd.className = cls || "n";
      vd.textContent = v;
      tr.append(kd, vd);
      table.append(tr);
    }
    into.append(h, table);
  };

  // ---- patch traffic and pending state --------------------------------------------------------
  //
  // Both are read from what the residue already counts. Nothing here derives a second account of
  // what the client is doing — a panel that computed its own could be the only wrong thing on the
  // screen, and it is the one thing on the screen a developer would believe.
  const draw = () => {
    const s = beck.stats;
    rows(traffic, "patch traffic", [
      ["socket", s.connected ? "open" : "reconnecting", "k"],
      ["frames applied", s.frames],
      ["ops applied", s.ops],
      ["bytes in", s.bytes_in],
      ["bytes out", s.bytes_out],
      ["frames sent", s.sent],
      ["navigations", s.navigations],
    ]);

    let d = {};
    try {
      d = beck.inspect.describe();
    } catch (e) {
      d = { mode: "?" };
    }
    const pending = d.pending || [];
    rows(client, "client", [
      ["mode", d.mode || "?", "k"],
      ["route", d.path || "", "k"],
      ["actor", d.actor || "", "k"],
      ["seq", d.seq === undefined ? "" : d.seq],
      ["pending", pending.length],
      ...pending.map((id) => ["", id.slice(0, 8), "k"]),
    ]);
  };

  document.addEventListener("beck:traffic", draw);
  document.addEventListener("beck:ready", draw);
  document.addEventListener("beck:rejected", draw);
  // A tick as well as the events, because two of the numbers move on *sending* — which is not a
  // patch and has no frame to hang an event on.
  setInterval(draw, 500);
  draw();

  // ---- the signal graph -----------------------------------------------------------------------
  //
  // The one thing the browser cannot know: a Mode A client is never sent a program. It comes from
  // `/beck-signals.json`, which is the running program's own graph and the same verdicts
  // `beck explain incremental` prints.
  fetch("/beck-signals.json")
    .then((r) => r.json())
    .then((g) => {
      const pairs = [
        ["program", g.program, "k"],
        ["page", g.page, "k"],
        ["mode", g.mode, "k"],
        ["reads of session", g.reads, "k"],
        ["operators", g.plan.operators],
        ["maintained", g.plan.maintained],
        ["recomputed", g.plan.recomputed],
        ["per session", g.plan.per_session],
      ];
      rows(graph, "signal graph", pairs);
      const table = document.createElement("table");
      for (const n of g.nodes) {
        const tr = document.createElement("tr");
        const name = document.createElement("td");
        name.className = "k";
        name.textContent = n.label;
        const what = document.createElement("td");
        what.className = n.verdict ? n.verdict.replace(" ", "-") : "k";
        what.textContent = n.op + (n.verdict ? " · " + n.verdict : "") + " · " + n.tier;
        tr.append(name, what);
        table.append(tr);
      }
      graph.append(table);
    })
    .catch((e) => rows(graph, "signal graph", [["unavailable", String(e), "k"]]));
})();
