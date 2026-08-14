//! The two symbols a compiled program links against.
//!
//! Behind the `abi` feature, so that the rlib linked into the compiler and into the playground's
//! WebAssembly exports nothing at all: a `#[no_mangle]` symbol in a library is a symbol every
//! artefact that links it exports, and two of those in one wasm module is a collision rather than
//! a convenience.
//!
//! Both entry points are `extern "C"` and neither takes a pointer. What that costs is a lock and a
//! bounds check per call; what it buys is [`crate::arena`]'s first paragraph.

use crate::{arena, perform, Answer, Op, Status};

/// Reserve the compiled program's heap and answer its base, or null.
///
/// Called once, by generated `main`, in place of the `malloc` that was there before. A null answer
/// is the arena's `limit` staying zero, which is already how both emitters turn a failed
/// reservation into a trap with a message instead of a fault at the first store.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn beck_prim_arena(bytes: i64) -> *mut u8 {
    arena::create(bytes)
}

/// Perform one primitive, and answer where the outcome record is.
///
/// `mark` is the arena's high-water mark and `a0`..`a2` are the argument words — an offset for
/// text, the value itself for a number. The answer is the new mark, at which sit two words: a
/// [`Status`] and its value. `-1` says the arena had no room, which the caller raises as its own
/// heap-exhausted trap, with the span it already knows.
///
/// An unknown `op` is `-1` as well. It cannot happen from an emitter in this workspace, and the
/// alternative — a panic — would turn a version skew between a program and the library it was
/// linked against into an abort with no message.
#[allow(unsafe_code)]
#[no_mangle]
pub extern "C" fn beck_prim(op: i32, mark: i64, a0: i64, a1: i64, a2: i64) -> i64 {
    let Some(op) = Op::from_code(op) else {
        return -1;
    };
    arena::with(|heap| {
        if mark < 0 || mark >= heap.limit() {
            return -1;
        }
        // Read the arguments, produce an owned answer, and only then write: the borrow of the
        // arena ends here, which is what lets one `&mut` do the allocating below.
        let words = [a0, a1, a2];
        let args: Vec<&str> = words[..op.text_args()]
            .iter()
            .map(|word| heap.text(*word))
            .collect();
        let answer = perform(op, &args, &words);

        let (status, value, text) = match answer {
            Answer::Word(w) => (Status::Value, w, None),
            Answer::Nothing => (Status::Nothing, 0, None),
            Answer::Text(t) => (Status::Value, 0, Some(t)),
            Answer::Raised(why) => (Status::Raised, 0, Some(why)),
        };
        let (mark, value) = match text {
            Some(t) => match heap.put_text(mark, &t) {
                Some(end) => (end, mark),
                None => return -1,
            },
            None => (mark, value),
        };
        heap.put_outcome(mark, status, value).unwrap_or(-1)
    })
}
