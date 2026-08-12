//! The heap both native backends compile against: what a value looks like in memory, and how one
//! crosses the pipe.
//!
//! # Why this is shared and the emitters are not
//!
//! [`crate::emit`] and `beck_clif::emit` are two implementations on purpose — `cranelift.rs`
//! asserts they accept and refuse the same definitions, and an agreement by construction would be
//! worth nothing. A *layout* is the opposite: it is a contract between three parties — the two
//! emitters and the host that marshals a [`Value`] into it — and three spellings of one contract
//! is the drift this workspace spends its gates on. So the shape of an object is decided once,
//! here, exactly as [`crate::Trap`]'s codes are.
//!
//! # The shape
//!
//! Every object is a whole number of 8-byte words:
//!
//! | Word | What |
//! |---|---|
//! | 0 | the **tag**: which variant this is, and `0` for a record or a newtype |
//! | 1.. | one word per field, in the order [`beck_core::core::Fields`] keeps them — by name |
//!
//! A field word holds an `i64`, an IEEE `double`, a `Bool` as `0` or `1`, or the **offset** of
//! another object. An offset and not a pointer, and that is the decision the rest follows from:
//! the host can build an argument graph as a flat byte string and the worker can adopt it with a
//! `memcpy`, because nothing in it points anywhere. Marshalling therefore needs **no generated
//! code** — the host walks the bytes with a layout, in Rust, on both sides of the call.
//! [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md) records
//! that decision and what it costs.
//!
//! Offset `0` is reserved so that no live object has it, which leaves it available as "nothing" —
//! the value an allocation that trapped returns.
//!
//! # Tags are ranked by name, not by declaration
//!
//! [`Value`]'s derived `Ord` compares a record's type name, then its variant **name**, then its
//! fields. So `Circle < Square` because `"Circle" < "Square"`, whatever order the `union` wrote
//! them in — and a compiled comparison that ranked variants by declaration would disagree with the
//! evaluator about `<` on half the unions in the world. [`Layout::variants`] is therefore sorted by
//! name, and the tag *is* that rank: comparing two tags is comparing two variant names.
//!
//! # Text
//!
//! A `Str` is an object too, and the only one whose shape is not a program's: two header words and
//! the UTF-8 bytes, padded so the next object still starts on a word.
//!
//! | Word | What |
//! |---|---|
//! | 0 | how many **bytes** the text is |
//! | 1 | how many **characters** it is |
//! | 2.. | the bytes, then zero padding to a whole word |
//!
//! Both counts are stored because [`beck_core::core::Text`] caches both and `str_len` answers the
//! second in constant time ([`docs/70`](../../../../../docs/70-the-evaluator-gets-fast-report.md)) —
//! a compiled `str_len` that counted would be `O(n)` where the evaluator is `O(1)`, and the loop
//! that walks a string by index would be quadratic here and linear there. The two counts are also
//! what makes the ASCII test free: a UTF-8 string has as many bytes as characters exactly when
//! every character is one byte, so `bytes == chars` *is* `is_ascii`, and a character index is then
//! a byte index.
//!
//! ## String literals are the host's, at a fixed offset
//!
//! A literal cannot be allocated where it is written — the arena is reset before every call, so the
//! first iteration of a loop would allocate it and the second would allocate it again. It is not a
//! global either: an offset is an offset *into the arena*, and a constant somewhere else could not
//! be one. So the literals of a module are a **pool** that the host writes as the first bytes of
//! every request's heap, at offsets fixed when the module was emitted — compiled code refers to one
//! by a constant, and neither emitter generates a byte to build it.
//!
//! What that costs is the pool copied down the pipe on every call, which is [`Heap::pool_bytes`]
//! and is a property of the program's literals rather than of its arguments.
//!
//! # What has a layout, and what does not
//!
//! `Int`, `Float` and `Bool` are [`Repr`]'s scalars and live in registers as they always have. A
//! `model`, a `union` and a `newtype` — including a recursive one, and including one that takes a
//! type parameter — have a layout, and a `Str` has the one above. Everything else does not, and
//! [`Heap::repr`] says which by name: a `list`, a `Map`, a closure, `Html` and `Unit` are refused,
//! and a definition that mentions one is left to the evaluator exactly as it was before there was a
//! heap at all.

use std::collections::BTreeMap;
use std::sync::Arc;

use beck_core::check::Program;
use beck_core::core::{Fields, Record};
use beck_core::ty::{instantiate_decl, Ty, TyDecl};
use beck_core::Value;

use crate::emit::Scalar;

/// Every field, every tag and every offset is this wide.
pub const WORD: u64 = 8;

/// The first offset an object may have.
///
/// One word, so that `0` is never a live object and can mean "nothing": an allocation that trapped
/// returns it, and a differential that compared it against a real value would be comparing against
/// a record at the arena's very start.
pub const FIRST: u64 = WORD;

/// How much heap a compiled program may use, in bytes.
///
/// Reserved with one `malloc` when the module has a layout in it at all, and never grown: an arena
/// that moved would invalidate nothing (a value is an offset), but a fixed reservation is one less
/// thing to be wrong about and 256 MiB is not a number any program in this tree approaches. Running
/// out is [`crate::Trap::HeapExhausted`], which is a message with a span rather than a `SIGSEGV`.
pub const ARENA_BYTES: u64 = 256 << 20;

/// The most layouts one module may have.
///
/// A bound rather than a judgement: `Repr` is resolved on demand and a type family that
/// instantiates itself at a bigger type on every step — `F[T]` with a field of `F[list[T]]` — would
/// ask for layouts forever. Nothing in this tree comes within two orders of magnitude.
pub const MAX_LAYOUTS: usize = 512;

