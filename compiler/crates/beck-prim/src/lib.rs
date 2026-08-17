//! The runtime library: the primitives a compiled program links against.
//!
//! # What this is
//!
//! A handful of Beck's primitives are neither arithmetic nor a shape the code generators can lay
//! out: a digest is somebody's table, base64 is somebody's grammar, case mapping is Unicode's
//! table, and `str_to_int` has to agree with Rust's own parser about every input that is not a
//! number. Both native backends refused all of them, and the refusal said so in as many words.
//!
//! There were three ways to answer that, and only one of them is fast **and** exact:
//!
//! * **Ask the host.** The four `nondet` primitives do
//!   ([`beck_llvm::Upcall`](../beck_llvm/emit/enum.Upcall.html)), because a clock and an id source
//!   are questions only the process outside can answer. A digest is not a question — it is a
//!   function of its argument — and a pipe round trip per call would make the compiled program
//!   *slower* than the tree-walker at the thing it exists to be faster at.
//! * **Emit the algorithm.** Hex and base64 would compile; BLAKE3 and a Unicode case table would
//!   be a second implementation of somebody else's specification, sitting beside the one the
//!   evaluator already calls and agreeing with it only as far as it had been tested.
//! * **Link the implementation.** This crate. The compiled program calls the **same Rust
//!   functions** the evaluator calls, so the differential between the backends is not a claim
//!   about two implementations agreeing; there is one.
//!
//! So this crate is the standard library's host half, in one place: [`digest`] and [`time`] moved
//! here from `beck-core` and `beck-eval` rather than being copied, [`text`] is where the three
//! primitives that are a `str::` method live, and the evaluator calls all of it.
//!
//! # The two exports, and why there is no pointer in them
//!
//! `abi` — the module, behind the feature of the same name — is the C entry points, and the
//! interesting property is what does **not** cross them.
//! The workspace forbids `unsafe_code`, and a runtime library that took `(*const u8, usize)` and
//! made a slice out of it would need an `unsafe` block in the first line of every primitive. So
//! the arena is turned around: this crate **owns** the compiled program's heap ([`arena`]), the
//! program is handed its base once at startup, and every call after that carries **offsets**. An
//! offset is an `i64`, reading one is an indexing operation on a `Vec<u8>` this crate holds, and a
//! bad offset is a panic rather than a fault — so there is no raw pointer dereference here, no
//! `unsafe` block, and nothing for `docs/43-threat-model.md`'s structural claim to give up.
//!
//! `adr/0026` had already made a value in that arena an **offset and not a pointer**, so that the
//! whole heap could cross a pipe as bytes. This crate is the second thing that property buys.
//!
//! # The protocol
//!
//! `beck_prim`, and a mark rather than a return value:
//!
//! 1. The caller passes the arena's high-water mark and up to three argument words.
//! 2. This library allocates what its answer needs, starting at that mark.
//! 3. It writes a **two-word outcome record** — a [`Status`] and a word — immediately *above*
//!    everything it allocated, and answers with that offset.
//! 4. The caller stores the answer as its new mark and reads the record from it.
//!
//! The record sitting above the mark is what makes a call cost no arena at all beyond its answer:
//! it is scratch, live until the next allocation, which is exactly as long as the caller needs it.
//! An arena with no room is `-1`, which the caller turns into its own heap-exhausted trap with the
//! span it already has.
//!
//! There is deliberately **no error path for a bad offset**. A caller here is a code generator in
//! this workspace, not a program, and a defensive answer would be a second contract to keep in
//! agreement with the first.
//!
//! # The second entry point, which carries no offsets at all
//!
//! [`math`] is here for a different reason from the rest of this crate. A digest is linked because
//! emitting it would be a second implementation of somebody else's table; a sine is linked because
//! **there is no answer to ask the host for** — IEEE 754 pins `sqrt` to one correctly-rounded
//! result and pins neither `sin` nor `cos`, so a fold that reaches the platform's libm replays to
//! a different state on a platform with a different one.
//!
//! A function from a double to a double needs no heap, so `beck_prim_f64` takes the argument
//! itself and answers the result. Routing it through the mark protocol above would cost a lock, a
//! bounds check and an outcome record per call, for a primitive whose whole input is 64 bits — and
//! the arena's first paragraph, which is what buys all of that, has nothing to buy here.

#[cfg(feature = "abi")]
pub mod abi;
pub mod arena;
pub mod digest;
pub mod math;
pub mod text;
pub mod time;

/// What the first word of an outcome record says.
///
/// Three cases rather than two, because `str_to_int` answers `Option[Int]` and an `Option` is laid
/// out by the code generator that asked: this library says *there is no value* and the emitter
/// builds the `None` its own layout calls for. The same division is what makes a raise work — the
/// library produces the message, the emitter builds the declared error value around it, because
/// the type of that value is a fact about the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum Status {
    /// The second word is the answer: a scalar, or the offset of a `Str`.
    Value = 0,
    /// The second word is the offset of a `Str` saying why. The caller raises.
    Raised = 1,
    /// There is no value. The caller answers `None`.
    Nothing = 2,
}

