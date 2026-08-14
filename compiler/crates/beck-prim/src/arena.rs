//! The compiled program's heap, held here so that no pointer has to cross the ABI.
//!
//! # Why this crate owns it
//!
//! A compiled program's values live in one arena of bytes and a value *is* an offset into it
//! (`adr/0026`). Which side allocates that arena is free — it was one `malloc` in generated code —
//! and moving it here is what lets every entry point in `crate::abi` take an `i64` where it would
//! otherwise take a `*const u8` and a length. The difference is not stylistic: a slice made from a
//! caller's pointer is an `unsafe` block, in a workspace whose threat model claims there are none.
//!
//! # What the compiled program is allowed to do with it
//!
//! It is handed the base address once, at startup, and it stores and loads through that address
//! for the rest of the process. That is the ordinary shape of a buffer handed to foreign code —
//! `read(2)` into a `Vec` is the same shape — and it is sound here because of three things, each
//! of which is a property of this design rather than a hope:
//!
//! * **The buffer is allocated once and never grown**, so the address it was handed stays the
//!   address it has. `beck_prim_arena` is called once by `main` and there is no path
//!   that reallocates.
//! * **No reference into it is live while compiled code runs.** The only way compiled code runs is
//!   between calls into this library, and every borrow here ends before its call returns.
//! * **Nothing is shared across threads.** A compiled program is one thread asking one question at
//!   a time; the lock below is what makes that a checked fact rather than an assumption.
//!
//! # What a bad offset does
//!
//! Panics, which aborts the process at the `extern "C"` boundary. That is the price of having no
//! `unsafe` here, and it is the right price: the only caller is a code generator in this
//! workspace, so a bad offset is a compiler bug, and a compiler bug that stops is better than one
//! that reads a neighbouring object and answers with it.

use std::sync::Mutex;

/// Every field, every tag and every offset is this wide — `beck_llvm::heap::WORD`.
pub const WORD: i64 = 8;

/// The two header words of a `Str`: its length in bytes, then in characters.
///
/// The same layout `beck_llvm::heap::str_bytes` describes, written again here because this crate
/// cannot depend on a backend — `beck-llvm` depends on *it*. `the_two_layouts_agree` in
/// `beck-llvm` is what holds the two constants together.
pub const STR_HEADER: i64 = 2 * WORD;

/// How many bytes a `Str` of `n` UTF-8 bytes occupies, header and padding included.
pub fn str_bytes(n: i64) -> i64 {
    // Rounded up to a word by masking rather than by `next_multiple_of`, which is unstable for a
    // signed integer. `WORD` is a power of two, and a length is not negative.
    STR_HEADER + ((n + WORD - 1) & !(WORD - 1))
}

/// The arena, or an empty buffer before `main` has asked for one.
static ARENA: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// Reserve the arena and answer its base, or a null pointer if it cannot be had.
///
/// **`vec![0u8; n]` and not `reserve` followed by `resize`**, and the difference is the whole
/// startup cost: the first is `alloc_zeroed`, which is `calloc`, which is a fresh anonymous mapping
/// the operating system does not touch until it is written; the second writes 256 MiB of zeroes
/// over pages that were already zero. The `malloc` this replaced in generated code was lazy, and a
/// runtime library that made every compiled program pay a quarter of a gigabyte of `memset` at
/// startup would be a regression nothing in a differential would show.
///
/// The reservation before it is a **probe**, and it is what keeps "there is no room" an answer
/// rather than an abort: `vec!` calls the allocation-error handler on failure, and this asks the
/// same question first, in a form that can say no. A null answer leaves the compiled program's
/// `limit` at zero, so its first allocation traps as an exhausted heap — a message with a span
/// rather than a fault at the first store, which is what generated code already did with a
/// `malloc` that answered null.
pub fn create(bytes: i64) -> *mut u8 {
    if bytes <= 0 {
        return std::ptr::null_mut();
    }
    let mut arena = lock();
    // A second call would hand out a second base for one heap. It cannot happen from generated
    // code — `main` asks once — and answering the same base is what makes that harmless rather
    // than silently wrong.
    if arena.is_empty() {
        let Ok(len) = usize::try_from(bytes) else {
            return std::ptr::null_mut();
        };
        let mut probe: Vec<u8> = Vec::new();
        if probe.try_reserve_exact(len).is_err() {
            return std::ptr::null_mut();
        }
        drop(probe);
        *arena = vec![0u8; len];
    }
    arena.as_mut_ptr()
}

/// Run `f` over the arena.
///
/// The whole of the locking, in one place, so that no entry point can forget it.
pub fn with<R>(f: impl FnOnce(&mut Heap<'_>) -> R) -> R {
    let mut arena = lock();
    let mut heap = Heap { bytes: &mut arena };
    f(&mut heap)
}

fn lock() -> std::sync::MutexGuard<'static, Vec<u8>> {
    // A poisoned lock means a previous call panicked, which aborts at the boundary — so this is
    // unreachable rather than tolerated, and taking the inner value is what keeps a second panic
    // from hiding the first.
    ARENA.lock().unwrap_or_else(|e| e.into_inner())
}

/// The arena, borrowed for the length of one call.
pub struct Heap<'a> {
    bytes: &'a mut Vec<u8>,
}

