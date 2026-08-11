// Mode B's browser half: load the kernel, hold the state, render locally.
//
// This file is compiler residue in the same sense `beck-thin.js` is — no todo, no command names,
// no view logic. What it adds over the thin client is *where the rendering happens*: it hands the
// kernel data patches and gets DOM patches back, so an interaction is a local fold rather than a
// round trip (docs/05 §5.1, docs/93).
//
// The wasm boundary is four exports and a length-prefixed byte buffer; see `crates/beck-wasm`.
(() => {
  const root = document.getElementById("b-root");
  if (!root) return;

  const actor = root.dataset.bActor || "dev";
  // What the provider said about this person, as the server verified it. The kernel renders the
  // view against a `Session` built from both, so a claims map missing here is a page that differs
  // from the one being hydrated — see `crates/beck-wasm`'s `Viewer`.
  let claims = {};
  try {
    claims = JSON.parse(root.dataset.bClaims || "{}");
  } catch (e) {
    beck.announce(root, "beck:error", { error: "unreadable claims: " + e });
  }
  const sub = beck.uuid7();
  // The position the server-rendered document reflects. It is *not* what this client resumes
  // from: a Mode A client resuming at `seq` is claiming to hold the page as of `seq`, and the
  // document is that page — but a Mode B client holds the *state*, and it starts holding nothing
  // but `init`. So the first connection says it holds nothing (`seq: null`, which the protocol
  // reads as "send me the world") and `state.seq` moves only as frames arrive.
  const painted = Number(root.dataset.bSeq) || 0;
  const state = { sub, seq: null, actor };

  let wasm = null;
  // Commands proposed and not yet acknowledged, in order. Restored from the local copy on load,
  // and sent whenever a socket opens.
  let queued = [];
  let flush = () => {};
  const memory = () => new Uint8Array(wasm.memory.buffer);

  // A response is `<u32 len><bytes>` at the returned pointer, and the module holds it until freed.
  const read = (ptr) => {
    const mem = memory();
    const len = mem[ptr] | (mem[ptr + 1] << 8) | (mem[ptr + 2] << 16) | (mem[ptr + 3] << 24);
    const body = new TextDecoder().decode(mem.subarray(ptr + 4, ptr + 4 + len));
    wasm.beck_free(ptr);
    return JSON.parse(body);
  };

  const write = (bytes) => {
    const ptr = wasm.beck_alloc(bytes.length);
    memory().set(bytes, ptr);
    return ptr;
  };

  const call = (request) => {
    const bytes = new TextEncoder().encode(JSON.stringify(request));
    return read(wasm.beck_call(write(bytes), bytes.length));
  };

  const apply = (response) => {
    if (response.error) {
      beck.announce(root, "beck:error", response);
      return;
    }
    if (response.dom && response.dom.length) beck.apply(root, response.dom);
    save();
  };

  // ---- the local copy (D7 rung 2) ------------------------------------------
  //
  // "A Mode B component holds a local copy of its state and queues commands while offline." The
  // copy is the kernel's confirmed state and its unsent commands; this is somewhere to put it.
  //
  // The key carries the program's wire id and the actor, so a deployment that changes the command
  // channel's types cannot restore a snapshot of the old one, and one person's queue is not
  // another's. The kernel refuses a mismatch as well — twice, because a key is a convention and a
  // check is a rule.
  let store = null;
  const key = () => "beck:" + store.wire + ":" + actor;

  // Writing costs the size of the *state*, not of the change, so it is coalesced: a burst of
  // events persists once. What would remove the cost rather than spreading it is an append-only
  // local log — which is what D7's later rungs are about, and is not this.
  let pendingSave = null;
  const save = () => {
    if (!store || pendingSave) return;
    pendingSave = setTimeout(() => {
      pendingSave = null;
      try {
        const out = call({ op: "snapshot" });
        // A kernel that cannot produce one is a kernel that does not match this shim, and a client
        // that silently stops keeping a local copy is the failure mode worth being loud about.
        if (out.error || !out.snapshot) {
          beck.announce(root, "beck:error", out.error ? out : { error: "no local copy" });
          store = null;
          return;
        }
        localStorage.setItem(key(), JSON.stringify(out.snapshot));
      } catch (e) {
        // A full quota, a private window, a disabled store: the component still works, it just
        // will not survive a reload. Saying so once is better than failing an interaction.
        beck.announce(root, "beck:error", { error: "cannot store locally: " + e });
        store = null;
      }
    }, 200);
  };

  // The shell, cached, so that a reload with no network is a page rather than an error
  // (`docs/94` §94.13). Registered before the kernel is fetched so a first visit primes the cache
  // while the network is there; a browser without service workers simply skips this and keeps
  // every other property of the mode.
  if (navigator.serviceWorker && beck.shell) {
    navigator.serviceWorker
      .register("/beck-sw.js")
      .catch((e) => beck.announce(root, "beck:error", { error: "no shell cache: " + e }));
  }

  const start = async () => {
    // Through `beck.asset`, not `fetch`, for the same reason the socket goes through `beck.dial`:
    // in a playground tab the bundle comes from the worker that derived it and there is no origin
    // to fetch either of them from (docs/103).
    const [module, bundle] = await Promise.all([
      WebAssembly.instantiateStreaming(beck.asset("beck-kernel.wasm"), {}),
      beck.asset("beck-bundle.bpk").then((r) => r.arrayBuffer()),
    ]);
    wasm = module.instance.exports;

    // `<u32 len><viewer json><bundle>` — the viewer first because the kernel needs it to build the
    // `Session` the view is rendered against.
    // The route is part of the viewer, and it is read off the address bar rather than restored
    // from the local copy: after a reload the URL is the browser's own answer to "where am I", and
    // a snapshot that disagreed with it would render a page the URL does not name. A document with
    // no URL of its own is at the root, which is `beck.here`'s job to know.
    const name = new TextEncoder().encode(
      JSON.stringify({ actor, claims, path: beck.here() }),
    );
    const payload = new Uint8Array(4 + name.length + bundle.byteLength);
    new DataView(payload.buffer).setUint32(0, name.length, true);
    payload.set(name, 4);
    payload.set(new Uint8Array(bundle), 4 + name.length);
    const loaded = read(wasm.beck_load(write(payload), payload.length));
    if (loaded.error) {
      beck.announce(root, "beck:error", loaded);
      return;
    }

    store = { wire: loaded.wire };

    // Restore before connecting, so a browser with no network shows the state it had rather than
    // the empty one the fold starts from.
    let restored = false;
    const saved = localStorage.getItem(key());
    if (saved) {
      const out = call({ op: "restore", snapshot: JSON.parse(saved) });
      if (out.error) {
        // A snapshot of another program, or of another actor. Dropping it is the whole recovery:
        // the subscription is about to send this client a state anyway.
        localStorage.removeItem(key());
      } else {
        restored = true;
        state.seq = out.seq;
        if (out.dom && out.dom.length) beck.apply(root, out.dom);
        queued = out.queued || [];
        beck.stats.pending = queued.length;
      }
    }

    let hydrated = false;
    const send = beck.connect(state, (msg) => {
      // `s` is the whole accumulator (a fresh subscription); `d` is the difference. Everything
      // else is the protocol both modes share.
      if (msg.t === "s") {
        state.seq = msg.q;
        // The document was rendered from this state by the same `view`, so this client's first
        // render *is* what the DOM shows: adopt it, no DOM work, nothing can differ (docs/93
        // §93.5). Otherwise an event landed between the render and this socket opening, and the
        // page on screen is not this state's page — one rebuild, once.
        const adopt = !hydrated && msg.q === painted;
        hydrated = true;
        apply(call({ op: "reset", seq: msg.q, state: msg.v, adopt }));
      } else if (msg.t === "d") {
        state.seq = msg.q;
        apply(call({ op: "data", seq: msg.q, ops: msg.o }));
      } else if (msg.t === "u" || msg.t === "w") state.seq = msg.q;
      else if (msg.t === "a") {
        call({ op: "settle", id: msg.id, seq: msg.q });
        queued = queued.filter((q) => q.id !== msg.id);
        beck.stats.pending = queued.length;
        save();
      }
      else if (msg.t === "n") {
        // The server refused a command this client accepted — a race rather than a bug, and the
        // correction is to drop the guess and re-render.
        apply(call({ op: "refused", id: msg.id }));
        beck.announce(root, "beck:rejected", msg);
      }
    }, () => flush());

    // A route change is a local render: the kernel moves `session.path` and re-renders from the
    // state it already holds, so the page changes with no round trip at all. The server is told
    // anyway — not for the page, which it is not rendering, but so that the `Session` it hands
    // `validate` is the one this client's own `validate` saw. Both travel on the one socket, so
    // the navigation precedes the commands proposed from the page it produced.
    beck.route((path) => {
      beck.stats.navigations += 1;
      apply(call({ op: "nav", path }));
      send({ t: "g", path });
    });

    beck.capture((command) => {
      const id = beck.uuid7();
      const out = call({ op: "propose", id, command, at: Date.now() });
      if (out.accepted === false) {
        // Refused by the program's own `validate`, running here. No round trip, and the reason is
        // the program's `Rejection` rather than a string this file invented.
        beck.announce(root, "beck:rejected", { id, e: out.why });
        return;
      }
      apply(out);
      queued.push({ id, command });
      beck.stats.pending = queued.length;
      send({ t: "c", id, command });
    });

    // What Mode B can say about itself that Mode A cannot: the commands it is holding are *applied*
    // rather than merely sent, so "pending" here is the difference between what this browser shows
    // and what the server has agreed to.
    beck.inspect.describe = () => {
      const info = call({ op: "info" });
      return {
        mode: "B",
        seq: info.seq,
        actor,
        path: info.path,
        component: info.component,
        optimistic: info.optimistic,
        pending: queued.map((q) => q.id),
        in_flight: info.pending,
      };
    };
    beck.devtools();

    // Whatever this client owes the server — from this session or from the last one — goes up as
    // soon as there is a socket. Each carries the id it was proposed with, and the server
    // de-duplicates by it (§4.3), so a command sent twice is appended once. That is the whole of
    // why an offline queue needs no agreement between the two sides.
    flush = () => queued.forEach((q) => send({ t: "c", id: q.id, command: q.command }));

    // The component is live: the kernel holds the bundle, the socket is open and interactions are
    // being captured. Before this, a click reaches nothing — the handlers are installed at the end
    // of an asynchronous load — so a page with a spinner, a devtools panel, or a test has to be
    // able to tell "not yet" from "nothing happened".
    beck.ready(root, "b");
  };

  start().catch((e) => beck.announce(root, "beck:error", { error: String(e) }));
})();
