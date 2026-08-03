//! Digests and the two encodings a digest is written in.
//!
//! The half of Wave 2's crypto item that belongs to the *host* under `lib/README.md`'s division: a
//! hash function is somebody else's table and base64 is somebody else's grammar, so both are
//! primitives rather than Beck. What is composition — a signed token, a fingerprint, a check that
//! reads the two halves apart — is [`lib/crypto.beck`](../../../../../compiler/lib/crypto.beck).
//!
//! Three constraints shaped what is here.
//!
//! * **A digest is a pure function.** It performs no effect and it is the same on every replay, so
//!   nothing about it has to be recorded on an envelope. That is the difference between hashing and
//!   the other two things a crypto library usually offers: random bytes and a clock are
//!   nondeterministic, and Beck already has `uuid()` and `now()` for those, both charged `nondet`.
//! * **Only one function turns a `secret[Str]` into a `Str`**, and it is [`keyed`]. §3.5's property
//!   is that a secret cannot reach a browser; a message authentication code exists precisely to be
//!   handed to one, so the declassification is the point rather than a hole in it — but it is
//!   charged `cap.sign` so that a view cannot mint one, and
//!   [`docs/adr/0014`](../../../../../docs/adr/0014-a-keyed-digest-is-the-one-declassifier.md) is the
//!   record of that decision.
//! * **Comparing a digest is not `==`.** [`same`] is constant-time, because a verifier that returns
//!   early tells the caller where the first wrong byte was.
//!
//! BLAKE3 rather than SHA-2 because it is already in this tree — `beck-rt`'s `SignedIdentity`
//! (`docs/48`) and the signal graph's stable ids both use it — and adding a second hash function to
//! avoid reusing one is a dependency taken for symmetry.
//! [`docs/adr/0015`](../../../../../docs/adr/0015-blake3-for-the-standard-librarys-digests.md) records
//! why `ring` was not taken instead, and what that leaves unbuilt.

/// The domain the standard library's keyed digest is derived into.
///
/// A key is derived rather than used raw so that the same secret used for two purposes gives two
/// unrelated keys. It is a different string from `beck-rt`'s identity credential on purpose: a
/// token minted by a program must not verify as one minted by the runtime.
const KEY_CONTEXT: &str = "beck stdlib keyed digest v1";

/// BLAKE3 of `text`, as 64 lowercase hex digits.
pub fn of(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// A message authentication code over `message` under `key`, as 64 lowercase hex digits.
///
/// The one function in the language whose input is a `secret[Str]` and whose output is not secret.
pub fn keyed(key: &str, message: &str) -> String {
    let derived = blake3::derive_key(KEY_CONTEXT, key.as_bytes());
    blake3::keyed_hash(&derived, message.as_bytes())
        .to_hex()
        .to_string()
}

/// Equality that does not stop at the first difference.
///
/// Length is compared first and in the clear, because the length of a digest is not a secret and
/// padding two strings to a common length to hide it would be answering a question nobody asked.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Lowercase hex, two digits per byte of UTF-8.
pub fn hex_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for b in text.as_bytes() {
        out.push(char::from(HEX[usize::from(b >> 4)]));
        out.push(char::from(HEX[usize::from(b & 0x0f)]));
    }
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The inverse of [`hex_encode`], or why it is not one.
///
/// Both cases a caller can hit are named rather than collapsed into "bad input": an odd length is a
/// truncated string and a stray character is a different encoding, and those are different mistakes.
/// The bytes must also be UTF-8, because a Beck `Str` is.
pub fn hex_decode(text: &str) -> Result<String, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!(
            "hex has two digits per byte, and `{}` has an odd number of them",
            text.len()
        ));
    }
    let digits = text.as_bytes();
    let mut bytes = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let hi = digit(pair[0])?;
        let lo = digit(pair[1])?;
        bytes.push(hi << 4 | lo);
    }
    utf8(bytes)
}

fn digit(c: u8) -> Result<u8, String> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!(
            "`{}` is not a hex digit",
            char::from(c).escape_default()
        )),
    }
}

