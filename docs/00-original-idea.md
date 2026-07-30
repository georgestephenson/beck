# 00 — The original idea (source conversation)

This is the seed conversation for the project, provided by George (the ending was supplied
separately after the shared copy cut off). Prose is condensed; **all code is verbatim**. Everything
else in `docs/` derives from, and defers to, this document.

## Exchange 1 — the seed

> **George:** I read SICP and had this idea. What if HTML, CSS, and Javascript files acted as both
> data and functions, like functions in Scheme. What if to make a working website, I just evaluate
> `(my-javascript (my-css (my-html)))`. Does anything like this exist already?

Answer, condensed: yes — Hiccup/Garden/ClojureScript (HTML/CSS/JS as s-expression data), Racket
X-expressions and Pollen, SXML/LAML. The noted wrinkle: `(my-css (my-html))` type-checks as document
→ styled document, but `my-javascript` isn't a pure function — it evolves the page over time. The
functional answer is Elm's `view : Model -> Html` plus `update : Msg -> Model -> Model`, with the
runtime folding events over state. React/JSX/Redux are the convergent mainstream form. The pipeline
also mirrors what browsers do internally (DOM ← data, render tree ← function of DOM+CSSOM, JS ←
mutation over time).

## Exchange 2 — the whole stack as one program

> **George:** Then what if you wanted to build a full-stack like this. Backend api and frontend
> become one source code. Your API just returns a function which is the whole web page as I
> described. And even infrastructure as code — it just takes the whole backend as an input. And
> databases. Could we write a full-stack like this?

Answer, condensed: this has an academic name — **multitier / tierless programming**. Client+server:
Links, Ur/Web, ML5 (placement as a modal type), Meteor, Electric Clojure, Lamdera, React Server
Components, Phoenix LiveView. Infrastructure as a function of the program: Nix/NixOS, Winglang,
Encore, Unison, Darklang ("deployless"). Databases: Datomic ("The Database as a Value"), event
sourcing (`state = foldl(update, init, events)`), Kleppmann's "Turning the Database Inside Out",
Materialize, Rama, Convex. Stacked up: **`(deploy (infra (backend (page))))` folded over an event
stream — every tier either a pure function or a fold.** Manifesto: *Out of the Tar Pit*.

