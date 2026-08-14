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
//! # Closures
//!
//! A closure is an object too, and the only one that never leaves: one word saying which `lam` it
//! came from — a **rank**, see [`CLOSURE_HEADER`] — and one word per captured value.
//!
//! | Word | What |
//! |---|---|
//! | 0 | the lambda's rank among the program's lambdas |
//! | 1.. | one word per capture, in the order [`Lambda::captures`] holds them |
//!
//! [`Heap::crossing`] is the rule that keeps it inside one call: a signature, a field, an element and
//! a map's key or value all refuse one, because the host would have to turn a rank and some words
//! back into a [`beck_core::Value::Closure`] — a body, an environment and a frame size. So there is
//! no closure to marshal, and nothing in the two halves below knows closures exist.
//!
//! # A view is the call that builds it
//!
//! An `Html` is an object too, and it is the one whose contents are not a value: what a compiled
//! `view` puts in the arena is the **call** `html_el(tag, attrs, children)` would have been given,
//! and the host builds the tree out of it with [`beck_core::html::element`] — the same function the
//! evaluator has always built one with.
//!
//! | Word | `Html` element | `Html` text | `Attr` plain | `Attr` handler | `Attr` key |
//! |---|---|---|---|---|---|
//! | 0 | tag `0` | tag `1` | tag `0` | tag `1` | tag `2` |
//! | 1 | the tag name, a `Str` | — | the name, a `Str` | the event, a `Str` | — |
//! | 2 | the attributes, a `list[Attr]` | the shape of the value | the shape of the value | the shape of the command | the shape of the key |
//! | 3 | the children, a `list[Html]` | the value | the value | the command | the key |
//!
//! The reason is what a page's leaves are made of. `html_text(x)` is `x`'s **rendering**, an
//! attribute's value is a rendering, and a handler's command is *JSON* — so a compiled `view` that
//! built the tree would need `Value::display` and `Value::to_json` generated per repr, which is a
//! second spelling of two functions the host already has and which a differential could only find
//! one shape at a time. Deferring them costs the two words in rows 2 and 3: the **shape** of the
//! value, as its index in the word table, and the value's own word. That index is the only place in
//! this backend where a repr is a datum rather than a fact fixed when the module was emitted, and
//! it is what lets `html_text(x)` compile for every `x` that has a shape at all.
//!
//! What it also means is that **an `Html` here is not a tree**: two nodes that render the same page
//! can be different objects, so [`Repr::order`] answers [`Order::Absent`] for one and a program that
//! compares two views is refused rather than answered from the recipe.
//!
//! Going the other way — a baked tree crossing *into* a compiled call — [`Heap::encode`] writes the
//! recipe back, and every leaf of it is text, because that is what a built tree holds. Replaying the
//! builder over the same strings in the same order is what makes the round trip exact, hashes
//! included.
//!
//! # What has a layout, and what does not
//!
//! `Int`, `Float` and `Bool` are [`Repr`]'s scalars and live in registers as they always have. A
//! `model`, a `union` and a `newtype` — including a recursive one, and including one that takes a
//! type parameter — have a layout; a `Str`, a `list`, a `Map`, an `Html` and an `Attr` have the ones
//! above; and a closure has the one above that, inside a call. What is left is `Unit`, and
//! [`Heap::repr`] says so by name: a definition that mentions one is left to the evaluator exactly
//! as it was before there was a heap at all.

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
/// have — `docs/93` §93.12 carries it as a difference. The number is chosen the way
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

/// The two header words of a `list`: how many elements it has, and where they are.
///
/// # Why a list is an indirection
///
/// It was one word and the elements after it, which is the shape every read wants and the one shape
/// an **append** cannot have. A list is immutable, so `list_append` must answer a list of `n + 1`
/// where the old one still says `n` — and with the count sitting in front of the elements, the only
/// ways to do that are to copy the elements (`O(n)`, which is
/// [`docs/46`](../../../../../docs/46-standard-library-report.md) §46.14's quadratic
/// accumulator) or to overwrite the count (which every other holder of that list can see).
///
/// So the count and the elements are separated. A **header** is two words and is written once,
/// never touched again; a **data block** is `[cap, used, …]` and is shared by every list that was
/// built from it. Appending writes at index `used` — a slot no header covers, because every header's
/// count is at most `used` — and answers a *new* header. Nothing a reader can see is ever rewritten,
/// so this needs no ownership analysis and no reference count: it is sound by the shape of the
/// writes rather than by an argument about who holds what.
///
/// | | word 0 | word 1 | word 2.. |
/// |---|---|---|---|
/// | header | how many elements | the data block's offset | — |
/// | data | how many the block holds (`cap`) | how many are written (`used`) | the elements |
///
/// What it costs is one load: [`docs/93`](../../../../../docs/93-the-native-backends-report.md)
/// reached an element with an add, and this reaches the block first. That is paid **once per
/// operation** rather than once per element — every generated loop takes the data pointer before it
/// starts — and `docs/93` measures what is left.
pub const LIST_HEADER: u64 = 2 * WORD;

/// The two header words of a list's data block: how many elements it can hold, and how many have
/// been written.
pub const DATA_HEADER: u64 = 2 * WORD;

/// How many bytes a `list` of `n` elements occupies: the header, and a data block sized exactly `n`.
///
/// Exactly `n` and not a doubled capacity, because this is what an allocation of a *known* list
/// costs — a literal, a slice, a map's keys, the answer of a loop. Only an **append** reserves more,
/// and the doubling that makes the idiom linear is written in each emitter's `beck.list.append`
/// rather than here: nothing outside generated code needs to know it, and a constant in this file
/// that neither emitter read would be a third place for it to be wrong.
pub fn list_bytes(n: u64) -> u64 {
    LIST_HEADER + DATA_HEADER + n * WORD
}

/// The one header word of a closure: which `lam` it came from.
///
/// A **rank**, not a code address. A value here is an offset into an arena that crosses a pipe as
/// bytes ([`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md)), so
/// a closure cannot hold a pointer to code and applying one cannot be an indirect call. What it
/// holds instead is the lambda's rank among the lambdas of the program, and an application is a
/// switch on that word into a direct call per rank — which is also why neither emitter needs a
/// function pointer, a jump table in data, or a relocation the arena would have to carry.
pub const CLOSURE_HEADER: u64 = WORD;

/// How many bytes a closure with `n` captured values occupies: the rank and one word each.
pub fn closure_bytes(n: u64) -> u64 {
    CLOSURE_HEADER + n * WORD
}

/// How many words a view node or an attribute occupies: a tag and three.
///
/// One size for every variant of either, rather than a variant's own — the two shapes are this
/// module's rather than a program's, they differ by one word at most, and a fixed size is what lets
/// the deferred value below sit at the same pair of slots whichever variant is around it.
pub const NODE_WORDS: u64 = 4;