/// How deep a value may nest before the host stops walking it.
///
/// The host decodes a reply by recursing, and the reply comes from a process whose output is bytes:
/// a bound here is what turns a compiler bug into a message rather than into a blown host stack.
///
/// It is also a limit on what a compiled definition may *answer* with, which the evaluator does not
/// have — `docs/101` §101.6 carries it as a difference. The number is chosen the way
/// [`adr/0007`](../../../../../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md) chooses
/// the evaluator's: a frame here is small, and this is a depth an ordinary thread's stack holds
/// several times over rather than the deepest one that would fit.
pub const MAX_DEPTH: usize = 2048;

/// The two header words of a `Str`: its length in bytes, then in characters.
pub const STR_HEADER: u64 = 2 * WORD;

/// How many bytes a `Str` of `n` UTF-8 bytes occupies, header and padding included.
///
/// Padded to a whole word so that the object allocated after one still starts where every other
/// object starts: a field is read with an aligned load, and the arena has one alignment rather than
/// one per kind of object.
pub fn str_bytes(n: u64) -> u64 {
    STR_HEADER + n.next_multiple_of(WORD)
}

/// The one header word of a `list`: how many elements it has.
pub const LIST_HEADER: u64 = WORD;

/// How many bytes a `list` of `n` elements occupies.
pub fn list_bytes(n: u64) -> u64 {
    LIST_HEADER + n * WORD
}

/// The one header word of a `Map`: how many entries it has.
pub const MAP_HEADER: u64 = WORD;

/// How many bytes a `Map` of `n` entries occupies: the count, `n` keys, then `n` values.
///
/// The keys and the values are two runs rather than interleaved pairs, so `map_keys` and
/// `map_values` are each one `memcpy` into a fresh list — and because the keys being contiguous is
/// what makes the search a binary one with a stride of one.
pub fn map_bytes(n: u64) -> u64 {
    MAP_HEADER + 2 * n * WORD
}

/// What one value is, at the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Repr {
    Int,
    Float,
    Bool,
    /// Text, carried as its offset into the arena. One shape for every `Str`, so there is no index.
    Str,
    /// A list, carried as its offset. The index is into [`Heap::element`] — a `list[Int]` and a
    /// `list[Point]` are the same shape and two different reprs, because what a word of one *is*
    /// depends on the element type and nothing at the machine records it.
    List(u32),
    /// A map, carried as its offset. The index is into [`Heap::entry`], which names the key's repr
    /// and the value's — both as indices into the same table a list's element uses, so the word
    /// comparison a map's binary search needs is the one a list's search already has.
    Map(u32),
    /// An object, carried as its offset into the arena. The index is into [`Heap::layouts`].
    Obj(u32),
}

impl Repr {
    /// The machine type this is carried in. An offset is an `i64`, whatever it points at.
    pub fn machine(self) -> Scalar {
        match self {
            Repr::Float => Scalar::Float,
            Repr::Bool => Scalar::Bool,
            Repr::Int | Repr::Str | Repr::List(_) | Repr::Map(_) | Repr::Obj(_) => Scalar::Int,
        }
    }

    /// Whether this is carried as an offset rather than as the value itself.
    ///
    /// What reads it is the two places the arena becomes visible: the host reserves room for a
    /// graph in the request, and the worker sends the used arena back with a reply. A `Str`, a
    /// `list` and a `Map` are on the heap for both purposes, and the name says *reference* rather
    /// than *object* because those three have no [`Layout`].
    pub fn is_ref(self) -> bool {
        matches!(
            self,
            Repr::Obj(_) | Repr::Str | Repr::List(_) | Repr::Map(_)
        )
    }

    /// The LLVM type, which is the machine type's.
    pub fn llvm(self) -> &'static str {
        self.machine().llvm()
    }

    /// How two values of this repr are ordered, and **the only place that decides**.
    ///
    /// [`docs/105`](../../../../../docs/105-lists-arrive-read-only-report.md) §105.4 records the
    /// same defect three times: a record's field comparison matched `Repr::Obj` and `Repr::Str`
    /// by name and let a `_` arm swallow whichever reference kind had just been added, so two
    /// equal values compared unequal because their **offsets** differed. Each time the differential
    /// caught it and each time it was one arm.
    ///
    /// This is what that section said would prevent a fourth. Every consumer matches on
    /// [`Order`]'s three cases rather than on `Repr`'s six, so a new reference kind is a compile
    /// error *here* — where its comparison has to be named — and nowhere else. A backend that
    /// forgot it would not build.
    pub fn order(self) -> Order {
        match self {
            // **Unsigned**, which is the whole point of the key: the transform maps every real onto
            // the unsigned order, so a signed comparison answers `-1.0 < 0.0` with `false`.
            Repr::Float => Order::Key,
            Repr::Int => Order::Words { signed: true },
            // A `Bool` is a 0 or a 1, and is therefore either way round.
            Repr::Bool => Order::Words { signed: false },
            Repr::Str => Order::Call("beck.str.cmp".into()),
            Repr::List(at) => Order::Call(format!("beck.list.cmp.{at}")),
            Repr::Map(at) => Order::Call(format!("beck.map.cmp.{at}")),
            Repr::Obj(at) => Order::Call(format!("beck.cmp.{at}")),
        }
    }
}

/// How two values of one [`Repr`] are put in order. See [`Repr::order`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Order {
    /// Compare the words directly.
    Words { signed: bool },
    /// Normalise and take `beck_core`'s order key first, then compare unsigned.
    Key,
    /// A three-way comparison function, by symbol: `-1`, `0` or `1`.
    Call(String),
}

/// One variant of a layout — or the only one, for a record.
#[derive(Clone, Debug)]
pub struct Variant {
    /// `None` for a `model` or a `newtype`, which have no constructor to name.
    pub name: Option<Arc<str>>,
    /// Sorted by name, because [`Fields`] is.
    pub fields: Vec<(Arc<str>, Repr)>,
}

