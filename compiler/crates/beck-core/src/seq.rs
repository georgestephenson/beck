//! A list's elements, in one of two layouts.
//!
//! [`docs/105-the-ecosystem-answer.md`](../../../../../docs/105-the-ecosystem-answer.md) §105.8:
//!
//! > `Value` is 16 bytes and a list is `List(Arc<Vec<Value>>)`, so a million doubles is a boxed
//! > 16 MB; and `Float(u64)` is stored as an **order-preserving key** rather than as `f64` bits,
//! > which is exactly right for the reason its doc comment gives — a map key and the state digest
//! > need a total order agreeing with arithmetic — and exactly wrong for a dense kernel, which pays
//! > a bit transform per operation. That is not a defect to fix in `Value`. It is a **second
//! > representation to add**.
//!
//! This is that representation, and the sentence that matters most about it is the one it is
//! **not**: it is not a second kind of list. A Beck program has one list type, one order, one
//! equality, one digest and one wire format, and this module's whole obligation is that a caller
//! cannot tell which layout it got.
//!
//! # The two layouts
//!
//! | Layout | Bytes an element | What it is for |
//! |---|---|---|
//! | [`Seq::Boxed`] | 16 | Anything at all — records, strings, nested lists. What every list has always been |
//! | [`Seq::Ints`] | 8 | A list of `Int`, dense |
//! | [`Seq::Floats`] | 8 | A list of `Float`, dense **and as `f64`** rather than as [`Value::Float`]'s order key |
//!
//! The `Floats` row is the one with a consequence beyond memory. A kernel — BLAS, an FFT, anything
//! this project has no business reimplementing ([`105`](../../../../../docs/105-the-ecosystem-answer.md)
//! §105.8) — takes a `*const f64`, and so does Apache Arrow: a `Float64Array`'s values buffer *is*
//! a contiguous `f64` run. [`Seq::floats`] is that pointer, and until it existed there was nothing
//! in this language to hand either of them.
//!
//! # What a caller may not be able to tell, stated as the four things
//!
//! Two `Seq`s holding the same elements are **the same list**, and four separate mechanisms have to
//! agree about that or replay determinism ([`04`](../../../../../docs/04-compiler-architecture.md)
//! §4.8) fails in a way that depends on how a value happened to be built:
//!
//! 1. **Equality and order.** `Ord` and `Eq` are written by hand here, over the logical sequence.
//!    A derived one would compare the *variant tag* first, making `Ints([1])` and `Boxed([Int(1)])`
//!    two different values and sorting every column before every list.
//! 2. **The digest** ([`crate::core::digest`]) hashes a tag, the length and each element, and
//!    reaches the elements through [`Seq::iter`] — so it is the same bytes either way by
//!    construction rather than by a second implementation agreeing.
//! 3. **The wire format** ([`crate::repr`]) does the same.
//! 4. **`Value`'s size.** The layout is an enum *behind* the `Arc`, so a `Value` is still 16 bytes
//!    and a list still costs one pointer. Putting the enum in the `Value` would have widened every
//!    value in the language to pay for a representation most of them do not use, which is the
//!    trade [`crate::core::Value`]'s own doc comment refused for [`crate::core::Record`].
//!
//! `seq.rs`'s own tests assert the first, and `beck-cli/tests/records.rs` asserts all four against
//! the layouts a program can actually produce.
//!
//! # Where a column comes from
//!
//! Nothing in the language says "make this a column", and nothing should: the layout is a fact
//! about the elements, so it is chosen where a list is *built*. [`Seq::pack`] takes the elements a
//! primitive produced and reads them; [`Seq::push`] promotes an **empty** list on its first element,
//! which is what makes the accumulator idiom `go(i + 1, list_append(done, x))` — how `lib/`, the
//! corpus and both SICP chapters build a list ([`70`](../../../../../docs/70-the-evaluator-gets-fast-report.md)
//! §70.6) — produce a column with no program changing a line.
//!
//! Promotion is only ever `O(1)`: an empty list on its first push, or a `pack` over elements the
//! caller had already built. A `Boxed` list of a million ints is **not** re-examined on every push,
//! because that check is what would turn the idiom quadratic.
//!
//! # The off switch
//!
//! Choosing a layout is a choice the runtime makes unbidden, so [`docs/08`](../../../../../docs/08-roadmap.md)
//! §8.3 item 8 applies: [`set_columns`] turns it off for the process, and the gate runs both
//! settings. With it off every list is [`Seq::Boxed`] and every answer is the same one — which is
//! what the switch is *for*, and what makes "a caller cannot tell" a test rather than a claim.
//!
//! Two callers reach it without recompiling: `beck_rt::AppConfig::columns` for a served
//! application, and `BECK_COLUMNS=0` for a `beck` process that is not one — `run`, `test`, `bench`
//! and `build` all build lists, and none of them has an `AppConfig`.
//!
//! It is **process-wide**, which is the one thing about it worth knowing before using it: a list is
//! built in a hundred places that have no configuration in scope, and a `Value` may not carry one
//! — it is 16 bytes on purpose. So two applications in one process share the setting, and a test
//! binary that flips it has to serialise the tests that do.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as Atomic};