/// `Html`'s tag for an element: a tag name, its attributes and its children.
pub const HTML_ELEMENT: u64 = 0;
/// `Html`'s tag for a text node, whose one field is a deferred value.
pub const HTML_TEXT: u64 = 1;
/// `Attr`'s tag for `html_attr` — a name and a deferred value.
pub const ATTR_PLAIN: u64 = 0;
/// `Attr`'s tag for `html_on` — an event name and the command, deferred.
pub const ATTR_ON: u64 = 1;
/// `Attr`'s tag for `html_key` — a deferred value and nothing else.
pub const ATTR_KEY: u64 = 2;

/// How many words a raised value occupies: its shape and its word.
///
/// The same pair a view node defers with ([`DEFERRED`]), in the one other place this backend has to
/// hand the host a value whose type is not on the signature — a `raise` may carry any declared type
/// and the definition that raised it answers with something else entirely, so the reply's own repr
/// says nothing about it.
pub const RAISED_WORDS: u64 = 2;

/// Which word of a view node or an attribute holds the first half of its deferred value.
///
/// The same slot in all five variants, which is the reason [`NODE_WORDS`] is one number: a `Text`
/// and an `Attr::Key` have nothing in slot 1 and pay a word for it, and in exchange the host reads
/// a deferred value out of one place rather than out of five.
pub const DEFERRED: usize = 2;

/// The one header word of a `Map`: how many entries it has.
pub const MAP_HEADER: u64 = WORD;

/// A `Map`'s node: its subtree's size, the key, the value, and the two children.
///
/// # Why a map is a tree
///
/// It was a sorted run — a count and then every key followed by every value — which makes a lookup
/// a binary search over contiguous words and makes an **insert** a copy of the whole run. That is
/// `O(n)` where [`beck_core::pmap`] is `O(log n)`, and
/// [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.7 refused to ship
/// it for that reason: *this backend does not ship an operation whose asymptote is worse than the
/// evaluator's*.
///
/// [`docs/93`](../../../../../docs/93-the-native-backends-report.md) removed the same refusal for a list
/// by separating the count from the elements, and §93.7 said in advance that the map's would
/// **survive** that trick — a sorted run has to shift however its header is arranged. What removes
/// it is the structure the evaluator already uses: a **weight-balanced tree**, whose insert rebuilds
/// the path and shares every subtree it did not touch. `beck_core::pmap`'s own module documentation
/// is the argument for that choice; this is the same tree with the same `DELTA` and `RATIO`, in an
/// arena.
///
/// | word | what |
/// |---|---|
/// | 0 | how many entries this subtree holds |
/// | 1 | the key |
/// | 2 | the value |
/// | 3 | the left child, or `0` |
/// | 4 | the right child, or `0` |
///
/// An empty map is the offset `0`, which is the one offset [`FIRST`] reserves so that no live object
/// has it. A node is never mutated after it is written, so an insert's answer and the map it was
/// given share every node off the path — which is what makes the fold that keeps a `Map` linear in
/// the events rather than quadratic.
pub const MAP_NODE: u64 = 5 * WORD;

/// Where a node's fields are, in words.
pub const NODE_KEY: usize = 1;
pub const NODE_VALUE: usize = 2;
pub const NODE_LEFT: usize = 3;
pub const NODE_RIGHT: usize = 4;

/// The weight-balanced tree's two constants, which are [`beck_core::pmap`]'s.
///
/// Written here as well as there because the rebalancing is generated code and a tree built with
/// one pair of constants and rebalanced with another is a tree with no invariant at all. They are
/// Adams's, by way of Haskell's `Data.Map`: a subtree may be up to `DELTA` times its sibling, and
/// the choice between a single and a double rotation is `RATIO`.
pub const DELTA: u64 = 3;
pub const RATIO: u64 = 2;

/// How many bytes a `Map` of `n` entries occupies: one node each.
///
/// A *fresh* map, which is what the host writes and what a literal builds. An insert allocates the
/// path it rebuilt — `O(log n)` nodes — and shares the rest, so what a fold leaves behind is not
/// this number times the events.
pub fn map_bytes(n: u64) -> u64 {
    n * MAP_NODE
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
    /// A closure, carried as its offset. The index is into [`Heap::family`] — every closure of one
    /// shape is one family, because what an application needs to know is the *signature* it is
    /// calling through and not which lambda is at the other end.
    Fn(u32),
    /// A view node, carried as its offset. One shape for every `Html`, so there is no index — and
    /// what is in it is the *call* that would build the tree rather than the tree.
    Html,
    /// One attribute of a view node, carried as its offset. One shape for all three of the things
    /// `html_attr`, `html_on` and `html_key` answer.
    Attr,
}

impl Repr {
    /// The machine type this is carried in. An offset is an `i64`, whatever it points at.
    pub fn machine(self) -> Scalar {
        match self {
            Repr::Float => Scalar::Float,
            Repr::Bool => Scalar::Bool,
            Repr::Int
            | Repr::Str
            | Repr::List(_)
            | Repr::Map(_)
            | Repr::Obj(_)
            | Repr::Fn(_)
            | Repr::Html
            | Repr::Attr => Scalar::Int,
        }
    }

    /// Whether this is carried as an offset rather than as the value itself.
    ///
    /// What reads it is the two places the arena becomes visible: the host reserves room for a
    /// graph in the request, and the worker sends the used arena back with a reply. A `Str`, a
    /// `list` and a `Map` are on the heap for both purposes, and the name says *reference* rather
    /// than *object* because those three have no [`Layout`].
    ///
    /// A closure is an object here and never reaches either of those places: [`Heap::repr`] admits
    /// one inside a body and every *boundary* — a signature, a field, an element, a map's key or
    /// value — refuses it, so the host has no closure to marshal. This answers `true` because the
    /// value is an offset, not because anything encodes one.
    pub fn is_ref(self) -> bool {
        matches!(
            self,
            Repr::Obj(_)
                | Repr::Str
                | Repr::List(_)
                | Repr::Map(_)
                | Repr::Fn(_)
                | Repr::Html
                | Repr::Attr
        )
    }

