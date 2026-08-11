// The playground's transport: a `MessageChannel` port where a deployment has a websocket.
//
// This is the whole of what a client iframe adds to a deployed Beck client. It is loaded between
// `beck-patch.js` (which defines `window.beck` and the transport seam) and `beck-thin.js` (which
// is unmodified, and does not know it is in a playground): docs/17 §17.2's "the thin patch client
// in an iframe, speaking the identical patch/command protocol over a `MessageChannel` instead of
// a websocket", taken literally.
(() => {
  let port = null;
  let handlers = null;

  // The page posts the port once the worker is holding the program. Until then `dial` has already
  // returned and the outbox in `beck.connect` is holding whatever the person clicked.
  window.addEventListener("message", (event) => {
    if (!event.data || event.data.k !== "port" || !event.ports[0]) return;
    port = event.ports[0];
    port.onmessage = (frame) => handlers && handlers.message(frame.data);
    port.start();
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
      close: () => {
        if (port) port.close();
        port = null;
        handlers = null;
      },
    };
  };
})();
