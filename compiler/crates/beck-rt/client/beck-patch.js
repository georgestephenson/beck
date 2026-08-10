// The patch interpreter, and the id source. Shared by both rendering modes.
//
// Mode A receives these ops from the server; Mode B's kernel produces them in the browser from its
// own two renders. Same vocabulary, same interpreter — which is not a convenience, it is the claim:
// "hand-written JavaScript never appears in the source — it's compiler residue", and there is one
// piece of residue rather than one per mode.
(() => {
  const build = (h) => {
    if (typeof h === "string") return document.createTextNode(h);
    const el = document.createElement(h[0]);
    // Pairs, in the order the server wrote them: an element rebuilt here carries its attributes in
    // the order the same element has in the server-rendered document.
    const attrs = h[1];
    for (let a = 0; a < attrs.length; a++) el.setAttribute(attrs[a][0], attrs[a][1]);
    const kids = h[2];
    for (let i = 0; i < kids.length; i++) el.appendChild(build(kids[i]));
    return el;
  };

  const apply = (root, ops) => {
    const at = (path) => {
      let node = root.firstElementChild;
      for (let i = 0; i < path.length; i++) node = node.childNodes[path[i]];
      return node;
    };
    for (let i = 0; i < ops.length; i++) {
      const op = ops[i];
      const path = op[1];
      switch (op[0]) {
        case 0: { // replace
          const next = build(op[2]);
          const node = at(path);
          if (node) node.replaceWith(next);
          else root.appendChild(next);
          break;
        }
        case 1: at(path).textContent = op[2]; break;        // set text
        case 2: at(path).setAttribute(op[2], op[3]); break;  // set attribute
        case 3: at(path).removeAttribute(op[2]); break;      // remove attribute
        case 4: { // insert child
          const parent = at(path);
          parent.insertBefore(build(op[3]), parent.childNodes[op[2]] || null);
          break;
        }
        case 5: { // remove child
          const parent = at(path);
          parent.removeChild(parent.childNodes[op[2]]);
          break;
        }
        case 6: { // move child (from > to), which is what preserves focus and scroll on reorder
          const parent = at(path);
          parent.insertBefore(parent.childNodes[op[2]], parent.childNodes[op[3]]);
          break;
        }
      }
    }
  };

  // "Client-generated UUIDs are the small tell that browsers here are replicas, not terminals":
  // the client must be able to name a todo before the server confirms it exists.
  const uuid7 = () => {
    const now = Date.now();
    const b = crypto.getRandomValues(new Uint8Array(16));
    b[0] = now / 2 ** 40; b[1] = now / 2 ** 32; b[2] = now / 2 ** 24;
    b[3] = now / 2 ** 16; b[4] = now / 2 ** 8;  b[5] = now;
    b[6] = (b[6] & 0x0f) | 0x70;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex = "";
    for (let i = 0; i < 16; i++) hex += b[i].toString(16).padStart(2, "0");
    return hex.slice(0, 8) + "-" + hex.slice(8, 12) + "-" + hex.slice(12, 16) + "-" +
           hex.slice(16, 20) + "-" + hex.slice(20);
  };

  // Handlers in `view` compiled to attributes, so no user JavaScript runs and `script-src` can
  // stay near-empty. `$id` and `$value` are the only two holes.
  const fill = (template, value) => {
    const command = JSON.parse(template);
    for (const key in command) {
      if (command[key] === "$id") command[key] = uuid7();
      else if (command[key] === "$value") command[key] = value;
    }
    return command;
  };

  // Declared handlers, captured once. `send` is whatever the mode does with a command: post it up
  // the socket (Mode A), or apply it locally first and then post it (Mode B).
  const capture = (send) => {
    document.addEventListener("click", (event) => {
      const el = event.target.closest && event.target.closest("[data-b-click]");
      if (el) send(fill(el.getAttribute("data-b-click"), null));
    });
    document.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      const el = event.target.closest && event.target.closest("[data-b-enter]");
      if (!el) return;
      const value = el.value || "";
      if (!value.trim()) return;
      send(fill(el.getAttribute("data-b-enter"), value));
      el.value = "";
    });
  };

  // The transport, as a seam. A deployment opens a websocket to the origin it was served from;
  // the playground hands the identical protocol a `MessageChannel` port to a worker in the same
  // tab (docs/17 §17.2). Everything above and below this function — the frames, the resumption
  // rule, the outbox — is the same code either way, which is what makes a tab a *host* rather
  // than a simulation.
  //
  // The contract, for whoever writes the third one: `dial(handlers)` returns `{send, ready}`, and
  // calls `handlers.open` *after* returning — a transport that is ready the moment it is dialled
  // has to defer, because `open` is where the `hello` frame is sent and it sends it through the
  // object `dial` has not handed back yet.
  const websocket = (handlers) => {
    const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/socket";
    const socket = new WebSocket(url);
    socket.onopen = handlers.open;
    socket.onmessage = (event) => handlers.message(JSON.parse(event.data));
    socket.onclose = handlers.close;
    socket.onerror = () => socket.close();
    return {
      send: (frame) => socket.send(JSON.stringify(frame)),
      ready: () => socket.readyState === 1,
    };
  };

  // One connection, resumable by `(subscription, seq)`. `on` is the frame handler the mode
  // supplies.
  //
  // `state` is read at every open rather than at the first one, so a caller that keeps
  // `state.seq` current resumes from where it actually is. A snapshot would make every reconnect
  // ask for the gap since first paint and then apply it to a DOM that had already moved.
  //
  // A null `state.seq` is sent as an *absent* field, which is the protocol's "I hold nothing".
  // Zero would mean "I hold the frame as of zero", which is true of a server-rendered document
  // and false of a client that has only just started.
  const connect = (state, on, opened) => {
    let backoff = 250;
    const outbox = [];
    let link = null;
    const open = () => {
      link = (beck.dial || websocket)({
        open: () => {
          backoff = 250;
          const hello = { t: "hello", sub: state.sub, actor: state.actor };
          if (state.seq !== null && state.seq !== undefined) hello.seq = state.seq;
          link.send(hello);
          // Commands sent while disconnected are safe to repeat: each carries an id, and the
          // server de-duplicates by it.
          while (outbox.length) link.send(outbox.shift());
          // Every open, not only the first: a client that has been away has a queue that predates
          // this connection, and possibly this page load (`beck-mode-b.js`).
          if (opened) opened();
        },
        message: on,
        close: () => {
          link = null;
          setTimeout(open, backoff);
          backoff = Math.min(backoff * 2, 5000);
        },
      });
    };
    open();
    return (frame) => {
      if (link && link.ready()) link.send(frame);
      else outbox.push(frame);
    };
  };

  // An event anybody can listen for, on any ancestor. `bubbles` because the natural place to
  // listen is `document` — a page showing "reconnecting…" should not have to know which element
  // the residue chose as its frame root.
  const announce = (root, kind, detail) =>
    root.dispatchEvent(new CustomEvent(kind, { detail, bubbles: true }));

  // "This component is live." `data-b-ready` is the mode's letter, so a stylesheet can hide a
  // spinner with a selector and a script can wait for one attribute whichever mode it is in.
  const ready = (root, mode) => {
    if (root.dataset.bReady === mode) return;
    root.dataset.bReady = mode;
    announce(root, "beck:ready", { mode });
  };

  // `dial` is deliberately absent rather than null: a page that wants a different transport sets
  // it on this object before the mode's script runs, and one that does not gets a websocket.
  window.beck = { build, apply, uuid7, fill, capture, connect, announce, ready };
})();
