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
//! # What has a layout, and what does not
//!
//! `Int`, `Float` and `Bool` are [`Repr`]'s scalars and live in registers as they always have. A
//! `model`, a `union` and a `newtype` — including a recursive one, and including one that takes a
//! type parameter — have a layout. Everything else does not, and [`Heap::repr`] says which by
//! name: a `Str`, a `list`, a `Map`, a closure, `Html` and `Unit` are refused, and a definition
//! that mentions one is left to the evaluator exactly as it was before there was a heap at all.

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
const MAX_DEPTH: usize = 2048;

/// What one value is, at the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Repr {
    Int,
    Float,
    Bool,
    /// An object, carried as its offset into the arena. The index is into [`Heap::layouts`].
    Obj(u32),
}

impl Repr {
    /// The machine type this is carried in. An object is an offset, and an offset is an `i64`.
    pub fn machine(self) -> Scalar {
        match self {
            Repr::Float => Scalar::Float,
            Repr::Bool => Scalar::Bool,
            Repr::Int | Repr::Obj(_) => Scalar::Int,
        }
    }

    pub fn is_obj(self) -> bool {
        matches!(self, Repr::Obj(_))
    }

    /// The LLVM type, which is the machine type's.
    pub fn llvm(self) -> &'static str {
        self.machine().llvm()
    }
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
}

impl Heap {
    pub fn new() -> Heap {
        Heap::default()
    }

    /// Whether anything in this module needs an arena at all.
    ///
    /// A program of pure arithmetic gets the module it always got: no `malloc`, no globals, and no
    /// blob on the wire. That is not an optimisation but the thing that keeps
    /// [`docs/93`](../../../../../docs/93-llvm-backend-report.md) §93.5's numbers about the same
    /// code they were measured on.
    pub fn is_empty(&self) -> bool {
        !self.slots.iter().any(|s| matches!(s, Slot::Done(_)))
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
            Ty::STR => return Err("a `Str`, and text is not on this heap yet".into()),
            Ty::LIST => return Err("a `list`, and a collection is not on this heap yet".into()),
            Ty::MAP => return Err("a `Map`, and a collection is not on this heap yet".into()),
            Ty::UNIT => return Err("the unit value, which has no machine representation".into()),
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
        if want.iter().any(|r| r.is_obj()) {
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
    pub fn decode(&self, cell: u64, r: Repr, blob: &[u8]) -> Result<Value, String> {
        self.decode_at(cell, r, blob, 0)
    }

    fn decode_at(&self, cell: u64, r: Repr, blob: &[u8], depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(format!(
                "the compiled program answered with a value nested more than {MAX_DEPTH} deep"
            ));
        }
        match r {
            Repr::Int => Ok(Value::Int(cell as i64)),
            Repr::Bool => Ok(Value::Bool(cell != 0)),
            // `Value::float` and not `Value::Float`: the constructor applies the order-key
            // transform and the canonicalisation, which is what makes this equal to what the
            // evaluator built.
            Repr::Float => Ok(Value::float(f64::from_bits(cell))),
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
                let mut fields = Vec::with_capacity(variant.fields.len());
                for (i, (name, field)) in variant.fields.iter().enumerate() {
                    let w = word(blob, cell + (i as u64 + 1) * WORD)?;
                    fields.push((name.clone(), self.decode_at(w, *field, blob, depth + 1)?));
                }
                Ok(Value::Data(Arc::new(Record {
                    ty: layout.name.clone(),
                    variant: variant.name.clone(),
                    fields: Fields::from_sorted(fields),
                })))
            }
        }
    }
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

fn walk(c: &beck_core::Core, program: &Program, heap: &mut Heap) {
    use beck_core::core::CoreKind;
    let _ = heap.repr(&c.ty, program);
    match &c.kind {
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
            for e in arms.iter().flat_map(|a| a.exprs()) {
                walk(e, program, heap);
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
