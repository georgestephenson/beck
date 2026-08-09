//! The data patch: what a Mode B subscription carries instead of DOM patches.
//!
//! [`docs/05-tier-lowering.md`](../../../../../docs/05-tier-lowering.md) §5.1's table has one row
//! that decides the whole mode — "Wire carries: **DOM patches** | **data patches (state diffs)**".
//! [`mod@crate::diff`] is the first half — the DOM patch — and this is the second.
//!
//! # Why a diff rather than the value
//!
//! Sending the accumulator on every event would make each event cost the size of the *state*
//! rather than the size of the *change*, which is the asymptote
//! [`docs/24-incremental-views-report.md`](../../../../../docs/24-incremental-views-report.md)
//! spent a whole report removing from the view path. A card moved on a thousand-card board is one
//! [`Op::Put`], and it stays one as the board grows: cost per event is a function of the change,
//! not of the collection.
//!
//! # Paths
//!
//! A [`Path`] is how to get from the root of the accumulator to the value that changed — a field
//! of a record, an index of a list, a key of a map. Records and maps are addressed by *name* and
//! by *key*, so an op stays applicable when its neighbours move; only a list is addressed by
//! position, which is why the list rules below are the conservative ones.
//!
//! # What this is not
//!
//! It is not a merge. Ops are produced by one writer against a state the reader has, and applied
//! in order at a known `seq` — the same discipline the DOM patch stream already has (§4.4). Two
//! writers would need [`docs/10-decisions.md`](../../../../../docs/10-decisions.md) D7's
//! CRDT-valued types, which are v1.x and are not this.

use serde::{Deserialize, Serialize};

use crate::core::{Fields, Value};
use crate::repr::Repr;

/// One step from a value to a value inside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Step {
    /// A record field, by name.
    Field(String),
    /// A list element, by position.
    Index(u32),
    /// A map entry, by key. The key is a whole value, because a Beck map's keys are.
    Key(Repr),
}

/// Where in the accumulator an op applies.
pub type Path = Vec<Step>;

/// One change to the state a client holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// Replace whatever is at `path`. The fallback for a scalar, and for any two values whose
    /// shapes differ enough that describing the difference would cost more than the value.
    Set { path: Path, value: Repr },
    /// Insert into the list at `path`, before `index`.
    Insert { path: Path, index: u32, value: Repr },
    /// Remove element `index` of the list at `path`.
    Remove { path: Path, index: u32 },
    /// Put an entry in the map at `path`.
    Put { path: Path, key: Repr, value: Repr },
    /// Drop an entry from the map at `path`.
    Drop { path: Path, key: Repr },
}

/// Why a patch could not be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadPatch {
    pub why: String,
}

impl std::fmt::Display for BadPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl std::error::Error for BadPatch {}

fn bad(why: impl Into<String>) -> BadPatch {
    BadPatch { why: why.into() }
}

/// The ops that turn `old` into `new`.
///
/// Empty when they are equal, which is the common case for a subscriber whose corner of the state
/// did not change — and the reason an idle client costs nothing.
pub fn diff(old: &Value, new: &Value) -> Vec<Op> {
    let mut ops = Vec::new();
    walk(&mut Vec::new(), old, new, &mut ops);
    ops
}

