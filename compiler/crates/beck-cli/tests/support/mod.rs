//! Shared fixtures for the harnesses.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_core::{Placed, Value};

pub const ACTORS: &[&str] = &["alice", "bob"];

/// The example program, compiled. Every harness runs against the file a reader can open.
pub fn todo_program() -> Placed {
    let src = include_str!("../../../../examples/todo.beck");
    let (placed, diags, map) = beck_core::compile_str("examples/todo.beck", src);
    assert!(!diags.has_errors(), "{}", diags.render(&map));
    placed.expect("the example compiles")
}

/// Build a `Command` value directly, as the wire decoder would.
///
/// `Id` is a newtype over `Str`, so its payload is wrapped — nominal in the type system,
/// transparent on the wire (§3.1).
pub fn command(variant: &str, fields: &[(&str, &str)]) -> Value {
    let mut map = BTreeMap::new();
    for (name, value) in fields {
        let v = if *name == "id" {
            Value::Data {
                ty: Arc::from("Id"),
                variant: None,
                fields: Arc::new(BTreeMap::from([(Arc::from("value"), Value::str_(value))])),
            }
        } else {
            Value::str_(value)
        };
        map.insert(Arc::from(*name), v);
    }
    Value::Data {
        ty: Arc::from("Command"),
        variant: Some(Arc::from(variant)),
        fields: Arc::new(map),
    }
}
