- **2026-08-16 — `awareness(f)` is built: the roster with a payload, for the half a session can
  answer.** [`docs/10`](../docs/10-decisions.md) D6 gains the construct beside presence:
  `awareness(f) : Signal[Map[Str, T]] ! {cap.presence}`, where `f : Session -> T` produces one
  client's contribution and **the runtime applies it to every connection it holds** — `f` is a
  function rather than a signal because the subscribers are the runtime's fact and not the graph's,
  so a program cannot name another connection's session. It is a fifth view parameter and a plan
  source beside `presence`, a `Roles::awareness` role beside the view, and `beck_rt::awareness`, a
  registry modelled on `beck_rt::presence` with a second bound presence needs no equivalent of: a
  roster of counts costs its capacity, a roster of values costs its capacity times whatever `f`
  returns, so a contribution past `Config::each` is refused and the actor keeps its last one.
  Refused at the chokepoint (`B0520`) and to a Mode B page (`B0521`), for `B0515`'s and `B0516`'s
  reasons with one noun changed. `corpus/33-awareness.beck` is the program;
  `beck-cli/tests/awareness.rs` is the gate, fourteen tests, including the end-to-end one that
  presence could not have: a second client **navigating** — nobody arriving, nobody leaving, nothing
  appended to the log — moves the first client's page. The control gate was rewritten after a
  mutation: asserting "no frame reaches a program that reads no awareness" passes even with the
  wakeup wrongly armed, because such a page renders identically and diffs to nothing, so what it
  asserts now is the **row** — a client of such a program holds none — which an unconditional join
  turns red ([`docs/82`](../docs/82-the-edge-report.md) §82.10). Client-local awareness — a cursor —
  is unchanged and still waits on a client-local stream
  ([`docs/104`](../docs/104-styling-and-the-component-library.md) §104.8).