fn walk(path: &mut Path, old: &Value, new: &Value, ops: &mut Vec<Op>) {
    if old == new {
        return;
    }
    match (old, new) {
        // A record's fields are fixed by its type, so two records of the same shape differ only in
        // their values — and each field is its own path.
        (Value::Data(a), Value::Data(b)) if a.ty == b.ty && a.variant == b.variant => {
            for (name, av) in a.fields.iter() {
                let Some(bv) = b.fields.get(name) else {
                    // A field one side does not have is a different shape wearing the same name.
                    set(path, new, ops);
                    return;
                };
                path.push(Step::Field(name.to_string()));
                walk(path, av, bv, ops);
                path.pop();
            }
        }
        (Value::Map(a), Value::Map(b)) => {
            // Both iterate in key order, so this is one merge rather than two lookups per key.
            let mut left = a.iter().peekable();
            let mut right = b.iter().peekable();
            loop {
                match (left.peek(), right.peek()) {
                    (None, None) => break,
                    (Some((k, _)), None) => {
                        drop_key(path, k, ops);
                        left.next();
                    }
                    (None, Some((k, v))) => {
                        put(path, k, v, ops);
                        right.next();
                    }
                    (Some((lk, lv)), Some((rk, rv))) => match lk.cmp(rk) {
                        std::cmp::Ordering::Less => {
                            drop_key(path, lk, ops);
                            left.next();
                        }
                        std::cmp::Ordering::Greater => {
                            put(path, rk, rv, ops);
                            right.next();
                        }
                        std::cmp::Ordering::Equal => {
                            if lv != rv {
                                // Descend: a card whose text changed is one `Set` at that card's
                                // field, not a whole card on the wire.
                                if let Ok(key) = Repr::of(lk) {
                                    path.push(Step::Key(key));
                                    walk(path, lv, rv, ops);
                                    path.pop();
                                } else {
                                    put(path, rk, rv, ops);
                                }
                            }
                            left.next();
                            right.next();
                        }
                    },
                }
            }
        }
        (Value::List(a), Value::List(b)) => {
            // A list is addressed by position, so the rules are the conservative ones: a shared
            // prefix and a shared suffix are left alone and the middle is rewritten. That is exact
            // for the shapes a fold produces — an append, a prepend, an element replaced — and it
            // degrades to `Set` rather than to a wrong patch for anything else.
            let common = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
            let tail = a
                .iter()
                .rev()
                .zip(b.iter().rev())
                .take_while(|(x, y)| x == y)
                .count()
                .min(a.len() - common)
                .min(b.len() - common);
            // Exactly one element differs and the rest match: describe *it* rather than the pair.
            if a.len() == b.len() && common + tail + 1 == a.len() {
                path.push(Step::Index(common as u32));
                walk(path, &a[common], &b[common], ops);
                path.pop();
                return;
            }
            // Removals from the back, so the indices ahead of each one are still the client's.
            for i in (common..a.len() - tail).rev() {
                ops.push(Op::Remove {
                    path: path.clone(),
                    index: i as u32,
                });
            }
            for (offset, v) in b[common..b.len() - tail].iter().enumerate() {
                let Ok(value) = Repr::of(v) else {
                    return set(path, new, ops);
                };
                ops.push(Op::Insert {
                    path: path.clone(),
                    index: (common + offset) as u32,
                    value,
                });
            }
        }
        _ => set(path, new, ops),
    }
}

fn set(path: &Path, new: &Value, ops: &mut Vec<Op>) {
    // A value that is not storable is not sendable either, and the checker refuses a state that
    // contains one (`B0413`). Nothing is emitted rather than something wrong: a client that
    // received no op for a change it cannot represent would be stale, and a client sent a
    // fabricated one would be wrong.
    if let Ok(value) = Repr::of(new) {
        ops.push(Op::Set {
            path: path.clone(),
            value,
        });
    }
}

fn put(path: &Path, key: &Value, value: &Value, ops: &mut Vec<Op>) {
    if let (Ok(key), Ok(value)) = (Repr::of(key), Repr::of(value)) {
        ops.push(Op::Put {
            path: path.clone(),
            key,
            value,
        });
    }
}

fn drop_key(path: &Path, key: &Value, ops: &mut Vec<Op>) {
    if let Ok(key) = Repr::of(key) {
        ops.push(Op::Drop {
            path: path.clone(),
            key,
        });
    }
}

/// Apply a patch, in order.
///
/// Fails rather than guesses: a path that does not exist means the client and the server disagree
/// about the state, and continuing from a state neither of them has is how a browser ends up
/// rendering something that never happened. The subscription's answer to a failure is the same as
/// the log's — ask for the whole value again ([`crate::render`]).
pub fn apply(state: &Value, ops: &[Op]) -> Result<Value, BadPatch> {
    let mut out = state.clone();
    for op in ops {
        out = apply_one(&out, op)?;
    }
    Ok(out)
}

