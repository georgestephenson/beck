//! The program and the inputs a differential over the **runtime library** needs.
//!
//! Shared for the reason [`super::textfix`] is shared: three backends are held to these — the
//! tree-walker, LLVM (`native.rs`) and Cranelift (`cranelift.rs`) — and a second copy would be a
//! second opinion about what the subset is.
//!
//! # What these fifteen primitives have in common
//!
//! Every one of them was refused by both code generators until `docs/93` §93.12, and every one of them
//! is a *pure function of its arguments* that the host already had: a digest is a table, base64 is
//! a grammar, case mapping is Unicode's table, `str_to_int` is Rust's parser. What compiles them
//! is not an emitter — it is a call into `beck-prim`, the same library the evaluator calls.
//!
//! # What the inputs are chosen to catch
//!
//! The agreement is by construction, so what is worth testing is the **ABI**, and each input below
//! is here because one specific way of getting it wrong would otherwise survive:
//!
//! * **The empty string**, whose `Str` is two header words and no bytes — the one input where the
//!   library allocates the minimum and the outcome record lands closest to it.
//! * **Text that is not ASCII** (`héllo`, `日本語`, `🎈`), where a byte length and a character
//!   count differ: the library writes both header words, and a reader that took one for the other
//!   would answer a truncated string on exactly these.
//! * **A string containing a NUL**, because the ABI carries a length and not a C string.
//! * **A string long enough to be padded oddly** — 17 bytes, neither a whole number of words nor
//!   one short of one — so a wrong rounding in the library's allocation shows as the *next*
//!   object being misaligned rather than as a wrong answer here.
//! * **Inputs each decoder refuses**, because a raise is the half of the protocol where the
//!   library produces a message and the emitter builds the value around it. An odd-length hex
//!   string and a stray character are different failures of `hex_decode`, and both messages have
//!   to be the evaluator's, to the character.
//! * **Every spelling of one UUID**, because `uuid_parse` normalises rather than validating: two
//!   spellings answering two strings would be two map keys for one identifier.
//! * **`str_to_int` over `-1`**, which is the value a "missing" flag would collide with if the
//!   `Option` were carried as a sentinel rather than as a status word.

#![allow(dead_code)] // each suite uses the half of this it needs

use beck_core::core::Fields;
use beck_core::Value;
use std::sync::Arc;

/// A `secret[Str]`, as `secret_env` would have built one.
pub fn secret(text: &str) -> Value {
    Value::data(
        Arc::from(beck_core::ty::Ty::SECRET),
        None,
        Fields::from_iter([(Arc::from("value"), Value::str_(text))]),
    )
}

