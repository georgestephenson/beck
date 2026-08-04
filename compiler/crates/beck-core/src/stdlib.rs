//! The standard library's Beck half, carried inside the compiler.
//!
//! [`compiler/lib/README.md`](../../../../lib/README.md) divides the standard library in two: a
//! host's table or grammar is a primitive in [`crate::prelude`], and composition is a file written
//! in Beck. The primitive half has always been in the binary. This is the other half, in the binary
//! for the same reason — so that `import bignum` means the library this compiler was built with,
//! wherever the program being compiled happens to sit.
//!
//! [`adr/0018`](../../../../../docs/adr/0018-the-standard-library-is-carried-in-the-compiler.md) is
//! the decision and the alternatives it was taken over; [`10`](../../../../../docs/10-decisions.md)
//! D23 is the language-level rule:
//!
//! > `import x` resolves against the root module's own directory first, and against the standard
//! > library second.
//!
//! The directory-first half is what keeps the files here editable: `lib/decimal.beck` imports
//! `bignum` and gets the file beside it, so working on the library does not mean rebuilding the
//! compiler to see the change. A program anywhere else gets this copy.

use crate::project::Sources;

/// Every module in `compiler/lib/`, under the name an `import` writes.
///
/// The list is here rather than discovered by a `build.rs` walk because a file appearing in the
/// standard library is an API addition — a new name every program in the language can suddenly
/// import — and one that should be written down and reviewed rather than picked up. It is not
/// trusted to be complete: `beck-cli/tests/stdlib.rs` reads the directory and fails if a file there
/// is missing from here, which is the same gate the directory has always had for its tests.
pub const MODULES: &[(&str, &str)] = &[
    ("bignum", include_str!("../../../lib/bignum.beck")),
    ("collections", include_str!("../../../lib/collections.beck")),
    ("crypto", include_str!("../../../lib/crypto.beck")),
    ("dates", include_str!("../../../lib/dates.beck")),
    ("decimal", include_str!("../../../lib/decimal.beck")),
    ("documents", include_str!("../../../lib/documents.beck")),
    ("format", include_str!("../../../lib/format.beck")),
    ("http", include_str!("../../../lib/http.beck")),
    ("money", include_str!("../../../lib/money.beck")),
    ("text", include_str!("../../../lib/text.beck")),
];

/// The source of a standard-library module, if there is one by that name.
pub fn source(name: &str) -> Option<&'static str> {
    MODULES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// Whether `name` is a standard-library module.
pub fn has(name: &str) -> bool {
    source(name).is_some()
}

/// Every name a program can import without a file beside it.
pub fn names() -> impl Iterator<Item = &'static str> {
    MODULES.iter().map(|(n, _)| *n)
}

/// The module as the project loader wants it.
///
/// The path is `<std>/<name>.beck` rather than a filesystem path, because there is no file: a
/// diagnostic inside a standard-library module has to say where it is, and saying `lib/bignum.beck`
/// would name a file that need not exist on the machine compiling.
pub fn sources(name: &str) -> Option<Sources> {
    source(name).map(|text| Sources {
        module: Some(text.to_string()),
        interface: None,
        path: Some(format!("<std>/{name}.beck")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        let names: Vec<&str> = names().collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            names, sorted,
            "the standard-library table is not a sorted set"
        );
    }

    #[test]
    fn every_module_carries_its_source() {
        for (name, text) in MODULES {
            assert!(!text.trim().is_empty(), "`{name}` is embedded empty");
        }
    }

    /// A module a program does not have beside it is not a standard-library module by accident.
    #[test]
    fn a_name_that_is_not_in_the_library_resolves_to_nothing() {
        assert!(source("nowhere").is_none());
        assert!(!has("prelude"));
    }
}