Honest caveats given: tier boundaries are where function application stops being a good model
(latency, partial failure — Waldo's *A Note on Distributed Computing*); the client is enemy
territory, so the partitioner must know which expressions carry secrets (location/effect typing);
a deploy means two versions of "the function" running simultaneously (Lamdera migrations, Erlang
hot-swap).

## Exchange 3 — streams, and a host/target strategy

> **George:** Can't streams solve a lot of these problems. Cross-network calls are a stream. A
> deploy is a stream. Is there an existing language you could implement this whole idea in? And
> could it be a layer that compiles into the best-in-class tech — for example Rust, because it has
> the best performance. I guess the web layer needs to be HTML/CSS/JS whatever you do?

Answer, condensed: streams are the historically correct move — SICP ch. 3 ends on exactly this fork,
admitting the unresolved wound: **merging streams from independent sources smuggles time and
nondeterminism back in**. Streams fix what RPC hid: failure becomes a value in the algebra
(`onNext* (onError|onCompleted)`), backpressure becomes compositional. Deploys-as-stream-events is
literal in Erlang (`code_change` arrives in the mailbox); a Kubernetes controller is a fold over
desired-state events; GitOps folds the commit log. Remaining hard part = the merge problem; modern
answers: synchronous dataflow (Lustre/Esterel), logical timestamps (Naiad/differential dataflow),
CRDTs, session types.

Host candidates: Racket (language-building), Clojure (Missionary/Electric/Datomic/Rama), Obelisk in
Haskell, OCaml (Eliom, MirageOS), the BEAM/Gleam. Compile-to-best-in-class precedents: Ur/Web,
DDlog (Datalog → differential-dataflow Rust), **Roc** (`(rust-platform (my-app))` — a pure
application handed to an effectful Rust host). Caveat: Rust is an awkward *direct* backend for a
GC'd functional language; the routes that work are first-order dataflow output, Perceus-style
refcounting (Roc/Koka), or **engine-in-Rust with the language as its configuration** (Materialize's
shape). Web layer: treat HTML/CSS/JS as the browser's **instruction set**, not an authoring format —
either WASM-first, or server-driven where the browser is `fold(apply-patch, initial-html,
patch-stream)` and JS shrinks to a tiny patch interpreter (LiveView, Electric). Suggested
architecture: a small typed core — λ-calculus + streams + placement/effect types — lowered
MLIR-style into per-tier artifacts.

## Exchange 4 — the sketch (verbatim)

> **George:** Could you sketch what the layer would look like for a small "hello world" type CRUD
> example?

The reply named the language **tier** and gave this, "a todo list, since that's the 'hello world' of
CRUD":

```racket
#lang tier   ;; one file: client, server, database, wire, deploy

;; ---------- Domain: two vocabularies, one trust boundary ----------

(type Id   Uuid)
(type Todo {id : Id, text : String, done : Bool})

(type Command                        ;; what clients may ASK
  (Add    {id : Id, text : String})  ;; client names the id — see "optimism"
  (Toggle {id : Id})
  (Delete {id : Id}))

(type Event                          ;; what the server RECORDS
  (Added   {id : Id, text : String})
  (Toggled {id : Id})
  (Deleted {id : Id}))
```

> Commands and events are split CQRS-style because the client is enemy territory: clients propose,
> the server decides. The boundary between them is the one impure place in the program — the merge
> point SICP warned us about, and the language makes you write it down explicitly:

```racket
;; ---------- The merge point: where time enters ----------

;; Every connected client's send!s, interleaved. Arbitrary order —
;; this is the nondeterminism; there is exactly one of these.
(def commands : (Stream (Session × Command)) @server
  (merge-clients))

(def validate : Session -> Command -> (Option Event) @server
  (fn [sess cmd]
    (match cmd
      (Add {id text})  (if (blank? text) None (Some (Added {:id id :text text})))
      (Toggle {id})    (Some (Toggled {:id id}))
      (Delete {id})    (Some (Deleted {:id id})))))

(def events : (Stream Event) @server
  (filter-map (fn [(sess cmd)] (validate sess cmd)) commands))
```

> Now the database. It isn't a thing you talk to — it's a durable fold over that event stream.
> `Stream` is discrete occurrences; `Signal` is a value-over-time (Conal Elliott's events vs.
> behaviors). `durable` is the entire database administration story: it tells the compiler this
> accumulator must survive restarts, so persist the log and snapshot the fold.

```racket
;; ---------- The database is a fold ----------

(def apply-event : (Map Id Todo) -> Event -> (Map Id Todo)   ;; pure, unplaced
  (fn [todos e]
    (match e
      (Added {id text}) (assoc todos id {:id id :text text :done false})
      (Toggled {id})    (update-at todos id (fn [t] (set t :done (not t.done))))
      (Deleted {id})    (dissoc todos id))))

(def todos : (Signal (Map Id Todo)) @server
  (durable (fold apply-event {} events)))

;; A "query" is just another pure function of the signal.
;; Keeping it incremental is the compiler's job, not yours.
(def remaining : (Signal Nat) @server
  (map (fn [ts] (count (filter (fn [t] (not t.done)) (vals ts)))) todos))
```

```racket
;; ---------- The page is a pure function of state ----------

(def view : (Map Id Todo) -> Nat -> Html        ;; pure, unplaced — runs anywhere
  (fn [todos remaining]
    [:main
     [:h1 "todos"]
     [:input {:placeholder "what needs doing?"
              :on-enter (fn [text] (send! (Add {:id (uuid!) :text text})))}]
     [:ul
      (for [t (sort-by :text (vals todos))]
        [:li {:key t.id :class (if t.done "done" "")}
         [:span {:on-click (fn [] (send! (Toggle {:id t.id})))} t.text]
         [:button {:on-click (fn [] (send! (Delete {:id t.id})))} "×"]])]
     [:footer (str remaining " remaining")]]))

(def styles : Css
  [[:main  {:max-width "40ch" :margin "0 auto" :font "16px system-ui"}]
   [:.done {:text-decoration "line-through" :opacity 0.5}]])

(def page : (Signal Html) @client
  (map2 view todos remaining))          ;; ← the tier crossing

(def app (document styles page))
```

The closing prose, condensed — each clause is a requirement on our compiler:

- **~40 lines, and note what's absent**: no HTTP routes, no JSON, no SQL, no schema migration files,
  no fetch calls, no Dockerfile.
- **Placement typechecks**: `events`, `validate` and the durable fold are `@server`, so the log and
  business rules provably never ship to a browser. `view` and `apply-event` are pure and unplaced,
  "so they compile twice, and that's not waste, it's the payoff."
- **Tier splitting**: `page` is a client signal consuming server signals; the server diffs
  successive values and streams patches down one websocket, while the browser becomes
  `fold apply-patch initial-html patches` — "a couple-kilobyte patch interpreter." `send!` is the
  reverse arrow, feeding `merge-clients` with the session attached.
- **First paint is free SSR**: evaluate pure `view` against the current accumulator, ship HTML.
- **Codegen**: the server fold lowered onto a differential-dataflow core in Rust so `remaining`
  updates incrementally instead of recounting; the log in an append-only store; the client as WASM
  or small JS; styles flattened to a static stylesheet.
- **`tier deploy` treats infrastructure as a function of the program**: it sees one durable fold, so
  it provisions one volume plus snapshotting; sees `merge-clients`, so it provisions a websocket
  ingress; emits a Nix closure. If the `Todo` type changed since last deploy, **it refuses to ship
  until you write `migrate : OldState -> NewState`** — and the deploy itself rides the stream: old
  fold drains, new fold resumes from snapshot plus log.
- **Optimism falls out of purity**: because `apply-event` is unplaced, the client already has the
  fold, so `send!` can speculatively apply the expected event locally and reconcile when the
  authoritative patch arrives — "Meteor's latency compensation, but principled, since both tiers
  are running the same function and merely disagree briefly about the fold's order." This is also
  why the client names the id in `Add`: it must be able to refer to the todo before the server
  confirms it exists. **"Client-generated UUIDs are the small tell that browsers here are
  replicas, not terminals."**

The reply closed with a construct-by-construct provenance table — "every construct is stolen from
something that ships":

| Construct | Prior art |
|---|---|
| `durable (fold ...)` as the database | Datomic, Rama, event sourcing |
| signal crossing → patch stream | Electric Clojure, LiveView |
| unplaced pure view, compiled twice | Ur/Web, multitier ML |
| incremental `remaining` | Materialize / differential dataflow |
| `migrate` demanded at deploy | Lamdera |
| deploy emits a closure | Nix / Obelisk |

— and three admissions that "even hello world is honest about the hard parts":

1. **"The merge point is the whole ballgame"**: arbitrary interleaving is fine for a todo list, but
   the moment two users edit the same text field you need CRDTs or operational transforms, "and no
   type system absolves you."
2. **`Session` is threaded but unused**: real auth means per-session signals —
   `(def mine @server (map (filter-by sess.user) todos))` — which turns one broadcast into
   per-client fanout, "and placement types are what make that safe to express."
3. **The log grows forever**: "snapshot-and-compact is this world's garbage collection."

And the full-circle observation: `(document styles page)` *is* the original
`(my-javascript (my-css (my-html)))` — "except the JavaScript never appears in the source at all.
It's compiler residue: the patch interpreter plus the compiled view. You stopped writing it the
moment the page became a function." Calibration pointers: Electric Clojure's TodoMVC and Lamdera's,
read side by side with the sketch.

## What the rest of `docs/` takes from this

1. **The database is a durable fold over an event stream**; queries are pure functions of signals,
   kept incremental by the compiler. → [`03`](03-type-and-effect-system.md),
   [`05`](05-tier-lowering.md)
2. **Commands vs events, and one explicit merge point** where time and nondeterminism enter. →
   [`03`](03-type-and-effect-system.md)
3. **The browser is a patch interpreter** by default; pure code compiles to both tiers, which is
   what makes optimistic UI principled. → [`05`](05-tier-lowering.md)
4. **Deploys ride the stream**: typed migration functions, drain/resume, refuse-to-ship without a
   migration. → [`06`](06-kubernetes-and-packaging.md)
5. **Infrastructure is derived from program analysis**: a durable fold ⇒ a volume + snapshots; a
   merge point ⇒ an ingress. → [`06`](06-kubernetes-and-packaging.md)
6. **Placement annotations are explicit (`@server`/`@client`) with purity meaning "unplaced —
   compiles anywhere"**; inference can come later. → [`03`](03-type-and-effect-system.md)
7. The surface here is typed s-expressions; George's follow-up question — keep the power, gain
   Python's mass appeal — is answered in [`02`](02-syntax.md).
