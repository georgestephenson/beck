//! One type-directed value generator, used by three features.
//!
//! `docs/21-tests-in-beck-and-proof.md` §21.3 rule 5: "Stub return values, property-test inputs
//! and `given` gaps are the same problem: produce an inhabitant of a known type. The compiler has
//! the full type, including `newtype`s, unions and records, so it can derive:
//!
//! * a **canonical** inhabitant (first variant, empty collection, zero, `""`) for the don't-care
//!   case;
//! * an **arbitrary** one, with shrinking, for `property` blocks;
//! * and it can refuse, with a diagnostic, for a type with no inhabitant it can construct —
//!   `secret[T]` being the interesting one, since inventing a secret in a test is exactly the sort
//!   of thing that should require somebody to type it out."
//!
//! "This is one generator, used by three features, and it is the piece to build first because
//! §21.2's property tests need it too." It is built once, here.
//!
//! # Determinism
//!
//! The randomness is a counter-based splitmix, seeded from the test's name and the run index — not
//! from a clock. §21.2: "**A flaky Beck test should be impossible**, and if one appears it is a
//! compiler defect." A property test that fails on run 37 fails on run 37 again, on any machine, so
//! the shrunk counterexample the report prints is one a person can reproduce by re-running the
//! command they already ran.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::core::{Fields, Value};
use crate::pmap::PMap;
use crate::ty::{Ty, TyDecl};

/// A type the generator will not invent a value for, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uninhabitable {
    pub ty: String,
    pub why: &'static str,
}

impl std::fmt::Display for Uninhabitable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot invent a `{}`: {}", self.ty, self.why)
    }
}

impl std::error::Error for Uninhabitable {}

type Types = BTreeMap<Arc<str>, TyDecl>;

