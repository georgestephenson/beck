# A three-module project

The exit criterion's last clause: "a 3-module project rebuilds incrementally without recompiling
dependencies whose signatures didn't change".

- `domain.beck` — the vocabulary and the fold. A **library**: no merge point, so nothing to run.
- `policy.beck` — authority over a vocabulary it did not define. Also a library, and the one that
  holds `cap.session` — which is why the capability check had to become a whole-program question
  rather than a per-module one.
- `app.beck` — the wiring, and nothing else.

`beck check app.beck` resolves the imports, checks each module against its dependencies'
*interfaces*, links the bodies, and slices the result. `beck iface domain.beck` publishes the
contract the other two compile against.
