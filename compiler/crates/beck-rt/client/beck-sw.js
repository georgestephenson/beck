// The service worker: the shell, so that a cold start with no network is a page rather than an
// error.
//
// `docs/94` §94.13 is why this exists. A Mode B client keeps its state in the browser and queues
// what it cannot send, so it survives a *reconnect* — but a **reload** with nothing listening never
// gets that far, because the document, the scripts and the kernel all come from the server. The
// local copy is fine and unreachable. A worker is the only thing a browser will run before the
// network is consulted, so it is the only place that can answer.
//
// Network first, cache second. A live server always wins, which is what keeps a deploy from being
// hostage to a cache: the only requests answered from the cache are the ones that *failed*.
//
// `%WIRE%` is substituted by the server from the program's content-derived wire id (§4.3), so a
// deployment that changes the command channel's types gets a new cache and the old one is deleted
// on activate. That is the same key `beck-mode-b.js` stores its local copy under, and for the same
// reason: two programs must not share one browser's idea of either.
const CACHE = "beck-shell-%WIRE%";

// Everything a Mode B tab needs before it can render anything at all. The document is `/` because
// that is what a reload asks for; the bundle and the kernel are the component and its backend.
const SHELL = [
  "/",
  "/beck.css",
  "/beck-patch.js",
  "/beck-mode-b.js",
  "/beck-bundle.bpk",
  "/beck-kernel.wasm",
];

self.addEventListener("install", (event) => {
  // `skipWaiting` so a tab that reloads onto a new deployment gets the new worker rather than the
  // one the previous page installed. The cache name changed with the wire id, so there is nothing
  // of the old program left to serve.
  event.waitUntil(
    caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(
          names.filter((n) => n.startsWith("beck-shell-") && n !== CACHE).map((n) => caches.delete(n)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const url = new URL(event.request.url);
  // Only this origin, only GETs, and never the socket: a command must reach the server or be
  // queued by the client, and a cached answer to one would be a lie about the log.
  if (url.origin !== self.location.origin) return;
  if (event.request.method !== "GET") return;
  if (url.pathname === "/socket") return;

  event.respondWith(
    fetch(event.request)
      .then((response) => {
        // Keep the shell current. A copy is taken because a Response body is read once.
        if (response.ok && SHELL.includes(url.pathname)) {
          const copy = response.clone();
          caches.open(CACHE).then((c) => c.put(event.request, copy));
        }
        return response;
      })
      .catch(() =>
        caches.match(event.request).then(
          (hit) =>
            hit ||
            // A document request with nothing cached: say so as a page rather than as a browser
            // error, because "the server is unreachable and this tab has never been here before"
            // is a different thing from "this site is broken".
            new Response("this application has not been loaded on this device before", {
              status: 503,
              headers: { "content-type": "text/plain" },
            }),
        ),
      ),
  );
});