use crate::core::Value;

/// Whether a list may be stored as a column.
///
/// A process-wide switch rather than a parameter because a list is built in a hundred places that
/// have no configuration in scope, and a `Value` may not carry one — it is 16 bytes on purpose.
/// Read once per list *built*, never per element.
static COLUMNS: AtomicBool = AtomicBool::new(true);

/// Turn the columnar layout on or off for this process, and answer what it was.
///
/// [`docs/08`](../../../../../docs/08-roadmap.md) §8.3 item 8's off switch. Nothing observable
/// changes: with it off every list is [`Seq::Boxed`], every answer is the same answer, and the
/// difference is memory. That is exactly why it is worth having — the switched-off path is what a
/// gate compares against, so "the two layouts are one list" is measured rather than asserted.
pub fn set_columns(on: bool) -> bool {
    COLUMNS.swap(on, Atomic::Relaxed)
}

/// Whether the columnar layout is on.
pub fn columns() -> bool {
    COLUMNS.load(Atomic::Relaxed)
}

/// How many columns this process has built.
///
/// [`docs/08`](../../../../../docs/08-roadmap.md) §8.3 item 9's half of the same obligation as the
/// switch above: a choice made unbidden should be answerable after the fact. This is the smallest
/// honest answer — not *which* list, but whether the layout is reaching anything at all — and it is
/// what lets a sweep say "the corpus builds none" rather than leaving that to be assumed.
///
/// Counted where a column is *created*: a `pack` that found one, or a `push` that promoted an empty
/// list. Not per element, and never on the boxed path.
pub fn built() -> u64 {
    BUILT.load(Atomic::Relaxed)
}

static BUILT: AtomicU64 = AtomicU64::new(0);

fn note() {
    BUILT.fetch_add(1, Atomic::Relaxed);
}

/// A list's elements. See the module docs for what a caller may not be able to tell.
#[derive(Clone, Debug)]
pub enum Seq {
    /// Any elements at all.
    Boxed(Vec<Value>),
    /// `Int` elements, dense.
    Ints(Vec<i64>),
    /// `Float` elements, dense and as `f64`.
    Floats(Vec<f64>),
}

impl Default for Seq {
    fn default() -> Seq {
        Seq::Boxed(Vec::new())
    }
}

impl Seq {
    /// The elements a primitive produced, in whatever layout they fit.
    ///
    /// One pass over a list that has just been built by a pass over something else, so this is a
    /// constant on a cost the caller was already paying — and it stops at the first element that
    /// does not fit, so a list of records pays a single comparison.
    pub fn pack(values: Vec<Value>) -> Seq {
        if !columns() || values.is_empty() {
            return Seq::Boxed(values);
        }
        // The first element decides which column to try, so a list of records — which is most of
        // them — pays one `matches!` and nothing else. There is deliberately no length threshold:
        // a threshold is a constant somebody would have to justify, and what it would buy is eight
        // bytes on a list of one.
        match &values[0] {
            Value::Int(_) => {
                let mut out = Vec::with_capacity(values.len());
                for v in &values {
                    match v {
                        Value::Int(i) => out.push(*i),
                        _ => return Seq::Boxed(values),
                    }
                }
                note();
                Seq::Ints(out)
            }
            Value::Float(_) => {
                let mut out = Vec::with_capacity(values.len());
                for v in &values {
                    match v.as_f64() {
                        Some(f) => out.push(f),
                        None => return Seq::Boxed(values),
                    }
                }
                note();
                Seq::Floats(out)
            }
            _ => Seq::Boxed(values),
        }
    }