impl Heap<'_> {
    /// How many bytes there are. `0` before the program has asked for an arena.
    pub fn limit(&self) -> i64 {
        self.bytes.len() as i64
    }

    /// The `Str` at `off`, as text.
    ///
    /// Bounds-checked and validated, because both are how this stays safe: an offset that is not a
    /// `Str` panics rather than reading whatever follows it, and text that is not UTF-8 cannot be
    /// handed to a `&str`. Neither can happen from a correct emitter — a `Str` in this arena is
    /// UTF-8 by construction — so both are the shape of the guarantee rather than a check with a
    /// cost worth avoiding.
    pub fn text(&self, off: i64) -> &str {
        let at = usize::try_from(off).expect("an offset is not negative");
        let len = self.word(at);
        let from = at + STR_HEADER as usize;
        let to = from + usize::try_from(len).expect("a length is not negative");
        std::str::from_utf8(&self.bytes[from..to]).expect("a `Str` in the arena is UTF-8")
    }

    fn word(&self, at: usize) -> i64 {
        let mut w = [0u8; 8];
        w.copy_from_slice(&self.bytes[at..at + 8]);
        i64::from_le_bytes(w)
    }

    fn put_word(&mut self, at: i64, value: i64) {
        let at = at as usize;
        self.bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// Write `s` as a `Str` at `mark`, and answer where the next object goes.
    ///
    /// `None` when it does not fit, which the caller turns into the arena's own exhausted trap.
    pub fn put_text(&mut self, mark: i64, s: &str) -> Option<i64> {
        let end = mark.checked_add(str_bytes(s.len() as i64))?;
        if end > self.limit() {
            return None;
        }
        self.put_word(mark, s.len() as i64);
        // The character count is a header word because `str_len` is constant time in both
        // implementations — `beck-eval` counts when the string is built, and so does this.
        self.put_word(mark + WORD, s.chars().count() as i64);
        let from = (mark + STR_HEADER) as usize;
        self.bytes[from..from + s.len()].copy_from_slice(s.as_bytes());
        // The padding to a whole word is not zeroed: it is never read, and an object's *content*
        // is what a comparison walks. It is written by the next allocation or it is not written.
        Some(end)
    }

    /// Write the two-word outcome record at `mark`, and answer `mark`.
    ///
    /// Above the high-water mark rather than below it, which is the whole of why a call costs no
    /// arena beyond its answer: the record is scratch, and the caller reads it before it allocates
    /// again. `None` when even that does not fit.
    pub fn put_outcome(&mut self, mark: i64, status: crate::Status, value: i64) -> Option<i64> {
        if mark.checked_add(2 * WORD)? > self.limit() {
            return None;
        }
        self.put_word(mark, status.word());
        self.put_word(mark + WORD, value);
        Some(mark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Status;

    /// A heap that is not the process's, so a test can have one of its own.
    fn heap(bytes: usize) -> Vec<u8> {
        vec![0; bytes]
    }

    fn over(buf: &mut Vec<u8>) -> Heap<'_> {
        Heap { bytes: buf }
    }

    #[test]
    fn text_written_at_a_mark_reads_back_as_itself() {
        let mut buf = heap(4096);
        let mut h = over(&mut buf);
        let mut mark = 8;
        let mut written = Vec::new();
        let long = "x".repeat(100);
        for s in ["", "a", "hello", "unicode: é☃", &long] {
            written.push((mark, s));
            mark = h.put_text(mark, s).expect("room");
            assert_eq!(mark % WORD, 0, "every object starts on a word");
        }
        // Read back *after* all of them, so an object that overwrote its neighbour is caught.
        for (at, s) in written {
            assert_eq!(h.text(at), s);
        }
    }

    #[test]
    fn a_string_that_does_not_fit_is_a_none_rather_than_a_panic() {
        let mut buf = heap(64);
        let mut h = over(&mut buf);
        assert!(h.put_text(8, &"x".repeat(80)).is_none(), "no room");
        assert!(
            h.put_text(8, "small").is_some(),
            "and the arena still works"
        );
    }

    #[test]
    fn the_outcome_record_sits_above_the_mark_and_leaves_it_alone() {
        let mut buf = heap(4096);
        let mut h = over(&mut buf);
        let mark = h.put_text(8, "answer").expect("room");
        let at = h.put_outcome(mark, Status::Value, 8).expect("room");
        assert_eq!(at, mark, "the record is at the mark it was given");
        assert_eq!(h.word(at as usize), Status::Value.word());
        assert_eq!(h.word(at as usize + 8), 8);
        assert_eq!(h.text(8), "answer", "and the answer is untouched");
    }

    #[test]
    fn the_character_count_is_characters_and_the_length_is_bytes() {
        let mut buf = heap(4096);
        let mut h = over(&mut buf);
        h.put_text(8, "é☃").expect("room");
        assert_eq!(h.word(8), 5, "two characters, five bytes");
        assert_eq!(h.word(16), 2);
    }

    #[test]
    fn the_padding_is_what_keeps_every_object_word_aligned() {
        for n in 0..24i64 {
            assert_eq!(str_bytes(n) % WORD, 0, "{n} bytes");
            assert!(str_bytes(n) >= STR_HEADER + n);
        }
    }
}