impl Variant {
    /// Which word of the object holds `name`, and what is in it.
    pub fn slot(&self, name: &str) -> Option<(usize, Repr)> {
        self.fields
            .iter()
            .position(|(f, _)| &**f == name)
            .map(|i| (i + 1, self.fields[i].1))
    }

    /// How many bytes an object of this variant occupies: the tag and one word per field.
    pub fn bytes(&self) -> u64 {
        (1 + self.fields.len() as u64) * WORD
    }
}

/// What one Beck type looks like in memory.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The type as a person writes it — `Point`, `Tree[Int]`. What a report and a refusal say.
    pub shown: String,
    /// The declared type's name, which is [`Record::ty`].
    pub name: Arc<str>,
    /// Sorted by name, so a tag is a variant's rank under the order [`Value`] compares by.
    pub variants: Vec<Variant>,
    /// Whether this is a `union`: its values carry a variant name and a record's do not.
    pub tagged: bool,
}

impl Layout {
    pub fn tag_of(&self, variant: Option<&str>) -> Option<u32> {
        self.variants
            .iter()
            .position(|v| v.name.as_deref() == variant)
            .map(|i| i as u32)
    }
}

/// A slot in the table, so a type that is *being* resolved can be referred to by the resolution.
#[derive(Clone, Debug)]
enum Slot {
    /// Under construction: a field of this type mentions it, which is what makes a type recursive.
    Pending,
    Done(Layout),
    /// Resolved once and refused; kept so the second mention gets the same reason and the indices
    /// of everything after it do not move.
    Refused(String),
}

/// Every layout one module needs, resolved on demand.
///
/// Built by the emitter as it walks the program and handed to the host in [`crate::Module`], which
/// is what makes "the bytes the host writes" and "the bytes the compiled code reads" one decision.
#[derive(Clone, Debug, Default)]
pub struct Heap {
    slots: Vec<Slot>,
    by_key: BTreeMap<String, u32>,
    /// Every distinct string literal in the module, in the order [`survey`] met them, with the
    /// offset the host writes each one to. Interned before a byte is emitted, so the pool is a
    /// function of the *program* rather than of which definitions turned out to compile — and
    /// therefore the same table for both emitters.
    strings: Vec<(Arc<str>, u64)>,
    by_str: BTreeMap<Arc<str>, u32>,
    /// Where the next literal would go, which is also where the arguments' graph begins.
    pool_end: u64,
    /// The element repr of every distinct list type, in the order they were resolved.
    ///
    /// A separate table from [`Heap::slots`] because a list has no [`Layout`] — there are no named
    /// fields and no variants, only "how many" and "of what".
    elements: Vec<Repr>,
    by_element: BTreeMap<Repr, u32>,
    /// The key and value reprs of every distinct map type, as indices into [`Heap::elements`] — so
    /// the word comparison a map's binary search needs is the one a list's search already has.
    entries: Vec<(u32, u32)>,
    by_entry: BTreeMap<(u32, u32), u32>,
    /// Whether any signature or body in this module mentions text.
    ///
    /// Separate from [`Heap::slots`] because a `Str` has no [`Layout`]: a definition taking one and
    /// returning one needs an arena and would otherwise get the module a program of pure arithmetic
    /// gets.
    text: bool,
}

impl Heap {
    pub fn new() -> Heap {
        Heap {
            pool_end: FIRST,
            ..Heap::default()
        }
    }

    /// Whether anything in this module needs an arena at all.
    ///
    /// A program of pure arithmetic gets the module it always got: no `malloc`, no globals, and no
    /// blob on the wire. That is not an optimisation but the thing that keeps
    /// [`docs/93`](../../../../../docs/93-llvm-backend-report.md) §93.5's numbers about the same
    /// code they were measured on.
    pub fn is_empty(&self) -> bool {
        !self.text
            && self.elements.is_empty()
            && self.entries.is_empty()
            && !self.slots.iter().any(|s| matches!(s, Slot::Done(_)))
    }

    /// What one element of list repr `at` is.
    pub fn element(&self, at: u32) -> Repr {
        self.elements[at as usize]
    }

    /// Which reprs map `at`'s keys and values are, as indices into the word-comparison table.
    pub fn entry(&self, at: u32) -> (u32, u32) {
        self.entries[at as usize]
    }

    pub fn uses_maps(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Every distinct map type this module needs.
    pub fn maps(&self) -> impl Iterator<Item = (u32, (u32, u32))> + '_ {
        self.entries
            .iter()
            .copied()
            .enumerate()
            .map(|(i, e)| (i as u32, e))
    }

    /// Whether any list appears in this module, and therefore whether its runtime is worth
    /// emitting. A list has no [`Layout`], so [`Heap::layouts`] cannot answer this.
    pub fn uses_lists(&self) -> bool {
        !self.elements.is_empty()
    }

