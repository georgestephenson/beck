// The patch interpreter, the router, the input capture and the id source. Shared by both modes.
//
// Mode A receives these ops from the server; Mode B's kernel produces them in the browser from its
// own two renders. Same vocabulary, same interpreter — which is not a convenience, it is the claim:
// "hand-written JavaScript never appears in the source — it's compiler residue", and there is one
// piece of residue rather than one per mode.
(() => {
  // An element belongs to a namespace, and `createElement` only ever guesses HTML.
  //
  // Server-side rendering goes through the browser's own HTML parser, which knows that `<svg>`
  // opens a different namespace and that `<foreignObject>` closes it again. This interpreter is the
  // other half of the same page and has to know the same thing: an `svg` built as HTML is not an
  // `SVGElement`, so it lays out as nothing and a chart that paints on first load vanishes the
  // first time its data changes. `createElementNS` is also what keeps `linearGradient` and
  // `clipPath` their own case, which `createElement` lowercases in an HTML document.
  const HTML = "http://www.w3.org/1999/xhtml";
  const SVG = "http://www.w3.org/2000/svg";

  // Which namespace a *child* of this node is built in. Inherited, except at the two edges.
  const within = (node) =>
    node && node.namespaceURI === SVG && node.localName !== "foreignObject" ? SVG : HTML;

  const build = (h, ns) => {
    if (typeof h === "string") return document.createTextNode(h);
    const tag = h[0];
    // A patch can carry a subtree whose root is `svg` or a subtree that starts inside one, so the
    // namespace comes from the tag when the tag opens one and from the destination otherwise.
    const here = tag === "svg" ? SVG : ns || HTML;
    const el = document.createElementNS(here, tag);
    // Pairs, in the order the server wrote them: an element rebuilt here carries its attributes in
    // the order the same element has in the server-rendered document.
    const attrs = h[1];
    for (let a = 0; a < attrs.length; a++) el.setAttribute(attrs[a][0], attrs[a][1]);
    const kids = h[2];
    const inner = here === SVG && tag !== "foreignObject" ? SVG : HTML;
    for (let i = 0; i < kids.length; i++) el.appendChild(build(kids[i], inner));
    return el;
  };

  // What this client has sent, received and applied. Read by the devtools panel and by nothing
  // else — no behaviour depends on it, which is the point: a counter that changes what the page
  // does is a second implementation of the page.
  const stats = {
    frames: 0,
    ops: 0,
    bytes_in: 0,
    bytes_out: 0,
    sent: 0,
    navigations: 0,
    pending: 0,
    connected: false,
  };

  // ---- where the caret and the scroll are, across a patch --------------------------------------
  //
  // A patch that replaces an ancestor of the focused element destroys it, and the browser's answer
  // to "what is focused now" is `body`. So the page loses the caret in the middle of typing, and
  // the list somebody had scrolled jumps back to the top. Neither is a defect of the diff — the
  // page really did change — and neither is the program's to think about, so it is handled here,
  // once, for both modes.
  //
  // The cost is proportional to *the patch* and not to the page: nothing is walked except the
  // subtrees a replace is about to destroy, and the focused element is one lookup.

  const indexPath = (from, node) => {
    const path = [];
    let at = node;
    while (at && at !== from) {
      const parent = at.parentNode;
      if (!parent) return null;
      path.unshift(Array.prototype.indexOf.call(parent.childNodes, at));
      at = parent;
    }
    return at === from ? path : null;
  };

  const descend = (from, path) => {
    let node = from;
    for (let i = 0; i < path.length && node; i++) node = node.childNodes[path[i]];
    return node;
  };

  // Enough of an element's identity to refuse to restore the caret into a *different* element that
  // happens to have taken its place. A key, a name or an id is the program's own answer; the tag is
  // the floor.
  const identity = (el) =>
    el.tagName +
    "|" +
    (el.getAttribute("data-b-k") || "") +
    "|" +
    (el.getAttribute("name") || "") +
    "|" +
    (el.id || "");

  const caret = (root) => {
    const el = document.activeElement;
    if (!el || el === document.body || !root.contains(el)) return null;
    const path = indexPath(root, el);
    if (!path) return null;
    const range =
      typeof el.selectionStart === "number"
        ? { start: el.selectionStart, end: el.selectionEnd, dir: el.selectionDirection }
        : null;
    return { el, path, range, id: identity(el), top: el.scrollTop, left: el.scrollLeft };
  };

  const restoreCaret = (root, was) => {
    // Still there: the patch did not touch it, and re-focusing would move the caret for nothing.
    if (!was || was.el.isConnected) return;
    const now = descend(root, was.path);
    if (!now || now.nodeType !== 1 || identity(now) !== was.id || !now.focus) return;
    now.focus();
    if (was.range && now.setSelectionRange) {
      try {
        now.setSelectionRange(was.range.start, was.range.end, was.range.dir);
      } catch (e) {
        // A type whose selection cannot be set (`number`, `email` in some browsers). The focus is
        // the part that matters and it is already restored.
      }
    }
    now.scrollTop = was.top;
    now.scrollLeft = was.left;
  };

  // Scroll offsets inside a subtree about to be replaced, by position within it. Best effort by
  // construction — a replaced subtree is one whose shape may have changed — so it is keyed by
  // position and restored only where the position still holds something scrollable.
  const scrollsIn = (node, path, out) => {
    if (node.nodeType !== 1) return;
    if (node.scrollTop || node.scrollLeft) {
      out.push({ path, top: node.scrollTop, left: node.scrollLeft });
    }
    for (let i = 0; i < node.childNodes.length; i++) {
      scrollsIn(node.childNodes[i], path.concat(i), out);
    }
  };

  const apply = (root, ops) => {
    const at = (path) => {
      let node = root.firstElementChild;
      for (let i = 0; i < path.length; i++) node = node.childNodes[path[i]];
      return node;
    };
    const was = caret(root);
    const scrolled = [];
    for (let i = 0; i < ops.length; i++) {
      if (ops[i][0] !== 0) continue; // only a replace rebuilds what was scrolled
      const victim = at(ops[i][1]);
      if (victim) scrollsIn(victim, [], (scrolled[i] = []));
    }

    for (let i = 0; i < ops.length; i++) {
      const op = ops[i];
      const path = op[1];
      switch (op[0]) {
        case 0: { // replace
          const node = at(path);
          // The namespace of what is being built comes from where it is going, which is the parent
          // of what it replaces — an op whose root is a `rect` says nothing about namespaces itself.
          const next = build(op[2], within(node ? node.parentNode : root));
          if (node) node.replaceWith(next);
          else root.appendChild(next);
          const kept = scrolled[i];
          for (let s = 0; kept && s < kept.length; s++) {
            const target = descend(next, kept[s].path);
            if (target && target.nodeType === 1) {
              target.scrollTop = kept[s].top;
              target.scrollLeft = kept[s].left;
            }
          }
          break;
        }
        case 1: at(path).textContent = op[2]; break;        // set text
        case 2: at(path).setAttribute(op[2], op[3]); break;  // set attribute
        case 3: at(path).removeAttribute(op[2]); break;      // remove attribute
        case 4: { // insert child
          const parent = at(path);
          parent.insertBefore(build(op[3], within(parent)), parent.childNodes[op[2]] || null);
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
    restoreCaret(root, was);
    stats.frames += 1;
    stats.ops += ops.length;
    announce(root, "beck:traffic", stats);
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
  // stay near-empty. Three holes, and a command is filled *recursively* — a command whose field is
  // a record has its holes one level down, and a filler that only looked at the top would leave
  // the literal `"$id"` in the log.
  //
  // | hole | filled with |
  // |---|---|
  // | `$id` | a fresh UUIDv7, minted here so the client can name a thing before the server has it |
  // | `$value` | the value of the element the handler is on |
  // | `$field:name` | the value of the form control called `name`, in the form being submitted |
  const fill = (template, value, form) => {
    const hole = (v) => {
      if (v === "$id") return uuid7();
      if (v === "$value") return value === null ? "" : value;
      if (typeof v === "string" && v.startsWith("$field:")) {
        const named = form && form.elements[v.slice("$field:".length)];
        if (!named) return "";
        if (named.type === "checkbox") return named.checked;
        return named.value === undefined ? "" : named.value;
      }
      return v;
    };
    const walk = (node) => {
      if (typeof node === "string") return hole(node);
      if (Array.isArray(node)) return node.map(walk);
      if (node && typeof node === "object") {
        const out = {};
        for (const key in node) out[key] = walk(node[key]);
        return out;
      }
      return node;
    };
    return walk(JSON.parse(template));
  };

  // Declared handlers, captured once. `send` is whatever the mode does with a command: post it up
  // the socket (Mode A), or apply it locally first and then post it (Mode B).
  //
  // Every event listened for is a `data-b-<event>` attribute the `ui:` macro wrote from an
  // `on_<event>=` in the program, so this file names events and never commands.
  const capture = (send) => {
    const on = (kind, attribute, handler) =>
      document.addEventListener(kind, (event) => {
        const el = event.target.closest && event.target.closest("[" + attribute + "]");
        if (el) handler(el, event);
      });

    on("click", "data-b-click", (el) => send(fill(el.getAttribute("data-b-click"), null)));

    on("keydown", "data-b-enter", (el, event) => {
      if (event.key !== "Enter") return;
      const value = el.value || "";
      if (!value.trim()) return;
      send(fill(el.getAttribute("data-b-enter"), value));
      el.value = "";
    });

    // A form. The browser's own submit — a button, or Enter in a single-line field — so a page
    // built out of `form:` and `input(name=…)` is one a keyboard and a screen reader already know
    // how to drive, and `$field:name` is how the program names what was typed.
    on("submit", "data-b-submit", (el, event) => {
      event.preventDefault();
      send(fill(el.getAttribute("data-b-submit"), null, el));
      el.reset();
    });

    // A control that reports as it changes. `input` fires per keystroke and `change` on commit,
    // which is the browser's distinction rather than one invented here.
    on("input", "data-b-input", (el) =>
      send(fill(el.getAttribute("data-b-input"), controlValue(el), el.form)));
    on("change", "data-b-change", (el) =>
      send(fill(el.getAttribute("data-b-change"), controlValue(el), el.form)));
  };

  const controlValue = (el) =>
    el.type === "checkbox" ? el.checked : el.value === undefined ? "" : el.value;

  // ---- the router -----------------------------------------------------------------------------
  //
  // A route is `session.path`, so navigating is not a fetch and not a route table: it is the same
  // page function of a different session, and the only thing this has to do is (a) keep the address
  // bar honest and (b) say where the client is. In Mode A that is a message and a patch back; in
  // Mode B the kernel re-renders locally and the server is told only so that the `Session` it hands
  // `validate` is the one the client's own `validate` saw.
  //
  // An ordinary `<a href>` — no `data-b-` attribute, no `onclick`, nothing in the program that
  // knows a router exists. Which is the point: a link that this file did not intercept is still a
  // link, and the page it lands on is server-rendered at that path.

  // Does this document have a URL of its own?
  //
  // A `srcdoc` iframe does not — which is what the playground's clients are (docs/98) — and neither
  // does a blob. Their `location.pathname` is `srcdoc` or a UUID, so a client that read a route off
  // it would report one no program could ever match, and nothing would say so.
  const addressed = () => location.protocol === "http:" || location.protocol === "https:";

  // Where this document is, as a route. The application's root when there is no address bar.
  const here = () => (addressed() ? location.pathname : "/");

  const route = (go) => {
    // A document with no URL cannot navigate: `pushState` has no address bar to move, and the
    // links inside it are not this page's to intercept.
    if (!addressed()) return;
    document.addEventListener("click", (event) => {
      if (event.defaultPrevented || event.button !== 0) return;
      if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const a = event.target.closest && event.target.closest("a[href]");
      // `target`, `download` and `rel=external` are three ways of writing "not an in-page
      // navigation", and each is the author's statement rather than a guess about intent.
      if (!a || a.target || a.hasAttribute("download") || a.getAttribute("rel") === "external") {
        return;
      }
      const url = new URL(a.getAttribute("href"), location.href);
      if (url.origin !== location.origin) return;
      if (url.pathname === location.pathname && url.hash) return; // an anchor on this page
      event.preventDefault();
      if (url.pathname === location.pathname && url.search === location.search) return;
      history.pushState(null, "", url.pathname + url.search + url.hash);
      go(location.pathname);
    });
    // Back and forward. The address bar has already moved by the time this fires, so the route is
    // read off `location` rather than carried in the history entry — one source for where the
    // client is, and it is the browser's.
    window.addEventListener("popstate", () => go(location.pathname));
  };

  // The transport, as a seam. A deployment opens a websocket to the origin it was served from;
  // the playground hands the identical protocol a `MessageChannel` port to a worker in the same
  // tab (docs/17 §17.2). Everything above and below this function — the frames, the resumption
  // rule, the outbox — is the same code either way, which is what makes a tab a *host* rather
  // than a simulation.
  //
  // The contract, for whoever writes the third one: `dial(handlers)` returns `{send, ready}` and
  // optionally `close`, and calls `handlers.open` *after* returning — a transport that is ready
  // the moment it is dialled has to defer, because `open` is where the `hello` frame is sent and
  // it sends it through the object `dial` has not handed back yet. A transport without `close`
  // is dropped rather than closed, so its connection lives until whatever owns it goes away.
  //
  // Where the mode's own artefacts come from, as a seam — the kernel module and the component's
  // bundle in Mode B. A deployment fetches them from the origin it was served from, on the two
  // reserved routes `beck_rt::http` answers; the playground has no server behind the frame, so it
  // hands over the bundle its worker derived and reads the kernel from the directory the page was
  // deployed to (docs/98).
  //
  // The contract: `asset(name)` returns a promise of a `Response`, because that is what
  // `WebAssembly.instantiateStreaming` takes and a synthetic one is a constructor call.
  const asset = (name) => fetch("/" + name);

  const websocket = (handlers) => {
    const url = (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/socket";
    const socket = new WebSocket(url);
    socket.onopen = handlers.open;
    // The bytes are counted here rather than in `connect`, because they are a fact about *this*
    // transport: a port transport hands over objects and never encodes one. A devtools panel
    // reading a zero is therefore reading the truth about a playground tab rather than a bug.
    socket.onmessage = (event) => {
      stats.bytes_in += event.data.length;
      handlers.message(JSON.parse(event.data));
    };
    socket.onclose = handlers.close;
    socket.onerror = () => socket.close();
    return {
      send: (frame) => {
        const text = JSON.stringify(frame);
        stats.bytes_out += text.length;
        socket.send(text);
      },
      ready: () => socket.readyState === 1,
      close: () => socket.close(),
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
  //
  // `state.path` rides on the `hello` for a related reason: a route established by a second frame
  // would leave every reconnection rendering the root's page until that frame arrived.
  //
  // The returned sender carries a `close`, because the reconnect below has to be stoppable by
  // something other than the frame being destroyed. Nothing else holds the retry timer.
  const connect = (state, on, opened) => {
    let backoff = 250;
    const outbox = [];
    let link = null;
    let retry = null;
    let stopped = false;
    const open = () => {
      retry = null;
      link = (beck.dial || websocket)({
        open: () => {
          backoff = 250;
          stats.connected = true;
          // The route rides on the `hello` for the reason above: a route established by a second
          // frame would leave every reconnection rendering the root's page until it arrived.
          const hello = {
            t: "hello",
            sub: state.sub,
            actor: state.actor,
            path: here(),
          };
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
          stats.connected = false;
          if (stopped) return;
          retry = setTimeout(open, backoff);
          backoff = Math.min(backoff * 2, 5000);
        },
      });
    };
    open();
    const send = (frame) => {
      // Counted here rather than in the transport, because a frame is a frame whichever one is
      // under it. The *bytes* are not: they are counted in `websocket` below, since a port
      // transport moves objects and has none.
      stats.sent += 1;
      if (link && link.ready()) link.send(frame);
      else outbox.push(frame);
    };
    // Stop dialling. Both lines are load-bearing, for the two ways a client is closed: after the
    // connection dropped there is a retry already armed, which is the `clearTimeout`; while it is
    // still up the close travels through the transport and the handler above runs on the way out,
    // which is the flag. A socket reports its own closing and cannot say whether it was asked for.
    send.close = () => {
      stopped = true;
      clearTimeout(retry);
      retry = null;
      const closing = link;
      link = null;
      stats.connected = false;
      if (closing && closing.close) closing.close();
    };
    return send;
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

  // What a devtools panel is allowed to know, and the only way it is allowed to know it: a mode
  // registers what it can say about itself, and the panel reads. Nothing here computes a second
  // account of anything.
  const inspect = { stats, describe: () => ({}) };

  // The panel is loaded on request, not on every page. `?devtools` turns it on and leaves the
  // switch behind, so a reload — and a route change — keeps it; `?devtools=off` clears it.
  const devtools = () => {
    let want = null;
    try {
      const asked = new URL(location.href).searchParams.get("devtools");
      if (asked !== null) {
        want = asked !== "off" && asked !== "0";
        localStorage.setItem("beck:devtools", want ? "1" : "");
      } else {
        want = localStorage.getItem("beck:devtools") === "1";
      }
    } catch (e) {
      want = false; // no `localStorage` in this context; the page is unaffected
    }
    if (!want) return;
    const script = document.createElement("script");
    script.src = "/beck-devtools.js";
    document.body.appendChild(script);
  };

  // `dial` is deliberately absent rather than null: a page that wants a different transport sets
  // it on this object before the mode's script runs, and one that does not gets a websocket.
  // `asset` is present and overridable for the same reason, and `shell` says whether this document
  // may cache itself — a frame with no origin of its own may not, and says so rather than failing a
  // registration nobody reads.
  window.beck = {
    build, apply, uuid7, fill, capture, connect, announce, ready, route, here, stats, inspect,
    devtools, asset, shell: true,
  };
})();
