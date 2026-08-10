// The playground page: an editor, the compiler's answers, and two clients of one application.
//
// Nothing here compiles, folds, validates or renders. Every answer on this page came out of the
// WebAssembly module in the worker, which is the compiler and the runtime this repository builds
// (docs/17 §17.1, §17.2). What this file does is arrange three things on a screen and pass ports
// between them.
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
  const analyse = () => {
    clearTimeout(pending);
    // Debounced, because a keystroke is not a question: the compiler is fast enough that this is
    // about not queueing four answers nobody will read, rather than about it being slow.
    pending = setTimeout(async () => {
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
      $("run").disabled = !answer.runnable;
      say(answer.errors ? `${answer.errors} error${answer.errors === 1 ? "" : "s"}` : "compiles", !!answer.errors);
      showSections();
    }, 250);
  };

  const diagnosticsTitle = (a) => {
    if (a.errors) return `Errors (${a.errors})`;
    return `Warnings (${a.warnings})`;
  };

  // ---------------------------------------------------------------- rung B: the application

  const CLIENTS = [
    { actor: "ana", frame: "client-ana" },
    { actor: "bo", frame: "client-bo" },
  ];

  // A client iframe is the *served document* of a Beck application, assembled here: the page the
  // server would have rendered, the position it reflects, and the residue. `beck-thin.js` is
  // unmodified — it finds `#b-root`, reads `data-b-seq` off it and resumes from there, exactly as
  // it does against `beck run`.
  const document_for = (actor, html, seq) => `<!doctype html>
<html><head><meta charset="utf-8"><link rel="stylesheet" href="client.css"></head>
<body>
<div id="b-root" data-b-actor="${actor}" data-b-seq="${seq}">${html}</div>
<script src="beck-patch.js"></script>
<script src="beck-play-port.js"></script>
<script src="beck-thin.js"></script>
</body></html>`;

  const open_client = async ({ actor, frame }) => {
    const rendered = await call({ op: "rendered", actor });
    const head = (await call({ op: "history" })).head;
    const iframe = $(frame);
    await new Promise((resolve) => {
      iframe.onload = resolve;
      iframe.srcdoc = document_for(actor, rendered.html, head);
    });
    // One channel per client: the worker keeps the end it is given, the iframe keeps the other,
    // and the page — which created both — never reads either again.
    const channel = new MessageChannel();
    worker.postMessage({ k: "client", port: channel.port1 }, [channel.port1]);
    iframe.contentWindow.postMessage({ k: "port" }, "*", [channel.port2]);
  };

  const run = async () => {
    await ready;
    say("loading the program…");
    try {
      const loaded = await call({ op: "load", source: $("source").value, now: Date.now() });
      $("run-panel").hidden = false;
      if (loaded.mode === "b") {
        // Honest rather than blank: this program renders in the browser, and the tab serves the
        // mode the thin client speaks (docs/96 §96.7).
        say("this page renders on the client (@render(client)); the tab serves Mode A", true);
        $("run-panel").hidden = true;
        return;
      }
      for (const client of CLIENTS) await open_client(client);
      await refresh_history();
      say("running — two clients, one log");
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

  const escape_text = (s) =>
    String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));

  // ---------------------------------------------------------------- wiring

  moved = () => {
    // A command landed. The log grew, so the history strip is stale — and this is the only place
    // the page hears about it, because the frames themselves go to the iframes.
    refresh_history().catch((why) => say(String(why.message || why), true));
  };

  $("source").addEventListener("input", analyse);
  $("run").addEventListener("click", run);
  $("scrub").addEventListener("input", (e) => show_at(Number(e.target.value)));

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
      analyse();
    };
    $("example").addEventListener("change", (e) => pick(e.target.value));
    pick("counter");
    document.body.dataset.ready = "1";
  })();
})();