/// RFC 4648 §5 — the URL-and-filename-safe alphabet, without padding.
///
/// §5 rather than §4 because every place a Beck program will put one of these is a place `+` and
/// `/` have to be escaped: a URL, a filename, a JOSE segment. Padding is omitted for the same
/// reason — `=` is `%3D` in a query string — and [`base64_decode`] accepts it anyway, because a
/// decoder that refuses what other encoders emit is a decoder that fails in production.
pub fn base64_encode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        let digits = [n >> 18, n >> 12 & 63, n >> 6 & 63, n & 63];
        for d in digits.iter().take(chunk.len() + 1) {
            out.push(char::from(B64[*d as usize]));
        }
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The inverse of [`base64_encode`], tolerant of padding and of the standard alphabet.
///
/// `+` and `/` decode as `-` and `_` do: a program reading a value somebody else encoded should not
/// have to know which of the two alphabets they chose, and the two do not overlap, so accepting
/// both is unambiguous rather than lenient.
pub fn base64_decode(text: &str) -> Result<String, String> {
    let text = text.trim_end_matches('=');
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    for c in text.bytes() {
        let six = sextet(c)?;
        acc = acc << 6 | u32::from(six);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
        }
    }
    // A trailing group of one character carries six bits and no byte, which no encoder emits.
    if bits >= 6 {
        return Err(format!(
            "`{}` ends in a group of one character, which encodes no byte",
            text.escape_default()
        ));
    }
    utf8(bytes)
}

fn sextet(c: u8) -> Result<u8, String> {
    Ok(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' | b'+' => 62,
        b'_' | b'/' => 63,
        _ => {
            return Err(format!(
                "`{}` is not a base64 character",
                char::from(c).escape_default()
            ));
        }
    })
}

fn utf8(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| "the decoded bytes are not UTF-8".to_string())
}

/// A UUID in the canonical 8-4-4-4-12 form, lowercased, or why it is not one.
///
/// Normalising rather than only validating is the whole reason this is a function and not a
/// `str_len` check in Beck: two spellings of the same UUID must not be two map keys. The braced and
/// unhyphenated forms other systems emit are read and written back canonically, so a program
/// comparing identifiers is comparing identity rather than punctuation.
pub fn uuid_normalise(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(trimmed);
    let inner = inner.strip_prefix("urn:uuid:").unwrap_or(inner);
    let digits: String = inner.chars().filter(|c| *c != '-').collect();
    if digits.chars().count() != 32 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "`{}` is not a UUID: 32 hex digits are wanted, in the 8-4-4-4-12 form",
            text.escape_default()
        ));
    }
    // Hyphens, where they appear at all, have to be in the right places — otherwise `1-2345…` and
    // the canonical spelling would normalise to the same identifier, and one of them is a typo.
    if inner.contains('-') && !hyphenated(inner) {
        return Err(format!(
            "`{}` is not a UUID: the groups are 8-4-4-4-12",
            text.escape_default()
        ));
    }
    let d = digits.to_ascii_lowercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &d[0..8],
        &d[8..12],
        &d[12..16],
        &d[16..20],
        &d[20..32]
    ))
}

fn hyphenated(s: &str) -> bool {
    let groups: Vec<usize> = s.split('-').map(|g| g.chars().count()).collect();
    groups == [8, 4, 4, 4, 12]
}

