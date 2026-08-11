// The playground's transport and asset seams: a `MessageChannel` port where a deployment has a
// websocket, and the worker's own bundle where a deployment has a reserved route.
//
// This is the whole of what a client iframe adds to a deployed Beck client. It is loaded between
// `beck-patch.js` (which defines `window.beck` and both seams) and the mode's script — `beck-thin.js`
// or `beck-mode-b.js`, whichever the program's rendering mode calls for, and neither is modified
// and neither knows it is in a playground: docs/17 §17.2's "the thin patch client in an iframe,
// speaking the identical patch/command protocol over a `MessageChannel` instead of a websocket",
// taken literally, and docs/103 for the Mode B half of it.
(() => {
  let port = null;
  let handlers = null;
  // The component's slice, which the worker derived from the running program. It arrives with the
  // port, and `beck-mode-b.js` asks for it before that — so the promise is what it waits on.
  let arrived = null;
  const bundle = new Promise((resolve) => {
    arrived = resolve;
  });

  // The page posts the port once the worker is holding the program. Until then `dial` has already
  // returned and the outbox in `beck.connect` is holding whatever the person clicked.
  window.addEventListener("message", (event) => {
    if (!event.data || event.data.k !== "port" || !event.ports[0]) return;
    port = event.ports[0];
    port.onmessage = (frame) => handlers && handlers.message(frame.data);
    port.start();
    if (event.data.bundle) arrived(event.data.bundle);
    if (handlers) handlers.open();
  });

  beck.dial = (h) => {
    handlers = h;
    // Deferred, per the contract in `beck-patch.js`: `open` sends the `hello` through the object
    // this call has not returned yet.
    if (port) setTimeout(() => h.open(), 0);
    return {
      send: (frame) => port && port.postMessage(frame),
      ready: () => !!port,
    };
  };

  // The kernel is a file of this deployment, beside the page — a relative URL, because a
  // playground written to a directory may be served from any prefix. The bundle is not a file at
  // all: it is derived from whatever program the worker is running, so it comes over the port
  // rather than out of a directory, and a client cannot load a slice of a program the tab is not
  // executing.
  beck.asset = (name) =>
    name === "beck-bundle.bpk"
      ? bundle.then((bytes) => new Response(bytes))
      : fetch(name);

  // No shell cache: a `srcdoc` frame has no URL of its own to register a service worker for, and
  // there is no server to survive the absence of.
  beck.shell = false;
})();
