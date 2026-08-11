// The playground page: an editor, the compiler's answers, and two clients of one application.
//
// Nothing here compiles, folds, validates, renders, highlights or completes. Every answer on this
// page came out of the WebAssembly module in the worker, which is the compiler and the runtime this
// repository builds (docs/17 §17.1, §17.2; docs/103 for the editor, the log and the link). What
// this file does is arrange things on a screen and pass ports between them.
(() => {
  const $ = (id) => document.getElementById(id);
  const worker = new Worker("beck-play-worker.js");

  // ---------------------------------------------------------------- the worker, as a promise

  let next = 0;
  const waiting = new Map();
  let booted = null;
  let moved = () => {};

  worker.onmessage = (event) => {
    const message = event.data;
    if (message.k === "booted") return booted();
    if (message.k === "moved") return moved();
    if (message.k === "failed") return say(message.why, true);
    if (message.k === "reply") {
      const pending = waiting.get(message.n);
      waiting.delete(message.n);
      if (!pending) return;
      if (message.err !== undefined) pending.reject(new Error(message.err));
      else pending.resolve(message.res);
    }
  };

  const call = (req) =>
    new Promise((resolve, reject) => {
      const n = ++next;
      waiting.set(n, { resolve, reject });
      worker.postMessage({ k: "call", n, req });
    });

  const ready = new Promise((resolve) => {
    booted = resolve;
  });
  worker.postMessage({ k: "boot", module: "beck-play.wasm" });

  const escape_text = (s) =>
    String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));

  // base64url, unpadded — what `beck_core::digest::base64_encode_bytes` writes, and what `atob`
  // does not read: the alphabet differs in two characters and the padding it wants is absent.
  const bytes_of = (text) => {
    const standard = text.replace(/-/g, "+").replace(/_/g, "/");
    const padded = standard + "=".repeat((4 - (standard.length % 4)) % 4);
    return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
  };

  // ---------------------------------------------------------------- the editor
  //
  // A `<textarea>` for the editing and a `<pre>` underneath it for the paint. The textarea is what
  // knows about undo, selection, IME and screen readers, and none of that is worth reimplementing
  // to get colour; what it cannot do is style its own contents, so the same text is laid out twice
  // in the same metrics and the top copy is transparent.
  //
  // Both the colours and the squiggles come from `beck_core::editor` — the module `beck lsp` asks
  // the same questions of. Every offset that crosses the boundary is a UTF-16 offset, because that
  // is what `textarea.value` counts in and the conversion belongs where the text is (docs/103).

  let marks = [];

  const paint = async () => {
    const source = $("source").value;
    const answer = await call({ op: "tokens", source });
    $("highlight").innerHTML = layers(source, answer.tokens, marks);
    sync_scroll();
  };

  // One pass over the text, cut at every boundary either a token or a diagnostic introduces, so a
  // squiggle that starts inside a string is still one span and the two never fight over a
  // character. Ordering by position and sweeping is the whole algorithm.
  const layers = (source, tokens, marks) => {
    const cuts = new Set([0, source.length]);
    for (const t of tokens) { cuts.add(t.s); cuts.add(t.e); }
    for (const m of marks) { cuts.add(m.s); cuts.add(m.e); }
    const points = [...cuts].filter((c) => c >= 0 && c <= source.length).sort((a, b) => a - b);
    let out = "";
    for (let i = 0; i + 1 < points.length; i++) {
      const [from, to] = [points[i], points[i + 1]];
      const token = tokens.find((t) => t.s <= from && t.e >= to);
      const mark = marks.find((m) => m.s <= from && m.e >= to);
      const classes = [token ? "k-" + token.k : "", mark ? (mark.error ? "m-error" : "m-warning") : ""]
        .filter(Boolean)
        .join(" ");
      const text = escape_text(source.slice(from, to));
      out += classes ? `<span class="${classes}">${text}</span>` : text;
    }
    // A trailing newline collapses in a `<pre>`; one more keeps the last line's height so the
    // caret on it is not painted over the box's edge.
    return out + "\n";
  };

  const sync_scroll = () => {
    $("highlight").scrollTop = $("source").scrollTop;
    $("highlight").scrollLeft = $("source").scrollLeft;
  };

  // The first error, under the editor, in the compiler's own words — a squiggle says *where* and a
  // person still has to be told *what*.
  const show_first_mark = () => {
    const first = marks.find((m) => m.error) || marks[0];
    $("under").textContent = first ? `${first.code}: ${first.message}` : "";
  };

  // ---------------------------------------------------------------- completion

  let offered = [];
  let chosen = 0;

  const complete = async () => {
    const source = $("source").value;
    const offset = $("source").selectionStart;
    const answer = await call({ op: "complete", source, offset });
    offered = answer.items.slice(0, 40);
    chosen = 0;
    show_completions(answer.prefix);
  };

  const show_completions = (prefix) => {
    const list = $("complete");
    if (!offered.length) return hide_completions();
    list.textContent = "";
    offered.forEach((item, i) => {
      const li = document.createElement("li");
      li.className = i === chosen ? "on" : "";
      li.dataset.label = item.label;
      const label = document.createElement("span");
      label.textContent = item.label;
      const detail = document.createElement("span");
      detail.className = "detail";
      detail.textContent = item.detail || item.kind;
      li.append(label, detail);
      li.onmousedown = (e) => {
        e.preventDefault();
        chosen = i;
        accept(prefix);
      };
      list.appendChild(li);
    });
    list.dataset.prefix = prefix;
    place_completions();
    list.hidden = false;
  };

  const hide_completions = () => {
    offered = [];
    $("complete").hidden = true;
  };

  // Where the caret is, measured rather than calculated: a hidden copy of the text up to the caret
  // is laid out in the editor's own metrics, and where it ends is where the caret is. The
  // alternative — multiplying a column by an assumed character width — is wrong for the first
  // proportional glyph anybody types.
  const place_completions = () => {
    const source = $("source");
    const ghost = document.createElement("pre");
    const style = getComputedStyle(source);
    for (const p of ["font", "padding", "tabSize", "whiteSpace", "lineHeight"]) ghost.style[p] = style[p];
    ghost.style.position = "absolute";
    ghost.style.visibility = "hidden";
    ghost.textContent = source.value.slice(0, source.selectionStart);
    const tail = document.createElement("span");
    tail.textContent = "​";
    ghost.appendChild(tail);
    $("editor").appendChild(ghost);
    const at = tail.getBoundingClientRect();
    const box = $("editor").getBoundingClientRect();
    ghost.remove();
    const list = $("complete");
    list.style.left = `${Math.max(0, at.left - box.left - source.scrollLeft)}px`;
    list.style.top = `${at.bottom - box.top - source.scrollTop}px`;
  };

  // Replace the word behind the caret with the chosen name. The prefix is the module's answer to
  // "what is being replaced", not this file's guess at it.
  const accept = (prefix) => {
    const item = offered[chosen];
    if (!item) return;
    const source = $("source");
    const at = source.selectionStart;
    const before = source.value.slice(0, at - prefix.length);
    const after = source.value.slice(at);
    source.value = before + item.label + after;
    const caret = before.length + item.label.length;
    source.setSelectionRange(caret, caret);
    hide_completions();
    changed();
  };

  const on_key = (event) => {
    if (event.ctrlKey && event.key === " ") {
      event.preventDefault();
      complete();
      return;
    }
    if ($("complete").hidden) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      chosen = (chosen + (event.key === "ArrowDown" ? 1 : offered.length - 1)) % offered.length;
      show_completions($("complete").dataset.prefix);
    } else if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      accept($("complete").dataset.prefix);
    } else if (event.key === "Escape") {
      hide_completions();
    }
  };

  // ---------------------------------------------------------------- rung A: what the compiler derives

  const say = (text, bad) => {
    $("status").textContent = text;
    $("status").classList.toggle("bad", !!bad);
  };

  let selected = "place";
  let sections = [];

  const showSections = () => {
    const tabs = $("tabs");
    tabs.textContent = "";
    if (!sections.some((s) => s.id === selected)) selected = sections.length ? sections[0].id : "";
    for (const section of sections) {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.textContent = section.title;
      tab.dataset.section = section.id;
      tab.className = section.id === selected ? "on" : "";
      tab.onclick = () => {
        selected = section.id;
        showSections();
      };
      tabs.appendChild(tab);
    }
    const shown = sections.find((s) => s.id === selected);
    $("out").textContent = shown ? shown.text : "";
  };

  let pending = null;
  const changed = () => {
    // Highlighting is not debounced: it costs a lex, it does not need a program, and a colour that
    // arrives a quarter of a second after the character is a colour that flickers.
    paint().catch(() => {});
    clearTimeout(pending);
    // The analysis is, because a keystroke is not a question: the compiler is fast enough that this
    // is about not queueing four answers nobody will read, rather than about it being slow.
    pending = setTimeout(() => analyse().catch((why) => say(String(why.message || why), true)), 250);
  };

  const analyse = async () => {
    await ready;
    const source = $("source").value;
    const answer = await call({ op: "analyse", source });
    // Diagnostics are a section like any other, and the first one when there are any: a program
    // that does not compile has no placement, and pretending otherwise is how a playground
    // teaches somebody the wrong thing.
    sections = answer.sections.slice();
    if (answer.diagnostics) {
      sections.unshift({ id: "diagnostics", title: diagnosticsTitle(answer), text: answer.diagnostics });
      if (answer.errors) selected = "diagnostics";
    }
    marks = answer.marks || [];
    show_first_mark();
    await paint();
    $("run").disabled = !answer.runnable;
    say(answer.errors ? `${answer.errors} error${answer.errors === 1 ? "" : "s"}` : "compiles", !!answer.errors);
    showSections();
  };

  const diagnosticsTitle = (a) => {
    if (a.errors) return `Errors (${a.errors})`;
    return `Warnings (${a.warnings})`;
  };

  // ---------------------------------------------------------------- the log a reload survives
  //
  // §17.2's log-storage row says IndexedDB, and this is it. The key is the program's **wire id**,
  // which is the identity a log actually has: two sources whose event types agree share it and can
  // read each other's history, and a change to those types is a new id and a new log — which is
  // §4.3's rule, not a rule this page invented.

  const DB = "beck-playground";
  const STORE = "logs";

  const db = () =>
    new Promise((resolve, reject) => {
      const req = indexedDB.open(DB, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(STORE, { keyPath: "wire" });
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });

  const transact = async (mode, work) => {
    const handle = await db();
    return new Promise((resolve, reject) => {
      const tx = handle.transaction(STORE, mode);
      const req = work(tx.objectStore(STORE));
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
  };

  // A private window, a disabled store, a full quota: the tab still works, it just will not survive
  // a reload. Saying so once is better than failing an interaction — the same posture Mode B's
  // local copy takes (docs/94 §94.13).
  const stored = {
    ok: typeof indexedDB !== "undefined",
    async read(wire) {
      if (!this.ok) return [];
      try {
        const row = await transact("readonly", (store) => store.get(wire));
        return (row && row.records) || [];
      } catch (why) {
        this.ok = false;
        return [];
      }
    },
    // The whole log, every time. A playground's history is tens of events, so the alternative — a
    // record per row and a cursor to read them back — would be a store's worth of machinery for a
    // saving nobody in a tab can measure. It is the reason a very long session gets slower to
    // *store*, and `docs/103` §103.6 says so rather than leaving it to be found.
    async write(wire, records) {
      if (!this.ok) return;
      try {
        await transact("readwrite", (store) => store.put({ wire, records }));
      } catch (why) {
        this.ok = false;
        say("this browser will not keep the log: " + why, true);
      }
    },
    async forget(wire) {
      if (!this.ok) return;
      try {
        await transact("readwrite", (store) => store.delete(wire));
      } catch (why) {
        this.ok = false;
      }
    },
  };

  let running = null; // { wire, mode, kept: [records], keeping: bool }

  // One at a time, and the position asked for is the *length of what is held* rather than a number
  // kept beside it. Both halves matter: a `hello` and a command each say "moved", so two of these
  // can be in flight at once, and one that read a stale position would store the same events twice
  // — a log that grows by folding its own history, which nothing downstream would notice until a
  // reload rendered a page no interaction produced.
  let keeping = Promise.resolve();
  const keep = () => {
    keeping = keeping
      .then(async () => {
        if (!running || !running.keeping) return;
        const answer = await call({ op: "records", after: running.kept.length });
        if (!answer.records.length) return;
        running.kept = running.kept.concat(answer.records);
        await stored.write(running.wire, running.kept);
        show_kept();
      })
      .catch((why) => say(String(why.message || why), true));
    return keeping;
  };

  const show_kept = () => {
    if (!running) return;
    $("kept").textContent = stored.ok
      ? `${running.kept.length} event${running.kept.length === 1 ? "" : "s"} kept in this browser`
      : "not kept: this browser has no storage for it";
  };

  // ---------------------------------------------------------------- rung B: the application

  const CLIENTS = [
    { actor: "ana", frame: "client-ana" },
    { actor: "bo", frame: "client-bo" },
  ];

  // A client iframe is the *served document* of a Beck application, assembled here exactly as
  // `beck run` assembles it: the page the server would have rendered, the position it reflects, and
  // the residue for the program's rendering mode. Neither `beck-thin.js` nor `beck-mode-b.js` is
  // modified, and neither knows it is in a playground.
  // Which residue a document needs is the component's rendering mode, and the tab is the one that
  // knows it — the same branch `beck_rt::http::document` makes, for the same reason: a Mode B
  // document that loaded the thin client would sit waiting for DOM patches nobody is going to send.
  const residue = (mode) => (mode === "b" ? "beck-mode-b.js" : "beck-thin.js");

  const document_for = (actor, html, seq, mode) => `<!doctype html>
<html><head><meta charset="utf-8"><link rel="stylesheet" href="client.css"></head>
<body>
<div id="b-root" data-b-actor="${actor}" data-b-seq="${seq}" data-b-claims="{}">${html}</div>
<script src="beck-patch.js"></script>
<script src="beck-play-port.js"></script>
<script src="${residue(mode)}"></script>
</body></html>`;

  const open_client = async ({ actor, frame }, bundle) => {
    const rendered = await call({ op: "rendered", actor });
    const head = (await call({ op: "history" })).head;
    const iframe = $(frame);
    await new Promise((resolve) => {
      iframe.onload = resolve;
      iframe.srcdoc = document_for(actor, rendered.html, head, running.mode);
    });
    // One channel per client: the worker keeps the end it is given, the iframe keeps the other,
    // and the page — which created both — never reads either again. The bundle rides along, for a
    // Mode B client that is about to ask for it.
    const channel = new MessageChannel();
    worker.postMessage({ k: "client", port: channel.port1 }, [channel.port1]);
    iframe.contentWindow.postMessage({ k: "port", bundle }, "*", [channel.port2]);
  };

  const run = async () => {
    await ready;
    say("loading the program…");
    try {
      const loaded = await call({ op: "load", source: $("source").value, now: Date.now() });
      running = { wire: loaded.wire, mode: loaded.mode, kept: [], keeping: true };
      $("run-panel").hidden = false;

      // The log this browser kept for this wire id, folded back in before anybody subscribes.
      const kept = await stored.read(running.wire);
      if (kept.length) {
        await call({ op: "restore", records: kept });
        running.kept = kept;
      }
      show_kept();

      // Mode B needs the component's slice; Mode A does not, and asking for one would be deriving
      // a bundle nothing is going to load.
      const bundle = running.mode === "b" ? bytes_of((await call({ op: "bundle" })).bundle) : null;

      for (const client of CLIENTS) await open_client(client, bundle);
      await refresh_history();
      say(
        running.mode === "b"
          ? "running — two clients rendering in the browser (Mode B), one log"
          : "running — two clients, one log",
      );
    } catch (why) {
      say(String(why.message || why), true);
    }
  };

  // ---------------------------------------------------------------- the scrubber

  let head = 0;

  const refresh_history = async () => {
    const history = await call({ op: "history" });
    head = history.head;
    $("scrub").max = String(head);
    $("scrub").value = String(head);
    $("at-head").textContent = String(head);
    $("at-seq").textContent = String(head);
    const rows = history.events
      .map((e) => `<tr><td>${e.seq}</td><td>${escape_text(e.actor)}</td><td>${escape_text(e.event)}</td></tr>`)
      .join("");
    $("log").tBodies[0].innerHTML = rows;
    await show_at(head);
  };

  // The page as of a position, folded from genesis every time. Not an undo stack and not a
  // recording: the state at `seq` is computed by the same fold that produced the live one, which
  // is the property worth being able to *see* rather than read about.
  const show_at = async (seq) => {
    const at = await call({ op: "at", seq, actor: "ana" });
    $("preview").innerHTML = at.html;
    $("at-seq").textContent = String(seq);
  };

  // ---------------------------------------------------------------- the share link
  //
  // §17.4's content addressing, as far as a tab with nothing behind it can take it: the fragment
  // carries the program and names its digest, and a fragment is the one part of a URL a browser
  // does not send to a server. `share.rs` is where both halves happen.

  const share = async () => {
    const answer = await call({ op: "share", source: $("source").value });
    const url = location.origin + location.pathname + "#p=" + answer.fragment;
    history.replaceState(null, "", "#p=" + answer.fragment);
    try {
      await navigator.clipboard.writeText(url);
      say(`link copied — ${answer.digest.slice(0, 12)}… (${url.length} characters)`);
    } catch (why) {
      // No clipboard permission, or an insecure context. The link is in the address bar either
      // way, which is the part that matters.
      say(`link is in the address bar — ${answer.digest.slice(0, 12)}…`);
    }
  };

  // A link somebody opened. It is *the* source of the editor's text when present, ahead of the
  // examples, because a person who followed a link came for that program.
  const opened_link = async () => {
    if (!location.hash.startsWith("#p=")) return null;
    try {
      const answer = await call({ op: "open", fragment: location.hash });
      return answer.source;
    } catch (why) {
      say(String(why.message || why), true);
      return null;
    }
  };

  // ---------------------------------------------------------------- wiring

  moved = () => {
    // A command landed. The log grew, so the history strip is stale and there are records to keep
    // — and this is the only place the page hears about it, because the frames themselves go to
    // the iframes.
    refresh_history()
      .then(keep)
      .catch((why) => say(String(why.message || why), true));
  };

  $("source").addEventListener("input", () => {
    hide_completions();
    changed();
  });
  $("source").addEventListener("scroll", sync_scroll);
  $("source").addEventListener("keydown", on_key);
  $("source").addEventListener("blur", hide_completions);
  $("run").addEventListener("click", run);
  $("share").addEventListener("click", () => share().catch((why) => say(String(why.message || why), true)));
  $("scrub").addEventListener("input", (e) => show_at(Number(e.target.value)));
  // Through the same chain a `keep` goes through, and it has to be: a command still being stored
  // when the button is pressed would otherwise finish *after* the clear, put its records back and
  // overwrite the label — a log that says it was forgotten and was not.
  $("forget").addEventListener("click", () => {
    keeping = keeping
      .then(async () => {
        if (!running) return;
        // Forgetting stops this session keeping anything *more*, and that is the whole of why it is
        // a flag rather than an empty array. A store that resumed at the next command would write a
        // log starting at seq 3, and a restore of one is refused — dense from 1 is the contract
        // every fold depends on. It also settles the race: a `keep` still in flight when the button
        // is pressed finds the flag down and writes nothing back.
        running.keeping = false;
        await stored.forget(running.wire);
        running.kept = [];
        $("kept").textContent =
          "forgotten — this tab will keep no more; reload to start from `init`";
      })
      .catch((why) => say(String(why.message || why), true));
  });

  (async () => {
    await ready;
    const examples = await call({ op: "examples" });
    for (const example of examples) {
      const option = document.createElement("option");
      option.value = example.name;
      option.textContent = example.name;
      $("example").appendChild(option);
    }
    const pick = (name) => {
      const chosen = examples.find((e) => e.name === name) || examples[0];
      $("source").value = chosen.source;
      $("example").value = chosen.name;
      $("run-panel").hidden = true;
      changed();
    };
    $("example").addEventListener("change", (e) => pick(e.target.value));
    const shared = await opened_link();
    if (shared === null) {
      pick("counter");
    } else {
      $("source").value = shared;
      $("run-panel").hidden = true;
      changed();
    }
    document.body.dataset.ready = "1";
  })();
})();
