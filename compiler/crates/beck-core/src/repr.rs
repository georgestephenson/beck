//! The storable shape of a [`Value`], and the binary encoding of it.
//!
//! # Why this is not `serde_json::Value`
//!
//! [`crate::core::value_to_repr`] produces a self-describing JSON tree, and that is the right thing
//! for anything a person reads. It is the wrong thing for the log, for two reasons that compound:
//!
//! 1. **Size.** The JSON repr tags every scalar: `Value::Int(1)` becomes `{"$":"int","v":1}` — 19
//!    bytes carrying eight bits. A `Toggled(id)` event is a few dozen bytes of information and
//!    around two hundred of punctuation.
//! 2. **Work.** Every append builds a whole `serde_json::Value` tree, then serialises it to text;
//!    every read parses text into a tree, then walks the tree to rebuild the `Value`. Four
//!    traversals and two allocations per event, in the one place §3.7 makes the whole system
//!    serial.
//!
//! Phase 0 stored events with `postcard` and measured 7,660 events/s through Postgres
//! ([`docs/18-phase-0-report.md`](../../../../../docs/18-phase-0-report.md) §18.3.2). Phase 1 rewrote
//! the log against `beck_core::Value` — which the runtime must not know the shape of — and reached
//! for JSON because it is self-describing, which is exactly what postcard is not. That was a real
//! constraint and this module is the answer to it: a **concrete** type postcard can encode, plus
//! total conversions to and from `Value`.
//!
//! # Why a second type rather than `Serialize` on `Value`
//!
//! `Value` carries `Arc`s, a persistent map, and three variants that are *not storable at all* —
//! `Html`, `Attr`, `Closure`. A derived `Serialize` would have to either panic on those or invent
//! an encoding for them, and inventing one is how a view ends up in the log
//! ([`crate::secure`], §3.5). Making the storable subset a separate type means **the encoder cannot
//! be handed something unstorable**: the conversion returns [`NotStorable`], at the boundary, once.
//!
//! # Format stability
//!
//! This encoding is on disk, so it is a compatibility surface. [`FORMAT`] is stamped into every
//! store and checked on open, because a log read back under a different encoding does not fail —
//! it produces plausible nonsense, which is the one outcome an append-only audit trail may never
//! have.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::{Fields, NotStorable, Value};
use crate::pmap::PMap;

/// The on-disk format version.
///
/// Bump when [`Repr`]'s shape changes in a way that makes old bytes decode differently. A store
/// stamped with a different version is refused rather than read: replay is the only description of
/// a program's history, and a misread log is worse than an unreadable one.
///
/// * `1` — JSON text (Phase 1, Phase 2).
/// * `2` — postcard over [`Repr`].
pub const FORMAT: u32 = 2;

/// A [`Value`] restricted to what may be stored, as a concrete type a non-self-describing codec can
/// encode.
///
/// The variants are exactly [`crate::core::value_to_repr`]'s cases, minus the three it refuses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Repr {
    Unit,
    Bool(bool),
    Int(i64),
    /// The bit pattern, as `Value` stores it — so that a round trip is exact for `NaN` and for
    /// negative zero, which a decimal rendering is not.
    Float(u64),
    Str(String),
    List(Vec<Repr>),
    /// Pairs rather than a map, because the key is a `Repr` and most map encodings insist on
    /// strings. Order is the `PMap`'s, which is sorted, so the encoding is canonical.
    Map(Vec<(Repr, Repr)>),
    Data {
        ty: String,
        variant: Option<String>,
        fields: Vec<(String, Repr)>,
    },
}

impl Repr {
    /// The storable projection of a value, or the reason there isn't one.
    pub fn of(v: &Value) -> Result<Repr, NotStorable> {
        Ok(match v {
            Value::Unit => Repr::Unit,
            Value::Bool(b) => Repr::Bool(*b),
            Value::Int(i) => Repr::Int(*i),
            Value::Float(bits) => Repr::Float(*bits),
            Value::Str(s) => Repr::Str(s.to_string()),
            Value::List(xs) => {
                let mut out = Vec::with_capacity(xs.len());
                xs.try_for_each(|x| Repr::of(x).map(|r| out.push(r)))?;
                Repr::List(out)
            }
            Value::Map(m) => {
                let mut pairs = Vec::with_capacity(m.len());
                for (k, val) in m.iter() {
                    pairs.push((Repr::of(k)?, Repr::of(val)?));
                }
                Repr::Map(pairs)
            }
            Value::Data(d) => Repr::Data {
                ty: d.ty.to_string(),
                variant: d.variant.as_ref().map(|v| v.to_string()),
                fields: d
                    .fields
                    .iter()
                    .map(|(k, val)| Ok((k.to_string(), Repr::of(val)?)))
                    .collect::<Result<_, NotStorable>>()?,
            },
            Value::Html(_) => return Err(NotStorable { kind: "view" }),
            Value::Attr(_) => return Err(NotStorable { kind: "attribute" }),
            Value::Closure(_) => return Err(NotStorable { kind: "closure" }),
        })
    }

