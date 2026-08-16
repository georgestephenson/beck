//! Beck's third emitter: `Core` compiled to **WebAssembly**, for the tier a person is sitting in
//! front of.
//!
//! # What this is, and what it is not yet
//!
//! [`docs/05`](../../../../docs/05-tier-lowering.md) §5.1 asks for "the component's pure code
//! compiled to WASM"; [`adr/0022`](../../../../docs/adr/0022-mode-b-ships-the-backend-it-has.md)
//! records why Mode B ships an *interpreter* today and what would reverse that decision — "a
//! `Core → WASM` backend… What it does not inherit is the heap, which is still the whole of the
//! remaining work." That sentence is still true, and it is this crate's boundary: **the scalar
//! subset compiles and the heap does not**, so a component's `view` — which is nothing but heap —
//! still runs in [`beck_wasm`](../../beck-wasm/index.html)'s kernel. ADR 0022 is not reversed by
//! this crate; it is the first half of what would reverse it.
//!
//! What *is* here is everything around a heap, which is the part that is new on this target and
//! could not be inherited from [`beck_llvm`]:
//!
//! * **A binary format written by hand** ([`binary`]), because a browser loads bytes and there is
//!   no assembler on the path.
//! * **Structured control flow.** WebAssembly has `block`, `loop` and `if` and no jumps, so a
//!   `match` is a nest of typed `if`s rather than a switch over labels.
//! * **A trap that is a value.** `unreachable` and `i64.div_s` abort the *instance*; a Beck
//!   program that overflows has failed the way its type says it can, so a trap is
//!   [`beck_llvm::Trap`]'s code in an exported global — one wire, decoded by
//!   [`beck_llvm::Trap::message`], rather than a third spelling of the same failures.
//! * **A tail call that is a proposal.** [`docs/93`](../../../../docs/93-the-native-backends-report.md)
//!   §93.4 makes a tail call a *guarantee*; WebAssembly's `return_call` is how that guarantee is
//!   spelled, and it is why the emitted module needs a runtime with tail calls (WebAssembly 2.0 —
//!   V8 since Chrome 112, and therefore every browser Mode B has ever run in).
//!
//! # What it shares, and why
//!
//! The monomorphiser, the trap codes and the refusal discipline are [`beck_llvm`]'s, taken as a
//! dependency for the reason [`beck_clif`](../../beck_clif/index.html) took them
//! ([`docs/93`](../../../../docs/93-the-native-backends-report.md) §93.8): a second — now third —
//! *emitter* is evidence, and a second *design* is drift. What is not shared is the emitter, so
//! the differential is comparing three implementations of one semantics rather than one
//! implementation with itself.
//!
//! # Using it
//!
//! ```
//! let (placed, _, _) = beck_core::compile_or_library_str("t.beck", "def twice(n: Int) -> Int:\n    return n + n\n");
//! let placed = placed.expect("compiles");
//! let module = beck_wasmgen::module(&placed.program);
//! assert!(module.signature("twice").is_some());
//! assert_eq!(&module.wasm[..4], b"\0asm");
//! ```

pub mod binary;
pub mod emit;
pub mod text;

pub use emit::{module, Module, MAX_PARAMS, TRAP, TRAP_PAYLOAD, TRAP_SPAN};

#[cfg(test)]
mod tests {
    use beck_llvm::Trap;

    fn compile(src: &str) -> crate::Module {
        let (placed, diags, map) = beck_core::compile_or_library_str("t.beck", src);
        assert!(!diags.has_errors(), "{}", diags.render(&map));
        crate::module(&placed.expect("compiles").program)
    }

    #[test]
    fn a_scalar_definition_compiles_and_a_heap_one_is_refused_by_name() {
        let m = compile(
            "def twice(n: Int) -> Int:\n    return n + n\n\n\
             def greet(who: Str) -> Str:\n    return who\n",
        );
        assert!(m.signature("twice").is_some(), "{:?}", m.refusals);
        let refused = m
            .refusals
            .iter()
            .find(|r| &*r.name == "greet")
            .expect("a heap-valued definition is refused");
        assert!(
            refused.reason.contains("heap"),
            "a refusal says what it refused and why: {}",
            refused.reason
        );
    }

    #[test]
    fn the_module_is_the_same_bytes_twice() {
        // A build wants this and a diff needs it: the order is the program's declaration order and
        // not a hash seed's (`docs/93` §93.1's readable-artefact property one target over).
        let src = "def a(n: Int) -> Int:\n    return n * 2\n\n\
                   def b(n: Int) -> Int:\n    return a(n) + 1\n";
        assert_eq!(compile(src).wasm, compile(src).wasm);
        assert_eq!(compile(src).text, compile(src).text);
    }

    #[test]
    fn a_trap_code_is_the_one_the_other_two_backends_store() {
        // Not a test of this emitter so much as of the decision behind it: the host decodes one
        // wire. If these ever diverge, `Trap::message` is answering about a different backend.
        let m = compile("def d(a: Int, b: Int) -> Int:\n    return a / b\n");
        assert!(m.signature("d").is_some(), "{:?}", m.refusals);
        assert!(m
            .text
            .contains(&format!("i32.const {}", Trap::DivOverflow.code())));
    }

    #[test]
    fn a_tail_call_is_a_jump() {
        // §93.4 is a guarantee rather than an optimisation, so it is asserted on the emitted
        // instruction and not on a stack depth somebody measured.
        let m = compile(
            "def go(n: Int, acc: Int) -> Int:\n\
             \x20   if n == 0:\n\
             \x20       return acc\n\
             \x20   return go(n - 1, acc + n)\n",
        );
        assert!(m.signature("go").is_some(), "{:?}", m.refusals);
        assert!(m.text.contains("return_call"), "{}", m.text);
    }

    #[test]
    fn sin_and_cos_are_refused_with_the_reason_rather_than_approximated() {
        let m = compile("def s(x: Float) -> Float:\n    return sin(x)\n");
        let refused = m
            .refusals
            .iter()
            .find(|r| &*r.name == "s")
            .expect("`sin` is refused");
        assert!(
            refused.reason.contains("runtime library"),
            "{}",
            refused.reason
        );
    }
}
