# ADR 0020 — The read model speaks pgwire by hand, and answers to nobody

**Status:** accepted
**Date:** 2026-08-08
**Context:** [`23`](../23-incremental-views-report.md), [`05`](../05-tier-lowering.md) §5.3,
[`07`](../07-dependencies.md) §7.2, [`43`](../43-threat-model.md)

## The decisions

Three, taken together because each is only defensible given the others.

1. **The PostgreSQL wire protocol is implemented in this repository** — `beck-rt/src/pgwire.rs`,
   about five hundred lines — rather than taken as a dependency.
2. **The port has no authentication and no transport security**, and therefore **binds to the
   loopback interface only**. A non-loopback address is refused with an error that says why.
3. **It is off by default.** `beck run --pgwire <addr>` turns it on; nothing turns it on for you.

## Why the protocol is written rather than taken

[`07`](../07-dependencies.md) §7.2 lists "pgwire protocol server" with **no alternative** in its
alternatives column, which reads like a licence to take the first crate that implements one. What
that column was actually recording is that there is no *alternative protocol* — every BI tool and
every driver speaks this one — and not that the implementation must be somebody else's.

What a server crate carries is the rest of a database: a type registry to answer `pg_type`
lookups, a prepared-statement machine with parameters and portals that suspend, a `pg_catalog`
emulation, an authentication ladder. A read model has none of those to expose. What is actually
needed is:

* the startup exchange, including saying "no" to `SSLRequest` in the one-byte form the protocol
  reserves for it;
* the simple query;
* the extended query with **zero** parameters, because this SQL has no placeholders;
* four type OIDs — `bool`, `int8`, `text`, `float8` — every one of which is in every driver's
  built-in table, so nothing ever asks the catalogue what it just received.

That is a bounded amount of code with a bounded amount of protocol behind it, and taking a
dependency for it would put a database's worth of surface into a crate whose §7.9 rule is that
everything is pinned and everything is justified. It also avoids a `Cargo.lock` collision, which
[`08`](../08-roadmap.md) §8.5.5 names as one of the four artefacts that serialise otherwise
independent branches.

**What replaces the confidence a mature crate would have bought**: the gate is somebody else's
client. `beck-cli/tests/read_models.rs` drives the server with `tokio-postgres` — already a
dependency, for the Postgres log store — over both the simple and the extended protocol, in binary
format. A protocol server tested by a client written beside it tests agreement with itself, which
is [`82`](../82-the-edge-report.md) §82.10's finding about four gates
that could not fail.

## Why there is no authentication, and why that is a bound rather than a gap

Authentication here would be one of two things, and both are worse than the bound.

* **A password checked against a configured value** invents a credential system beside the one
  [`48`](../48-identity-report.md) built as a *seam*. Beck has one story about who is asking, and a
  second one reachable only from a SQL port is how a system ends up with two.
* **The program's own identity provider** is the right answer and is not available: `SignedIdentity`
  verifies a credential a caller presents, and no Postgres client will present one.

So the port answers `AuthenticationOk` to everyone, and the compensating control is that it cannot
be reached from another host. A read of an application's entire state with no credential belongs on
the same machine as the process; a deployment that wants it elsewhere forwards it — `kubectl
port-forward`, an SSH tunnel, a sidecar — which puts the authentication in the thing that already
has some.

There is deliberately **no flag to lift the bound**. A flag would be the whole decision, taken by
whoever is in a hurry; lifting it is a change to this record.

[`43`](../43-threat-model.md) is updated in the same change, and the absence is asserted
executably rather than by a grep: `there_is_no_authentication_and_the_port_is_loopback_only`
connects with no password and expects to succeed, so it goes red on the day one is required.

## Why it is off by default

The three defaults `beck run` has are a store, an address and a dashboard, and all three are things
a developer asked for by running the command. A SQL port is not: it is a second way in, and
[`06`](../06-kubernetes-and-packaging.md) §6.5's whole argument is that what a program exposes
should be derived from what the program says rather than from what the runtime felt like opening.

Nothing in the language currently *says* "publish these read models", so there is nothing to derive
it from, and the honest default for an undeclared capability is off. If a later phase gives a
program a way to declare it — an effect atom, a signal annotation — this flag is what that would
drive, and [`23`](../23-incremental-views-report.md) §23.19 records it as the open question.

## What this rules out

* `psql`'s backslash commands. `\d` is a join over four `pg_catalog` relations and this SQL has no
  joins; `select * from beck_columns` is the substitute, and it is a table rather than a special
  form.
* Any client that requires TLS. `sslmode=prefer` — the default nearly everywhere — negotiates down
  and works; `sslmode=require` does not connect.
* Writes, at any privilege. Not a permission that could be granted: the log is the only way state
  changes, and a read model that accepted an `insert` would be a second one.