fn apply_one(state: &Value, op: &Op) -> Result<Value, BadPatch> {
    let (path, edit): (&Path, Edit) = match op {
        Op::Set { path, value } => (path, Edit::Set(value.to_value())),
        Op::Insert { path, index, value } => (path, Edit::Insert(*index, value.to_value())),
        Op::Remove { path, index } => (path, Edit::Remove(*index)),
        Op::Put { path, key, value } => (path, Edit::Put(key.to_value(), value.to_value())),
        Op::Drop { path, key } => (path, Edit::Drop(key.to_value())),
    };
    edit_at(state, path, &edit)
}

enum Edit {
    Set(Value),
    Insert(u32, Value),
    Remove(u32),
    Put(Value, Value),
    Drop(Value),
}

fn edit_at(state: &Value, path: &[Step], edit: &Edit) -> Result<Value, BadPatch> {
    let Some((step, rest)) = path.split_first() else {
        return here(state, edit);
    };
    match (step, state) {
        (Step::Field(name), Value::Data(d)) => {
            let old = d
                .fields
                .get(name.as_str())
                .ok_or_else(|| bad(format!("no field `{name}` here")))?;
            let next = edit_at(old, rest, edit)?;
            let mut fields = Fields::new();
            for (k, v) in d.fields.iter() {
                fields.insert(
                    k.clone(),
                    if k.as_ref() == name.as_str() {
                        next.clone()
                    } else {
                        v.clone()
                    },
                );
            }
            Ok(Value::data(d.ty.clone(), d.variant.clone(), fields))
        }
        (Step::Index(i), Value::List(xs)) => {
            let i = *i as usize;
            let old = xs
                .get(i)
                .ok_or_else(|| bad(format!("this list has no element {i}")))?;
            let next = edit_at(old, rest, edit)?;
            let mut items = xs.as_ref().clone();
            items[i] = next;
            Ok(Value::List(std::sync::Arc::new(items)))
        }
        (Step::Key(k), Value::Map(m)) => {
            let key = k.to_value();
            let old = m.get(&key).ok_or_else(|| bad("no such key here"))?;
            let next = edit_at(old, rest, edit)?;
            Ok(Value::Map(m.insert(key, next)))
        }
        (step, other) => Err(bad(format!(
            "cannot follow {} into a {}",
            match step {
                Step::Field(n) => format!("`.{n}`"),
                Step::Index(i) => format!("`[{i}]`"),
                Step::Key(_) => "a key".to_string(),
            },
            other.display()
        ))),
    }
}

