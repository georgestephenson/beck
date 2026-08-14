//! The runtime library, from the compiler's side: which primitives it answers, and the archive.
//!
//! `beck-prim` is the other half of this; its module documentation is where the design is. What is
//! here is the three things a code generator needs and the library itself must not know:
//!
//! * **Which `Prim` is which [`Op`]** ([`op_of`]). The mapping is here because `Prim` is
//!   `beck-core`'s and the library does not depend on the compiler — the arrow only points one way.
//! * **The archive**, embedded, because a release is one executable ([`ARCHIVE`]).
//! * **That the two agree about a `Str`** — the layout constant is written down in both crates,
//!   and `the_two_crates_lay_out_a_str_the_same_way` below is what stops them from drifting.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use beck_core::core::Prim;

pub use beck_prim::{Op, Raise, Status};

/// The symbol a compiled program calls to perform one primitive.
pub const CALL: &str = "beck_prim";

/// The symbol that reserves the arena when the runtime library is linked.
///
/// A program that links the library takes its heap from it rather than from `malloc`, because the
/// library reads that heap through a `Vec` it owns — see `beck_prim::arena`, whose first paragraph
/// is why there is no pointer in this ABI at all.
pub const ARENA: &str = "beck_prim_arena";

/// The primitive `op` is here, or `None` for one the emitters compile or refuse themselves.
///
/// The list is the answer to a question `docs/119` §119.5 left open about the *other* class of
/// refusal: these are pure functions of their arguments that the host already has, so neither
/// emitting them nor asking the host for them is right. Linking them is.
pub fn op_of(op: Prim) -> Option<Op> {
    Some(match op {
        Prim::Digest => Op::Digest,
        Prim::DigestKeyed => Op::DigestKeyed,
        Prim::DigestEq => Op::DigestEq,
        Prim::HexEncode => Op::HexEncode,
        Prim::HexDecode => Op::HexDecode,
        Prim::Base64Encode => Op::Base64Encode,
        Prim::Base64Decode => Op::Base64Decode,
        Prim::UuidParse => Op::UuidParse,
        Prim::UuidVersion => Op::UuidVersion,
        Prim::StrUpper => Op::StrUpper,
        Prim::StrLower => Op::StrLower,
        Prim::StrToInt => Op::StrToInt,
        Prim::StrReplace => Op::StrReplace,
        Prim::TimeFormat => Op::TimeFormat,
        Prim::TimeParse => Op::TimeParse,
        _ => return None,
    })
}

/// The runtime library, as a DEFLATE stream of the static archive.
///
/// Compressed because a `staticlib` is self-contained — Rust's standard library is in there
/// whether a primitive reaches it or not — and 21 MiB of archive would be 21 MiB of `beck`, of
/// which a linked program takes about a quarter. What is *not* done is stripping it,
/// which would save another sixth and would make the binary depend on whether the machine that
/// built it had `strip`: `docs/109`'s provenance is worth more than the sixth.
pub const ARCHIVE: &[u8] = include_bytes!(env!("BECK_PRIM_ARCHIVE"));

/// Write the archive into `dir`, and answer where it is.
///
/// Called only by a link step that has something to link it for, so a program that reaches no
/// runtime-library primitive pays neither the decompression nor the linker's read of it.
pub fn stage(dir: &Path) -> Result<PathBuf, String> {
    let at = dir.join("libbeck_prim.a");
    let mut bytes = Vec::new();
    flate2::read::DeflateDecoder::new(ARCHIVE)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("unpacking the runtime library: {e}"))?;
    std::fs::write(&at, &bytes).map_err(|e| format!("writing {}: {e}", at.display()))?;
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap;

    /// The two crates each write down what a `Str` is, and neither can import the other's.
    ///
    /// `beck-prim` cannot depend on a backend — the backends depend on *it* — so the layout is in
    /// both, and this is the seam that would catch a change to one of them. It is written against
    /// the *function*, not only the constants: a padding rule that differed by a byte would put
    /// the library's answer where the next object goes.
    #[test]
    fn the_two_crates_lay_out_a_str_the_same_way() {
        assert_eq!(beck_prim::arena::WORD as u64, heap::WORD);
        assert_eq!(beck_prim::arena::STR_HEADER as u64, heap::STR_HEADER);
        for n in 0..64u64 {
            assert_eq!(
                beck_prim::arena::str_bytes(n as i64) as u64,
                heap::str_bytes(n),
                "a `Str` of {n} bytes"
            );
        }
    }

    /// Every primitive the library answers is one the emitters can ask for, and by its own name.
    ///
    /// The map is written by hand in two crates that cannot see each other, so both directions are
    /// worth asserting: a library primitive no `Prim` reaches is dead code in every compiled
    /// program, and a `Prim` mapped to the wrong code is a call that computes the wrong function
    /// and says nothing about it. Comparing the *names* is what catches the second — `Op::name`
    /// and `Prim::name` are independent spellings of one primitive.
    #[test]
    fn every_primitive_the_library_answers_is_one_the_compiler_can_ask_for() {
        let mut reached: Vec<Op> = Vec::new();
        // The prelude's own table, so this asks about every primitive the language has rather than
        // about a list written next to the one it is checking.
        for (name, p, scheme) in beck_core::prelude::prims() {
            let Some(op) = op_of(p) else { continue };
            assert_eq!(op.name(), name, "`{name}` is mapped to the wrong code");
            if let beck_core::ty::Ty::Fun(params, _, _) = &scheme.ty {
                assert_eq!(op.arity(), params.len(), "`{name}` takes another number");
            }
            reached.push(op);
        }
        for op in Op::ALL {
            assert!(
                reached.contains(&op),
                "no `Prim` reaches `{}`, so nothing can call it",
                op.name()
            );
        }
        assert_eq!(reached.len(), Op::ALL.len(), "and no primitive twice");
    }

    #[test]
    fn the_archive_is_an_archive() {
        let dir = std::env::temp_dir().join(format!("beck-prim-stage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let at = stage(&dir).expect("the archive unpacks");
        let bytes = std::fs::read(&at).expect("readable");
        assert!(
            bytes.starts_with(b"!<arch>\n"),
            "what was embedded should be an `ar` archive"
        );
        assert!(bytes.len() > 1 << 20, "and the whole of it");
        std::fs::remove_dir_all(&dir).ok();
    }
}
