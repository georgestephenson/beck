// The thin client: `fold(apply_patch, initial_html, patch_stream)`.
//
// This file is compiler residue. Nothing here is application-specific — no todo, no command
// names, no view logic — which is the point: "the JavaScript never appears in the source at all"
// (docs/00-original-idea.md). It applies patches, captures declarative handlers, and resumes a
// subscription by (subscription, seq). That is the whole of Mode A on the browser side.
//
// The patch interpreter itself is in `beck-patch.js`, because Mode B applies the same ops from the
// same vocabulary — it just produces them in the browser instead of receiving them (docs/93).
(() => {
  const root = document.getElementById("b-root");
  if (!root) return;

  const actor = root.dataset.bActor || "dev";
  const sub = beck.uuid7();
  // The seq the server-rendered HTML reflects. The socket resumes from here, so first paint and
  // first patch cannot disagree, and hydration costs zero DOM work.
  let seq = Number(root.dataset.bSeq) || 0;

  const send = beck.connect({ sub, seq, actor }, (msg) => {
    if (msg.t === "p") { beck.apply(root, msg.o); seq = msg.q; }
    // "current as of q, nothing changed" — keeps `seq` moving so a later reconnect does not
    // ask the server to replay a gap that turns out to be empty.
    else if (msg.t === "u" || msg.t === "w") { seq = msg.q; }
    else if (msg.t === "n") { root.dispatchEvent(new CustomEvent("beck:rejected", { detail: msg })); }
  });

  beck.capture((command) => send({ t: "c", id: beck.uuid7(), command }));
})();
