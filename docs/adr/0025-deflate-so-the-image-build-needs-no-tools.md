# ADR 0025 — DEFLATE, so the image build needs no tools

**Status:** accepted
**Date:** 2026-08-09
**Context:** [`06`](../06-kubernetes-and-packaging.md) §6.2, [`98`](../98-supply-chain-report.md),
[`07`](../07-dependencies.md), [`0004`](0004-full-cargo-deny-gate.md)

## The decision

`flate2` is a workspace dependency, used by `beck-infra` and by nothing else. It is the DEFLATE
implementation the in-process image build reads its inputs and writes its output with. Its default
backend — `miniz_oxide`, which is Rust — is the one taken; no zlib is linked and no C is compiled.

Tar is **not** taken as a dependency. `beck-infra::apk` reads tar and `beck-infra::oci` writes it,
in about a hundred and fifty lines between them.

## Why a compression library at all

[`06`](../06-kubernetes-and-packaging.md) §6.2 says `beck build` should become "one process with no
external tools", and the artefact it names is an OCI image assembled from Wolfi packages. Both ends
of that are DEFLATE:

- an **APK** is a concatenation of gzip members, so a build that cannot inflate cannot read the
  packages the image is made of;
- an **OCI layer** is conventionally a gzipped tar, so a build that cannot deflate cannot write one
  a registry or a runtime expects.

There is no version of this work that does not need an implementation of RFC 1951, and writing one
would be several hundred lines of somebody else's well-specified algorithm with a correctness
property no test in this repository is well placed to establish.

## Why tar is written here and DEFLATE is not

The asymmetry is deliberate, and it is about what each one has to guarantee.

**Tar is a header format with one property this project needs and no library offers: byte
determinism.** The image's identity is the digest of the layer, so every field that could vary
between two runs — mtime, uid, gid, the order members appear in, the presence of pax records — has
to be a constant chosen here. A tar crate's writer would have to be *configured* into that shape
and re-verified against every upgrade, and the reading half is a hundred lines of parsing a
fixed-width header. So it is written, and `image.rs` holds it against the system `tar` in both
directions: fixtures GNU tar produced are read by this code, and the layer this code produces is
listed *and extracted* by GNU tar.

**DEFLATE is a compression algorithm**, and nothing in the image build depends on which bytes come
out of it — only on their being the same bytes twice, which any deterministic encoder gives. The
gzip *header* is the part that carries a clock and a hostname, and that is written explicitly
(mtime zero, OS byte 255) rather than left to a default.

## The cost, named

- **A layer digest that can move when `flate2` does.** A compressor is free to emit different valid
  output between versions, so the *compressed* layer digest — and therefore the image digest — is
  reproducible for a fixed toolchain rather than for all time. The **`diff_id`**, the digest of the
  uncompressed tar, is not: it is a property of this repository's own tar writer. Both are in the
  image config, and [`98`](../98-supply-chain-report.md) §98.4 states which claim rests on which.
- **Licences.** `flate2` MIT/Apache-2.0, `miniz_oxide` MIT/Apache-2.0/Zlib, `crc32fast`
  MIT/Apache-2.0 — all inside [`0004`](0004-full-cargo-deny-gate.md)'s allowlist, and the gate is
  what enforces that rather than this sentence.
- **A decompression bomb is a reachable input.** `Contents::read` inflates a package into memory,
  and a hostile repository could serve one that expands without bound. What limits it today is the
  fetcher's reply cap and nothing else; [`98`](../98-supply-chain-report.md) §98.7 records this as
  open rather than solved.

## What would reverse it

An OCI ecosystem that reads uncompressed layers as a matter of course — the media type exists
(`application/vnd.oci.image.layer.v1.tar`), and a build that emitted one would need no compressor
at all. The *reading* half would still need DEFLATE for as long as APKs are gzip, so this would
halve the dependency's job rather than remove it, and it is not worth an image most tools handle
worse.