    /// The elements, unpacked — what a caller that wants a `Vec<Value>` gets.
    pub fn to_vec(&self) -> Vec<Value> {
        match self {
            Seq::Boxed(v) => v.clone(),
            _ => self.iter().collect(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Seq::Boxed(v) => v.len(),
            Seq::Ints(v) => v.len(),
            Seq::Floats(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One element, as a [`Value`]. By value rather than by reference, because a column has no
    /// `Value` to lend — and an `Int` or a `Float` is two words, so there is nothing to save.
    pub fn get(&self, i: usize) -> Option<Value> {
        match self {
            Seq::Boxed(v) => v.get(i).cloned(),
            Seq::Ints(v) => v.get(i).map(|&x| Value::Int(x)),
            Seq::Floats(v) => v.get(i).map(|&x| Value::float(x)),
        }
    }

    /// Every element, in order.
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            seq: self,
            at: 0,
            end: self.len(),
        }
    }

    /// The elements as a dense `i64` run, if that is what this list is.
    pub fn ints(&self) -> Option<&[i64]> {
        match self {
            Seq::Ints(v) => Some(v),
            _ => None,
        }
    }

    /// **The elements as a dense `f64` run**, if that is what this list is — the pointer a kernel
    /// and an Arrow `Float64Array` both take, and the reason this module exists
    /// ([`105`](../../../../../docs/105-the-ecosystem-answer.md) §105.8).
    ///
    /// `None` for a boxed list rather than a copy of one: a caller that gets a slice knows it cost
    /// nothing, and a caller that gets `None` can decide whether a copy is worth it. Handing back a
    /// materialised buffer here would make the cheap case and the expensive one look alike, which
    /// is the shape of every "why is this slow" question a zero-copy interface exists to prevent.
    pub fn floats(&self) -> Option<&[f64]> {
        match self {
            Seq::Floats(v) => Some(v),
            _ => None,
        }
    }

    /// Every element, **borrowed** where the layout has one to lend.
    ///
    /// [`Seq::iter`] yields by value, which is right for a caller that wanted an owned `Value` and
    /// wrong for one that only wanted to look: cloning a `Value::Data` is an atomic increment and a
    /// later decrement, and the digest, the wire format and `to_json` each walk every element of
    /// every list without keeping any of them. Those walk through here, so the boxed layout — which
    /// is every list that is not a column — pays exactly what it paid before this module existed.
    ///
    /// A column has no `Value` to lend, so one is built on the stack and lent; it is two words and
    /// no allocation.
    pub fn for_each(&self, mut f: impl FnMut(&Value)) {
        match self {
            Seq::Boxed(v) => v.iter().for_each(f),
            _ => {
                for i in 0..self.len() {
                    if let Some(x) = self.get(i) {
                        f(&x);
                    }
                }
            }
        }
    }

    /// [`Seq::for_each`] for a walk that can fail, which is what a wire encoder is.
    pub fn try_for_each<E>(&self, mut f: impl FnMut(&Value) -> Result<(), E>) -> Result<(), E> {
        match self {
            Seq::Boxed(v) => v.iter().try_for_each(f),
            _ => {
                for i in 0..self.len() {
                    if let Some(x) = self.get(i) {
                        f(&x)?;
                    }
                }
                Ok(())
            }
        }
    }

    /// The elements as a slice, borrowed when the layout allows and materialised when it does not.
    pub fn as_values(&self) -> std::borrow::Cow<'_, [Value]> {
        match self {
            Seq::Boxed(v) => std::borrow::Cow::Borrowed(v),
            _ => std::borrow::Cow::Owned(self.to_vec()),
        }
    }

    /// The smallest element, and the largest — over the dense buffer where there is one.
    ///
    /// This is the half of [`105`](../../../../../docs/105-the-ecosystem-answer.md) §105.10's
    /// aggregate row that costs nothing to take: `min` over an `Ints` column is a pass over `i64`s
    /// with no `Value` built at all, where the boxed form builds one per element to compare it.
    pub fn min(&self) -> Option<Value> {
        match self {
            Seq::Ints(v) => v.iter().min().map(|&x| Value::Int(x)),
            _ => self.iter().min(),
        }
    }

    pub fn max(&self) -> Option<Value> {
        match self {
            Seq::Ints(v) => v.iter().max().map(|&x| Value::Int(x)),
            _ => self.iter().max(),
        }
    }

    /// Whether this list is stored as a column — for a gate and for a report, never for a decision
    /// about what a program means.
    pub fn is_column(&self) -> bool {
        !matches!(self, Seq::Boxed(_))
    }

    /// What this list occupies on the heap, in bytes, not counting anything its elements point at.
    ///
    /// The number the second layout exists to move, so it is readable rather than inferred.
    pub fn heap_bytes(&self) -> usize {
        match self {
            Seq::Boxed(v) => v.capacity() * std::mem::size_of::<Value>(),
            Seq::Ints(v) => v.capacity() * std::mem::size_of::<i64>(),
            Seq::Floats(v) => v.capacity() * std::mem::size_of::<f64>(),
        }
    }

    /// Add one element to the end.
    ///
    /// An **empty** list promotes to the layout its first element fits, which is what gives the
    /// accumulator idiom a column; a list that already has elements keeps its layout, or falls back
    /// to [`Seq::Boxed`] once for an element that does not fit. Both are `O(1)` amortised. What is
    /// deliberately *not* here is a re-examination of a boxed list on every push: that would be the
    /// quadratic [`70`](../../../../../docs/70-the-evaluator-gets-fast-report.md) removed from this
    /// idiom, put straight back.
    pub fn push(&mut self, value: Value) {
        match (&mut *self, &value) {
            (Seq::Ints(v), Value::Int(x)) => v.push(*x),
            (Seq::Floats(v), Value::Float(_)) => v.push(value.as_f64().unwrap_or(0.0)),
            (Seq::Boxed(v), Value::Int(x)) if v.is_empty() && columns() => {
                note();
                *self = Seq::Ints(vec![*x]);
            }
            (Seq::Boxed(v), Value::Float(_)) if v.is_empty() && columns() => {
                note();
                *self = Seq::Floats(vec![value.as_f64().unwrap_or(0.0)]);
            }
            (Seq::Boxed(v), _) => v.push(value),
            // A column that has met an element it cannot hold. Once per list, and never again.
            _ => {
                let mut v = self.to_vec();
                v.push(value);
                *self = Seq::Boxed(v);
            }
        }
    }

    /// Add every element of another list to the end.
    pub fn extend(&mut self, other: &Seq) {
        match (&mut *self, other) {
            (Seq::Ints(a), Seq::Ints(b)) => a.extend_from_slice(b),
            (Seq::Floats(a), Seq::Floats(b)) => a.extend_from_slice(b),
            (Seq::Boxed(a), Seq::Boxed(b)) if !a.is_empty() || !columns() => a.extend_from_slice(b),
            _ => {
                for v in other.iter() {
                    self.push(v);
                }
            }
        }
    }

    /// A range of the elements, as a list of its own.
    pub fn slice(&self, from: usize, to: usize) -> Seq {
        let (from, to) = (from.min(self.len()), to.min(self.len()));
        if from >= to {
            return Seq::default();
        }
        match self {
            Seq::Boxed(v) => Seq::Boxed(v[from..to].to_vec()),
            Seq::Ints(v) => Seq::Ints(v[from..to].to_vec()),
            Seq::Floats(v) => Seq::Floats(v[from..to].to_vec()),
        }
    }

    /// Replace one element, keeping the layout where the new element fits it.
    pub fn set(&mut self, i: usize, value: Value) {
        match (&mut *self, &value) {
            (Seq::Boxed(v), _) if i < v.len() => v[i] = value,
            (Seq::Ints(v), Value::Int(x)) if i < v.len() => v[i] = *x,
            (Seq::Floats(v), Value::Float(_)) if i < v.len() => {
                v[i] = value.as_f64().unwrap_or(0.0)
            }
            _ => {
                let mut v = self.to_vec();
                if i < v.len() {
                    v[i] = value;
                }
                *self = Seq::Boxed(v);
            }
        }
    }
}

impl From<Vec<Value>> for Seq {
    fn from(v: Vec<Value>) -> Seq {
        Seq::pack(v)
    }
}

impl FromIterator<Value> for Seq {
    fn from_iter<I: IntoIterator<Item = Value>>(iter: I) -> Seq {
        Seq::pack(iter.into_iter().collect())
    }
}

/// Every element of a [`Seq`], in order, as [`Value`]s.
pub struct Iter<'a> {
    seq: &'a Seq,
    at: usize,
    end: usize,
}

