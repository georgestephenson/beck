- **2026-08-16 — CI retries the toolchain download it cannot control.**
  `rustup target add wasm32-unknown-unknown` failed a run with `Connection reset by peer` part-way
  through a component download from `static.rust-lang.org`; rustup keeps the partial file and says
  "please try again" in as many words, so trying again is the whole fix. Three attempts with a
  growing pause, and the loop still fails when the failure is real — checked both ways by hand,
  because a retry that swallowed a genuine failure would be worse than the flake it replaced.