impl Status {
    pub fn word(self) -> i64 {
        self as i64
    }
}

/// A primitive this library computes, and the code the two backends call it by.
///
/// The numbers are written out rather than derived from the order, because they are a protocol
/// between a compiled program and a library it was linked against: a primitive removed from the
/// middle of this list must not renumber the ones after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Digest = 1,
    DigestKeyed = 2,
    DigestEq = 3,
    HexEncode = 4,
    HexDecode = 5,
    Base64Encode = 6,
    Base64Decode = 7,
    UuidParse = 8,
    UuidVersion = 9,
    StrUpper = 10,
    StrLower = 11,
    StrToInt = 12,
    StrReplace = 13,
    TimeFormat = 14,
    TimeParse = 15,
}

impl Op {
    /// Every one of them, so a caller can build a table without repeating the list.
    pub const ALL: [Op; 15] = [
        Op::Digest,
        Op::DigestKeyed,
        Op::DigestEq,
        Op::HexEncode,
        Op::HexDecode,
        Op::Base64Encode,
        Op::Base64Decode,
        Op::UuidParse,
        Op::UuidVersion,
        Op::StrUpper,
        Op::StrLower,
        Op::StrToInt,
        Op::StrReplace,
        Op::TimeFormat,
        Op::TimeParse,
    ];

    pub fn code(self) -> i32 {
        self as i32
    }

    pub fn from_code(code: i32) -> Option<Op> {
        Op::ALL.into_iter().find(|o| o.code() == code)
    }

    /// How many argument words the call carries.
    ///
    /// A property of the primitive rather than of the call, for the reason `beck_llvm`'s upcall
    /// arity is one: an arity read out of the call is an arity nobody checked.
    pub fn arity(self) -> usize {
        match self {
            Op::StrReplace => 3,
            Op::DigestKeyed | Op::DigestEq => 2,
            _ => 1,
        }
    }

    /// How many of the argument words are the offset of a `Str`.
    ///
    /// The text arguments come first and there is only one primitive here whose argument is not
    /// text at all, so this is a count rather than a mask — and it is what stops the ABI from
    /// reading a millisecond as an offset.
    pub fn text_args(self) -> usize {
        match self {
            Op::TimeFormat => 0,
            _ => self.arity(),
        }
    }

    /// The name the Beck program wrote, which is also `beck_core::Prim::name`.
    pub fn name(self) -> &'static str {
        match self {
            Op::Digest => "digest",
            Op::DigestKeyed => "digest_keyed",
            Op::DigestEq => "digest_eq",
            Op::HexEncode => "hex_encode",
            Op::HexDecode => "hex_decode",
            Op::Base64Encode => "base64_encode",
            Op::Base64Decode => "base64_decode",
            Op::UuidParse => "uuid_parse",
            Op::UuidVersion => "uuid_version",
            Op::StrUpper => "str_upper",
            Op::StrLower => "str_lower",
            Op::StrToInt => "str_to_int",
            Op::StrReplace => "str_replace",
            Op::TimeFormat => "time_format",
            Op::TimeParse => "time_parse",
        }
    }

    /// The declared value a failure of this primitive raises.
    ///
    /// `None` for one that cannot fail. This library produces the *message* and nothing else: the
    /// value around it is a declared type with a layout, and a layout belongs to whichever code
    /// generator asked. So the shape is described here, where the failure is, and built there.
    pub fn raises(self) -> Option<Raise> {
        let (ty, variant, constants): (_, _, &'static [(&'static str, &'static str)]) = match self {
            Op::HexDecode => ("EncodingError", "BadEncoding", &[("encoding", "hex")]),
            Op::Base64Decode => ("EncodingError", "BadEncoding", &[("encoding", "base64")]),
            Op::UuidParse | Op::UuidVersion => ("UuidError", "BadUuid", &[]),
            Op::TimeParse => ("TimeError", "BadTime", &[]),
            _ => return None,
        };
        Some(Raise {
            ty,
            variant,
            constants,
            why: "why",
        })
    }
}

/// The value a primitive's failure raises, described rather than built.
///
/// Every field of it is here: the declared type, its variant, the fields whose values are a
/// constant of the primitive rather than of the input, and the one field the message goes in. A
/// caller that filled these in from its own table would be a second place for the evaluator's
/// `EncodingError.BadEncoding(encoding = "hex", …)` to be written down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raise {
    pub ty: &'static str,
    pub variant: &'static str,
    /// Fields the primitive fixes: `hex_decode` always raises with `encoding = "hex"`.
    pub constants: &'static [(&'static str, &'static str)],
    /// The field the message goes in.
    pub why: &'static str,
}