    /// Every distinct list type this module needs, which is what an emitter defines a comparison
    /// for.
    pub fn lists(&self) -> impl Iterator<Item = (u32, Repr)> + '_ {
        self.elements
            .iter()
            .copied()
            .enumerate()
            .map(|(i, r)| (i as u32, r))
    }

    /// Whether text appears anywhere in this module, and therefore whether its runtime is worth
    /// emitting. A `Str` has no [`Layout`], so [`Heap::layouts`] cannot answer this.
    pub fn uses_text(&self) -> bool {
        self.text
    }

    /// A repr's place in the word-comparison table, interned.
    ///
    /// One table for two purposes: a list's element and a map's key or value are both "a word this
    /// module has to be able to compare", and a second table would mean two comparison functions
    /// for one repr.
    fn intern_repr(&mut self, inner: Repr) -> u32 {
        if let Some(at) = self.by_element.get(&inner) {
            return *at;
        }
        let at = self.elements.len() as u32;
        self.elements.push(inner);
        self.by_element.insert(inner, at);
        at
    }

    /// The list repr for elements of `inner`, interned.
    fn list_of(&mut self, inner: Repr) -> u32 {
        self.intern_repr(inner)
    }

    /// The map repr for keys of `key` and values of `value`, interned.
    fn map_of(&mut self, key: u32, value: u32) -> u32 {
        if let Some(at) = self.by_entry.get(&(key, value)) {
            return *at;
        }
        let at = self.entries.len() as u32;
        self.entries.push((key, value));
        self.by_entry.insert((key, value), at);
        at
    }

    /// The literal `s`, interned: its index, which is what an emitter records.
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(at) = self.by_str.get(s) {
            return *at;
        }
        let key: Arc<str> = Arc::from(s);
        let at = self.strings.len() as u32;
        let offset = self.pool_end;
        self.pool_end += str_bytes(s.len() as u64);
        self.strings.push((key.clone(), offset));
        self.by_str.insert(key, at);
        self.text = true;
        at
    }

    /// Where the host wrote literal `at`. A compile-time constant, which is the whole point.
    pub fn string_offset(&self, at: u32) -> u64 {
        self.strings[at as usize].1
    }

    /// How many bytes of every request are the literal pool.
    pub fn pool_bytes(&self) -> u64 {
        self.pool_end - FIRST
    }

    pub fn strings(&self) -> impl Iterator<Item = (u32, &str, u64)> {
        self.strings
            .iter()
            .enumerate()
            .map(|(i, (s, at))| (i as u32, &**s, *at))
    }

    /// The reserved word and then the literals, which is what every request's heap starts with.
    fn write_pool(&self, blob: &mut Vec<u8>) {
        blob.resize(FIRST as usize, 0);
        for (s, at) in &self.strings {
            debug_assert_eq!(blob.len() as u64, *at);
            write_text(s, blob);
        }
        debug_assert_eq!(blob.len() as u64, self.pool_end);
    }

    pub fn layouts(&self) -> impl Iterator<Item = (u32, &Layout)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| match s {
            Slot::Done(l) => Some((i as u32, l)),
            _ => None,
        })
    }

    pub fn layout(&self, at: u32) -> &Layout {
        match self.slots.get(at as usize) {
            Some(Slot::Done(l)) => l,
            _ => panic!("layout {at} was asked for and is not resolved"),
        }
    }

    /// How to say what a `Repr` is, in a report or a refusal.
    pub fn show(&self, r: Repr) -> String {
        match r {
            Repr::Int => "Int".into(),
            Repr::Float => "Float".into(),
            Repr::Bool => "Bool".into(),
            Repr::Str => "Str".into(),
            Repr::List(i) => format!("list[{}]", self.show(self.element(i))),
            Repr::Map(i) => {
                let (k, v) = self.entry(i);
                format!(
                    "Map[{}, {}]",
                    self.show(self.element(k)),
                    self.show(self.element(v))
                )
            }
            Repr::Obj(i) => self.layout(i).shown.clone(),
        }
    }

    /// What `ty` looks like at the machine, or the reason it has no shape here.
    ///
    /// Resolves through aliases and instantiates a declaration's parameters, so `Tree[Int]` and
    /// `Tree[Str]` are two questions and only the first has an answer.
    pub fn repr(&mut self, ty: &Ty, program: &Program) -> Result<Repr, String> {
        let Ty::Con(name, args) = ty else {
            return Err(match ty {
                Ty::Fun(..) => "a function value, which is a closure".into(),
                _ => format!("`{ty}`, whose type is not known here"),
            });
        };
        match &**name {
            Ty::INT => return Ok(Repr::Int),
            Ty::FLOAT => return Ok(Repr::Float),
            Ty::BOOL => return Ok(Repr::Bool),
            Ty::STR => {
                self.text = true;
                return Ok(Repr::Str);
            }
            Ty::LIST => {
                let Some(element) = args.first() else {
                    return Err("a `list` with no element type".into());
                };
                let inner = self
                    .repr(element, program)
                    .map_err(|why| format!("a `list` whose element is {why}"))?;
                return Ok(Repr::List(self.list_of(inner)));
            }
            Ty::MAP => {
                let [key, value] = args.as_slice() else {
                    return Err("a `Map` without both of its type arguments".into());
                };
                let k = self
                    .repr(key, program)
                    .map_err(|why| format!("a `Map` whose key is {why}"))?;
                let v = self
                    .repr(value, program)
                    .map_err(|why| format!("a `Map` whose value is {why}"))?;
                let (k, v) = (self.intern_repr(k), self.intern_repr(v));
                return Ok(Repr::Map(self.map_of(k, v)));
            }
            Ty::UNIT => return Err("the unit value, which has no machine representation".into()),
            // Named rather than left to fall through to "not a type this module declares", which
            // is where they landed and is a true sentence about the wrong thing: `Html` is a
            // builtin, and what it lacks is a layout. `docs/106` §106.7 is the correction.
            Ty::HTML => {
                return Err(
                    "`Html`, which is a tree of children and follows the collections rather than \
                     text"
                        .into(),
                )
            }
            Ty::ATTR => return Err("an `Attr`, which follows `Html`".into()),
            _ => {}
        }

        let key = ty.to_string();
        if let Some(at) = self.by_key.get(&key) {
            return match &self.slots[*at as usize] {
                // The back edge of a recursive type: the layout is not finished, and it does not
                // have to be — what a field holds is an offset, whatever is at the other end of it.
                Slot::Pending | Slot::Done(_) => Ok(Repr::Obj(*at)),
                Slot::Refused(why) => Err(why.clone()),
            };
        }

        let Some(decl) = program.types.get(name) else {
            return Err(format!("`{ty}`, which is not a type this module declares"));
        };
        if let TyDecl::Alias { ty: aliased, .. } = decl {
            return self.repr(&instantiate_decl(aliased, args), program);
        }
        if self.slots.len() >= MAX_LAYOUTS {
            return Err(format!(
                "`{ty}`, past the {MAX_LAYOUTS}th distinct type this module would lay out"
            ));
        }

        let at = self.slots.len() as u32;
        self.slots.push(Slot::Pending);
        self.by_key.insert(key.clone(), at);

        let built = self.build(decl, args, &key, program);
        self.slots[at as usize] = match built {
            Ok(layout) => Slot::Done(layout),
            Err(why) => {
                self.slots[at as usize] = Slot::Refused(why.clone());
                return Err(why);
            }
        };
        Ok(Repr::Obj(at))
    }

    fn build(
        &mut self,
        decl: &TyDecl,
        args: &[Ty],
        shown: &str,
        program: &Program,
    ) -> Result<Layout, String> {
        let (mut variants, tagged) = match decl {
            TyDecl::Model { fields, .. } => (
                vec![(None, self.fields(fields, args, program, shown)?)],
                false,
            ),
            // A newtype is a record of one field called `value` — the checker builds it that way,
            // so it is laid out that way rather than unwrapped. `docs/93`'s `Scalar::of` already
            // refused to treat one as its inner type for the same reason: it is a `Value::Data` at
            // run time however zero-cost it is in the type system.
            TyDecl::Newtype { inner, .. } => (
                vec![(
                    None,
                    self.fields(
                        std::slice::from_ref(&(Arc::from("value"), inner.clone())),
                        args,
                        program,
                        shown,
                    )?,
                )],
                false,
            ),
            TyDecl::Union { variants, .. } => {
                let mut out = Vec::with_capacity(variants.len());
                for v in variants {
                    out.push((
                        Some(v.name.clone()),
                        self.fields(&v.fields, args, program, shown)?,
                    ));
                }
                (out, true)
            }
            TyDecl::Alias { .. } => unreachable!("an alias is resolved before it is built"),
        };
        // By name, because a tag has to be the rank `Value`'s `Ord` compares variants by.
        variants.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(Layout {
            shown: shown.to_string(),
            name: decl.name().clone(),
            tagged,
            variants: variants
                .into_iter()
                .map(|(name, fields)| Variant { name, fields })
                .collect(),
        })
    }

    fn fields(
        &mut self,
        declared: &[(Arc<str>, Ty)],
        args: &[Ty],
        program: &Program,
        shown: &str,
    ) -> Result<Vec<(Arc<str>, Repr)>, String> {
        let mut out: Vec<(Arc<str>, Repr)> = Vec::with_capacity(declared.len());
        for (name, ty) in declared {
            let ty = instantiate_decl(ty, args);
            let repr = self
                .repr(&ty, program)
                .map_err(|why| format!("a `{shown}`, whose field `{name}` is {why}"))?;
            out.push((name.clone(), repr));
        }
        // The order `Fields` keeps, which is the order the digest, the wire format and `Ord` all
        // read a record in.
        out.sort_by(|(a, _), (b, _)| beck_core::core::field_order(a, b));
        Ok(out)
    }

    // -- marshalling ------------------------------------------------------------------------

    /// The cells and the blob one call's arguments become.
    ///
    /// The blob is the bytes of an arena: byte `i` of it is offset `i`, so an offset in a cell or
    /// in a field needs no adjusting at either end. It is empty when nothing is an object, which is
    /// every call a program of arithmetic makes.
    pub fn encode_args(
        &self,
        args: &[Value],
        want: &[Repr],
    ) -> Result<(Vec<u64>, Vec<u8>), String> {
        let mut blob = Vec::new();
        // The pool goes in whether or not *this* call passes a reference: the literals belong to
        // the module, and a definition taking two `Int`s may still compare one against `"x"`.
        if !self.strings.is_empty() {
            self.write_pool(&mut blob);
        } else if want.iter().any(|r| r.is_ref()) {
            blob.resize(FIRST as usize, 0);
        }
        let mut cells = Vec::with_capacity(args.len());
        for (v, r) in args.iter().zip(want) {
            cells.push(self.encode(v, *r, &mut blob)?);
        }
        Ok((cells, blob))
    }

    /// One value as the word that carries it, appending whatever it needs to `blob`.
    pub fn encode(&self, v: &Value, r: Repr, blob: &mut Vec<u8>) -> Result<u64, String> {
        match (v, r) {
            (Value::Int(i), Repr::Int) => Ok(*i as u64),
            (Value::Bool(b), Repr::Bool) => Ok(u64::from(*b)),
            // Through `as_f64` rather than off the discriminant: `Value::Float` holds the *order
            // key*, and compiled code works in ordinary IEEE bits.
            (Value::Float(_), Repr::Float) => v
                .as_f64()
                .map(f64::to_bits)
                .ok_or_else(|| "a real that is not a real".to_string()),
            (Value::Str(t), Repr::Str) => {
                let offset = blob.len() as u64;
                write_text(t.as_str(), blob);
                Ok(offset)
            }
            // Depth first, exactly as a record's fields are: an element's offset has to exist
            // before the word that holds it is written.
            (Value::List(xs), Repr::List(at)) => {
                let element = self.element(at);
                let mut words = Vec::with_capacity(xs.len() + 1);
                words.push(xs.len() as u64);
                for x in xs.iter() {
                    words.push(self.encode(x, element, blob)?);
                }
                let offset = blob.len() as u64;
                for w in words {
                    blob.extend_from_slice(&w.to_ne_bytes());
                }
                Ok(offset)
            }
            // The keys in key order and then the values in the same order, which is what a `PMap`
            // iterates and therefore what a binary search here can rely on.
            (Value::Map(m), Repr::Map(at)) => {
                let (k, v) = self.entry(at);
                let (key, value) = (self.element(k), self.element(v));
                let mut words = Vec::with_capacity(2 * m.len() + 1);
                words.push(m.len() as u64);
                for (k, _) in m.iter() {
                    words.push(self.encode(k, key, blob)?);
                }
                let mut values = Vec::with_capacity(m.len());
                for (_, v) in m.iter() {
                    values.push(self.encode(v, value, blob)?);
                }
                words.extend(values);
                let offset = blob.len() as u64;
                for w in words {
                    blob.extend_from_slice(&w.to_ne_bytes());
                }
                Ok(offset)
            }
            (Value::Data(record), Repr::Obj(at)) => self.encode_object(record, at, blob),
            _ => Err(format!(
                "a {} where the signature says {}",
                kind_of(v),
                self.show(r)
            )),
        }
    }

    fn encode_object(&self, record: &Record, at: u32, blob: &mut Vec<u8>) -> Result<u64, String> {
        let layout = self.layout(at);
        if record.ty != layout.name {
            return Err(format!(
                "a `{}` where the signature says `{}`",
                record.ty, layout.name
            ));
        }
        let tag =
            layout
                .tag_of(record.variant.as_deref())
                .ok_or_else(|| match &record.variant {
                    Some(v) => format!("`{}` is not a variant of `{}`", v, layout.shown),
                    None => format!(
                        "`{}` is a union and this value names no variant",
                        layout.shown
                    ),
                })?;
        let variant = &layout.variants[tag as usize];
        if record.fields.len() != variant.fields.len() {
            return Err(format!(
                "a `{}` with {} fields where the layout has {}",
                layout.shown,
                record.fields.len(),
                variant.fields.len()
            ));
        }

        // Depth first: a field's offset has to exist before the word that holds it is written.
        let mut words = Vec::with_capacity(variant.fields.len() + 1);
        words.push(u64::from(tag));
        for (name, field) in &variant.fields {
            let value = record
                .fields
                .get(name)
                .ok_or_else(|| format!("a `{}` with no field `{name}`", layout.shown))?;
            words.push(self.encode(value, *field, blob)?);
        }

        let offset = blob.len() as u64;
        for w in words {
            blob.extend_from_slice(&w.to_ne_bytes());
        }
        Ok(offset)
    }

    /// The value a word carries, reading whatever it points at out of `blob`.
    ///
    /// **Iterative, with its own stack.** It was recursive, and that made [`MAX_DEPTH`] a number
    /// about the *host thread* rather than about the value: a debug build spent enough frame per
    /// level that a value 1,600 deep aborted the process while the declared ceiling said 2,048, so
    /// which replies could be read depended on how the compiler was built — which is
    /// [`docs/64`](../../../../../docs/64-compile-speed-report.md) §64.4's defect one subsystem
    /// over, and [`adr/0007`](../../../../../docs/adr/0007-evaluator-stack-is-declared-not-discovered.md)'s
    /// property this did not have. With the stack on the heap the ceiling is the only limit and it
    /// is the same one in every profile.
    pub fn decode(&self, cell: u64, r: Repr, blob: &[u8]) -> Result<Value, String> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut next = Some((cell, r));
        let mut done: Option<Value> = None;
        loop {
            if let Some((cell, r)) = next.take() {
                match self.begin(cell, r, blob)? {
                    Begun::Leaf(v) => done = Some(v),
                    Begun::Nested(frame) => {
                        if stack.len() >= MAX_DEPTH {
                            return Err(format!(
                                "the compiled program answered with a value nested more than \
                                 {MAX_DEPTH} deep"
                            ));
                        }
                        stack.push(frame);
                    }
                }
            }
            let Some(frame) = stack.last_mut() else {
                return Ok(done.expect("the first thing decoded is a value"));
            };
            if let Some(v) = done.take() {
                frame.absorb(v);
            }
            match frame.next_child(blob)? {
                Some(child) => next = Some(child),
                None => {
                    let frame = stack.pop().expect("just looked at it");
                    done = Some(frame.finish());
                }
            }
        }
    }

    /// What one word turns into: a finished value, or a frame whose children are still to come.
    fn begin(&self, cell: u64, r: Repr, blob: &[u8]) -> Result<Begun, String> {
        match r {
            Repr::Int => Ok(Begun::Leaf(Value::Int(cell as i64))),
            Repr::Bool => Ok(Begun::Leaf(Value::Bool(cell != 0))),
            // `Value::float` and not `Value::Float`: the constructor applies the order-key
            // transform and the canonicalisation, which is what makes this equal to what the
            // evaluator built.
            Repr::Float => Ok(Begun::Leaf(Value::float(f64::from_bits(cell)))),
            Repr::Str => self.decode_text(cell, blob).map(Begun::Leaf),
            Repr::Map(at) => {
                let (k, v) = self.entry(at);
                let count = word(blob, cell)?;
                // Checked against the arena before it is trusted as a capacity, for the reason a
                // list's count is: it came from another process.
                let bytes = count
                    .checked_mul(2 * WORD)
                    .and_then(|b| b.checked_add(WORD));
                if bytes.is_none_or(|b| cell + b > blob.len() as u64) {
                    return Err(format!(
                        "the compiled program answered with a map of {count} at offset {cell}, \
                         and its heap is {} bytes",
                        blob.len()
                    ));
                }
                Ok(Begun::Nested(Frame::Map {
                    cell,
                    key: self.element(k),
                    value: self.element(v),
                    count,
                    done: Vec::with_capacity(2 * count as usize),
                }))
            }
            Repr::List(at) => {
                let element = self.element(at);
                let count = word(blob, cell)?;
                // Checked against the arena before it is trusted as a capacity: the count comes
                // from another process, and `Vec::with_capacity` of whatever the bytes said is the
                // one place a wrong word becomes an allocation rather than an error.
                let bytes = count.checked_mul(WORD).and_then(|b| b.checked_add(WORD));
                if bytes.is_none_or(|b| cell + b > blob.len() as u64) {
                    return Err(format!(
                        "the compiled program answered with a list of {count} at offset {cell}, \
                         and its heap is {} bytes",
                        blob.len()
                    ));
                }
                Ok(Begun::Nested(Frame::List {
                    cell,
                    element,
                    count,
                    done: Vec::with_capacity(count as usize),
                }))
            }
            Repr::Obj(at) => {
                let layout = self.layout(at);
                let tag = word(blob, cell)?;
                let variant = layout.variants.get(tag as usize).ok_or_else(|| {
                    format!(
                        "the compiled program answered with tag {tag} for `{}`, which has {}",
                        layout.shown,
                        layout.variants.len()
                    )
                })?;
                Ok(Begun::Nested(Frame::Obj {
                    cell,
                    ty: layout.name.clone(),
                    variant: variant.name.clone(),
                    fields: variant.fields.clone(),
                    done: Vec::with_capacity(variant.fields.len()),
                }))
            }
        }
    }

    /// Text, which reaches nothing and is therefore always a leaf.
    fn decode_text(&self, cell: u64, blob: &[u8]) -> Result<Value, String> {
        let bytes = word(blob, cell)? as usize;
        let start = (cell + STR_HEADER) as usize;
        let end = start
            .checked_add(bytes)
            .ok_or("an offset past the end of the machine")?;
        if end > blob.len() {
            return Err(format!(
                "the compiled program answered with {bytes} bytes of text at offset {cell}, and \
                 its heap is {} bytes",
                blob.len()
            ));
        }
        // Checked rather than assumed: the bytes came from another process, and a `Text` built out
        // of invalid UTF-8 would be a `Value` nothing else in the language can produce. Every
        // operation this backend compiles preserves well-formedness — a slice cuts on a character
        // boundary and a concatenation joins two whole strings — so this failing is a compiler bug
        // reported as one.
        let text = std::str::from_utf8(&blob[start..end])
            .map_err(|e| format!("the compiled program answered with invalid UTF-8: {e}"))?;
        let value = Value::str_(text);
        // The count the compiled code carried has to be the count the host would compute, because
        // it is what `str_len` answered inside the call.
        let claimed = word(blob, cell + WORD)?;
        match &value {
            Value::Str(t) if t.chars_len() as u64 == claimed => Ok(value),
            _ => Err(format!(
                "the compiled program said its text is {claimed} characters and it is {}",
                text.chars().count()
            )),
        }
    }
}

