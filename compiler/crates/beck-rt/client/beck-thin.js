// The thin client: `fold(apply_patch, initial_html, patch_stream)`.
//
// This file is compiler residue. Nothing here is application-specific — no todo, no command
// names, no view logic — which is the point: "the JavaScript never appears in the source at all"
// (docs/00-original-idea.md). It applies patches, captures declarative handlers, follows links,
// and resumes a subscription by (subscription, seq). That is the whole of Mode A on the browser
// side.
//
// The patch interpreter, the socket, the router and the id source are in `beck-patch.js`, because
// Mode B needs the same four and produces the same patch ops — it just produces them in the
// browser instead of receiving them (docs/93).
(() => {
  const root = document.getElementById("b-root");
  if (!root) return;

  // `state` is shared with the socket rather than copied into it: `state.seq` is what a *reconnect*
  // resumes from, so it has to be the position of the last frame applied and not the position the
  // document was painted at. The two are the same only until the first patch arrives.
  const state = {
    sub: beck.uuid7(),
    actor: root.dataset.bActor || "dev",
    // The seq the server-rendered HTML reflects. The socket resumes from here, so first paint and
    // first patch cannot disagree, and hydration costs zero DOM work.
    seq: Number(root.dataset.bSeq) || 0,
  };

  // Commands proposed and not yet answered. In Mode A this is the whole of "pending": the server
  // holds the state, so a client has nothing to show for a command until the patch comes back, and
  // what it can say is *how many it is waiting on* — which is what a devtools panel means by
  // pending and what a page could show as a spinner.
  const pending = new Map();

  const send = beck.connect(state, (msg) => {
    if (msg.t === "p") {
      beck.apply(root, msg.o);
      state.seq = msg.q;
    } else if (msg.t === "u" || msg.t === "w") {
      // "current as of q, nothing changed" — keeps `seq` moving so a later reconnect does not ask
      // the server to replay a gap that turns out to be empty.
      state.seq = msg.q;
      // A welcome means the subscription exists, which is the whole of readiness in Mode A: the
      // handlers were installed synchronously and the page was rendered by the server.
      if (msg.t === "w") beck.ready(root, "a");
    } else if (msg.t === "a" || msg.t === "n") {
      pending.delete(msg.id);
      beck.stats.pending = pending.size;
      if (msg.t === "n") beck.announce(root, "beck:rejected", msg);
    }
  });

  beck.capture((command) => {
    const id = beck.uuid7();
    pending.set(id, command);
    beck.stats.pending = pending.size;
    send({ t: "c", id, command });
  });

  // A link is a route, and a route is `session.path`. In Mode A the server re-renders for the new
  // session and answers with a patch, so a navigation costs exactly what any other change costs:
  // the difference between two pages.
  beck.route((path) => {
    beck.stats.navigations += 1;
    send({ t: "g", path });
  });

  beck.inspect.describe = () => ({
    mode: "A",
    seq: state.seq,
    actor: state.actor,
    path: beck.here(),
    pending: Array.from(pending.keys()),
  });

  beck.devtools();
})();