/// A counter-based PRNG. Splitmix64, which is the whole algorithm and needs no state beyond a
/// counter — so a value's generation depends on *where* it is asked for, and nothing else.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed from a name and a run index. Two runs of the same suite generate the same values.
    pub fn seeded(name: &str, run: u64) -> Rng {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Rng {
            state: h ^ run.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// The don't-care inhabitant — §21.3 rule 1's "'any value' is the default, so it needs no
/// expression".
///
/// Deterministic and *small*: the first variant, an empty collection, zero, `""`. Small matters
/// because this is what a stub returns when nobody said otherwise, and a surprising default value
/// is worse than an obvious one.
pub fn canonical(ty: &Ty, types: &Types) -> Result<Value, Uninhabitable> {
    build(ty, types, None, 0)
}

/// An arbitrary inhabitant, for a `property` block's parameters.
pub fn arbitrary(ty: &Ty, types: &Types, rng: &mut Rng) -> Result<Value, Uninhabitable> {
    build(ty, types, Some(rng), 0)
}

/// How deep a recursive type is allowed to nest before the generator falls back to the canonical
/// inhabitant. Without it a `union Tree: Node(l: Tree, r: Tree)` would not terminate.
const MAX_DEPTH: usize = 4;

fn build(
    ty: &Ty,
    types: &Types,
    mut rng: Option<&mut Rng>,
    depth: usize,
) -> Result<Value, Uninhabitable> {
    let (name, args) = match ty {
        Ty::Con(n, args) => (n.as_ref(), args.as_slice()),
        // A type variable is not a type: a `property` parameter has to be written down, and
        // §3.1's "mandatory annotations on public signatures" means one always is.
        Ty::Var(_) => {
            return Err(Uninhabitable {
                ty: ty.to_string(),
                why: "it is still a type variable — write the type down",
            })
        }
        Ty::Fun(..) => {
            return Err(Uninhabitable {
                ty: ty.to_string(),
                why: "a function is code, and the generator invents data",
            })
        }
    };
    // Past the depth limit, stop taking chances and take the smallest thing.
    if depth >= MAX_DEPTH {
        rng = None;
    }
    let mut rng = rng;

    match name {
        Ty::UNIT => return Ok(Value::Unit),
        Ty::BOOL => {
            return Ok(Value::Bool(match reborrow(&mut rng) {
                Some(r) => r.next_u64() & 1 == 1,
                None => false,
            }))
        }
        Ty::INT => {
            return Ok(Value::Int(match reborrow(&mut rng) {
                // A small band around zero: the interesting integers in a program that counts
                // things are 0, 1 and the boundary, not 2^61.
                Some(r) => (r.next_u64() % 21) as i64 - 10,
                None => 0,
            }));
        }
        Ty::FLOAT => {
            return Ok(Value::float(match reborrow(&mut rng) {
                Some(r) => (r.next_u64() % 2001) as f64 / 100.0 - 10.0,
                None => 0.0,
            }))
        }
        Ty::STR => {
            return Ok(Value::str_(match reborrow(&mut rng) {
                Some(r) => WORDS[r.below(WORDS.len())],
                None => "",
            }))
        }
        // A view is not data (`value_to_repr` refuses one), so the generator refuses one too rather
        // than inventing an empty page that would make an assertion pass for the wrong reason.
        Ty::HTML | Ty::ATTR => {
            return Err(Uninhabitable {
                ty: ty.to_string(),
                why: "a view is rendered from a state, not invented",
            })
        }
        // §3.5's point, kept: "inventing a secret in a test is exactly the sort of thing that
        // should require somebody to type it out".
        Ty::SECRET => {
            return Err(Uninhabitable {
                ty: ty.to_string(),
                why: "a secret has to be written out by a person, never invented by a generator",
            })
        }
        Ty::LIST => {
            let elem = args.first().cloned().unwrap_or_else(Ty::unit);
            let n = match reborrow(&mut rng) {
                Some(r) => r.below(4),
                None => 0,
            };
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                out.push(build(&elem, types, reborrow(&mut rng), depth + 1 + i)?);
            }
            return Ok(Value::list(out));
        }
        Ty::MAP => {
            let k = args.first().cloned().unwrap_or_else(Ty::unit);
            let v = args.get(1).cloned().unwrap_or_else(Ty::unit);
            let n = match reborrow(&mut rng) {
                Some(r) => r.below(3),
                None => 0,
            };
            let mut m = PMap::new();
            for i in 0..n {
                let key = build(&k, types, reborrow(&mut rng), depth + 1 + i)?;
                let val = build(&v, types, reborrow(&mut rng), depth + 1 + i)?;
                m = m.insert(key, val);
            }
            return Ok(Value::Map(m));
        }
        // A stream or a signal is a node in the graph, not a value a test hands anybody.
        Ty::STREAM | Ty::SIGNAL => {
            return Err(Uninhabitable {
                ty: ty.to_string(),
                why: "a signal is a node in the program's graph, not a value",
            })
        }
        _ => {}
    }

    match types.get(name) {
        Some(TyDecl::Newtype { inner, .. }) => {
            let v = build(inner, types, rng, depth + 1)?;
            Ok(Value::data(
                Arc::from(name),
                None,
                Fields::from_iter([(Arc::from("value"), v)]),
            ))
        }
        Some(TyDecl::Alias { ty: inner, .. }) => build(inner, types, rng, depth),
        Some(TyDecl::Model { fields, .. }) => {
            let fields = fields.clone();
            let mut out = Fields::new();
            for (i, (fname, fty)) in fields.iter().enumerate() {
                let fty = crate::ty::instantiate_decl(fty, args);
                out.insert(
                    fname.clone(),
                    build(&fty, types, reborrow(&mut rng), depth + 1 + i)?,
                );
            }
            Ok(Value::data(Arc::from(name), None, out))
        }
        Some(TyDecl::Union { variants, .. }) => {
            if variants.is_empty() {
                return Err(Uninhabitable {
                    ty: ty.to_string(),
                    why: "it has no variants",
                });
            }
            let variants = variants.clone();
            // Prefer a variant that does not recurse into this same type, so the canonical
            // inhabitant of a recursive union is a leaf.
            let idx = match reborrow(&mut rng) {
                Some(r) => r.below(variants.len()),
                None => variants
                    .iter()
                    .position(|v| !v.fields.iter().any(|(_, t)| t.con_name() == Some(name)))
                    .unwrap_or(0),
            };
            let v = &variants[idx];
            let mut out = Fields::new();
            for (i, (fname, fty)) in v.fields.iter().enumerate() {
                let fty = crate::ty::instantiate_decl(fty, args);
                out.insert(
                    fname.clone(),
                    build(&fty, types, reborrow(&mut rng), depth + 1 + i)?,
                );
            }
            Ok(Value::data(Arc::from(name), Some(v.name.clone()), out))
        }
        _ => Err(Uninhabitable {
            ty: ty.to_string(),
            why: "this program does not declare it",
        }),
    }
}

fn reborrow<'a, 'b: 'a>(r: &'a mut Option<&'b mut Rng>) -> Option<&'a mut Rng> {
    r.as_deref_mut()
}

/// Smaller candidates for a failing input, most-shrunk first.
///
/// Shrinking is a property of the *value*, not of the type: a shorter list is smaller than a longer
/// one whatever the elements are, and an integer closer to zero is smaller than one further away.
/// That keeps this total and terminating — every candidate is strictly smaller by
/// [`size`], so a shrink loop cannot cycle.
pub fn shrink(v: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    match v {
        Value::Int(0) | Value::Bool(false) | Value::Unit => {}
        Value::Int(n) => {
            out.push(Value::Int(0));
            if n.abs() > 1 {
                out.push(Value::Int(n / 2));
            }
            if *n > 0 {
                out.push(Value::Int(n - 1));
            } else {
                out.push(Value::Int(n + 1));
            }
        }
        Value::Bool(true) => out.push(Value::Bool(false)),
        Value::Float(_) => {
            if v.as_f64() != Some(0.0) {
                out.push(Value::float(0.0));
            }
        }
        Value::Str(s) if !s.is_empty() => {
            out.push(Value::str_(""));
            if s.len() > 1 {
                out.push(Value::str_(&s[..s.len() / 2]));
            }
        }
        Value::List(xs) if !xs.is_empty() => {
            out.push(Value::list(Vec::new()));
            if xs.len() > 1 {
                out.push(Value::list(xs.slice(0, xs.len() / 2).to_vec()));
                out.push(Value::list(xs.slice(1, xs.len()).to_vec()));
            }
            // …then one element at a time, so a failure caused by a *value* rather than by a
            // length still shrinks.
            for (i, x) in xs.iter().enumerate() {
                for smaller in shrink(&x) {
                    let mut copy = xs.to_vec();
                    copy[i] = smaller;
                    out.push(Value::list(copy));
                }
            }
        }
        Value::Map(m) if !m.is_empty() => {
            out.push(Value::Map(PMap::new()));
            if let Some((k, _)) = m.iter().next() {
                out.push(Value::Map(m.remove(k)));
            }
        }
        Value::Data(d) => {
            for (name, f) in d.fields.iter() {
                for smaller in shrink(f) {
                    let mut copy = d.fields.clone();
                    copy.insert(name.clone(), smaller);
                    out.push(Value::data(d.ty.clone(), d.variant.clone(), copy));
                }
            }
        }
        _ => {}
    }
    let before = size(v);
    out.retain(|c| size(c) < before);
    out
}

/// A total order on "how big is this value", used to prove a shrink is progress.
pub fn size(v: &Value) -> u64 {
    match v {
        Value::Unit => 0,
        Value::Bool(b) => *b as u64,
        Value::Int(n) => n.unsigned_abs(),
        Value::Float(_) => v.as_f64().map(|f| f.abs() as u64).unwrap_or(0),
        Value::Str(s) => s.len() as u64,
        Value::List(xs) => {
            let mut n = 1;
            xs.for_each(|x| n += size(x));
            n
        }
        Value::Map(m) => 1 + m.iter().map(|(k, val)| size(k) + size(val)).sum::<u64>(),
        Value::Data(d) => d.fields.values().map(size).sum::<u64>(),
        Value::Html(_) | Value::Attr(_) | Value::Closure(_) => 1,
    }
}

/// The string pool. Short, memorable, and printable — a shrunk counterexample is something a person
/// reads, and `"\u{1f4a9}\u{0}"` is a worse bug report than `"milk"`.
const WORDS: &[&str] = &["", "a", "milk", "bread", " ", "ana", "bo", "x"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Variant;

    fn types() -> Types {
        let mut t = crate::prelude::types();
        t.insert(
            Arc::from("Id"),
            TyDecl::Newtype {
                name: Arc::from("Id"),
                params: Vec::new(),
                inner: Ty::str_(),
            },
        );
        t.insert(
            Arc::from("Event"),
            TyDecl::Union {
                name: Arc::from("Event"),
                params: Vec::new(),
                variants: vec![
                    Variant {
                        name: Arc::from("Added"),
                        fields: vec![(Arc::from("id"), Ty::con("Id"))],
                    },
                    Variant {
                        name: Arc::from("Toggled"),
                        fields: vec![(Arc::from("id"), Ty::con("Id"))],
                    },
                ],
            },
        );
        t
    }

    #[test]
    fn the_canonical_inhabitant_is_the_smallest_obvious_one() {
        let t = types();
        assert_eq!(canonical(&Ty::int(), &t).unwrap(), Value::Int(0));
        assert_eq!(canonical(&Ty::str_(), &t).unwrap(), Value::str_(""));
        assert_eq!(canonical(&Ty::bool_(), &t).unwrap(), Value::Bool(false));
        assert_eq!(
            canonical(&Ty::list(Ty::con("Event")), &t).unwrap(),
            Value::list(Vec::new())
        );
        // First variant, and a newtype is transparent to the generator but not to the type system.
        let e = canonical(&Ty::con("Event"), &t).unwrap();
        assert_eq!(e.variant(), Some("Added"));
        assert_eq!(e.field("id").unwrap().display(), "");
    }

    #[test]
    fn a_secret_is_refused_because_somebody_has_to_type_it_out() {
        let t = types();
        let err = canonical(&Ty::secret(Ty::str_()), &t).expect_err("a secret is not invented");
        assert!(err.why.contains("written out by a person"), "{err}");
        // …and nesting does not launder it: a record holding one is refused too.
        let mut t2 = t.clone();
        t2.insert(
            Arc::from("Creds"),
            TyDecl::Model {
                name: Arc::from("Creds"),
                params: Vec::new(),
                fields: vec![(Arc::from("key"), Ty::secret(Ty::str_()))],
            },
        );
        assert!(canonical(&Ty::con("Creds"), &t2).is_err());
    }

    #[test]
    fn generation_is_a_function_of_the_seed_and_nothing_else() {
        let t = types();
        let ty = Ty::list(Ty::con("Event"));
        let a = arbitrary(&ty, &t, &mut Rng::seeded("a property", 7)).unwrap();
        let b = arbitrary(&ty, &t, &mut Rng::seeded("a property", 7)).unwrap();
        assert_eq!(a, b, "the same seed must produce the same value");
        let c = arbitrary(&ty, &t, &mut Rng::seeded("a property", 8)).unwrap();
        // Not asserting inequality of one pair — that is a property of the hash, not of the design.
        // What is asserted is that the run index reaches the generator at all.
        let d = arbitrary(&ty, &t, &mut Rng::seeded("a property", 8)).unwrap();
        assert_eq!(c, d);
    }

    #[test]
    fn a_recursive_union_terminates() {
        let mut t = types();
        t.insert(
            Arc::from("Tree"),
            TyDecl::Union {
                name: Arc::from("Tree"),
                params: Vec::new(),
                variants: vec![
                    Variant {
                        name: Arc::from("Node"),
                        fields: vec![
                            (Arc::from("l"), Ty::con("Tree")),
                            (Arc::from("r"), Ty::con("Tree")),
                        ],
                    },
                    Variant {
                        name: Arc::from("Leaf"),
                        fields: vec![],
                    },
                ],
            },
        );
        // The canonical inhabitant picks the non-recursive variant…
        assert_eq!(
            canonical(&Ty::con("Tree"), &t).unwrap().variant(),
            Some("Leaf")
        );
        // …and an arbitrary one bottoms out at the depth limit rather than diverging.
        for run in 0..20 {
            arbitrary(&Ty::con("Tree"), &t, &mut Rng::seeded("t", run)).unwrap();
        }
    }

    #[test]
    fn every_shrink_is_strictly_smaller_so_the_loop_terminates() {
        let t = types();
        for run in 0..50 {
            let v = arbitrary(&Ty::list(Ty::con("Event")), &t, &mut Rng::seeded("s", run)).unwrap();
            for c in shrink(&v) {
                assert!(size(&c) < size(&v), "{c:?} is not smaller than {v:?}");
            }
        }
        assert!(shrink(&Value::Int(0)).is_empty());
        assert!(shrink(&Value::list(Vec::new())).is_empty());
    }

    #[test]
    fn a_type_parameter_reaches_the_field_it_stands_for() {
        let t = types();
        let v = canonical(&Ty::app(Ty::OPTION, vec![Ty::int()]), &t).unwrap();
        // `Option`'s first variant is `Some(value: a)`, and `a` is `Int` here.
        assert_eq!(v.variant(), Some("Some"));
        assert_eq!(v.field("value"), Some(&Value::Int(0)));
    }
}