    /// The LLVM type, which is the machine type's.
    pub fn llvm(self) -> &'static str {
        self.machine().llvm()
    }

    /// How two values of this repr are ordered, and **the only place that decides**.
    ///
    /// [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.8 records the
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
            // `Closure`'s `Ord` in `beck_core::core` is its parameters and then where its body
            // starts — the captured frame is *not* in it, so two closures from one `lam` are equal
            // however differently they were built. A rank is assigned in exactly that order
            // ([`survey`]), so comparing two ranks is comparing two code positions and the tag
            // word answers all six operators.
            Repr::Fn(_) => Order::Call("beck.fn.cmp".into()),
            // The one repr with no order, and the reason is what a view node *is* here: an
            // `Html` in the arena is the call that would build the tree, so two nodes that
            // produce the same page — `html_text(3)` and `html_text("3")` — are different
            // objects, and `beck_core::Html`'s derived `Ord` compares the pages. A comparison
            // that answered from the recipe would disagree with the evaluator on exactly the
            // programs nobody writes, which is worse than refusing.
            Repr::Html | Repr::Attr => Order::Absent(
                "a view node, which is carried as the call that builds it rather than as the tree \
                 — so two of them can be ordered by what they render and not by what they are",
            ),
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
    /// No order at all, and the reason — a value this backend can build and hand back but cannot
    /// put in sequence.
    ///
    /// [`Heap::ordered`] is what keeps this out of a generated function: every demand for a
    /// comparison asks it first, and it walks a record's fields and a list's elements, so a module
    /// is never assembled with a comparison it would have to leave undefined. An emitter reaching
    /// this arm is therefore writing a function nothing calls, and both of them write nothing.
    Absent(&'static str),
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

/// Every closure of one shape: what applying one takes and what it answers.
///
/// Interned by its *reprs* rather than by the type as written, because an effect row is not part of
/// a machine shape: `(Int) -> Int` and `(Int) -> Int ! io` are one family, and a lambda inferred at
/// one of those types must be applicable at a site that says the other — otherwise the switch an
/// application compiles to would be missing the arm for the closure standing in front of it.
#[derive(Clone, Debug)]
pub struct Family {
    pub params: Vec<Repr>,
    pub ret: Repr,
    /// The type as a person writes it, for a report and a refusal. The first spelling met.
    pub shown: String,
    /// Every lambda of this shape, by rank, in rank order — which is what an application switches
    /// over. A rank here does not promise the lambda *compiled*: an emitter switches over the ones
    /// it emitted, and this is the set they are drawn from.
    pub ranks: Vec<u32>,
}

/// One `lam` of the program, ranked.
#[derive(Clone, Debug)]
pub struct Lambda {
    /// The parameters, which are the first half of the order a rank is assigned in.
    pub params: Vec<beck_core::core::VarId>,
    /// Where the body starts, which is the second half — and [`beck_core::core::Closure`]'s own
    /// tie-breaker.
    pub span_start: u32,
    /// The shape, when it has one here. A lambda over a `Map[Str, Html]` has no family, and any use
    /// of it is refused for the reason `Html` is.
    pub family: Option<u32>,
    /// What the closure carries: every variable the body reads and does not bind, in `VarId` order,
    /// with the type it is read at.
    ///
    /// The order is the *object's* order, and it is decided here so that the code which builds a
    /// closure and the code which reads its captures back cannot disagree — the same reason a
    /// layout is decided here rather than in an emitter. The type comes from a `Var` node inside
    /// the body: a captured variable is captured *because* the body reads it, so there is always
    /// one, and it may be under a nested `lam` rather than at the top.
    pub captures: Vec<(beck_core::core::VarId, Ty)>,
    /// Whether this is a definition's own outermost `lam` — the one that *is* a compiled function.
    /// Using such a definition as a value builds a closure of no captures whose arm calls it.
    pub def: Option<Arc<str>>,
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
    /// Every distinct closure shape, interned by its reprs.
    families: Vec<Family>,
    by_family: BTreeMap<(Vec<Repr>, Repr), u32>,
    /// Every `lam` in the program, in rank order — so the index *is* the rank.
    lams: Vec<Lambda>,
    by_lam: BTreeMap<(Vec<beck_core::core::VarId>, u32), u32>,
    /// Whether this program builds a closure, and therefore needs an arena to build it in.
    ///
    /// Not "has a `lam` in it": every definition's body *is* one, and a program of pure arithmetic
    /// must keep getting the module `docs/93` §93.5 measured — no `malloc`, no globals, no blob on
    /// the wire. What sets this is a `lam` under something, or a definition named as a value.
    closures: bool,
    /// The `list[Attr]` and `list[Html]` reprs, once anything in this module mentions `Html`.
    ///
    /// Both an "is there a view here" flag and the two indices the host decodes an element's
    /// collections with. Separate from [`Heap::slots`] for the reason text is: neither `Html` nor
    /// `Attr` has a [`Layout`] — their shape is this module's rather than a program's.
    html: Option<(u32, u32)>,
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
    /// [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.5's numbers about the same
    /// code they were measured on.
    pub fn is_empty(&self) -> bool {
        !self.text
            && !self.closures
            && self.html.is_none()
            && self.elements.is_empty()
            && self.entries.is_empty()
            && !self.slots.iter().any(|s| matches!(s, Slot::Done(_)))
    }

    /// Whether this program builds a closure, and therefore needs an arena.
    pub fn uses_closures(&self) -> bool {
        self.closures
    }

    /// What applying a closure of family `at` takes and answers.
    pub fn family(&self, at: u32) -> &Family {
        &self.families[at as usize]
    }

    /// Every closure shape this module has, which is what an emitter writes an application for.
    pub fn families(&self) -> impl Iterator<Item = (u32, &Family)> {
        self.families.iter().enumerate().map(|(i, f)| (i as u32, f))
    }

    /// The lambda of rank `at`.
    pub fn lam(&self, at: u32) -> &Lambda {
        &self.lams[at as usize]
    }

    /// Which rank a `lam` was given, by the two things that decide it.
    ///
    /// `None` when the program does not contain this lambda, which is a compiler bug rather than a
    /// program's problem — an emitter walks the same bodies [`survey`] did.
    pub fn rank_of(&self, params: &[beck_core::core::VarId], span_start: u32) -> Option<u32> {
        self.by_lam.get(&(params.to_vec(), span_start)).copied()
    }

    /// Give every lambda of the program its rank, in the order two closures are compared in.
    ///
    /// The order is [`beck_core::core::Closure`]'s own — the parameters, then where the body starts
    /// — so a rank is not an arbitrary index: comparing two ranks *is* comparing two closures the
    /// way the evaluator does, which is what lets [`Repr::order`] answer with one word comparison
    /// and agree. Assigned here, from the whole program, so that both emitters give one lambda one
    /// rank and neither has to have compiled anything to know it.
    fn rank(&mut self, lams: BTreeMap<(Vec<beck_core::core::VarId>, u32), Lambda>) {
        for (rank, (key, lam)) in lams.into_iter().enumerate() {
            let rank = rank as u32;
            if let Some(at) = lam.family {
                self.families[at as usize].ranks.push(rank);
            }
            self.by_lam.insert(key, rank);
            self.lams.push(lam);
        }
    }

    /// The reason a closure may not be *inside* something, when that is what `r` is.
    ///
    /// A closure is an offset like every other object, and the arena it points into is copied down
    /// a pipe and read back by the host — which would have to turn one into a
    /// [`beck_core::Value::Closure`], and that is a body, an environment and a frame size rather
    /// than bytes. So a closure lives inside one compiled call: it may be built, bound, captured
    /// and applied there, and every boundary refuses it. A field is a boundary because a record
    /// crosses; an element and a map's key or value are boundaries for the same reason.
    pub fn crossing(r: Repr) -> Result<(), String> {
        match r {
            Repr::Fn(_) => Err("a function value, which is built and applied inside one \
                                compiled call and has no form the host can read back"
                .into()),
            _ => Ok(()),
        }
    }

    /// The reason a value of this repr may not be handed *in* to a compiled call, when there is
    /// one.
    ///
    /// The one **directional** rule on this boundary, and it exists because one encoding is lossy in
    /// one direction. An `Attr` in the arena keeps a handler's command as a value; the host cannot
    /// name a repr for a [`Value`] it was handed, so [`Heap::encode`] writes a handler as the plain
    /// attribute it would become — which is exact for a *tree*, where that is what the attribute
    /// already is, and lossy for a bare `Attr`, where the evaluator would still be holding an
    /// `AttrValue::On`. So a definition may answer with one and may not take one.
    ///
    /// Recursive for the reason [`Heap::ordered`] is: a `list[Attr]` parameter is the same problem
    /// one collection out, and so is a record with an `Attr` field.
    pub fn inbound(&self, r: Repr) -> Result<(), String> {
        let mut seen = Vec::new();
        self.inbound_from(r, &mut seen)
    }

    fn inbound_from(&self, r: Repr, seen: &mut Vec<u32>) -> Result<(), String> {
        match r {
            Repr::Attr => Err(
                "an `Attr`, which a compiled definition may answer with and may not \
                               take: a handler's command is a value in the arena, and the host \
                               writing one back would have to name a shape for a value it was \
                               handed"
                    .into(),
            ),
            Repr::Obj(at) => {
                if seen.contains(&at) {
                    return Ok(());
                }
                seen.push(at);
                if let Slot::Done(layout) = &self.slots[at as usize] {
                    for v in &layout.variants {
                        for (name, f) in &v.fields {
                            self.inbound_from(*f, seen).map_err(|why| {
                                format!("`{}`'s field `{name}` is {why}", layout.shown)
                            })?;
                        }
                    }
                }
                Ok(())
            }
            Repr::List(at) => self
                .inbound_from(self.element(at), seen)
                .map_err(|why| format!("a list whose element is {why}")),
            Repr::Map(at) => {
                let (k, v) = self.entry(at);
                self.inbound_from(self.element(k), seen)
                    .map_err(|why| format!("a map whose key is {why}"))?;
                self.inbound_from(self.element(v), seen)
                    .map_err(|why| format!("a map whose value is {why}"))
            }
            _ => Ok(()),
        }
    }

    /// The reason two values of this repr cannot be put in order, when there is one.
    ///
    /// Asked at the **demand** — an `==`, a search, a sort key, a map's key — rather than where a
    /// comparison is written, and it recurses because a record is compared field by field and a
    /// list element by element: a `model Card { body: Html }` has no order for the same reason its
    /// field has none, and finding that out when the module is being assembled would be a link
    /// error rather than a refusal with a definition's name on it.
    pub fn ordered(&self, r: Repr) -> Result<(), String> {
        let mut seen = Vec::new();
        self.ordered_from(r, &mut seen)
    }

    fn ordered_from(&self, r: Repr, seen: &mut Vec<u32>) -> Result<(), String> {
        match r.order() {
            Order::Absent(why) => return Err(why.to_string()),
            Order::Words { .. } | Order::Key | Order::Call(_) => {}
        }
        match r {
            Repr::Obj(at) => {
                // A recursive type's back edge: a `Tree[Int]` whose field is a `Tree[Int]` is one
                // question, and asking it twice would not terminate.
                if seen.contains(&at) {
                    return Ok(());
                }
                seen.push(at);
                if let Slot::Done(layout) = &self.slots[at as usize] {
                    for v in &layout.variants {
                        for (name, f) in &v.fields {
                            self.ordered_from(*f, seen).map_err(|why| {
                                format!("`{}`'s field `{name}` is {why}", layout.shown)
                            })?;
                        }
                    }
                }
                Ok(())
            }
            Repr::List(at) => self
                .ordered_from(self.element(at), seen)
                .map_err(|why| format!("a list whose element is {why}")),
            Repr::Map(at) => {
                let (k, v) = self.entry(at);
                self.ordered_from(self.element(k), seen)
                    .map_err(|why| format!("a map whose key is {why}"))?;
                self.ordered_from(self.element(v), seen)
                    .map_err(|why| format!("a map whose value is {why}"))
            }
            _ => Ok(()),
        }
    }

    /// The `list[Attr]` and `list[Html]` reprs, which every view node's two collections are.
    ///
    /// Interned when `Html` is first resolved rather than when a program writes one down, because
    /// the host reads an element's attributes and children out of the arena and needs the same two
    /// indices the emitters stored — and a program can build a node without ever naming either
    /// list type.
    pub fn html_lists(&self) -> Option<(u32, u32)> {
        self.html
    }

    /// The family for a shape, interned.
    fn family_of(&mut self, params: Vec<Repr>, ret: Repr, shown: &str) -> u32 {
        let key = (params.clone(), ret);
        if let Some(at) = self.by_family.get(&key) {
            return *at;
        }
        let at = self.families.len() as u32;
        self.families.push(Family {
            params,
            ret,
            shown: shown.to_string(),
            ranks: Vec::new(),
        });
        self.by_family.insert(key, at);
        at
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

    /// Reserve what a view node needs: text for a tag name, and the two lists an element holds.
    ///
    /// Idempotent, and it is the *only* way `Repr::Html` is handed out — so a module that has one
    /// has both list reprs, whether or not the program ever writes `list[Html]` down.
    fn intern_html(&mut self) {
        if self.html.is_none() {
            self.text = true;
            // Interned so that a deferred value that is text has an index to name, which is the
            // one the host writes when a baked tree crosses *into* a call.
            self.word_of(Repr::Str);
            let attrs = self.list_of(Repr::Attr);
            let children = self.list_of(Repr::Html);
            self.html = Some((attrs, children));
        }
    }

    /// Where `inner` already is in the word-comparison table, if anything has interned it.
    ///
    /// Read by an emitter that has to *name* `beck.elem.cmp.{at}` for a repr it interned earlier in
    /// the same body — the index is the name, so asking twice has to answer twice the same.
    pub fn word_at(&self, inner: Repr) -> Option<u32> {
        self.by_element.get(&inner).copied()
    }

    /// The repr a shape word names — the inverse of [`Heap::word_of`].
    ///
    /// A *datum* rather than a fact fixed at emit time, which is what lets the host service an
    /// upcall without a second table saying what each primitive's argument and result types are:
    /// the compiled code sends the shape beside the word, exactly as a view's deferred leaf does
    /// (a view node's deferred value), and this is how the host reads it back.
    pub fn shape(&self, at: u32) -> Option<Repr> {
        self.elements.get(at as usize).copied()
    }

    /// A repr's place in the word-comparison table, for something that is not a list's element.
    ///
    /// `sort_by` is what wants this: its keys are values of one repr, held in a run of words and
    /// compared pairwise, which is exactly what the table is for — and the *keys* are not a list any
    /// program wrote down, so nothing else would have interned their repr. The caller must record the
    /// index the way a comparison is recorded, or the function this names is not generated.
    pub fn word_of(&mut self, inner: Repr) -> u32 {
        self.intern_repr(inner)
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
            Repr::Html => "Html".into(),
            Repr::Attr => "Attr".into(),
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
            Repr::Fn(i) => self.family(i).shown.clone(),
        }
    }

    /// What `ty` looks like at the machine, or the reason it has no shape here.
    ///
    /// Resolves through aliases and instantiates a declaration's parameters, so `Tree[Int]` and
    /// `Tree[Str]` are two questions and only the first has an answer.
    pub fn repr(&mut self, ty: &Ty, program: &Program) -> Result<Repr, String> {
        // A closure has a shape but no [`Layout`]: what a family records is the signature an
        // application goes through, and the object's own words are the lambda's captures — which
        // differ between two lambdas of one family and are therefore not a property of the type.
        if let Ty::Fun(params, ret, _) = ty {
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(
                    self.repr(p, program)
                        .map_err(|why| format!("a function whose parameter is {why}"))?,
                );
            }
            let r = self
                .repr(ret, program)
                .map_err(|why| format!("a function that answers {why}"))?;
            let at = self.family_of(ps, r, &ty.to_string());
            return Ok(Repr::Fn(at));
        }
        let Ty::Con(name, args) = ty else {
            return Err(format!("`{ty}`, whose type is not known here"));
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
                Heap::crossing(inner).map_err(|why| format!("a `list` whose element is {why}"))?;
                return Ok(Repr::List(self.list_of(inner)));
            }
            Ty::MAP => {
                let [key, value] = args.as_slice() else {
                    return Err("a `Map` without both of its type arguments".into());
                };
                let k = self
                    .repr(key, program)
                    .map_err(|why| format!("a `Map` whose key is {why}"))?;
                Heap::crossing(k).map_err(|why| format!("a `Map` whose key is {why}"))?;
                let v = self
                    .repr(value, program)
                    .map_err(|why| format!("a `Map` whose value is {why}"))?;
                Heap::crossing(v).map_err(|why| format!("a `Map` whose value is {why}"))?;
                let (k, v) = (self.intern_repr(k), self.intern_repr(v));
                return Ok(Repr::Map(self.map_of(k, v)));
            }
            Ty::UNIT => return Err("the unit value, which has no machine representation".into()),
            Ty::HTML => {
                self.intern_html();
                return Ok(Repr::Html);
            }
            Ty::ATTR => {
                self.intern_html();
                return Ok(Repr::Attr);
            }
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

        // `secret[T]` and `internal[T]` are the two wrappers the language has and no program
        // declares. Each is a `Value::Data` with one field called `value` at run time — which is
        // what keeps a `secret[Str]` distinguishable from the `Str` it holds everywhere the wire
        // format and the digest look at a value — so each is laid out as the newtype it already
        // behaves like, rather than unwrapped. Unwrapping would make the two indistinguishable in
        // compiled code and §3.5's whole claim is that they are not.
        let wrapper;
        let decl = match &**name {
            Ty::SECRET | Ty::INTERNAL => {
                let Some(inner) = args.first() else {
                    return Err(format!("`{ty}`, with no type inside it"));
                };
                wrapper = TyDecl::Newtype {
                    name: Arc::from(&**name),
                    params: Vec::new(),
                    inner: inner.clone(),
                };
                &wrapper
            }
            _ => match program.types.get(name) {
                Some(decl) => decl,
                None => return Err(format!("`{ty}`, which is not a type this module declares")),
            },
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
            Heap::crossing(repr)
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
                let mut words = Vec::with_capacity(xs.len() + 2);
                // The data block: what it can hold, what is written, then the elements. Exactly the
                // length, because a list the host writes is one nothing has appended to yet.
                words.push(xs.len() as u64);
                words.push(xs.len() as u64);
                for x in xs.iter() {
                    words.push(self.encode(x, element, blob)?);
                }
                let data = self.write_words(words, blob);
                Ok(self.write_words(vec![xs.len() as u64, data], blob))
            }
            // The keys in key order and then the values in the same order, which is what a `PMap`
            // iterates and therefore what a binary search here can rely on.
            (Value::Map(m), Repr::Map(at)) => {
                let (k, v) = self.entry(at);
                let (key, value) = (self.element(k), self.element(v));
                let mut pairs = Vec::with_capacity(m.len());
                for (k, v) in m.iter() {
                    pairs.push((self.encode(k, key, blob)?, self.encode(v, value, blob)?));
                }
                Ok(self.encode_tree(&pairs, blob))
            }
            (Value::Data(record), Repr::Obj(at)) => self.encode_object(record, at, blob),
            (Value::Html(h), Repr::Html) => self.encode_html(h, blob),
            (Value::Attr(a), Repr::Attr) => self.encode_attr(a, blob),
            _ => Err(format!(
                "a {} where the signature says {}",
                kind_of(v),
                self.show(r)
            )),
        }
    }

    /// A tree, back into the call that would have built it.
    ///
    /// The direction nothing in a program needs and the boundary needs anyway: a compiled
    /// definition may *take* an `Html`, and what the host holds by then is a baked
    /// [`beck_core::html::Html`] with its hashes computed. Every leaf of the recipe it becomes is
    /// text — a text node's rendering, an attribute's value, a key — so the deferred value here is
    /// always a `Str`, and decoding it replays the same builder over the same strings in the same
    /// order. That is what makes the round trip exact rather than approximate: `attrs` keeps its
    /// order, a key is written *after* the attributes and sets the node's key rather than an
    /// attribute of it, and both are what [`beck_core::html::element`] does with them.
    fn encode_html(&self, node: &beck_core::html::Html, blob: &mut Vec<u8>) -> Result<u64, String> {
        use beck_core::html::Html;
        let words = match node {
            Html::Text { text, .. } => {
                let at = self.text_word(text, blob)?;
                vec![HTML_TEXT, 0, at.0, at.1]
            }
            Html::Element {
                tag,
                attrs,
                key,
                children,
                ..
            } => {
                let tag = self.encode(&Value::str_(tag.as_str()), Repr::Str, blob)?;
                let mut items: Vec<u64> = Vec::with_capacity(attrs.len() + 1);
                for (k, v) in attrs {
                    items.push(self.encode_attr(
                        &beck_core::core::AttrValue::Plain(Arc::from(&**k), Arc::from(&**v)),
                        blob,
                    )?);
                }
                if let Some(k) = key {
                    items.push(
                        self.encode_attr(&beck_core::core::AttrValue::Key(Arc::from(&**k)), blob)?,
                    );
                }
                let attrs = self.encode_words(items, blob);
                let mut kids = Vec::with_capacity(children.len());
                for c in children {
                    kids.push(self.encode_html(c, blob)?);
                }
                let children = self.encode_words(kids, blob);
                vec![HTML_ELEMENT, tag, attrs, children]
            }
        };
        Ok(self.write_words(words, blob))
    }

    /// One attribute, in the shape the three `html_*` primitives build.
    ///
    /// A handler is encoded as the **plain attribute it would become** — `data-b-<event>` carrying
    /// the command's JSON — rather than as an `Attr::On`, because an `On` in the arena holds the
    /// command as a value and the host cannot name a repr for a [`Value`] it was handed. What that
    /// costs is nothing: `beck_core::html::element` turns an `On` into exactly that pair, so the
    /// tree the recipe bakes into is the one it came from.
    fn encode_attr(
        &self,
        attr: &beck_core::core::AttrValue,
        blob: &mut Vec<u8>,
    ) -> Result<u64, String> {
        use beck_core::core::AttrValue;
        let words = match attr {
            AttrValue::Plain(name, value) => {
                let name = self.encode(&Value::str_(&**name), Repr::Str, blob)?;
                let (repr, word) = self.text_word(value, blob)?;
                vec![ATTR_PLAIN, name, repr, word]
            }
            AttrValue::On(event, cmd) => {
                let name = self.encode(&Value::str_(format!("data-b-{event}")), Repr::Str, blob)?;
                let (repr, word) = self.text_word(&cmd.to_json().to_string(), blob)?;
                vec![ATTR_PLAIN, name, repr, word]
            }
            AttrValue::Key(k) => {
                let (repr, word) = self.text_word(k, blob)?;
                vec![ATTR_KEY, 0, repr, word]
            }
        };
        Ok(self.write_words(words, blob))
    }

    /// A deferred value that is text: the repr index the host will read it back with, and the
    /// offset it was written at.
    fn text_word(&self, s: &str, blob: &mut Vec<u8>) -> Result<(u64, u64), String> {
        let at = self.word_at(Repr::Str).ok_or_else(|| {
            "a view crossed into a call in a module whose heap has no text in it".to_string()
        })?;
        let offset = self.encode(&Value::str_(s), Repr::Str, blob)?;
        Ok((u64::from(at), offset))
    }

    /// A list of words already encoded, as a list: a data block and a header over it.
    fn encode_words(&self, items: Vec<u64>, blob: &mut Vec<u8>) -> u64 {
        let n = items.len() as u64;
        let mut words = Vec::with_capacity(items.len() + 2);
        words.push(n);
        words.push(n);
        words.extend(items);
        let data = self.write_words(words, blob);
        self.write_words(vec![n, data], blob)
    }

    fn write_words(&self, words: Vec<u64>, blob: &mut Vec<u8>) -> u64 {
        let offset = blob.len() as u64;
        for w in words {
            blob.extend_from_slice(&w.to_ne_bytes());
        }
        offset
    }

    /// A sorted run of entries as a **perfectly balanced** tree.
    ///
    /// The middle entry is the root and each half is built the same way, so the two subtrees differ
    /// in size by at most one — which satisfies `DELTA` with room to spare, and is what lets the
    /// compiled code insert into a map the host wrote without rebalancing it first. Recursive over
    /// `log n` frames, on a run the host already holds.
    fn encode_tree(&self, pairs: &[(u64, u64)], blob: &mut Vec<u8>) -> u64 {
        if pairs.is_empty() {
            // The one offset `FIRST` reserves, which is what an empty map is.
            return 0;
        }
        let mid = pairs.len() / 2;
        let left = self.encode_tree(&pairs[..mid], blob);
        let right = self.encode_tree(&pairs[mid + 1..], blob);
        let (key, value) = pairs[mid];
        self.write_words(vec![pairs.len() as u64, key, value, left, right], blob)
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
                    done = Some(frame.finish()?);
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
                // The nodes in key order, worked out before any of them is decoded — the walk is
                // the only place the *shape* of the tree matters, and doing it here keeps the frame
                // below a flat list of cells exactly as a list's is.
                let nodes = self.in_order(cell, blob)?;
                Ok(Begun::Nested(Frame::Map {
                    key: self.element(k),
                    value: self.element(v),
                    count: nodes.len() as u64,
                    nodes,
                    done: Vec::new(),
                }))
            }
            Repr::List(at) => {
                let element = self.element(at);
                let count = word(blob, cell)?;
                // The header says where the elements are; the block in front of them says how many
                // it holds. See [`LIST_HEADER`] for why a list is two objects.
                let data = word(blob, cell + WORD)?;
                // Checked against the arena before it is trusted as a capacity: the count comes
                // from another process, and `Vec::with_capacity` of whatever the bytes said is the
                // one place a wrong word becomes an allocation rather than an error.
                let bytes = count
                    .checked_mul(WORD)
                    .and_then(|b| b.checked_add(DATA_HEADER));
                if bytes.is_none_or(|b| data + b > blob.len() as u64) {
                    return Err(format!(
                        "the compiled program answered with a list of {count} at offset {cell}, \
                         and its heap is {} bytes",
                        blob.len()
                    ));
                }
                // A header that claims more than its block holds is a compiler bug reported as one
                // rather than a read of whatever follows the block.
                let used = word(blob, data + WORD)?;
                if count > used {
                    return Err(format!(
                        "the compiled program answered with a list of {count} over a block holding \
                         {used}"
                    ));
                }
                Ok(Begun::Nested(Frame::List {
                    cell: data + DATA_HEADER,
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
            // A view node and an attribute are the two reprs whose *shape* decides what to read
            // next, because what is in the arena is the call rather than the tree: an element has
            // three arguments and a text node has one, and which is which is the tag.
            Repr::Html => {
                let tag = word(blob, cell)?;
                let slots = match tag {
                    HTML_ELEMENT => {
                        let (attrs, children) = self.html.ok_or_else(|| {
                            "the compiled program answered with an element in a module whose heap \
                             has no view in it"
                                .to_string()
                        })?;
                        vec![
                            (word(blob, cell + WORD)?, Repr::Str),
                            (word(blob, cell + 2 * WORD)?, Repr::List(attrs)),
                            (word(blob, cell + 3 * WORD)?, Repr::List(children)),
                        ]
                    }
                    HTML_TEXT => vec![self.deferred(cell, blob)?],
                    other => {
                        return Err(format!(
                            "the compiled program answered with tag {other} for a view node, \
                             which is an element or a text node"
                        ))
                    }
                };
                Ok(Begun::Nested(Frame::Node {
                    tag,
                    done: Vec::with_capacity(slots.len()),
                    slots,
                }))
            }
            Repr::Attr => {
                let tag = word(blob, cell)?;
                let deferred = self.deferred(cell, blob)?;
                let slots = match tag {
                    ATTR_PLAIN | ATTR_ON => {
                        vec![(word(blob, cell + WORD)?, Repr::Str), deferred]
                    }
                    ATTR_KEY => vec![deferred],
                    other => {
                        return Err(format!(
                            "the compiled program answered with tag {other} for an attribute, \
                             which is a name, a handler or a key"
                        ))
                    }
                };
                Ok(Begun::Nested(Frame::Attr {
                    tag,
                    done: Vec::with_capacity(slots.len()),
                    slots,
                }))
            }
            // Unreachable rather than unwritten, and it is [`Heap::crossing`] that makes it so: a
            // closure is refused in every position the host reads — a result, a field, an element,
            // a map's key or value — so no reply can contain one to decode. Written as the reason
            // rather than as a `panic!`, because a bug in that rule should be a message about a
            // compiler and not a crash in a host.
            Repr::Fn(at) => Err(format!(
                "the compiled program answered with `{}`, and a closure has no form here",
                self.family(at).shown
            )),
        }
    }

    /// Every node of a map, in key order.
    ///
    /// **Iterative, with its own stack**, for the reason [`Heap::decode`] is: the tree came from
    /// another process, and a recursive walk would make [`MAX_DEPTH`] a claim about the host's stack
    /// rather than about the value. A weight-balanced tree of `n` entries is about `2.4 log n` deep,
    /// so the ceiling is reached by a tree nothing could have built — which is exactly when it
    /// should be.
    fn in_order(&self, root: u64, blob: &[u8]) -> Result<Vec<u64>, String> {
        let mut out = Vec::new();
        let mut stack = Vec::new();
        let mut node = root;
        loop {
            while node != 0 {
                if stack.len() >= MAX_DEPTH {
                    return Err(format!(
                        "the compiled program answered with a map nested more than {MAX_DEPTH} deep"
                    ));
                }
                stack.push(node);
                node = word(blob, node + NODE_LEFT as u64 * WORD)?;
            }
            let Some(seen) = stack.pop() else {
                return Ok(out);
            };
            // A size word that disagrees with the walk is a compiler bug reported as one rather
            // than a decoder that runs until the arena ends.
            if out.len() as u64 >= word(blob, root)? {
                return Err(format!(
                    "the compiled program answered with a map whose root says {} entries and whose \
                     walk found more",
                    word(blob, root)?
                ));
            }
            out.push(seen);
            node = word(blob, seen + NODE_RIGHT as u64 * WORD)?;
        }
    }

    /// The value a `raise` carried, out of the pair the compiled code left in the arena.
    ///
    /// [`crate::Trap::Raised`]'s payload is that pair's offset, and the arena travels with the
    /// reply for this one failure — so the message the host builds is the evaluator's own, made out
    /// of the value rather than out of the fact that there was one.
    pub fn raised(&self, cell: u64, blob: &[u8]) -> Result<Value, String> {
        let at = word(blob, cell)?;
        let repr = self.elements.get(at as usize).copied().ok_or_else(|| {
            format!(
                "the compiled program raised a value of shape {at}, and this \
                                    module has {}",
                self.elements.len()
            )
        })?;
        self.decode(word(blob, cell + WORD)?, repr, blob)
    }

    /// The word a view node or an attribute deferred, and what to read it as.
    ///
    /// The one place in this backend where a repr is a **datum** rather than a fact fixed when the
    /// module was emitted. It is what lets `html_text(x)` compile for every `x` that has a shape at
    /// all: the compiled code stores the index of `x`'s repr beside `x`'s word and renders nothing,
    /// and the host — which holds `Value::display` and `Value::to_json`, the two functions a page's
    /// leaves are made with — reads it back and renders it. A generated renderer would be a second
    /// spelling of both.
    fn deferred(&self, cell: u64, blob: &[u8]) -> Result<(u64, Repr), String> {
        let at = word(blob, cell + DEFERRED as u64 * WORD)?;
        let repr = self.elements.get(at as usize).copied().ok_or_else(|| {
            format!(
                "the compiled program deferred a value of shape {at}, and \
                                    this module has {}",
                self.elements.len()
            )
        })?;
        Ok((word(blob, cell + (DEFERRED as u64 + 1) * WORD)?, repr))
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
    /// A key and then its value, entry by entry in key order.
    ///
    /// `nodes` is the tree's in-order walk, done once in [`Heap::begin`] — so this frame is a flat
    /// list of cells exactly as a list's is, and the *shape* of the tree is not something the
    /// decoder's stack has to carry.
    Map {
        nodes: Vec<u64>,
        key: Repr,
        value: Repr,
        count: u64,
        done: Vec<Value>,
    },
    /// A view node, and an attribute of one.
    ///
    /// Both carry their words as a list worked out in [`Heap::begin`] rather than as a layout,
    /// because which words there are is decided by the tag — and because one of them is a deferred
    /// value whose repr was read out of the arena rather than known when the module was emitted.
    Node {
        tag: u64,
        slots: Vec<(u64, Repr)>,
        done: Vec<Value>,
    },
    Attr {
        tag: u64,
        slots: Vec<(u64, Repr)>,
        done: Vec<Value>,
    },
}

impl Frame {
    /// Take the child that was just finished.
    fn absorb(&mut self, v: Value) {
        match self {
            Frame::List { done, .. }
            | Frame::Map { done, .. }
            | Frame::Node { done, .. }
            | Frame::Attr { done, .. } => done.push(v),
            Frame::Obj { fields, done, .. } => {
                let name = fields[done.len()].0.clone();
                done.push((name, v));
            }
        }
    }

    /// The next child to decode, or `None` when every one is in.
    fn next_child(&self, blob: &[u8]) -> Result<Option<(u64, Repr)>, String> {
        match self {
            // `cell` is the first *element*, not the header: a list's elements live in a block of
            // their own, and `begin` resolved it.
            Frame::List {
                cell,
                element,
                count,
                done,
            } => {
                if done.len() as u64 == *count {
                    return Ok(None);
                }
                let w = word(blob, cell + done.len() as u64 * WORD)?;
                Ok(Some((w, *element)))
            }
            Frame::Map {
                nodes,
                key,
                value,
                count,
                done,
            } => {
                let i = done.len() as u64;
                if i == 2 * count {
                    return Ok(None);
                }
                // A key and then its value, so the two reprs alternate.
                let node = nodes[(i / 2) as usize];
                let (slot, repr) = if i.is_multiple_of(2) {
                    (NODE_KEY, *key)
                } else {
                    (NODE_VALUE, *value)
                };
                Ok(Some((word(blob, node + slot as u64 * WORD)?, repr)))
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
            // Already read, because the tag decided them: see [`Frame::Node`].
            Frame::Node { slots, done, .. } | Frame::Attr { slots, done, .. } => {
                Ok(slots.get(done.len()).copied())
            }
        }
    }

    /// The value this frame's children add up to.
    ///
    /// Fallible for one of the five: an element is finished by handing its three arguments to
    /// [`beck_core::html::element`], which is the evaluator's own `html_el` and refuses an
    /// attribute list holding something that is not an attribute. Nothing a compiled program can
    /// build reaches that refusal — the types were checked before a word was emitted — but the
    /// bytes came from another process, and this is the one frame whose contents are interpreted
    /// rather than copied.
    fn finish(self) -> Result<Value, String> {
        let v = match self {
            Frame::Node { tag, mut done, .. } => {
                return match tag {
                    HTML_TEXT => Ok(Value::Html(Arc::new(beck_core::html::text_of(
                        done.first().unwrap_or(&Value::Unit),
                    )))),
                    _ => {
                        let empty: &[Value] = &[];
                        let (tag, attrs, children) = (
                            done.first().cloned().unwrap_or(Value::Unit),
                            done.get(1).cloned().unwrap_or(Value::Unit),
                            done.pop().unwrap_or(Value::Unit),
                        );
                        beck_core::html::element(
                            &tag,
                            attrs.as_list().map(|v| v.as_slice()).unwrap_or(empty),
                            children.as_list().map(|v| v.as_slice()).unwrap_or(empty),
                        )
                        .map(|h| Value::Html(Arc::new(h)))
                    }
                }
            }
            // `display` and not the bytes: `html_attr` renders its arguments and `html_key`
            // renders its one, so what the evaluator puts in an `AttrValue` is a rendering. A
            // handler's command is the exception and is kept whole, because what turns it into
            // JSON is `beck_core::html::element` and it is the same function either way.
            Frame::Attr { tag, done, .. } => {
                use beck_core::core::AttrValue;
                let at = |i: usize| done.get(i).cloned().unwrap_or(Value::Unit);
                Value::Attr(Arc::new(match tag {
                    ATTR_PLAIN => {
                        AttrValue::Plain(Arc::from(at(0).display()), Arc::from(at(1).display()))
                    }
                    ATTR_ON => AttrValue::On(Arc::from(at(0).display()), at(1)),
                    _ => AttrValue::Key(Arc::from(at(0).display())),
                }))
            }
            Frame::List { done, .. } => Value::List(Arc::new(done)),
            // Pairs, in the order they were asked for.
            Frame::Map { done, .. } => Value::Map(
                done.chunks_exact(2)
                    .map(|kv| (kv[0].clone(), kv[1].clone()))
                    .collect(),
            ),
            Frame::Obj {
                ty, variant, done, ..
            } => Value::Data(Arc::new(Record {
                ty,
                variant,
                fields: Fields::from_sorted(done),
            })),
        };
        Ok(v)
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
    let mut lams: BTreeMap<(Vec<beck_core::core::VarId>, u32), Lambda> = BTreeMap::new();
    for name in &program.def_order {
        let Some(def) = program.defs.get(name) else {
            continue;
        };
        for (_, _, ty) in &def.params {
            let _ = heap.repr(ty, program);
        }
        let _ = heap.repr(&def.ret, program);
        walk(&def.body, Some(name), program, heap, &mut lams);
    }
    heap.rank(lams);
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
        // Walked for the same reason every other pattern is: what decides the pool is the
        // program, not what turned out to compile. It said "refused by both emitters" until
        // `docs/93` compiled one.
        Pattern::List { items, .. } => {
            for sub in items {
                pattern(sub, heap);
            }
        }
    }
}

/// The type each of `want`'s variables is read at, found in the expression that reads it.
///
/// A capture has no declaration to look the type up in — a `VarId` is a slot rather than a name — so
/// the type comes from a use. Every free variable of a `lam` has one somewhere under it, which is
/// what makes it free, and [`beck_core::core::children`] is what makes "somewhere" a walk nothing
/// has to keep in step with `CoreKind`.
fn var_types(
    c: &beck_core::Core,
    want: &std::collections::BTreeSet<beck_core::core::VarId>,
    out: &mut BTreeMap<beck_core::core::VarId, Ty>,
) {
    if let beck_core::core::CoreKind::Var(v) = &c.kind {
        if want.contains(v) {
            out.entry(*v).or_insert_with(|| c.ty.clone());
        }
    }
    for child in beck_core::core::children(c) {
        var_types(child, want, out);
    }
}

/// One expression: its literals, its layouts and its lambdas.
///
/// `def` is the name of the definition whose *outermost* `lam` this is, and `None` everywhere else.
/// The distinction is the one thing this walk knows that a walk for literals would not have to: a
/// definition's own lambda is the compiled function, and a `lam` under anything is a closure that
/// has to be built — which is what decides whether a program needs an arena.
fn walk(
    c: &beck_core::Core,
    def: Option<&Arc<str>>,
    program: &Program,
    heap: &mut Heap,
    lams: &mut BTreeMap<(Vec<beck_core::core::VarId>, u32), Lambda>,
) {
    use beck_core::core::{Const, CoreKind};
    let shape = heap.repr(&c.ty, program);
    match &c.kind {
        // Interned here rather than where a body is emitted, so the pool is the same table in both
        // emitters and in the host: a literal in a definition that turns out not to compile still
        // takes its place, exactly as a layout's index does.
        CoreKind::Const(Const::Str(s)) => {
            heap.intern(s);
        }
        CoreKind::Const(_) | CoreKind::Var(_) => {}
        // A definition named where a value is expected. The closure it evaluates to carries nothing
        // and its arm calls the definition, but it is still an object in the arena — so this is one
        // of the two things that makes a program of arithmetic need a heap.
        CoreKind::Global(_) => heap.closures = true,
        CoreKind::Lam { params, body } => {
            let key = (params.to_vec(), body.span.start);
            let mut free = std::collections::BTreeSet::new();
            beck_core::core::free_vars(c, &mut std::collections::BTreeSet::new(), &mut free);
            let mut types = BTreeMap::new();
            var_types(c, &free, &mut types);
            lams.entry(key).or_insert_with(|| Lambda {
                params: params.to_vec(),
                span_start: body.span.start,
                family: match shape {
                    Ok(Repr::Fn(at)) => Some(at),
                    _ => None,
                },
                captures: types.into_iter().collect(),
                def: def.cloned(),
            });
            if def.is_none() {
                heap.closures = true;
            }
            walk(body, None, program, heap, lams);
        }
        CoreKind::App { func, args } => {
            // A call to a named definition is a call and not a value: walking `func` here would set
            // `closures` for every call in every program, and the arena would stop being a function
            // of what a program builds.
            if !matches!(func.kind, CoreKind::Global(_)) {
                walk(func, None, program, heap, lams);
            }
            for a in args {
                walk(a, None, program, heap, lams);
            }
        }
        CoreKind::Prim { op, args } => {
            // `str(b)` answers with one of two literals, and a pool the survey did not decide is a
            // pool that depends on the fixed point rather than on the program — which is exactly
            // what `the_literal_pool_is_a_function_of_the_program` exists to catch, and did.
            if *op == beck_core::core::Prim::ToStr
                && args.first().is_some_and(|a| a.ty == Ty::bool_())
            {
                heap.intern("true");
                heap.intern("false");
            }
            for a in args {
                walk(a, None, program, heap, lams);
            }
        }
        CoreKind::Let { value, body, .. } => {
            walk(value, None, program, heap, lams);
            walk(body, None, program, heap, lams);
        }
        CoreKind::If { cond, then, alt } => {
            walk(cond, None, program, heap, lams);
            walk(then, None, program, heap, lams);
            walk(alt, None, program, heap, lams);
        }
        CoreKind::Match { scrutinee, arms } => {
            walk(scrutinee, None, program, heap, lams);
            for arm in arms {
                // A pattern's constants are not expressions and `Arm::exprs` does not reach them,
                // so `case "one":` would otherwise be a literal the pool learned about while a
                // body was being emitted rather than before one was.
                pattern(&arm.pattern, heap);
                for e in arm.exprs() {
                    walk(e, None, program, heap, lams);
                }
            }
        }
        CoreKind::Make { fields, .. } => {
            for (_, f) in fields {
                walk(f, None, program, heap, lams);
            }
        }
        CoreKind::Field { base, .. } => walk(base, None, program, heap, lams),
        CoreKind::With { base, fields } => {
            walk(base, None, program, heap, lams);
            for (_, f) in fields {
                walk(f, None, program, heap, lams);
            }
        }
        CoreKind::ListLit(xs) => {
            for x in xs {
                walk(x, None, program, heap, lams);
            }
        }
        CoreKind::MapLit(kvs) => {
            for (k, v) in kvs {
                walk(k, None, program, heap, lams);
                walk(v, None, program, heap, lams);
            }
        }
    }
}