    /// Back to a value. Total: every `Repr` denotes a `Value`, which is the point of the type.
    pub fn to_value(&self) -> Value {
        match self {
            Repr::Unit => Value::Unit,
            Repr::Bool(b) => Value::Bool(*b),
            Repr::Int(i) => Value::Int(*i),
            Repr::Float(bits) => Value::Float(*bits),
            Repr::Str(s) => Value::str_(s),
            Repr::List(xs) => Value::list(xs.iter().map(Repr::to_value).collect()),
            Repr::Map(pairs) => {
                let mut m = PMap::new();
                for (k, v) in pairs {
                    m = m.insert(k.to_value(), v.to_value());
                }
                Value::Map(m)
            }
            Repr::Data {
                ty,
                variant,
                fields,
            } => Value::data(
                Arc::from(ty.as_str()),
                variant.as_deref().map(Arc::from),
                fields
                    .iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), v.to_value()))
                    .collect::<Fields>(),
            ),
        }
    }
}

/// Encode a value for storage.
pub fn to_bytes(v: &Value) -> Result<Vec<u8>, NotStorable> {
    let repr = Repr::of(v)?;
    // The only failure postcard has for an owned `Vec` sink is allocation, and a `Repr` built from
    // a `Value` in memory cannot exceed it by construction.
    Ok(postcard::to_allocvec(&repr).expect("a Repr is encodable"))
}

/// Decode a value written by [`to_bytes`].
pub fn from_bytes(bytes: &[u8]) -> Result<Value, postcard::Error> {
    postcard::from_bytes::<Repr>(bytes).map(|r| r.to_value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value_to_repr;

    fn sample() -> Value {
        let mut m = PMap::new();
        m = m.insert(Value::str_("a"), Value::Int(1));
        m = m.insert(Value::str_("b"), Value::Bool(false));
        Value::data(
            Arc::from("Todo"),
            Some(Arc::from("Added")),
            Fields::from_iter([
                (Arc::from("id"), Value::str_("t-1")),
                (Arc::from("text"), Value::str_("milk")),
                (Arc::from("n"), Value::Int(-7)),
                (Arc::from("f"), Value::Float(f64::to_bits(1.5))),
                (
                    Arc::from("xs"),
                    Value::list(vec![Value::Unit, Value::Int(2)]),
                ),
                (Arc::from("m"), Value::Map(m)),
            ]),
        )
    }

    #[test]
    fn every_storable_shape_round_trips_exactly() {
        let v = sample();
        let bytes = to_bytes(&v).expect("it is storable");
        assert_eq!(from_bytes(&bytes).expect("it decodes"), v);
    }

    #[test]
    fn a_float_survives_as_its_bit_pattern() {
        // `Value` stores the bits so that it can be `Ord`; an encoding that went through a decimal
        // would lose `NaN`'s payload and merge `-0.0` with `0.0`, and both are map keys.
        for bits in [
            f64::to_bits(0.0),
            f64::to_bits(-0.0),
            f64::to_bits(f64::NAN),
            f64::to_bits(f64::INFINITY),
            f64::to_bits(0.1 + 0.2),
        ] {
            let v = Value::Float(bits);
            assert_eq!(from_bytes(&to_bytes(&v).unwrap()).unwrap(), v);
        }
    }

    #[test]
    fn the_three_unstorable_variants_are_refused_at_the_encoder() {
        // The same refusal `value_to_repr` makes, at the same place, for the same §3.5 reason —
        // and now unreachable from a program that compiles, because `secure::storable` proves it.
        let html = Value::Html(Arc::new(crate::html::Html::Text {
            text: "x".to_string(),
            hash: 0,
        }));
        assert!(to_bytes(&html).is_err());
        assert_eq!(to_bytes(&html).unwrap_err().kind, "view");

        // …and a value that merely *contains* one is refused too, because the walk is total.
        let nested = Value::list(vec![Value::Int(1), html]);
        assert!(to_bytes(&nested).is_err());
    }

    #[test]
    fn the_binary_encoding_is_much_smaller_than_the_json_one() {
        // The reason this module exists, as a number rather than an assertion. The margin is
        // deliberately loose — this is a regression guard, not a benchmark; `beck bench log` is
        // where the throughput question is answered.
        let v = sample();
        let binary = to_bytes(&v).expect("storable").len();
        let json = value_to_repr(&v).expect("storable").to_string().len();
        assert!(
            binary * 3 < json,
            "binary {binary} B against JSON {json} B — the encoding change stopped paying"
        );
    }

    #[test]
    fn the_encoding_is_canonical() {
        // Two equal values encode to the same bytes, whatever order they were built in. The log's
        // digest depends on it (`beck replay --verify`), and a map that iterated in insertion order
        // would break it silently.
        let mut a = PMap::new();
        a = a.insert(Value::Int(2), Value::str_("b"));
        a = a.insert(Value::Int(1), Value::str_("a"));
        let mut b = PMap::new();
        b = b.insert(Value::Int(1), Value::str_("a"));
        b = b.insert(Value::Int(2), Value::str_("b"));
        assert_eq!(
            to_bytes(&Value::Map(a)).unwrap(),
            to_bytes(&Value::Map(b)).unwrap()
        );
    }
}