fn here(state: &Value, edit: &Edit) -> Result<Value, BadPatch> {
    match edit {
        Edit::Set(v) => Ok(v.clone()),
        Edit::Insert(i, v) => {
            let Value::List(xs) = state else {
                return Err(bad("insert applies to a list"));
            };
            let i = *i as usize;
            if i > xs.len() {
                return Err(bad(format!(
                    "cannot insert at {i} in a list of {}",
                    xs.len()
                )));
            }
            let mut items = xs.as_ref().clone();
            items.insert(i, v.clone());
            Ok(Value::List(std::sync::Arc::new(items)))
        }
        Edit::Remove(i) => {
            let Value::List(xs) = state else {
                return Err(bad("remove applies to a list"));
            };
            let i = *i as usize;
            if i >= xs.len() {
                return Err(bad(format!("this list has no element {i}")));
            }
            let mut items = xs.as_ref().clone();
            items.remove(i);
            Ok(Value::List(std::sync::Arc::new(items)))
        }
        Edit::Put(k, v) => {
            let Value::Map(m) = state else {
                return Err(bad("put applies to a map"));
            };
            Ok(Value::Map(m.insert(k.clone(), v.clone())))
        }
        Edit::Drop(k) => {
            let Value::Map(m) = state else {
                return Err(bad("drop applies to a map"));
            };
            Ok(Value::Map(m.remove(k)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmap::PMap;
    use std::sync::Arc;

    fn card(id: &str, text: &str, column: i64) -> Value {
        Value::data(
            Arc::from("Card"),
            None,
            Fields::from_iter([
                (Arc::from("id"), Value::str_(id)),
                (Arc::from("text"), Value::str_(text)),
                (Arc::from("column"), Value::Int(column)),
            ]),
        )
    }

    fn board(cards: &[(&str, &str, i64)]) -> Value {
        let mut m = PMap::new();
        for (id, text, column) in cards {
            m = m.insert(Value::str_(id), card(id, text, *column));
        }
        Value::data(
            Arc::from("Board"),
            None,
            Fields::from_iter([(Arc::from("cards"), Value::Map(m))]),
        )
    }

    /// The round trip is the whole contract: whatever the ops are, applying them has to produce
    /// the value they were derived from.
    fn round_trip(old: &Value, new: &Value) -> Vec<Op> {
        let ops = diff(old, new);
        assert_eq!(&apply(old, &ops).expect("applies"), new, "ops: {ops:?}");
        ops
    }

    #[test]
    fn an_unchanged_state_is_no_ops() {
        assert!(round_trip(&board(&[("1", "a", 0)]), &board(&[("1", "a", 0)])).is_empty());
    }

    #[test]
    fn a_card_moved_on_a_large_board_is_one_op() {
        let many: Vec<(String, String, i64)> = (0..500)
            .map(|i| (format!("{i:03}"), format!("card {i}"), 0))
            .collect();
        let before: Vec<(&str, &str, i64)> = many
            .iter()
            .map(|(a, b, c)| (a.as_str(), b.as_str(), *c))
            .collect();
        let mut after = before.clone();
        after[250].2 = 1;

        let ops = round_trip(&board(&before), &board(&after));
        assert_eq!(ops.len(), 1, "{ops:?}");
        // And it names the path rather than carrying the board: field, key, field.
        match &ops[0] {
            Op::Set { path, value } => {
                assert_eq!(path.len(), 3, "{path:?}");
                assert_eq!(*value, Repr::Int(1));
            }
            other => panic!("expected a set, got {other:?}"),
        }
    }

    #[test]
    fn an_added_card_is_one_put_and_a_dropped_card_is_one_drop() {
        let ops = round_trip(
            &board(&[("1", "a", 0)]),
            &board(&[("1", "a", 0), ("2", "b", 0)]),
        );
        assert!(matches!(ops.as_slice(), [Op::Put { .. }]), "{ops:?}");

        let ops = round_trip(
            &board(&[("1", "a", 0), ("2", "b", 0)]),
            &board(&[("1", "a", 0)]),
        );
        assert!(matches!(ops.as_slice(), [Op::Drop { .. }]), "{ops:?}");
    }

    #[test]
    fn a_list_appends_removes_and_replaces() {
        let list = |xs: &[i64]| Value::List(Arc::new(xs.iter().copied().map(Value::Int).collect()));

        let ops = round_trip(&list(&[1, 2, 3]), &list(&[1, 2, 3, 4]));
        assert!(
            matches!(ops.as_slice(), [Op::Insert { index: 3, .. }]),
            "{ops:?}"
        );

        let ops = round_trip(&list(&[1, 2, 3]), &list(&[1, 3]));
        assert!(
            matches!(ops.as_slice(), [Op::Remove { index: 1, .. }]),
            "{ops:?}"
        );

        let ops = round_trip(&list(&[1, 2, 3]), &list(&[1, 9, 3]));
        assert!(matches!(ops.as_slice(), [Op::Set { .. }]), "{ops:?}");

        round_trip(&list(&[1, 2, 3]), &list(&[]));
        round_trip(&list(&[]), &list(&[7, 8]));
        round_trip(&list(&[1, 2, 3]), &list(&[3, 2, 1]));
    }

    #[test]
    fn a_patch_against_the_wrong_state_fails_rather_than_guesses() {
        let ops = diff(&board(&[("1", "a", 0)]), &board(&[("1", "b", 0)]));
        // A client that never saw card 1 cannot apply an op about card 1's text.
        assert!(apply(&board(&[]), &ops).is_err());
    }

    #[test]
    fn a_variant_change_replaces_rather_than_descends() {
        let some = Value::data(
            Arc::from("Option"),
            Some(Arc::from("Some")),
            Fields::from_iter([(Arc::from("value"), Value::Int(1))]),
        );
        let none = Value::data(Arc::from("Option"), Some(Arc::from("None")), Fields::new());
        let ops = round_trip(&some, &none);
        assert!(matches!(ops.as_slice(), [Op::Set { .. }]), "{ops:?}");
    }
}
