//! A share link, and why it is the program rather than a pointer to one.
//!
//! [`docs/17`](../../../../../docs/17-playground.md) §17.4: *"a playground is a program, and Beck
//! programs are content-addressed artefacts: a share link is a digest; forks are new digests."*
//! That sentence describes a link resolved through a CDN, and resolving one needs something to
//! resolve *against* — the registry [`docs/16`](../../../../../docs/16-packages-and-ecosystem.md)
//! describes and Phase 3 does not have.
//!
//! So a link **carries** the program and **names** its digest:
//!
//! ```text
//! https://play.beck.dev/#p=b3a71c2e5f9d04a8.eJxLy...
//!                          └── the first 16 hex digits of the digest
//!                                            └── the source, deflated and base64url'd
//! ```
//!
//! Three properties fall out, and they are the ones §17.4 wanted:
//!
//! * **Content-addressed.** The digest is [`beck_core::digest::of`] over the source — the same
//!   BLAKE3 the standard library's `digest()` computes — so the same program is the same link
//!   wherever it was written, and an edit is a different link. Forks are new digests because a fork
//!   is different bytes.
//! * **Self-certifying.** [`unpack`] recomputes the digest and refuses a mismatch, so a link
//!   truncated by a chat client or edited in transit is an error rather than a *different program*
//!   opening under a name somebody trusted.
//! * **Nothing is sent anywhere.** It is a fragment, and a fragment is the one part of a URL a
//!   browser does not put in the request. A playground with no backend should not acquire one by
//!   accident when somebody presses *share*.
//!
//! # What it is not
//!
//! Not short. A link is proportional to the program, and a 4 KB program is roughly a 1.5 KB link —
//! §17.4's embeds and its "a reproduction *is* a digest" want a resolver, and this is the half of
//! it that works with no server. Not private either: a fragment is not sent to a server, and it is
//! still in whatever the person pasted it into.
//!
//! # Why DEFLATE
//!
//! Because base64 of raw source is 4/3 of the source and a program is mostly repetition of a small
//! vocabulary. `flate2` is already this workspace's compressor
//! ([`adr/0025`](../../../../../docs/adr/0025-deflate-so-the-image-build-needs-no-tools.md)) with a
//! `miniz_oxide` backend that is Rust rather than a vendored zlib, which is why it crosses to
//! `wasm32-unknown-unknown` without a build-tooling decision.

use std::io::{Read, Write};

use beck_core::digest;

/// The prefix of the digest a link carries. Sixteen hex digits is 64 bits — enough that a
/// *corruption* is caught, which is what this check is for. It is not a signature and does not
/// pretend to be one: whoever holds the link holds the program.
const NAMED: usize = 16;

/// The most a link may expand to. A fragment is attacker-controlled input and DEFLATE is a
/// compressor, so a bounded reader is the difference between "that link is broken" and a tab that
/// allocates until it dies.
const LIMIT: u64 = 1 << 20;

/// A program, as the fragment of a share link.
pub fn pack(source: &str) -> String {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    // Writing to a `Vec` cannot fail, and neither can finishing one.
    let packed = encoder
        .write_all(source.as_bytes())
        .and_then(|()| encoder.finish())
        .unwrap_or_default();
    format!(
        "{}.{}",
        &digest::of(source)[..NAMED],
        digest::base64_encode_bytes(&packed)
    )
}

/// The program a fragment carries, and its digest — or why it is not one.
pub fn unpack(fragment: &str) -> Result<(String, String), String> {
    let fragment = fragment.trim_start_matches('#').trim_start_matches("p=");
    let (named, packed) = fragment
        .split_once('.')
        .ok_or("this is not a Beck share link: it has no digest")?;
    let bytes = digest::base64_decode_bytes(packed)?;
    let mut source = String::new();
    flate2::read::DeflateDecoder::new(&bytes[..])
        .take(LIMIT)
        .read_to_string(&mut source)
        .map_err(|why| format!("this link does not decompress to a program: {why}"))?;
    let digest = digest::of(&source);
    // Constant-time, because `digest::same` is the only comparison this repository makes on a
    // digest and having two would be having one that is sometimes not.
    if !digest::same(named, &digest[..NAMED.min(digest.len())]) {
        return Err(
            "this link does not match its digest: it was truncated or altered in transit".into(),
        );
    }
    Ok((source, digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_round_trips_and_names_the_program() {
        let source = "def f(x: Int) -> Int:\n    return x + 1\n";
        let fragment = pack(source);
        let (back, digest) = unpack(&fragment).expect("it unpacks");
        assert_eq!(back, source);
        assert_eq!(digest, beck_core::digest::of(source));
        assert!(fragment.starts_with(&digest[..NAMED]));
        // And with the `#p=` a browser's address bar carries, because that is what the page will
        // hand it.
        assert_eq!(
            unpack(&format!("#p={fragment}")).expect("it unpacks").0,
            source
        );
    }

    #[test]
    fn a_fork_is_a_new_digest() {
        let one = pack("def f() -> Int:\n    return 1\n");
        let two = pack("def f() -> Int:\n    return 2\n");
        assert_ne!(one, two);
    }

    #[test]
    fn a_link_that_was_truncated_is_refused_rather_than_opened() {
        // The failure this exists to prevent is not a broken link — it is a link that decodes to a
        // *different program* than the one whose digest it carries.
        let source = "def f() -> Int:\n    return 1\n";
        let honest = pack(source);
        let (named, _) = honest.split_once('.').expect("a digest and a payload");
        let lie = format!(
            "{named}.{}",
            pack("def f() -> Int:\n    return 2\n")
                .split_once('.')
                .expect("a payload")
                .1
        );
        let why = unpack(&lie).expect_err("it is refused");
        assert!(why.contains("digest"), "{why}");
    }

    #[test]
    fn a_fragment_that_is_not_a_link_says_so_rather_than_panicking() {
        assert!(unpack("").is_err());
        assert!(unpack("nonsense").is_err());
        assert!(unpack("aaaaaaaaaaaaaaaa.!!!!").is_err());
        assert!(unpack("aaaaaaaaaaaaaaaa.").is_err());
    }

    /// A program larger than the source of most programs, to say that the size of a link is a
    /// property somebody can check rather than a claim in a module comment.
    #[test]
    fn a_link_is_smaller_than_the_program_it_carries() {
        let source = crate::tab::examples()["todo"];
        let fragment = pack(source);
        assert!(
            fragment.len() < source.len(),
            "{} bytes of link for {} bytes of program",
            fragment.len(),
            source.len()
        );
        assert_eq!(unpack(&fragment).expect("it unpacks").0, source);
    }
}
