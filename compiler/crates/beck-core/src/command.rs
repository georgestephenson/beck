//! What a client may send, resolved once: the `Command` union as a decoder.
//!
//! §3.5: "the client's entire write surface is `send(cmd)` into a typed `Command` union. There is
//! no other mutation path — mass assignment and over-posting have no representation." That
//! property is enforced here, and here only: a field the union does not declare is not decoded, it
//! is rejected.
//!
//! # Why this is a schema rather than a lookup
//!
//! The decoder used to read the program's type table on every command, which is fine when the
//! decoder and the type table are in the same process. Mode B's client has neither — it holds a
//! bundle, not a program ([`crate::bundle`]) — and the one thing worse than a client that cannot
//! decode a command is a *second* decoder written to a second reading of the same union. So the
//! union is resolved to this at compile time, both tiers decode with the same function, and
//! "the client and the server disagree about what `Toggle` is" stops being expressible.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::{Fields, Value};
use crate::split::Placed;
use crate::ty::{Ty, TyDecl};

/// The program's command union, flattened to what a decoder needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    /// The union's name — `Command`, unless the program named it something else.
    pub ty: String,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<(String, FieldTy)>,
}

/// A field's type, resolved through however many newtypes wrap it.
///
/// "A newtype is transparent on the wire and nominal in the type system" — the whole point of
/// §3.1's "ids of different entities must not be interchangeable" — so the wrappers are recorded
/// in the order they have to be rebuilt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldTy {
    Str,
    Int,
    Bool,
    Float,
    /// `Id(value=<inner>)`.
    Newtype(String, Box<FieldTy>),
    /// A type the wire format has no encoding for. Carried rather than dropped so the refusal
    /// names it, and so a bundle built for a program with one is not silently missing a field.
    Undecodable(String),
}

impl Schema {
    pub fn of(placed: &Placed) -> Schema {
        Schema::of_union(
            placed,
            placed.roles.command_ty.con_name().unwrap_or("Command"),
        )
    }

    /// The same, for any union a client may construct values of.
    ///
    /// D30's gestures need one: a gesture is built by a handler in the page exactly as a command
    /// is, so it needs the same resolved decoder — and it needs a *separate* one, because the two
    /// unions are different types and a client that could decode a gesture as a command would have
    /// found the write surface §3.5 closes.
    pub fn of_union(placed: &Placed, name: &str) -> Schema {
        let name = name.to_string();
        let variants = match placed.program.types.get(name.as_str()) {
            Some(TyDecl::Union { variants, .. }) => variants
                .iter()
                .map(|v| Variant {
                    name: v.name.to_string(),
                    fields: v
                        .fields
                        .iter()
                        .map(|(f, ty)| (f.to_string(), FieldTy::of(ty, placed)))
                        .collect(),
                })
                .collect(),
            // Not a union, so nothing can be decoded against it. The refusal happens per command
            // rather than here: a library has no command type at all, and building one of these
            // for it must not be an error.
            _ => Vec::new(),
        };
        Schema { ty: name, variants }
    }

    /// Decode a command from the wire, against the program's own `Command` union.
    pub fn decode(&self, json: &serde_json::Value) -> Result<Value, String> {
        let tag = json
            .get("c")
            .and_then(|c| c.as_str())
            .ok_or("a command needs a `c` tag naming its variant")?;
        let variant = self
            .variants
            .iter()
            .find(|v| v.name == tag)
            .ok_or_else(|| format!("`{tag}` is not a variant of `{}`", self.ty))?;

        let mut fields = Fields::new();
        for (field, ty) in &variant.fields {
            let raw = json
                .get(field.as_str())
                .ok_or_else(|| format!("`{tag}` needs a `{field}`"))?;
            fields.insert(Arc::from(field.as_str()), ty.decode(raw)?);
        }
        Ok(Value::data(
            Arc::from(self.ty.as_str()),
            Some(Arc::from(variant.name.as_str())),
            fields,
        ))
    }
}

impl FieldTy {
    fn of(ty: &Ty, placed: &Placed) -> FieldTy {
        let name = ty.con_name().unwrap_or("");
        if let Some(TyDecl::Newtype { inner, .. }) = placed.program.types.get(name) {
            return FieldTy::Newtype(name.to_string(), Box::new(FieldTy::of(inner, placed)));
        }
        match name {
            Ty::STR => FieldTy::Str,
            Ty::INT => FieldTy::Int,
            Ty::BOOL => FieldTy::Bool,
            Ty::FLOAT => FieldTy::Float,
            other => FieldTy::Undecodable(other.to_string()),
        }
    }

    fn decode(&self, raw: &serde_json::Value) -> Result<Value, String> {
        match self {
            FieldTy::Newtype(name, inner) => Ok(Value::data(
                Arc::from(name.as_str()),
                None,
                Fields::from_iter([(Arc::from("value"), inner.decode(raw)?)]),
            )),
            FieldTy::Str => raw
                .as_str()
                .map(Value::str_)
                .ok_or_else(|| format!("expected a string, got {raw}")),
            FieldTy::Int => raw
                .as_i64()
                .map(Value::Int)
                .ok_or_else(|| format!("expected an integer, got {raw}")),
            FieldTy::Bool => raw
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| format!("expected a boolean, got {raw}")),
            // A real crosses the wire as a JSON number, and an integral one arrives as an integer
            // — `1` and `1.0` are the same JSON token — so this accepts either and canonicalises
            // through `Value::float` (`docs/27` §27.2).
            FieldTy::Float => raw
                .as_f64()
                .map(Value::float)
                .ok_or_else(|| format!("expected a number, got {raw}")),
            FieldTy::Undecodable(other) => Err(format!("cannot decode `{other}` from the wire")),
        }
    }
}