/// What one call answers, before it is written into the arena.
///
/// Owned, and that is the point: the argument words are read out of the arena by reference and
/// this is produced from them, so the borrow ends before anything is written back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A scalar — an `Int`, or a `Bool` as `0` and `1`.
    Word(i64),
    /// Text, to be allocated as a `Str`.
    Text(String),
    /// `None`.
    Nothing,
    /// A failure, with the message the declared error value carries.
    Raised(String),
}

/// Perform one primitive over text already read out of the arena.
///
/// Separated from the ABI so that it can be tested without an arena at all, and so that the one
/// place a `Prim` becomes an answer is a `match` a reader can check against `beck-eval`'s.
pub fn perform(op: Op, args: &[&str], words: &[i64]) -> Answer {
    match op {
        Op::Digest => Answer::Text(digest::of(args[0])),
        Op::DigestKeyed => Answer::Text(digest::keyed(args[0], args[1])),
        Op::DigestEq => Answer::Word(i64::from(digest::same(args[0], args[1]))),
        Op::HexEncode => Answer::Text(digest::hex_encode(args[0])),
        Op::HexDecode => answer(digest::hex_decode(args[0])),
        Op::Base64Encode => Answer::Text(digest::base64_encode(args[0])),
        Op::Base64Decode => answer(digest::base64_decode(args[0])),
        Op::UuidParse => answer(digest::uuid_normalise(args[0])),
        Op::UuidVersion => match digest::uuid_normalise(args[0]) {
            Ok(canonical) => Answer::Word(digest::uuid_version(&canonical)),
            Err(why) => Answer::Raised(why),
        },
        Op::StrUpper => Answer::Text(text::upper(args[0])),
        Op::StrLower => Answer::Text(text::lower(args[0])),
        Op::StrToInt => match text::to_int(args[0]) {
            Some(n) => Answer::Word(n),
            None => Answer::Nothing,
        },
        Op::StrReplace => Answer::Text(text::replace(args[0], args[1], args[2])),
        Op::TimeFormat => Answer::Text(time::format(words[0])),
        Op::TimeParse => match time::parse(args[0]) {
            Ok(ms) => Answer::Word(ms),
            Err(why) => Answer::Raised(why),
        },
    }
}

fn answer(r: Result<String, String>) -> Answer {
    match r {
        Ok(text) => Answer::Text(text),
        Err(why) => Answer::Raised(why),
    }
}

/// A primitive that is a function from a double to a double, and the code the backends call it by.
///
/// A separate vocabulary from [`Op`] rather than two more of its variants, because the two share
/// no part of their protocol: an [`Op`] reads text out of the arena and writes an outcome record
/// above the mark, and one of these takes a number and answers one. Merging them would give every
/// arm of the arena protocol a case that cannot happen.
///
/// The numbers are written out for [`Op`]'s reason: they are a protocol between a compiled program
/// and a library it was linked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOp {
    Sin = 1,
    Cos = 2,
}

impl FloatOp {
    pub const ALL: [FloatOp; 2] = [FloatOp::Sin, FloatOp::Cos];

    pub fn code(self) -> i32 {
        self as i32
    }

    pub fn from_code(code: i32) -> Option<FloatOp> {
        FloatOp::ALL.into_iter().find(|o| o.code() == code)
    }

    /// The name the Beck program wrote, which is also `beck_core::Prim::name`.
    pub fn name(self) -> &'static str {
        match self {
            FloatOp::Sin => "sin",
            FloatOp::Cos => "cos",
        }
    }
}

/// Perform one primitive that is a function from a double to a double.
///
/// Separated from the ABI so that the evaluator reaches the same `match` a compiled program does,
/// which is what makes the three-way differential a statement about one implementation rather than
/// about three that agree.
pub fn perform_f64(op: FloatOp, x: f64) -> f64 {
    match op {
        FloatOp::Sin => math::sin(x),
        FloatOp::Cos => math::cos(x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_reads_back_as_the_primitive_that_wrote_it() {
        for op in Op::ALL {
            assert_eq!(Op::from_code(op.code()), Some(op), "{}", op.name());
        }
        assert_eq!(Op::from_code(0), None, "no primitive is zero");
        assert_eq!(Op::from_code(99), None);
        // A duplicate code would make two primitives one call.
        let mut codes: Vec<i32> = Op::ALL.iter().map(|o| o.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), Op::ALL.len(), "the codes are distinct");
    }

    #[test]
    fn a_primitive_that_can_fail_names_what_it_raises() {
        // The pairing is what the emitter builds a value from, so a primitive whose answer can be
        // `Raised` and which names no type would be a raise with nothing to raise.
        for (op, args) in [
            (Op::HexDecode, "zz"),
            (Op::Base64Decode, "!"),
            (Op::UuidParse, "nope"),
            (Op::UuidVersion, "nope"),
            (Op::TimeParse, "nope"),
        ] {
            assert!(
                matches!(perform(op, &[args], &[]), Answer::Raised(_)),
                "{} should fail on {args:?}",
                op.name()
            );
            assert!(op.raises().is_some(), "{} names no error type", op.name());
        }
    }
}