/// What [`Heap::begin`] found at a word.
enum Begun {
    Leaf(Value),
    Nested(Frame),
}

/// A value being decoded, with the children it is still waiting for.
///
/// The decoder's stack, on the heap: see [`Heap::decode`] for why it is not the thread's.
enum Frame {
    List {
        cell: u64,
        element: Repr,
        count: u64,
        done: Vec<Value>,
    },
    Obj {
        cell: u64,
        ty: Arc<str>,
        variant: Option<Arc<str>>,
        fields: Vec<(Arc<str>, Repr)>,
        done: Vec<(Arc<str>, Value)>,
    },
    /// Every key, then every value: the order the words are laid out in, so the walk is one pass.
    Map {
        cell: u64,
        key: Repr,
        value: Repr,
        count: u64,
        done: Vec<Value>,
    },
}

impl Frame {
    /// Take the child that was just finished.
    fn absorb(&mut self, v: Value) {
        match self {
            Frame::List { done, .. } | Frame::Map { done, .. } => done.push(v),
            Frame::Obj { fields, done, .. } => {
                let name = fields[done.len()].0.clone();
                done.push((name, v));
            }
        }
    }

    /// The next child to decode, or `None` when every one is in.
    fn next_child(&self, blob: &[u8]) -> Result<Option<(u64, Repr)>, String> {
        match self {
            Frame::List {
                cell,
                element,
                count,
                done,
            } => {
                if done.len() as u64 == *count {
                    return Ok(None);
                }
                let w = word(blob, cell + (done.len() as u64 + 1) * WORD)?;
                Ok(Some((w, *element)))
            }
            Frame::Map {
                cell,
                key,
                value,
                count,
                done,
            } => {
                let i = done.len() as u64;
                if i == 2 * count {
                    return Ok(None);
                }
                let w = word(blob, cell + (i + 1) * WORD)?;
                Ok(Some((w, if i < *count { *key } else { *value })))
            }
            Frame::Obj {
                cell, fields, done, ..
            } => {
                let Some((_, repr)) = fields.get(done.len()) else {
                    return Ok(None);
                };
                let w = word(blob, cell + (done.len() as u64 + 1) * WORD)?;
                Ok(Some((w, *repr)))
            }
        }
    }