/// The strings every text primitive below is exercised over.
pub fn strings() -> Vec<Value> {
    [
        "",
        "a",
        "abc",
        "ABC",
        "Straße",
        "İstanbul",
        "ΟΔΟΣ",
        "héllo",
        "日本語",
        "🎈x",
        "a\0b",
        "the seventeen ok",
        "68656c6c6f",
        "Zm9vYmFy",
        "f47ac10b-58cc-4372-a567-0e02b2c3d479",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// What a decoder is asked to read, valid and not.
///
/// Both encodings' refusals are in one list because both definitions are asked every string: an
/// input `hex_decode` refuses is one `base64_decode` may accept, and a message that named the
/// wrong encoding would show up as the pair disagreeing rather than as one of them failing.
pub fn encoded() -> Vec<Value> {
    [
        "",
        "68656c6c6f",
        "68656C6C6F",
        "abc",
        "zz",
        "6",
        "Zm9vYmFy",
        "Zm9vYmE=",
        "Zm9vYmE==",
        "YWE+",
        "YWE-",
        "Z",
        "!",
        "日本語",
        "00",
        "0",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// Every spelling of a UUID this tree accepts, and several that are not one.
pub fn identifiers() -> Vec<Value> {
    [
        "f47ac10b-58cc-4372-a567-0e02b2c3d479",
        "F47AC10B-58CC-4372-A567-0E02B2C3D479",
        "f47ac10b58cc4372a5670e02b2c3d479",
        "{f47ac10b-58cc-4372-a567-0e02b2c3d479}",
        "urn:uuid:f47ac10b-58cc-4372-a567-0e02b2c3d479",
        "  f47ac10b-58cc-4372-a567-0e02b2c3d479  ",
        "01890a5d-ac96-774b-bcce-b302099a8057",
        "f47ac10b-58cc-4372-a567-0e02b2c3d47",
        "f47a-c10b-58cc-4372-a567-0e02b2c3d479",
        "not a uuid at all",
        "",
        "gggggggg-58cc-4372-a567-0e02b2c3d479",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// What `str_to_int` is asked to read: a number, and every shape of thing that is not one.
pub fn numerals() -> Vec<Value> {
    [
        "0",
        "7",
        "-1",
        "+7",
        "007",
        "9223372036854775807",
        "-9223372036854775808",
        "9223372036854775808",
        "",
        " 7",
        "7 ",
        "7.0",
        "0x10",
        "seven",
        "٣",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// The instants `time_format` is asked for: the epoch, either side of it, and the ends.
pub fn instants() -> Vec<Value> {
    [
        0,
        1,
        -1,
        999,
        1000,
        -1000,
        1_700_000_000_000,
        -2_208_988_800_000,
        253_402_300_799_000,
    ]
    .iter()
    .map(|ms| Value::Int(*ms))
    .collect()
}

/// The text `time_parse` is asked to read.
pub fn stamps() -> Vec<Value> {
    [
        "1970-01-01T00:00:00.000Z",
        "1969-12-31T23:59:59.999Z",
        "2023-11-14T22:13:20.000Z",
        "2023-11-14 22:13:20.000Z",
        "2023-11-14T22:13:20Z",
        "2023-11-14T22:13:20.5Z",
        "2023-11-14T22:13:20.123456Z",
        "2023-11-14T22:13:20+01:00",
        "2023-13-14T22:13:20.000Z",
        "1970-01-01",
        "",
        "nope",
    ]
    .iter()
    .map(Value::str_)
    .collect()
}

/// One definition per primitive, and four that put several in a row.
///
/// The `try:` forms matter as much as the raising ones: a raise that never reaches a handler is a
/// raise nothing compares the *value* of, and the value is where the emitter's half of the failure
/// lives — the declared type, the variant, and the fields the library does not fill in.
pub const RUNTIME: &str = r#"
def hashed(s: Str) -> Str:
    return digest(s)

def mac(key: secret[Str], message: Str) -> Str:
    return digest_keyed(key, message)

def same_digest(a: Str, b: Str) -> Bool:
    return digest_eq(a, b)

def hexed(s: Str) -> Str:
    return hex_encode(s)

def unhexed(s: Str) -> Str:
    return hex_decode(s)

def read_hex(s: Str) -> Result[Str, EncodingError]:
    return try: hex_decode(s)

def b64(s: Str) -> Str:
    return base64_encode(s)

def unb64(s: Str) -> Str:
    return base64_decode(s)

def read_b64(s: Str) -> Result[Str, EncodingError]:
    return try: base64_decode(s)

def canonical(s: Str) -> Str:
    return uuid_parse(s)

def which_version(s: Str) -> Int:
    return uuid_version(s)

def read_uuid(s: Str) -> Result[Str, UuidError]:
    return try: uuid_parse(s)

def shout(s: Str) -> Str:
    return str_upper(s)

def whisper(s: Str) -> Str:
    return str_lower(s)

def numbered(s: Str) -> Option[Int]:
    return str_to_int(s)

def defaulted(s: Str) -> Int:
    return unwrap_or(str_to_int(s), -1)

def swapped(s: Str, needle: Str, to: Str) -> Str:
    return str_replace(s, needle, to)

def stamped(ms: Int) -> Str:
    return time_format(ms)

def instant(s: Str) -> Int:
    return time_parse(s)

def read_time(s: Str) -> Result[Int, TimeError]:
    return try: time_parse(s)

## Four that put several in a row, so the arena's mark is asked to survive one call and carry the
## next. A primitive that reported the wrong mark would answer correctly once and then write over
## its own answer.
def fingerprint(s: Str) -> Str:
    return str_slice(digest(str_lower(s)), 0, 8)

def round_trip(s: Str) -> Bool:
    return base64_decode(base64_encode(s)) == s

def twice_over(s: Str) -> Str:
    return hex_encode(hex_encode(s)) + "|" + str_upper(s)

def counted(s: Str) -> Int:
    return str_len(digest(s)) + str_len(hex_encode(s)) + str_len(base64_encode(s))

## `n` digests in one call, answering the last — so the arena a call leaves behind is a function of
## `n`, and what it costs *per call* can be read off two sizes with no clock anywhere near it.
def hashes(n: Int, tail: Str) -> Str:
    if n <= 0:
        return tail
    return hashes(n - 1, digest("x"))
"#;

/// What one `digest` leaves in the arena: 64 hex characters, and a `Str`'s two header words.
///
/// The number the shape gate is written against. A digest is 32 bytes and its text is 64, which is
/// a whole number of words already — so this is the answer and nothing else, and an outcome record
/// written *below* the high-water mark rather than above it would make it 96.
pub const DIGEST_BYTES: usize = 2 * 8 + 64;
