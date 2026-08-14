//! Shared fixtures for the harnesses.

pub mod browser;
pub mod clofix;
pub mod failfix;
pub mod genfix;
pub mod heapfix;
pub mod hostfix;
pub mod listfix;
pub mod lsp;
pub mod mapfix;
pub mod relfix;
pub mod scalar;
pub mod socket;
pub mod textfix;
pub mod viewfix;

use std::sync::Arc;

use beck_core::{Placed, Value};
use beck_rt::Runtime;

#[allow(dead_code)] // used by the differential harness, not by every test binary
pub const ACTORS: &[&str] = &["alice", "bob"];

/// The example program, compiled. Every harness runs against the file a reader can open.
pub fn todo_program() -> Placed {
    let src = include_str!("../../../../examples/todo.beck");
    let (placed, diags, map) = beck_core::compile_str("examples/todo.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("the example compiles")
}

/// The example program, prepared for execution by the default backend.
///
/// The harnesses go through this rather than naming a backend each, so that running them against
/// a second backend — §4.8's differential-between-backends test, once one exists — is one edit.
#[allow(dead_code)] // not every harness starts an App
pub fn todo_runtime() -> Runtime {
    let placed = todo_program();
    let backend = beck_eval::backend(&placed);
    Runtime::new(placed, backend).expect("the example prepares")
}

#[allow(dead_code)] // used by the differential harness, not by every test binary
/// Build a `Command` value directly, as the wire decoder would.
///
/// `Id` is a newtype over `Str`, so its payload is wrapped — nominal in the type system,
/// transparent on the wire (§3.1).
pub fn command(variant: &str, fields: &[(&str, &str)]) -> Value {
    let mut map = beck_core::core::Fields::new();
    for (name, value) in fields {
        let v = if *name == "id" {
            Value::data(
                Arc::from("Id"),
                None,
                beck_core::core::Fields::from_iter([(Arc::from("value"), Value::str_(value))]),
            )
        } else {
            Value::str_(value)
        };
        map.insert(Arc::from(*name), v);
    }
    Value::data(Arc::from("Command"), Some(Arc::from(variant)), map)
}
