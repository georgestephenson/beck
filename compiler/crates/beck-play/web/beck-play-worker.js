// The worker-server: the compiler and the application, off the page's thread.
//
// docs/17 §17.2: "the 'server' — ingress, `validate`, folds, Mode A rendering — in a **web
// worker**". Everything below is transport. What decides anything is the WebAssembly module, and
// what the module contains is `beck_host::sequence`, `beck_host::Runtime` and `beck_core::diff` —
// the same code `beck run` executes.
//
// The boundary is four exports and a length-prefixed byte buffer, exactly as Mode B's kernel does
// it: no `wasm-bindgen`, no generated glue.

let wasm = null;

const bytes = (text) => new TextEncoder().encode(text);

// One request, one response. The module keeps every buffer it hands out until `beck_free`, so a
// reply is read out of linear memory and released here rather than trusted to a garbage collector
// that does not know about the other heap.
const call = (request) => {
  const body = bytes(JSON.stringify(request));
  const into = wasm.beck_alloc(body.length);
  new Uint8Array(wasm.memory.buffer, into, body.length).set(body);
  const out = wasm.beck_call(into, body.length);
  const header = new DataView(wasm.memory.buffer, out, 4);
  const length = header.getUint32(0, true);
  const text = new TextDecoder().decode(new Uint8Array(wasm.memory.buffer, out + 4, length));
  wasm.beck_free(out);
  const answer = JSON.parse(text);
  if (answer && answer.error !== undefined) throw new Error(answer.error);
  return answer;
};

// A client iframe's port, by the subscription it said it was. The map is what makes one command
// from one client move every client's page: a frame carries the subscription it is for, and this
// is where that name becomes a destination.
const ports = new Map();
const subscriptions = new WeakMap();

const route = (answer) => {
  for (const frame of (answer && answer.out) || []) {
    const port = ports.get(frame.sub);
    if (port) port.postMessage(frame.msg);
  }
};

const fromClient = (port, msg) => {
  try {
    if (msg.t === "hello") {
      ports.set(msg.sub, port);
      subscriptions.set(port, msg.sub);
      route(call({
        op: "hello",
        sub: msg.sub,
        actor: msg.actor,
        path: msg.path,
        seq: msg.seq,
        now: Date.now(),
      }));
      postMessage({ k: "moved" });
      return;
    }
    if (msg.t === "c") {
      const sub = subscriptions.get(port);
      route(call({ op: "command", sub, id: msg.id, command: msg.command, now: Date.now() }));
      postMessage({ k: "moved" });
      return;
    }
    // A route change. The page is a function of `session.path`, so in Mode A this comes back as a
    // patch and in Mode B as nothing at all — the kernel rendered it locally and this is the tab
    // being told, so that the session its `validate` sees is the one the client's own saw.
    if (msg.t === "g") {
      const sub = subscriptions.get(port);
      route(call({ op: "nav", sub, path: msg.path, now: Date.now() }));
      return;
    }
    if (msg.t === "ping") port.postMessage({ t: "pong" });
  } catch (why) {
    // A client that asked for something impossible learns that its command was refused; the page
    // learns why. Neither is left waiting, which is the failure this catch exists to prevent.
    if (msg.id) port.postMessage({ t: "n", id: msg.id, e: String(why.message || why) });
    postMessage({ k: "failed", why: String(why.message || why) });
  }
};

onmessage = async (event) => {
  const message = event.data;

  if (message.k === "boot") {
    const source = await WebAssembly.instantiateStreaming(fetch(message.module), {});
    wasm = source.instance.exports;
    postMessage({ k: "booted" });
    return;
  }

  // A new client. Its port is transferred, so the page cannot read what the two of them say to
  // each other — which is not a security boundary here, but is the shape a security boundary has.
  if (message.k === "client") {
    const port = event.ports[0];
    port.onmessage = (frame) => fromClient(port, frame.data);
    port.start();
    return;
  }

  if (message.k === "call") {
    try {
      postMessage({ k: "reply", n: message.n, res: call(message.req) });
    } catch (why) {
      postMessage({ k: "reply", n: message.n, err: String(why.message || why) });
    }
  }
};