    fn finish(self) -> Value {
        match self {
            Frame::List { done, .. } => Value::List(Arc::new(done)),
            Frame::Map {
                count, mut done, ..
            } => {
                let values = done.split_off(count as usize);
                Value::Map(done.into_iter().zip(values).collect())
            }
            Frame::Obj {
                ty, variant, done, ..
            } => Value::Data(Arc::new(Record {
                ty,
                variant,
                fields: Fields::from_sorted(done),
            })),
        }
    }
}

/// One `Str` appended to a blob: the two counts, the bytes, and the padding to a whole word.
///
/// The one place text is written into an arena, called by the literal pool and by an argument
/// alike — because the compiled code reading them cannot tell the two apart and must not have to.
fn write_text(s: &str, blob: &mut Vec<u8>) {
    blob.extend_from_slice(&(s.len() as u64).to_ne_bytes());
    blob.extend_from_slice(&(s.chars().count() as u64).to_ne_bytes());
    blob.extend_from_slice(s.as_bytes());
    let pad = s.len().next_multiple_of(WORD as usize) - s.len();
    blob.extend(std::iter::repeat_n(0u8, pad));
}

/// One word out of the blob, or the reason there is not one there.
fn word(blob: &[u8], at: u64) -> Result<u64, String> {
    let start = at as usize;
    let end = start
        .checked_add(WORD as usize)
        .ok_or("an offset past the end of the machine")?;
    if !at.is_multiple_of(WORD) || end > blob.len() {
        return Err(format!(
            "the compiled program answered with offset {at}, and its heap is {} bytes",
            blob.len()
        ));
    }
    Ok(u64::from_ne_bytes(
        blob[start..end].try_into().expect("eight bytes"),
    ))
}