impl Iterator for Iter<'_> {
    type Item = Value;

    fn next(&mut self) -> Option<Value> {
        if self.at >= self.end {
            return None;
        }
        let out = self.seq.get(self.at);
        self.at += 1;
        out
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.end.saturating_sub(self.at);
        (left, Some(left))
    }
}

impl ExactSizeIterator for Iter<'_> {}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Value> {
        if self.at >= self.end {
            return None;
        }
        self.end -= 1;
        self.seq.get(self.end)
    }
}

impl<'a> IntoIterator for &'a Seq {
    type Item = Value;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

// ---------------------------------------------------------------------------------------------
// The four things a caller may not be able to tell
// ---------------------------------------------------------------------------------------------
//
// Written by hand, and that is the point rather than an inconvenience. A derived `Ord` compares the
// enum's discriminant first, so `Ints([1])` would sort before `Boxed([Int(1)])` — two values a
// program cannot tell apart, ordered differently, in the order that reaches the rendered page and
// the replay digest.

impl PartialEq for Seq {
    fn eq(&self, other: &Seq) -> bool {
        match (self, other) {
            (Seq::Ints(a), Seq::Ints(b)) => a == b,
            (Seq::Boxed(a), Seq::Boxed(b)) => a == b,
            // Two `Floats` columns included: `f64`'s own `==` says `NaN != NaN` and `-0.0 == 0.0`,
            // and `Value::float` says the opposite of both on purpose.
            _ => self.len() == other.len() && self.cmp(other).is_eq(),
        }
    }
}

impl Eq for Seq {}

impl PartialOrd for Seq {
    fn partial_cmp(&self, other: &Seq) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Seq {
    /// Lexicographic over the elements, which is what `Vec<Value>`'s derived order was.
    ///
    /// A `Floats` column is compared **through [`Value::float`]**, not as raw `f64`. The two agree
    /// on every ordinary number because the order key is monotone, and disagree on exactly the two
    /// IEEE values [`Value::float`] exists to canonicalise — `NaN`, which has no `partial_cmp`, and
    /// `-0.0`, which compares equal to `0.0` and hashes differently. Going through the constructor
    /// is how those two stay the one value the language says they are.
    fn cmp(&self, other: &Seq) -> Ordering {
        if let (Seq::Ints(a), Seq::Ints(b)) = (self, other) {
            return a.cmp(b);
        }
        if let (Seq::Boxed(a), Seq::Boxed(b)) = (self, other) {
            return a.cmp(b);
        }
        let mut left = self.iter();
        let mut right = other.iter();
        loop {
            match (left.next(), right.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(a), Some(b)) => match a.cmp(&b) {
                    Ordering::Equal => continue,
                    other => return other,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`set_columns`] is process-wide, and `cargo test` runs a binary's tests on several threads.
    /// Every test that flips it takes this first, so one test's `off` is not another's answer.
    static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn boxed(v: Vec<Value>) -> Seq {
        Seq::Boxed(v)
    }

    /// The fourth thing a caller may not be able to tell: what a `Value` costs.
    ///
    /// The layout enum lives behind the `Arc`, so it is a word inside an allocation a list already
    /// had. In the `Value` it would have widened **every** value in the language — an `Int`, a
    /// `Bool`, a `Unit` — to pay for a representation none of them uses.
    #[test]
    fn a_value_is_still_two_words() {
        assert_eq!(std::mem::size_of::<Value>(), 16);
    }

    #[test]
    fn a_column_and_a_boxed_list_of_the_same_elements_are_one_value() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let ints = Seq::pack(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let same = boxed(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert!(
            ints.is_column(),
            "the elements fit a column and it did not take one"
        );
        assert!(!same.is_column());
        assert_eq!(ints, same);
        assert_eq!(ints.cmp(&same), Ordering::Equal);
        assert_eq!(ints.to_vec(), same.to_vec());
        assert_eq!(ints.iter().collect::<Vec<_>>(), same.to_vec());
    }

    /// The one that a derived `Ord` gets wrong, and it gets it wrong in the direction that reaches
    /// a rendered page: every column would sort before every list.
    #[test]
    fn the_order_is_over_the_elements_and_not_over_the_layout() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let column = Seq::pack(vec![Value::Int(5), Value::Int(6)]);
        let smaller = boxed(vec![Value::Int(1), Value::Int(2)]);
        assert!(
            column > smaller,
            "a column sorted below a smaller boxed list"
        );
        let longer = Seq::pack(vec![Value::Int(5), Value::Int(6), Value::Int(7)]);
        assert!(longer > column, "a prefix did not sort below its extension");
    }

    /// `-0.0` and `NaN` are the two IEEE values `Value::float` canonicalises, and a raw `f64`
    /// column holds them as themselves — so the comparison has to go back through the constructor.
    #[test]
    fn a_float_column_canonicalises_the_two_values_ieee_will_not() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let column = Seq::pack(vec![Value::float(-0.0), Value::float(f64::NAN)]);
        let same = boxed(vec![Value::float(0.0), Value::float(f64::NAN)]);
        assert!(column.is_column());
        assert_eq!(column, same);
        // And the raw buffer really does hold the un-canonicalised bits, so the test above is not
        // passing because the constructor already flattened them.
        let raw = Seq::Floats(vec![-0.0, f64::NAN]);
        assert_eq!(raw, same);
        assert_eq!(raw.get(0), Some(Value::float(0.0)));
    }

    #[test]
    fn an_accumulator_starting_from_nothing_becomes_a_column() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = Seq::default();
        for i in 0..8 {
            s.push(Value::Int(i));
        }
        assert!(
            s.is_column(),
            "the accumulator idiom did not produce a column"
        );
        assert_eq!(s.len(), 8);
        assert_eq!(s.floats(), None);
        assert_eq!(s.ints().map(<[i64]>::len), Some(8));

        // And one element that does not fit falls back once, keeping every element it had.
        s.push(Value::str_("nine"));
        assert!(!s.is_column());
        assert_eq!(s.len(), 9);
        assert_eq!(s.get(0), Some(Value::Int(0)));
        assert_eq!(s.get(8), Some(Value::str_("nine")));
    }

    #[test]
    fn the_switch_turns_it_off_and_changes_no_answer() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let was = set_columns(false);
        let off = Seq::pack(vec![Value::Int(1), Value::Int(2)]);
        set_columns(true);
        let on = Seq::pack(vec![Value::Int(1), Value::Int(2)]);
        set_columns(was);
        assert!(!off.is_column(), "the switch did not turn the layout off");
        assert!(on.is_column());
        assert_eq!(off, on);
        assert_eq!(off.to_vec(), on.to_vec());
        // The difference is the one thing it is allowed to be.
        assert!(on.heap_bytes() < off.heap_bytes());
    }

