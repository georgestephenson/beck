//! The buffer table and the frame two WebAssembly modules hand their host page.
//!
//! # Why this is a crate and not thirty lines in each module
//!
//! `beck-play` and `beck-wasm` are two `cdylib`s with two host pages —
//! `beck-play/web/playground.js` loads one, `beck-rt/client/beck-mode-b.js` the other — and both
//! sides of both pairs agree on one thing: a call hands back an `i32`, and at that address is a
//! little-endian `u32` length followed by that many bytes. That is a **contract**, and the reason
//! it lives in one place is the reason `beck_llvm::heap` does: a contract with two spellings
//! drifts, and this one had two. Named rather than linked because this crate depends on neither —
//! a shared contract that depended on both its holders would be the wrong way round.
//!
//! What is *not* here is the exports. `beck_alloc`, `beck_free` and `beck_call` stay in the module
//! that answers them, because `#[no_mangle]` is a symbol in a linked artefact rather than a shared
//! implementation — and because the extent of each crate's `forbid(unsafe_code)` exception is
//! counted per crate by `playground.rs` and `mode_b.rs`, which is a gate worth keeping local.
//!
//! # Why a thread-local is the whole of the synchronisation
//!
//! WebAssembly is single-threaded here. One module instance is one tab, and every call arrives on
//! the same thread the buffers were made on.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    /// Buffers this module has handed the host, by the address of each one's allocation.
    ///
    /// Moving a `Vec` moves three words and not the bytes, so the address stays the one that was
    /// returned.
    static BUFFERS: RefCell<BTreeMap<i32, Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
}

/// Hand the host `len` zeroed bytes to write a request into, and keep them alive until [`take`].
pub fn reserve(len: usize) -> i32 {
    let mut buffer = vec![0u8; len];
    let ptr = buffer.as_mut_ptr() as i32;
    BUFFERS.with(|b| b.borrow_mut().insert(ptr, buffer));
    ptr
}

/// Reclaim a buffer by the address [`reserve`] or [`respond`] returned.
pub fn take(ptr: i32) -> Option<Vec<u8>> {
    BUFFERS.with(|b| b.borrow_mut().remove(&ptr))
}

/// Hand the host a length-prefixed response and keep it alive until `beck_free`.
pub fn respond(body: Vec<u8>) -> i32 {
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    let ptr = framed.as_mut_ptr() as i32;
    // The frame is what the host reads, so the address that identifies it is the frame's own.
    BUFFERS.with(|b| b.borrow_mut().insert(ptr, framed));
    ptr
}

/// The same, for a JSON answer.
pub fn reply(value: serde_json::Value) -> i32 {
    respond(serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"error\":\"unencodable\"}".to_vec()))
}

/// A failure, in the one shape both host pages read: an object with an `error` string.
pub fn error(why: impl std::fmt::Display) -> i32 {
    reply(serde_json::json!({ "error": why.to_string() }))
}

/// The bytes of a request the host wrote into `ptr`, clamped to what was actually reserved.
///
/// `len` is the host's claim about how much it wrote. Trusting it would read off the end of the
/// buffer, so it is a maximum rather than a length.
pub fn request(ptr: i32, len: i32) -> Option<Vec<u8>> {
    let mut bytes = take(ptr)?;
    bytes.truncate((len.max(0) as usize).min(bytes.len()));
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_its_length_then_its_body() {
        let ptr = respond(b"hello".to_vec());
        let frame = take(ptr).expect("the frame is on the table");
        assert_eq!(&frame[..4], &5u32.to_le_bytes());
        assert_eq!(&frame[4..], b"hello");
    }

    #[test]
    fn a_taken_buffer_is_gone() {
        let ptr = reserve(8);
        assert!(take(ptr).is_some());
        assert!(take(ptr).is_none(), "a second free would be a double free");
    }

    #[test]
    fn a_host_that_overstates_the_length_reads_no_further_than_it_reserved() {
        let ptr = reserve(4);
        let bytes = request(ptr, 4096).expect("the buffer is on the table");
        assert_eq!(bytes.len(), 4, "the claim is a maximum, not a length");
    }

    #[test]
    fn an_error_is_an_object_the_page_can_read() {
        let ptr = error("no such buffer");
        let frame = take(ptr).expect("the frame is on the table");
        let body: serde_json::Value = serde_json::from_slice(&frame[4..]).expect("json");
        assert_eq!(body["error"], "no such buffer");
    }
}
