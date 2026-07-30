# tier

One language for the frontend, the backend, the database, the container, and the cluster.

Born from SICP: what if a working website were just `(my-javascript (my-css (my-html)))`? In `tier`,
that expression is literal — the page is a pure function of state, the database is a durable fold
over an event stream, infrastructure is a function of the program, and a deploy is an event on the
same stream it deploys. The compiler partitions one program into browser patch-streams, native
services, incrementally-maintained views, reproducible OCI images, and a Kubernetes object graph.

No code yet. The design and implementation plan live in **[`docs/`](docs/)**:

- [`docs/00-original-idea.md`](docs/00-original-idea.md) — the seed conversation the project grew
  from, sketch preserved verbatim
- [`docs/README.md`](docs/README.md) — plan overview and index