/// What kind of value this is, for a message about one that is the wrong kind.
fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Unit => "unit",
        Value::Bool(_) => "Bool",
        Value::Int(_) => "Int",
        Value::Float(_) => "Float",
        Value::Str(_) => "Str",
        Value::List(_) => "list",
        Value::Map(_) => "Map",
        Value::Data(_) => "record",
        Value::Html(_) => "Html",
        Value::Attr(_) => "Attr",
        Value::Closure(_) => "function",
    }
}

/// Resolve every layout `program` needs, before either emitter writes a byte.
///
/// Two things want this rather than the layouts appearing as bodies are walked. A module knows
/// whether it needs an arena at all — which is what keeps a program of pure arithmetic compiling to
/// the module it compiled to before there was a heap — and it knows it *before* `main` is written,
/// which is where the two emitters differ most in when they write what. And a layout's index is
/// then a function of the program's own order rather than of which definition happened to be
/// emitted first, so the IR is the same bytes twice.
///
/// Errors are dropped on purpose: a type with no layout is a definition this backend refuses, and
/// the refusal is written where the definition is, with the reason.
pub fn survey(program: &Program, heap: &mut Heap) {
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        for (_, _, ty) in &def.params {
            let _ = heap.repr(ty, program);
        }
        let _ = heap.repr(&def.ret, program);
        walk(&def.body, program, heap);
    }
}