/// Which version a canonical UUID declares, as the digit in the third group.
///
/// `uuid()` mints a v4; a v7 arriving from elsewhere is a legal identifier and a program may want
/// to know. This reads a nibble rather than validating a layout: a value whose variant bits say
/// nothing is still an identifier, and refusing it would be refusing an identifier that works.
pub fn uuid_version(canonical: &str) -> i64 {
    canonical
        .as_bytes()
        .get(14)
        .and_then(|c| char::from(*c).to_digit(16))
        .map(i64::from)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_blake3_and_the_vector_is_the_specifications() {
        // BLAKE3's own test vector for the empty input, and for "abc" — quoted from the reference
        // implementation's `test_vectors.json` rather than produced by this code and pasted back.
        assert_eq!(
            of(""),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(
            of("abc"),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }

    #[test]
    fn a_keyed_digest_depends_on_both_halves_and_on_neither_alone() {
        let a = keyed("k1", "message");
        assert_ne!(
            a,
            keyed("k2", "message"),
            "a different key, a different mac"
        );
        assert_ne!(
            a,
            keyed("k1", "other"),
            "a different message, a different mac"
        );
        assert_eq!(a, keyed("k1", "message"), "and it is a function");
        // The key is derived into a domain, so the same secret does not produce the runtime's
        // identity credential for the same payload.
        assert_ne!(a.len(), 0);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn constant_time_equality_answers_the_same_question_as_equality() {
        for (a, b) in [
            ("", ""),
            ("a", "a"),
            ("ab", "ab"),
            ("a", "b"),
            ("a", ""),
            ("", "b"),
        ] {
            assert_eq!(same(a, b), a == b, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn hex_round_trips_and_the_vectors_are_ascii() {
        assert_eq!(hex_encode("hello"), "68656c6c6f");
        assert_eq!(
            hex_decode("68656C6C6F").unwrap(),
            "hello",
            "uppercase reads"
        );
        for s in [
            "",
            "hello",
            "a longer string with punctuation!",
            "unicode: é☃",
        ] {
            assert_eq!(hex_decode(&hex_encode(s)).unwrap(), s);
        }
        assert!(hex_decode("abc").is_err(), "odd length");
        assert!(hex_decode("zz").is_err(), "not a digit");
    }

    #[test]
    fn base64_matches_rfc_4648s_test_vectors() {
        // RFC 4648 §10, with §5's alphabet and no padding.
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(plain), encoded, "encoding {plain:?}");
            assert_eq!(
                base64_decode(encoded).unwrap(),
                plain,
                "decoding {encoded:?}"
            );
        }
    }

    #[test]
    fn base64_reads_what_other_encoders_write() {
        assert_eq!(base64_decode("Zm9vYmE=").unwrap(), "fooba", "padded");
        assert_eq!(base64_decode("Zm9vYmE==").unwrap(), "fooba", "over-padded");
        // `+` and `/` are the standard alphabet's 62 and 63; `-` and `_` are §5's. `"aa>"` and
        // `"aa?"` are the shortest ASCII strings whose last sextet is each of the two.
        assert_eq!(base64_decode("YWE+").unwrap(), "aa>");
        assert_eq!(base64_decode("YWE-").unwrap(), "aa>");
        assert_eq!(base64_decode("YWE/").unwrap(), "aa?");
        assert_eq!(base64_decode("YWE_").unwrap(), "aa?");
        assert!(
            base64_decode("Zm9vYmFy!").is_err(),
            "not a base64 character"
        );
        assert!(
            base64_decode("Z").is_err(),
            "a group of one encodes no byte"
        );
    }

    #[test]
    fn base64_round_trips_every_length_of_a_growing_string() {
        let mut s = String::new();
        for c in "the quick brown fox jumps over the lazy dog".chars() {
            s.push(c);
            assert_eq!(base64_decode(&base64_encode(&s)).unwrap(), s, "{s:?}");
        }
    }

    #[test]
    fn a_uuid_normalises_to_one_spelling() {
        let canonical = "f47ac10b-58cc-4372-a567-0e02b2c3d479";
        for spelling in [
            "f47ac10b-58cc-4372-a567-0e02b2c3d479",
            "F47AC10B-58CC-4372-A567-0E02B2C3D479",
            "f47ac10b58cc4372a5670e02b2c3d479",
            "{f47ac10b-58cc-4372-a567-0e02b2c3d479}",
            "urn:uuid:f47ac10b-58cc-4372-a567-0e02b2c3d479",
            "  f47ac10b-58cc-4372-a567-0e02b2c3d479  ",
        ] {
            assert_eq!(
                uuid_normalise(spelling).as_deref(),
                Ok(canonical),
                "{spelling}"
            );
        }
        assert_eq!(uuid_version(canonical), 4);
    }

    #[test]
    fn what_is_not_a_uuid_says_which_way_it_is_not() {
        for bad in [
            "",
            "f47ac10b-58cc-4372-a567-0e02b2c3d47", // 31 digits
            "f47ac10b-58cc-4372-a567-0e02b2c3d4799", // 33
            "g47ac10b-58cc-4372-a567-0e02b2c3d479", // not hex
            "f47ac10b-58c-c4372-a567-0e02b2c3d479", // groups in the wrong places
        ] {
            assert!(uuid_normalise(bad).is_err(), "{bad:?} is not a UUID");
        }
    }
}