    #[test]
    fn a_mixed_list_stays_boxed_and_an_empty_one_has_no_layout_to_choose() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!Seq::pack(vec![Value::Int(1), Value::str_("x")]).is_column());
        assert!(!Seq::pack(vec![Value::str_("x"), Value::Int(1)]).is_column());
        assert!(!Seq::pack(Vec::new()).is_column());
        // No length threshold: one element is a column too, because the alternative is a constant
        // whose whole benefit is eight bytes.
        assert!(Seq::pack(vec![Value::Int(1)]).is_column());
    }

    /// `crate::delta` walks a list from both ends to find the shared prefix and suffix of two
    /// versions, so the reversed iterator is on the path that decides what a patch says.
    #[test]
    fn walking_a_column_backwards_gives_the_elements_in_reverse() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let s = Seq::pack((0..5).map(Value::Int).collect());
        assert!(s.is_column());
        let back: Vec<Value> = s.iter().rev().collect();
        assert_eq!(back, (0..5).rev().map(Value::Int).collect::<Vec<_>>());
        // And the two ends meet without overlapping, which is the property a shared-prefix and
        // shared-suffix walk over one iterator would otherwise get wrong.
        let mut it = s.iter();
        assert_eq!(it.next(), Some(Value::Int(0)));
        assert_eq!(it.next_back(), Some(Value::Int(4)));
        assert_eq!(it.len(), 3);
        assert_eq!(
            it.collect::<Vec<_>>(),
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        );
    }

    #[test]
    fn a_slice_of_a_column_is_a_column_and_holds_what_it_should() {
        let _held = SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let s = Seq::pack((0..10).map(Value::Int).collect());
        let mid = s.slice(2, 5);
        assert!(mid.is_column());
        assert_eq!(
            mid.to_vec(),
            vec![Value::Int(2), Value::Int(3), Value::Int(4)]
        );
        assert!(s.slice(5, 5).is_empty());
        assert_eq!(s.slice(8, 100).len(), 2);
    }
}