/// Every string constant a pattern tests against, interned along with the bodies'.
fn pattern(p: &beck_core::core::Pattern, heap: &mut Heap) {
    use beck_core::core::{Const, Pattern};
    match p {
        Pattern::Const(Const::Str(s)) => {
            heap.intern(s);
        }
        Pattern::Wildcard | Pattern::Bind(_) | Pattern::Const(_) => {}
        Pattern::At { inner, .. } => pattern(inner, heap),
        Pattern::Or(alts) => {
            for alt in alts {
                pattern(alt, heap);
            }
        }
        Pattern::Ctor { binds, .. } => {
            for (_, sub) in binds {
                pattern(sub, heap);
            }
        }
        // A list pattern is refused by both emitters, and is walked anyway: what decides the pool
        // is the program, not what turned out to compile.
        Pattern::List { items, .. } => {
            for sub in items {
                pattern(sub, heap);
            }
        }
    }
}

fn walk(c: &beck_core::Core, program: &Program, heap: &mut Heap) {
    use beck_core::core::{Const, CoreKind};
    let _ = heap.repr(&c.ty, program);
    match &c.kind {
        // Interned here rather than where a body is emitted, so the pool is the same table in both
        // emitters and in the host: a literal in a definition that turns out not to compile still
        // takes its place, exactly as a layout's index does.
        CoreKind::Const(Const::Str(s)) => {
            heap.intern(s);
        }
        CoreKind::Const(_) | CoreKind::Var(_) | CoreKind::Global(_) => {}
        CoreKind::Lam { body, .. } => walk(body, program, heap),
        CoreKind::App { func, args } => {
            walk(func, program, heap);
            for a in args {
                walk(a, program, heap);
            }
        }
        CoreKind::Prim { args, .. } => {
            for a in args {
                walk(a, program, heap);
            }
        }
        CoreKind::Let { value, body, .. } => {
            walk(value, program, heap);
            walk(body, program, heap);
        }
        CoreKind::If { cond, then, alt } => {
            walk(cond, program, heap);
            walk(then, program, heap);
            walk(alt, program, heap);
        }
        CoreKind::Match { scrutinee, arms } => {
            walk(scrutinee, program, heap);
            for arm in arms {
                // A pattern's constants are not expressions and `Arm::exprs` does not reach them,
                // so `case "one":` would otherwise be a literal the pool learned about while a
                // body was being emitted rather than before one was.
                pattern(&arm.pattern, heap);
                for e in arm.exprs() {
                    walk(e, program, heap);
                }
            }
        }
        CoreKind::Make { fields, .. } => {
            for (_, f) in fields {
                walk(f, program, heap);
            }
        }
        CoreKind::Field { base, .. } => walk(base, program, heap),
        CoreKind::With { base, fields } => {
            walk(base, program, heap);
            for (_, f) in fields {
                walk(f, program, heap);
            }
        }
        CoreKind::ListLit(xs) => {
            for x in xs {
                walk(x, program, heap);
            }
        }
        CoreKind::MapLit(kvs) => {
            for (k, v) in kvs {
                walk(k, program, heap);
                walk(v, program, heap);
            }
        }
    }
}
