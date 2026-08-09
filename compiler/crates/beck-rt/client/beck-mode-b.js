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
  const sub = beck.uuid7();
  // The position the server-rendered document reflects. It is *not* what this client resumes
  // from: a Mode A client resuming at `seq` is claiming to hold the page as of `seq`, and the
  // document is that page — but a Mode B client holds the *state*, and it starts holding nothing
  // but `init`. So the first connection asks for everything (`seq: 0`) and `state.seq` moves only
  // as frames arrive.
  const painted = Number(root.dataset.bSeq) || 0;
  const state = { sub, seq: 0, actor };

  let wasm = null;
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
      root.dispatchEvent(new CustomEvent("beck:error", { detail: response }));
      return;
    }
    if (response.dom && response.dom.length) beck.apply(root, response.dom);
  };

  const start = async () => {
    const [module, bundle] = await Promise.all([
      WebAssembly.instantiateStreaming(fetch("/beck-kernel.wasm"), {}),
      fetch("/beck-bundle.bpk").then((r) => r.arrayBuffer()),
    ]);
    wasm = module.instance.exports;

    // `<u32 len><actor><bundle>` — the actor first because the kernel needs it to build the
    // `Session` the view is rendered against.
    const name = new TextEncoder().encode(actor);
    const payload = new Uint8Array(4 + name.length + bundle.byteLength);
    new DataView(payload.buffer).setUint32(0, name.length, true);
    payload.set(name, 4);
    payload.set(new Uint8Array(bundle), 4 + name.length);
    const loaded = read(wasm.beck_load(write(payload), payload.length));
    if (loaded.error) {
      root.dispatchEvent(new CustomEvent("beck:error", { detail: loaded }));
      return;
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
      else if (msg.t === "a") call({ op: "settle", id: msg.id, seq: msg.q });
      else if (msg.t === "n") {
        // The server refused a command this client accepted — a race rather than a bug, and the
        // correction is to drop the guess and re-render.
        apply(call({ op: "refused", id: msg.id }));
        root.dispatchEvent(new CustomEvent("beck:rejected", { detail: msg }));
      }
    });

    beck.capture((command) => {
      const id = beck.uuid7();
      const out = call({ op: "propose", id, command, at: Date.now() });
      if (out.accepted === false) {
        // Refused by the program's own `validate`, running here. No round trip, and the reason is
        // the program's `Rejection` rather than a string this file invented.
        root.dispatchEvent(new CustomEvent("beck:rejected", { detail: { id, e: out.why } }));
        return;
      }
      apply(out);
      send({ t: "c", id, command });
    });
  };

  start().catch((e) =>
    root.dispatchEvent(new CustomEvent("beck:error", { detail: { error: String(e) } })),
  );
})();
