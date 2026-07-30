// The thin client: `fold(apply_patch, initial_html, patch_stream)`.
//
// This file is compiler residue. Nothing here is application-specific — no todo, no command
// names, no view logic — which is the point: "the JavaScript never appears in the source at all"
// (docs/00-original-idea.md). It applies patches, captures declarative handlers, and resumes a
// subscription by (subscription, seq). That is the whole of Mode A on the browser side.
(() => {
  const root = document.getElementById("b-root");
  if (!root) return;

  const actor = root.dataset.bActor || "dev";
  const scope = root.dataset.bScope || "all";
  const sub = uuid7();
  // The seq the server-rendered HTML reflects. The socket resumes from here, so first paint and
  // first patch cannot disagree, and hydration costs zero DOM work.
  let seq = Number(root.dataset.bSeq) || 0;

  // ---- patch interpreter -------------------------------------------------
  const at = (path) => {
    let node = root.firstElementChild;
    for (let i = 0; i < path.length; i++) node = node.childNodes[path[i]];
    return node;
  };

  const build = (h) => {
    if (typeof h === "string") return document.createTextNode(h);
    const el = document.createElement(h[0]);
    const attrs = h[1];
    for (const name in attrs) el.setAttribute(name, attrs[name]);
    const kids = h[2];
    for (let i = 0; i < kids.length; i++) el.appendChild(build(kids[i]));
    return el;
  };

  const apply = (ops) => {
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

  // ---- connection --------------------------------------------------------
  let socket = null;
  let backoff = 250;
  const outbox = [];

  const connect = () => {
    const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/socket";
    socket = new WebSocket(url);
    socket.onopen = () => {
      backoff = 250;
      socket.send(JSON.stringify({ t: "hello", sub, seq, actor, scope }));
      // Commands sent while disconnected are safe to repeat: each carries an id, and the server
      // de-duplicates by it.
      while (outbox.length) socket.send(outbox.shift());
    };
    socket.onmessage = (event) => {
      const msg = JSON.parse(event.data);
      if (msg.t === "p") { apply(msg.o); seq = msg.q; }
      // "current as of q, nothing changed" — keeps `seq` moving so a later reconnect does not
      // ask the server to replay a gap that turns out to be empty.
      else if (msg.t === "u" || msg.t === "w") { seq = msg.q; }
      else if (msg.t === "n") { root.dispatchEvent(new CustomEvent("beck:rejected", { detail: msg })); }
    };
    socket.onclose = () => {
      socket = null;
      setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 5000);
    };
    socket.onerror = () => socket && socket.close();
  };

  const send = (command) => {
    const frame = JSON.stringify({ t: "c", id: uuid7(), command });
    if (socket && socket.readyState === 1) socket.send(frame);
    else outbox.push(frame);
  };

  // ---- input capture -----------------------------------------------------
  // Handlers in `view` compiled to attributes, so no user JavaScript runs here and `script-src`
  // can stay near-empty. `$id` and `$value` are the only two holes.
  const fill = (template, value) => {
    const command = JSON.parse(template);
    for (const key in command) {
      if (command[key] === "$id") command[key] = uuid7();
      else if (command[key] === "$value") command[key] = value;
    }
    return command;
  };

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

  // ---- client-minted ids -------------------------------------------------
  // "Client-generated UUIDs are the small tell that browsers here are replicas, not terminals":
  // the client must be able to name a todo before the server confirms it exists.
  function uuid7() {
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
  }

  connect();
})();
