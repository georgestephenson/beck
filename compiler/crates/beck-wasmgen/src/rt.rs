//! The functions a module holds beside the program's own: the arena, text, lists, maps,
//! comparisons and the application of a closure.
//!
//! # Why these are functions and not inline code
//!
//! [`beck_llvm`] writes the same set, for the same reason: a comparison over a layout is recursive
//! in the *type*, so it has to be recursive in the code, and a `list_map` over a closure family is
//! one loop however many call sites reach it. What is different here is only the target — a stack
//! machine with structured control flow and no jumps, so a loop is a `loop` with an explicit `br`
//! back to it and every early exit is a `br` out of an enclosing `block`.
//!
//! # One arena, in linear memory
//!
//! [`adr/0033`](../../../../../docs/adr/0033-the-webassembly-heap-is-the-arena-in-linear-memory.md)
//! is the decision. A value that does not fit in a register is a **byte offset into the module's
//! own linear memory**, laid out by [`beck_llvm::heap`] — the same layout the two native backends
//! read and the host marshals against, so a compiled `view` takes a state the host wrote into the
//! memory and answers a tree the host reads back out of it, with no generated marshalling at
//! either end.
//!
//! Allocation is a bump pointer in an exported global, and the memory **grows** rather than being
//! reserved: 256 MiB of untouched reservation is free on a server and is not free in a browser
//! tab, which is the one place this differs from
//! [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md). Running
//! past [`beck_llvm::heap::ARENA_BYTES`] is [`Trap::HeapExhausted`], which is the same message the
//! native backends give.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use beck_llvm::heap::{self, Heap, Order, Repr};
use beck_llvm::{Signature, Trap};

use crate::binary::{Body, Ins, ValType};
use crate::emit::{HEAP, TRAP, TRAP_PAYLOAD, TRAP_SPAN};

/// How many bytes one WebAssembly page is.
pub const PAGE: u64 = 64 * 1024;

/// The most pages the memory may grow to, which is [`heap::ARENA_BYTES`] exactly — so a program
/// that runs out gets the message the native backends give rather than a different bound.
pub const MAX_PAGES: u32 = (heap::ARENA_BYTES / PAGE) as u32;

/// One function under construction: its shape, its locals, and its instructions.
///
/// Written imperatively rather than as a tree, because that is what the encoder takes and a second
/// representation between the two would be a second thing to keep true.
pub struct Fun {
    pub params: Vec<ValType>,
    pub result: ValType,
    pub locals: Vec<ValType>,
    pub code: Vec<Ins>,
}

impl Fun {
    pub fn new(params: &[ValType], result: ValType) -> Fun {
        Fun {
            params: params.to_vec(),
            result,
            locals: Vec::new(),
            code: Vec::new(),
        }
    }

    /// A fresh local. Never reused: a WebAssembly local costs a slot in a frame, and a reuse
    /// analysis would be an optimisation with a correctness question attached.
    pub fn local(&mut self, ty: ValType) -> u32 {
        self.locals.push(ty);
        (self.params.len() + self.locals.len() - 1) as u32
    }

    pub fn ins(&mut self, i: Ins) {
        self.code.push(i);
    }

    pub fn all(&mut self, xs: impl IntoIterator<Item = Ins>) {
        self.code.extend(xs);
    }

    pub fn get(&mut self, l: u32) {
        self.ins(Ins::LocalGet(l));
    }

    pub fn set(&mut self, l: u32) {
        self.ins(Ins::LocalSet(l));
    }

    pub fn body(self) -> Body {
        Body {
            locals: self.locals,
            code: self.code,
        }
    }

    /// The i32 address of the object at the i64 offset on top of the stack.
    ///
    /// The arena is at most [`heap::ARENA_BYTES`], so the wrap is exact rather than a truncation
    /// anything could reach: an offset that did not fit would have had to come from an allocation
    /// this module refused to make.
    pub fn addr(&mut self) {
        self.ins(Ins::I32WrapI64);
    }

    /// Store the trap and return the function's zero.
    pub fn trap(&mut self, trap: Trap, span: Span) {
        self.all([Ins::I32Const(trap.code() as i32), Ins::GlobalSet(TRAP)]);
        match span {
            Span::Local(l) => self.get(l),
            Span::At(i) => self.ins(Ins::I32Const(i as i32)),
        }
        self.ins(Ins::GlobalSet(TRAP_SPAN));
        self.zero();
        self.ins(Ins::Return);
    }

    /// The zero of this function's result, which is what a trapped computation answers with.
    pub fn zero(&mut self) {
        let z = match self.result {
            ValType::I32 => Ins::I32Const(0),
            ValType::I64 => Ins::I64Const(0),
            ValType::F64 => Ins::F64Const(0.0),
        };
        self.ins(z);
    }

    /// After a call that may have trapped: stop, leaving the reason where the caller put it.
    pub fn checked(&mut self) {
        self.all([Ins::GlobalGet(TRAP), Ins::If(None)]);
        self.zero();
        self.all([Ins::Return, Ins::End]);
    }
}

/// Where a trap's span comes from inside a runtime function: a parameter it was given, or a
/// constant this one supplies.
#[derive(Clone, Copy, Debug)]
pub enum Span {
    Local(u32),
    At(u32),
}

/// One function of the runtime, named by what it does and by the shape it does it for.
///
/// The shape indices are [`Heap`]'s — a list's element table, a map's entry table, a layout, a
/// closure family — so a name here is a name there and the two cannot drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Helper {
    Alloc,
    /// Three-way comparison over the words of one repr, by the repr's place in the element table.
    ElemCmp(u32),
    ObjCmp(u32),
    FnCmp,
    StrAlloc,
    StrCmp,
    StrConcat,
    StrByteOf,
    StrCharAt,
    StrSlice,
    StrWs,
    StrPiece,
    StrTrim,
    StrMatchAt,
    StrFindAt,
    StrRepeat,
    StrSplit,
    StrFromInt,
    StrJoin,
    ListBlock,
    ListHead,
    ListAlloc,
    ListAppend,
    ListCopy,
    ListReverse,
    ListConcat,
    ListCmp(u32),
    ListFind(u32),
    ListSort(u32),
    MapNode,
    MapBalance,
    MapMin,
    MapPop,
    MapNth,
    MapInto,
    MapRun,
    MapFind(u32),
    MapIns(u32),
    MapDel(u32),
    MapMerge(u32),
    MapCmp(u32),
    /// Applying a closure of one family, through that family's table.
    Apply(u32),
    /// The function one `lam` of the program became, by rank.
    Lam(u32),
}

impl Helper {
    /// The name a listing shows. Not a symbol — nothing links these — but the same shape
    /// [`beck_llvm`]'s symbols have, so a reader comparing two backends' artefacts reads one name.
    pub fn name(self) -> String {
        match self {
            Helper::Alloc => "beck.alloc".into(),
            Helper::ElemCmp(at) => format!("beck.elem.cmp.{at}"),
            Helper::ObjCmp(at) => format!("beck.cmp.{at}"),
            Helper::FnCmp => "beck.fn.cmp".into(),
            Helper::StrAlloc => "beck.str.alloc".into(),
            Helper::StrCmp => "beck.str.cmp".into(),
            Helper::StrConcat => "beck.str.concat".into(),
            Helper::StrByteOf => "beck.str.byteof".into(),
            Helper::StrCharAt => "beck.str.charat".into(),
            Helper::StrSlice => "beck.str.slice".into(),
            Helper::StrWs => "beck.str.ws".into(),
            Helper::StrPiece => "beck.str.piece".into(),
            Helper::StrTrim => "beck.str.trim".into(),
            Helper::StrMatchAt => "beck.str.matchat".into(),
            Helper::StrFindAt => "beck.str.findat".into(),
            Helper::StrRepeat => "beck.str.repeat".into(),
            Helper::StrSplit => "beck.str.split".into(),
            Helper::StrFromInt => "beck.str.from_int".into(),
            Helper::StrJoin => "beck.str.join".into(),
            Helper::ListBlock => "beck.list.block".into(),
            Helper::ListHead => "beck.list.head".into(),
            Helper::ListAlloc => "beck.list.alloc".into(),
            Helper::ListAppend => "beck.list.append".into(),
            Helper::ListCopy => "beck.list.copy".into(),
            Helper::ListReverse => "beck.list.reverse".into(),
            Helper::ListConcat => "beck.list.concat".into(),
            Helper::ListCmp(at) => format!("beck.list.cmp.{at}"),
            Helper::ListFind(at) => format!("beck.list.find.{at}"),
            Helper::ListSort(at) => format!("beck.list.sort.{at}"),
            Helper::MapNode => "beck.map.node".into(),
            Helper::MapBalance => "beck.map.balance".into(),
            Helper::MapMin => "beck.map.min".into(),
            Helper::MapPop => "beck.map.pop".into(),
            Helper::MapNth => "beck.map.nth".into(),
            Helper::MapInto => "beck.map.into".into(),
            Helper::MapRun => "beck.map.run".into(),
            Helper::MapFind(at) => format!("beck.map.find.{at}"),
            Helper::MapIns(at) => format!("beck.map.ins.{at}"),
            Helper::MapDel(at) => format!("beck.map.del.{at}"),
            Helper::MapMerge(at) => format!("beck.map.merge.{at}"),
            Helper::MapCmp(at) => format!("beck.map.cmp.{at}"),
            Helper::Apply(at) => format!("beck.apply.{at}"),
            Helper::Lam(rank) => format!("beck.lam.{rank}"),
        }
    }
}

/// Every function of the module, by index, with the runtime's built on demand.
///
/// The program's own definitions are registered first, so a compiled definition's index is its
/// place in the export order and a listing reads in the order a person wrote the program.
pub struct Registry {
    /// `name` per defined function, in index order. The index space's zero is the first *import*,
    /// so what this holds begins where the imports end.
    pub names: Vec<String>,
    by_name: BTreeMap<String, u32>,
    /// The helpers whose bodies are still to be built, in the order they were first asked for.
    queue: Vec<Helper>,
    /// Every helper ever asked for, so one is built once.
    pub wanted: BTreeMap<Helper, u32>,
    base: u32,
}

impl Registry {
    pub fn new(base: u32) -> Registry {
        Registry {
            names: Vec::new(),
            by_name: BTreeMap::new(),
            queue: Vec::new(),
            wanted: BTreeMap::new(),
            base,
        }
    }

    /// The function index a name has, assigned on first mention.
    pub fn index(&mut self, name: &str) -> u32 {
        if let Some(i) = self.by_name.get(name) {
            return *i;
        }
        let i = self.base + self.names.len() as u32;
        self.names.push(name.to_string());
        self.by_name.insert(name.to_string(), i);
        i
    }

    /// The index of a runtime function, queueing its body the first time it is asked for.
    pub fn helper(&mut self, h: Helper) -> u32 {
        if let Some(i) = self.wanted.get(&h) {
            return *i;
        }
        let i = self.index(&h.name());
        self.wanted.insert(h, i);
        self.queue.push(h);
        i
    }

    pub fn take_queue(&mut self) -> Vec<Helper> {
        std::mem::take(&mut self.queue)
    }
}

/// The word offset of a slot, in bytes — what a load's immediate carries.
pub fn at(slot: usize) -> u32 {
    (slot as u64 * heap::WORD) as u32
}

/// The runtime being built: the registry a helper names its callees through, and the layouts.
pub struct Rt<'a> {
    pub reg: &'a mut Registry,
    pub heap: &'a Heap,
    /// Where a `call_indirect`'s type is interned, so an application and the arms it reaches
    /// agree about the shape by construction rather than by two spellings.
    pub types: &'a mut crate::binary::ModuleBuilder,
    /// The ranks that became a `beck.lam.N`, so a family's table holds the arms that exist.
    pub emitted: &'a BTreeSet<u32>,
    /// The definitions that compiled, for the ranks that are a definition's own lambda.
    pub compiled: &'a BTreeMap<Arc<str>, Signature>,
}

/// The comparison function a repr decides through.
///
/// Exhaustive on [`Repr`] rather than on the symbol [`Repr::order`] spells, so that a new
/// reference kind is a compile error *here* as well as there — which is the property §93.8 asks
/// for and the reason `Repr::order` exists at all.
pub fn cmp_helper(r: Repr) -> Helper {
    match r {
        Repr::Str => Helper::StrCmp,
        Repr::List(at) => Helper::ListCmp(at),
        Repr::Map(at) => Helper::MapCmp(at),
        Repr::Obj(at) => Helper::ObjCmp(at),
        Repr::Fn(_) => Helper::FnCmp,
        Repr::Int | Repr::Float | Repr::Bool | Repr::Html | Repr::Attr => {
            unreachable!("{r:?} is compared inline — `Repr::order` is what says so")
        }
    }
}

impl Fun {
    /// The i32 address of a `Str`'s bytes: past its two header words.
    fn str_data(&mut self, s: u32) {
        self.get(s);
        self.addr();
        self.all([Ins::I32Const(heap::STR_HEADER as i32), Ins::I32Add]);
    }

    /// The i32 address of a list's elements: through the block, which is the one load the
    /// header's indirection costs.
    fn data_of(&mut self, xs: u32) {
        self.word(xs, 1);
        self.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::DATA_HEADER as i32),
            Ins::I32Add,
        ]);
    }

    /// One word of the object at the offset in `obj`.
    fn word(&mut self, obj: u32, slot: usize) {
        self.get(obj);
        self.addr();
        self.ins(Ins::I64Load(at(slot)));
    }

    /// `-1`, `0` or `1` for two words already on the stack, by the order `r` decides through.
    ///
    /// Branch-free where it can be: `(a > b) - (a < b)` is two comparisons and a subtraction, and
    /// it is the shape every one of these takes so that a three-way answer is never a nest of
    /// `if`s somebody has to read.
    fn three_way(&mut self, lhs: u32, rhs: u32, signed: bool) {
        self.get(lhs);
        self.get(rhs);
        self.ins(if signed { Ins::I64GtS } else { Ins::I64GtU });
        self.ins(Ins::I64ExtendI32U);
        self.get(lhs);
        self.get(rhs);
        self.ins(if signed { Ins::I64LtS } else { Ins::I64LtU });
        self.ins(Ins::I64ExtendI32U);
        self.ins(Ins::I64Sub);
    }

    /// `beck_core`'s order key for the raw bits of a real, which is what a stored real holds.
    ///
    /// No normalisation: [`crate::emit::Function::store_field`] does it on the way in, so every
    /// real in the arena is already the one the evaluator would hold.
    fn key_of(&mut self, bits: u32) {
        self.get(bits);
        self.all([Ins::I64Const(0), Ins::I64LtS, Ins::If(Some(ValType::I64))]);
        self.get(bits);
        self.all([Ins::I64Const(-1), Ins::I64Xor, Ins::Else]);
        self.get(bits);
        self.all([Ins::I64Const(i64::MIN), Ins::I64Xor, Ins::End]);
    }
}

impl Rt<'_> {
    /// Emit "compare the two i64 locals as values of `r`", leaving `-1`, `0` or `1`.
    fn compare_words(&mut self, f: &mut Fun, r: Repr, a: u32, b: u32) {
        match r.order() {
            Order::Words { signed } => f.three_way(a, b, signed),
            Order::Key => {
                let ka = f.local(ValType::I64);
                let kb = f.local(ValType::I64);
                f.key_of(a);
                f.set(ka);
                f.key_of(b);
                f.set(kb);
                f.three_way(ka, kb, false);
            }
            Order::Call(_) => {
                let index = self.reg.helper(cmp_helper(r));
                f.get(a);
                f.get(b);
                f.ins(Ins::Call(index));
            }
            // Unreachable by construction: `Function::wants` asks `Heap::ordered` before it records
            // a demand, and that walk refuses a record whose field has no order. Answering "equal"
            // is the one answer that cannot make a comparison asymmetric.
            Order::Absent(_) => f.ins(Ins::I64Const(0)),
        }
    }

    /// The body of one runtime function, or `None` for the one kind the emitter builds: a `lam`,
    /// whose body is a program's and not this module's.
    pub fn build(&mut self, h: Helper) -> Option<Fun> {
        Some(match h {
            Helper::Alloc => self.alloc(),
            Helper::ElemCmp(at) => self.elem_cmp(at),
            Helper::ObjCmp(at) => self.obj_cmp(at),
            Helper::FnCmp => self.fn_cmp(),
            Helper::StrAlloc => self.str_alloc(),
            Helper::StrCmp => self.str_cmp(),
            Helper::StrConcat => self.str_concat(),
            Helper::StrByteOf => self.str_byteof(),
            Helper::StrCharAt => self.str_charat(),
            Helper::StrSlice => self.str_slice(),
            Helper::StrWs => self.str_ws(),
            Helper::StrPiece => self.str_piece(),
            Helper::StrTrim => self.str_trim(),
            Helper::StrMatchAt => self.str_matchat(),
            Helper::StrFindAt => self.str_findat(),
            Helper::StrRepeat => self.str_repeat(),
            Helper::StrSplit => self.str_split(),
            Helper::StrFromInt => self.str_from_int(),
            Helper::StrJoin => self.str_join(),
            Helper::ListBlock => self.list_block(),
            Helper::ListHead => self.list_head(),
            Helper::ListAlloc => self.list_alloc(),
            Helper::ListAppend => self.list_append(),
            Helper::ListCopy => self.list_copy(),
            Helper::ListReverse => self.list_reverse(),
            Helper::ListConcat => self.list_concat(),
            Helper::ListCmp(at) => self.list_cmp(at),
            Helper::ListFind(at) => self.list_find(at),
            Helper::ListSort(at) => self.list_sort(at),
            Helper::MapNode => self.map_node(),
            Helper::MapBalance => self.map_balance(),
            Helper::MapMin => self.map_min(),
            Helper::MapPop => self.map_pop(),
            Helper::MapNth => self.map_nth(),
            Helper::MapInto => self.map_into(),
            Helper::MapRun => self.map_run(),
            Helper::MapFind(at) => self.map_find(at),
            Helper::MapIns(at) => self.map_ins(at),
            Helper::MapDel(at) => self.map_del(at),
            Helper::MapMerge(at) => self.map_merge(at),
            Helper::MapCmp(at) => self.map_cmp(at),
            Helper::Apply(at) => self.apply(at),
            // A definition named as a value: the arm is a jump into the definition, because a
            // table has one signature and a compiled definition does not take a closure.
            Helper::Lam(rank) => self.thunk(rank)?,
        })
    }

    // -- the arena ---------------------------------------------------------------------------

    /// `beck.alloc(bytes, span) -> offset`: a bump pointer, and a memory that grows under it.
    ///
    /// Growing rather than reserving is the one place this differs from
    /// [`adr/0026`](../../../../../docs/adr/0026-the-native-heap-is-an-arena-of-offsets.md): a
    /// browser tab pays for the pages a `WebAssembly.Memory` declares, so a module whose program
    /// allocates nothing must hold one page and not four thousand. The **bound** is the same one —
    /// the memory's declared maximum is [`heap::ARENA_BYTES`] exactly, so a program that runs out
    /// gets [`Trap::HeapExhausted`] and the message that names the same number.
    fn alloc(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (bytes, span) = (0, 1);
        let old = f.local(ValType::I64);
        let need = f.local(ValType::I64);
        f.all([Ins::GlobalGet(HEAP), Ins::LocalTee(old)]);
        f.get(bytes);
        f.all([Ins::I64Add, Ins::LocalSet(need)]);

        f.ins(Ins::Block(None));
        f.get(need);
        f.all([
            Ins::MemorySize,
            Ins::I64ExtendI32U,
            Ins::I64Const(16),
            Ins::I64Shl,
            Ins::I64LeU,
            Ins::BrIf(0),
        ]);
        // Whole pages, rounded up, for the shortfall alone.
        f.get(need);
        f.all([
            Ins::MemorySize,
            Ins::I64ExtendI32U,
            Ins::I64Const(16),
            Ins::I64Shl,
            Ins::I64Sub,
            Ins::I64Const((PAGE - 1) as i64),
            Ins::I64Add,
            Ins::I64Const(16),
            Ins::I64ShrU,
            Ins::I32WrapI64,
            Ins::MemoryGrow,
            Ins::I32Const(-1),
            Ins::I32Eq,
            Ins::If(None),
        ]);
        f.trap(Trap::HeapExhausted, Span::Local(span));
        f.all([Ins::End, Ins::End]);

        f.get(need);
        f.ins(Ins::GlobalSet(HEAP));
        f.get(old);
        f
    }

    // -- comparisons -------------------------------------------------------------------------

    /// Two words of one repr, compared. What a list's search, a list's order and a sort key use.
    fn elem_cmp(&mut self, table: u32) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let r = self.heap.element(table);
        self.compare_words(&mut f, r, 0, 1);
        f
    }

    /// A three-way comparison over one layout, and the same answer [`beck_core::Value`]'s derived
    /// `Ord` gives: the tag first, then the fields in name order.
    fn obj_cmp(&mut self, at: u32) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (a, b) = (0, 1);
        let layout = self.heap.layout(at).clone();
        let x = f.local(ValType::I64);
        let y = f.local(ValType::I64);
        let decided = f.local(ValType::I64);

        if layout.tagged {
            f.word(a, 0);
            f.set(x);
            f.word(b, 0);
            f.set(y);
            f.three_way(x, y, false);
            f.all([
                Ins::LocalTee(decided),
                Ins::I64Eqz,
                Ins::I32Eqz,
                Ins::If(None),
            ]);
            f.get(decided);
            f.all([Ins::Return, Ins::End]);
        }

        // One `if` per variant, testing the tag that is already known to be equal. A record has one
        // variant and no test at all.
        let variants = layout.variants.len();
        for (i, variant) in layout.variants.iter().enumerate() {
            if layout.tagged {
                f.word(a, 0);
                f.all([Ins::I64Const(i as i64), Ins::I64Eq, Ins::If(None)]);
            }
            for (slot, (_, repr)) in variant.fields.iter().enumerate() {
                f.word(a, slot + 1);
                f.set(x);
                f.word(b, slot + 1);
                f.set(y);
                self.compare_words(&mut f, *repr, x, y);
                f.all([
                    Ins::LocalTee(decided),
                    Ins::I64Eqz,
                    Ins::I32Eqz,
                    Ins::If(None),
                ]);
                f.get(decided);
                f.all([Ins::Return, Ins::End]);
            }
            if layout.tagged {
                f.all([Ins::I64Const(0), Ins::Return, Ins::End]);
            }
            let _ = variants;
        }
        // Every field agreed — or, for a union, a tag no variant of this layout has, which the host
        // cannot write and this module cannot build. "Equal" is the one answer that cannot make a
        // comparison asymmetric.
        f.ins(Ins::I64Const(0));
        f
    }

    /// Two closures, compared by rank — which is a code position, because that is what
    /// [`beck_core::core::Closure`]'s own `Ord` compares and the captures are not in it.
    fn fn_cmp(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let x = f.local(ValType::I64);
        let y = f.local(ValType::I64);
        f.word(0, 0);
        f.set(x);
        f.word(1, 0);
        f.set(y);
        f.three_way(x, y, false);
        f
    }
}

impl Rt<'_> {
    fn idx(&mut self, h: Helper) -> u32 {
        self.reg.helper(h)
    }

    // -- text ---------------------------------------------------------------------------------

    /// A `Str`: two header words and the bytes, padded to a whole word.
    ///
    /// The padding is **zeroed** rather than left as the arena found it, so that two runs of one
    /// program leave the same bytes behind and a memory read back byte for byte is a fair
    /// comparison.
    fn str_alloc(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (bytes, chars, span) = (0, 1, 2);
        let body = f.local(ValType::I64);
        let off = f.local(ValType::I64);
        f.get(bytes);
        f.all([
            Ins::I64Const(7),
            Ins::I64Add,
            Ins::I64Const(-8),
            Ins::I64And,
            Ins::LocalSet(body),
        ]);
        f.get(body);
        f.all([Ins::I64Const(heap::STR_HEADER as i64), Ins::I64Add]);
        f.get(span);
        let alloc = self.idx(Helper::Alloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(off)]);
        f.checked();
        f.get(off);
        f.addr();
        f.get(bytes);
        f.ins(Ins::I64Store(at(0)));
        f.get(off);
        f.addr();
        f.get(chars);
        f.ins(Ins::I64Store(at(1)));
        f.get(body);
        f.all([Ins::I64Eqz, Ins::I32Eqz, Ins::If(None)]);
        f.get(off);
        f.addr();
        f.get(body);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Add,
            Ins::I64Const(0),
            Ins::I64Store(0),
            Ins::End,
        ]);
        f.get(off);
        f
    }

    /// Two strings, in the order `beck_core::Text` gives: byte by byte, then by length.
    ///
    /// A byte loop rather than a `memcmp`, because WebAssembly has no such instruction — the one
    /// bulk operation it has is `memory.copy`.
    fn str_cmp(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (a, b) = (0, 1);
        let la = f.local(ValType::I64);
        let lb = f.local(ValType::I64);
        let pa = f.local(ValType::I32);
        let pb = f.local(ValType::I32);
        let n = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let x = f.local(ValType::I32);
        let y = f.local(ValType::I32);
        f.word(a, 0);
        f.set(la);
        f.word(b, 0);
        f.set(lb);
        f.str_data(a);
        f.set(pa);
        f.str_data(b);
        f.set(pb);
        f.get(la);
        f.get(lb);
        f.get(la);
        f.get(lb);
        f.all([Ins::I64LtU, Ins::Select, Ins::LocalSet(n)]);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(pa);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::LocalSet(x),
        ]);
        f.get(pb);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::LocalSet(y),
        ]);
        f.get(x);
        f.get(y);
        f.all([
            Ins::I32Ne,
            Ins::If(None),
            Ins::I64Const(-1),
            Ins::I64Const(1),
        ]);
        f.get(x);
        f.get(y);
        f.all([Ins::I32LtU, Ins::Select, Ins::Return, Ins::End]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.three_way(la, lb, false);
        f
    }

    fn str_concat(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (a, b, span) = (0, 1, 2);
        let r = f.local(ValType::I64);
        let la = f.local(ValType::I64);
        let lb = f.local(ValType::I64);
        f.word(a, 0);
        f.set(la);
        f.word(b, 0);
        f.set(lb);
        f.get(la);
        f.get(lb);
        f.ins(Ins::I64Add);
        f.word(a, 1);
        f.word(b, 1);
        f.ins(Ins::I64Add);
        f.get(span);
        let alloc = self.idx(Helper::StrAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.str_data(r);
        f.str_data(a);
        f.get(la);
        f.all([Ins::I32WrapI64, Ins::MemoryCopy]);
        f.str_data(r);
        f.get(la);
        f.ins(Ins::I32WrapI64);
        f.ins(Ins::I32Add);
        f.str_data(b);
        f.get(lb);
        f.all([Ins::I32WrapI64, Ins::MemoryCopy]);
        f.get(r);
        f
    }

    /// Which byte character `i` begins at.
    ///
    /// Every character is one byte exactly when there are as many bytes as characters, so the two
    /// counts the header already carries are the ASCII test and no flag is stored for it.
    fn str_byteof(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (s, i) = (0, 1);
        let len = f.local(ValType::I64);
        let chars = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let at_ = f.local(ValType::I64);
        let seen = f.local(ValType::I64);
        let k = f.local(ValType::I64);
        f.word(s, 0);
        f.set(len);
        f.word(s, 1);
        f.set(chars);
        f.get(i);
        f.get(chars);
        f.all([Ins::I64GeS, Ins::If(None)]);
        f.all([Ins::LocalGet(len), Ins::Return, Ins::End]);
        f.get(i);
        f.all([
            Ins::I64Const(0),
            Ins::I64LeS,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        f.get(len);
        f.get(chars);
        f.all([
            Ins::I64Eq,
            Ins::If(None),
            Ins::LocalGet(i),
            Ins::Return,
            Ins::End,
        ]);
        f.str_data(s);
        f.set(p);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(at_),
            Ins::I64Const(0),
            Ins::LocalSet(seen),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(seen);
        f.get(i);
        f.all([Ins::I64Eq, Ins::BrIf(1)]);
        // One character is its lead byte and every byte after it whose top two bits are `10`.
        f.get(at_);
        f.all([
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(k),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(k);
        f.get(len);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(p);
        f.get(k);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::I32Const(0xc0),
            Ins::I32And,
            Ins::I32Const(0x80),
            Ins::I32Ne,
            Ins::BrIf(1),
        ]);
        f.all([
            Ins::LocalGet(k),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(k),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(k),
            Ins::LocalSet(at_),
            Ins::LocalGet(seen),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(seen),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(at_);
        f
    }

    /// How many characters begin before byte `byte` — the inverse of [`Helper::StrByteOf`], and
    /// the reason it exists is that a search answers in bytes and the language indexes in
    /// characters.
    fn str_charat(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (s, byte) = (0, 1);
        let len = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let k = f.local(ValType::I64);
        let n = f.local(ValType::I64);
        f.word(s, 0);
        f.set(len);
        f.get(len);
        f.word(s, 1);
        f.all([
            Ins::I64Eq,
            Ins::If(None),
            Ins::LocalGet(byte),
            Ins::Return,
            Ins::End,
        ]);
        f.str_data(s);
        f.set(p);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(k),
            Ins::I64Const(0),
            Ins::LocalSet(n),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(k);
        f.get(byte);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(n);
        f.get(p);
        f.get(k);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::I32Const(0xc0),
            Ins::I32And,
            Ins::I32Const(0x80),
            Ins::I32Ne,
            Ins::I64ExtendI32U,
            Ins::I64Add,
            Ins::LocalSet(n),
        ]);
        f.all([
            Ins::LocalGet(k),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(k),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(n);
        f
    }

    /// `str_slice`, clamped exactly where the evaluator clamps.
    ///
    /// A negative index or a negative length is zero, which is what `i64::max(0)` does there; and
    /// `start + len` saturates rather than wrapping, which is its `saturating_add`.
    fn str_slice(&mut self) -> Fun {
        let mut f = Fun::new(
            &[ValType::I64, ValType::I64, ValType::I64, ValType::I32],
            ValType::I64,
        );
        let (s, start, len, span) = (0, 1, 2, 3);
        let chars = f.local(ValType::I64);
        let from = f.local(ValType::I64);
        let take = f.local(ValType::I64);
        let upto = f.local(ValType::I64);
        let a = f.local(ValType::I64);
        // `select` takes its *first* operand when the condition holds.
        let floor = |f: &mut Fun, v: u32, into: u32| {
            f.all([Ins::I64Const(0), Ins::LocalGet(v), Ins::LocalGet(v)]);
            f.all([
                Ins::I64Const(0),
                Ins::I64LtS,
                Ins::Select,
                Ins::LocalSet(into),
            ]);
        };
        let ceiling = |f: &mut Fun, v: u32, cap: u32| {
            f.all([Ins::LocalGet(cap), Ins::LocalGet(v), Ins::LocalGet(v)]);
            f.all([
                Ins::LocalGet(cap),
                Ins::I64GtU,
                Ins::Select,
                Ins::LocalSet(v),
            ]);
        };
        floor(&mut f, start, from);
        floor(&mut f, len, take);
        f.get(from);
        f.get(take);
        f.ins(Ins::I64Add);
        f.all([
            Ins::LocalTee(upto),
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::If(None),
        ]);
        f.all([Ins::I64Const(i64::MAX), Ins::LocalSet(upto), Ins::End]);
        f.word(s, 1);
        f.set(chars);
        ceiling(&mut f, from, chars);
        ceiling(&mut f, upto, chars);
        let byteof = self.idx(Helper::StrByteOf);
        f.get(s);
        f.get(from);
        f.all([Ins::Call(byteof), Ins::LocalSet(a)]);
        let piece = self.idx(Helper::StrPiece);
        f.get(s);
        f.get(a);
        f.get(s);
        f.get(upto);
        f.ins(Ins::Call(byteof));
        f.get(span);
        f.ins(Ins::Call(piece));
        f
    }

    /// The bytes of `s` in `[from, to)`, as a `Str` of its own.
    ///
    /// The character count is the bytes in the range that are not continuations, and the range is
    /// always a whole number of characters because every caller cuts at a boundary a scan stopped
    /// on.
    fn str_piece(&mut self) -> Fun {
        let mut f = Fun::new(
            &[ValType::I64, ValType::I64, ValType::I64, ValType::I32],
            ValType::I64,
        );
        let (s, from, to, span) = (0, 1, 2, 3);
        let p = f.local(ValType::I32);
        let k = f.local(ValType::I64);
        let chars = f.local(ValType::I64);
        let r = f.local(ValType::I64);
        f.str_data(s);
        f.set(p);
        f.all([
            Ins::LocalGet(from),
            Ins::LocalSet(k),
            Ins::I64Const(0),
            Ins::LocalSet(chars),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(k);
        f.get(to);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(chars);
        f.get(p);
        f.get(k);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::I32Const(0xc0),
            Ins::I32And,
            Ins::I32Const(0x80),
            Ins::I32Ne,
            Ins::I64ExtendI32U,
            Ins::I64Add,
            Ins::LocalSet(chars),
        ]);
        f.all([
            Ins::LocalGet(k),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(k),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(to);
        f.get(from);
        f.ins(Ins::I64Sub);
        f.get(chars);
        f.get(span);
        let alloc = self.idx(Helper::StrAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.str_data(r);
        f.str_data(s);
        f.get(from);
        f.all([Ins::I32WrapI64, Ins::I32Add]);
        f.get(to);
        f.get(from);
        f.all([Ins::I64Sub, Ins::I32WrapI64, Ins::MemoryCopy]);
        f.get(r);
        f
    }
}

impl Rt<'_> {
    /// The byte width of the whitespace character beginning at `i`, or `0` if what is there is not
    /// whitespace.
    ///
    /// Every one of `White_Space`'s 25 code points is one, two or three bytes, and no continuation
    /// byte can be `0xC2`, `0xE1`, `0xE2` or `0xE3` — continuations are `0x80..0xBF` — so this may
    /// be asked at *any* byte of well-formed UTF-8 and never answers inside a character. That is
    /// what lets [`Helper::StrTrim`] walk a byte at a time without decoding what it skips.
    fn str_ws(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I32, ValType::I64, ValType::I64], ValType::I64);
        let (p, i, len) = (0, 1, 2);
        let b0 = f.local(ValType::I32);
        let b1 = f.local(ValType::I32);
        let b2 = f.local(ValType::I32);
        let byte = |f: &mut Fun, k: i32| {
            f.get(p);
            f.get(i);
            f.all([
                Ins::I32WrapI64,
                Ins::I32Add,
                Ins::I32Const(k),
                Ins::I32Add,
                Ins::I32Load8U(0),
            ]);
        };
        byte(&mut f, 0);
        f.set(b0);
        // U+0009..U+000D and U+0020.
        f.get(b0);
        f.all([
            Ins::I32Const(9),
            Ins::I32Sub,
            Ins::I32Const(5),
            Ins::I32LeU,
            Ins::If(None),
        ]);
        f.all([Ins::I64Const(1), Ins::Return, Ins::End]);
        f.get(b0);
        f.all([
            Ins::I32Const(32),
            Ins::I32Eq,
            Ins::If(None),
            Ins::I64Const(1),
            Ins::Return,
            Ins::End,
        ]);
        // Everything else is two or three bytes, and needs the room for them.
        f.get(b0);
        f.all([Ins::I32Const(0xc2), Ins::I32Eq, Ins::If(None)]);
        f.get(i);
        f.all([Ins::I64Const(1), Ins::I64Add]);
        f.get(len);
        f.all([Ins::I64LtU, Ins::If(None)]);
        byte(&mut f, 1);
        f.set(b1);
        // U+0085 NEL and U+00A0 NBSP.
        f.get(b1);
        f.all([Ins::I32Const(0x85), Ins::I32Eq]);
        f.get(b1);
        f.all([
            Ins::I32Const(0xa0),
            Ins::I32Eq,
            Ins::I32Or,
            Ins::If(None),
            Ins::I64Const(2),
            Ins::Return,
            Ins::End,
            Ins::End,
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        // The three-byte lead bytes, each with the same room test.
        f.get(b0);
        f.all([Ins::I32Const(0xe1), Ins::I32Eq]);
        f.get(b0);
        f.all([Ins::I32Const(0xe2), Ins::I32Eq, Ins::I32Or]);
        f.get(b0);
        f.all([
            Ins::I32Const(0xe3),
            Ins::I32Eq,
            Ins::I32Or,
            Ins::I32Eqz,
            Ins::If(None),
        ]);
        f.all([Ins::I64Const(0), Ins::Return, Ins::End]);
        f.get(i);
        f.all([Ins::I64Const(2), Ins::I64Add]);
        f.get(len);
        f.all([
            Ins::I64GeU,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        byte(&mut f, 1);
        f.set(b1);
        byte(&mut f, 2);
        f.set(b2);
        // U+1680 OGHAM SPACE MARK.
        f.get(b0);
        f.all([Ins::I32Const(0xe1), Ins::I32Eq]);
        f.get(b1);
        f.all([Ins::I32Const(0x9a), Ins::I32Eq, Ins::I32And]);
        f.get(b2);
        f.all([
            Ins::I32Const(0x80),
            Ins::I32Eq,
            Ins::I32And,
            Ins::If(None),
            Ins::I64Const(3),
            Ins::Return,
            Ins::End,
        ]);
        // U+3000 IDEOGRAPHIC SPACE.
        f.get(b0);
        f.all([Ins::I32Const(0xe3), Ins::I32Eq]);
        f.get(b1);
        f.all([Ins::I32Const(0x80), Ins::I32Eq, Ins::I32And]);
        f.get(b2);
        f.all([
            Ins::I32Const(0x80),
            Ins::I32Eq,
            Ins::I32And,
            Ins::If(None),
            Ins::I64Const(3),
            Ins::Return,
            Ins::End,
        ]);
        // U+2000..U+200A, U+2028, U+2029, U+202F — all `E2 80 xx` — and U+205F, `E2 81 9F`.
        f.get(b0);
        f.all([
            Ins::I32Const(0xe2),
            Ins::I32Eq,
            Ins::I32Eqz,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        f.get(b1);
        f.all([Ins::I32Const(0x80), Ins::I32Eq, Ins::If(None)]);
        f.get(b2);
        f.all([
            Ins::I32Const(0x80),
            Ins::I32Sub,
            Ins::I32Const(10),
            Ins::I32LeU,
        ]);
        f.get(b2);
        f.all([Ins::I32Const(0xa8), Ins::I32Eq, Ins::I32Or]);
        f.get(b2);
        f.all([Ins::I32Const(0xa9), Ins::I32Eq, Ins::I32Or]);
        f.get(b2);
        f.all([
            Ins::I32Const(0xaf),
            Ins::I32Eq,
            Ins::I32Or,
            Ins::If(None),
            Ins::I64Const(3),
            Ins::Return,
            Ins::End,
            Ins::End,
        ]);
        f.get(b1);
        f.all([Ins::I32Const(0x81), Ins::I32Eq]);
        f.get(b2);
        f.all([
            Ins::I32Const(0x9f),
            Ins::I32Eq,
            Ins::I32And,
            Ins::If(None),
            Ins::I64Const(3),
            Ins::Return,
            Ins::End,
        ]);
        f.ins(Ins::I64Const(0));
        f
    }

    /// `str_trim`, in one pass.
    ///
    /// The leading run is skipped whole; then every byte is either the start of a whitespace
    /// character — skipped, and *not* recorded — or one byte of something else, which moves the
    /// end. So `end` finishes one past the last byte of the last non-whitespace character, which is
    /// what `str::trim` answers.
    fn str_trim(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (s, span) = (0, 1);
        let len = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let start = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let end = f.local(ValType::I64);
        let w = f.local(ValType::I64);
        let ws = self.idx(Helper::StrWs);
        f.word(s, 0);
        f.set(len);
        f.str_data(s);
        f.set(p);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(start),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(start);
        f.get(len);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(p);
        f.get(start);
        f.get(len);
        f.all([Ins::Call(ws), Ins::LocalTee(w), Ins::I64Eqz, Ins::BrIf(1)]);
        f.get(start);
        f.get(w);
        f.all([
            Ins::I64Add,
            Ins::LocalSet(start),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(start),
            Ins::LocalSet(i),
            Ins::LocalGet(start),
            Ins::LocalSet(end),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(len);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(p);
        f.get(i);
        f.get(len);
        f.all([Ins::Call(ws), Ins::LocalTee(w), Ins::I64Eqz, Ins::If(None)]);
        f.get(i);
        f.all([
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalTee(i),
            Ins::LocalSet(end),
            Ins::Else,
        ]);
        f.get(i);
        f.get(w);
        f.all([
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::End,
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        let piece = self.idx(Helper::StrPiece);
        f.get(s);
        f.get(start);
        f.get(end);
        f.get(span);
        f.ins(Ins::Call(piece));
        f
    }

    /// Whether the needle's bytes occur in the haystack at byte `from`.
    fn str_matchat(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I64], ValType::I64);
        let (h, n, from) = (0, 1, 2);
        let lh = f.local(ValType::I64);
        let ln = f.local(ValType::I64);
        let ph = f.local(ValType::I32);
        let pn = f.local(ValType::I32);
        let i = f.local(ValType::I64);
        f.word(h, 0);
        f.set(lh);
        f.word(n, 0);
        f.set(ln);
        f.get(from);
        f.all([Ins::I64Const(0), Ins::I64LtS]);
        f.get(from);
        f.get(ln);
        f.ins(Ins::I64Add);
        f.get(lh);
        f.all([
            Ins::I64GtU,
            Ins::I32Or,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        f.str_data(h);
        f.get(from);
        f.all([Ins::I32WrapI64, Ins::I32Add, Ins::LocalSet(ph)]);
        f.str_data(n);
        f.set(pn);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(ln);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(ph);
        f.get(i);
        f.all([Ins::I32WrapI64, Ins::I32Add, Ins::I32Load8U(0)]);
        f.get(pn);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::I32Ne,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.ins(Ins::I64Const(1));
        f
    }

    /// The first byte position of `n` in `h` at or after `from`, or `-1`.
    ///
    /// Naive, and correct on UTF-8 for the reason a byte search is: the encoding is
    /// self-synchronising, so a well-formed needle cannot match starting inside a character.
    fn str_findat(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I64], ValType::I64);
        let (h, n, from) = (0, 1, 2);
        let last = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let matchat = self.idx(Helper::StrMatchAt);
        f.word(h, 0);
        f.word(n, 0);
        f.all([
            Ins::I64Sub,
            Ins::LocalTee(last),
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::If(None),
        ]);
        f.all([Ins::I64Const(-1), Ins::Return, Ins::End]);
        f.all([Ins::I64Const(0)]);
        f.get(from);
        f.all([
            Ins::LocalGet(from),
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::Select,
            Ins::LocalSet(i),
        ]);
        f.all([Ins::Block(None), Ins::Loop(None)]);
        f.get(i);
        f.get(last);
        f.all([Ins::I64GtS, Ins::BrIf(1)]);
        f.get(h);
        f.get(n);
        f.get(i);
        f.all([
            Ins::Call(matchat),
            Ins::I64Eqz,
            Ins::I32Eqz,
            Ins::If(None),
            Ins::LocalGet(i),
            Ins::Return,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.ins(Ins::I64Const(-1));
        f
    }

    /// `str_repeat`, clamped to a million as the evaluator clamps it — "because `"x" * 10_000_000_000`
    /// is a request nobody makes on purpose".
    fn str_repeat(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (s, n, span) = (0, 1, 2);
        let k = f.local(ValType::I64);
        let lb = f.local(ValType::I64);
        let r = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        f.all([Ins::I64Const(0)]);
        f.get(n);
        f.all([
            Ins::LocalGet(n),
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::Select,
            Ins::LocalSet(k),
        ]);
        f.all([Ins::I64Const(1_000_000)]);
        f.get(k);
        f.all([
            Ins::LocalGet(k),
            Ins::I64Const(1_000_000),
            Ins::I64GtS,
            Ins::Select,
            Ins::LocalSet(k),
        ]);
        f.word(s, 0);
        f.set(lb);
        f.get(lb);
        f.get(k);
        f.ins(Ins::I64Mul);
        f.word(s, 1);
        f.get(k);
        f.ins(Ins::I64Mul);
        f.get(span);
        let alloc = self.idx(Helper::StrAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(k);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.str_data(r);
        f.get(i);
        f.get(lb);
        f.all([Ins::I64Mul, Ins::I32WrapI64, Ins::I32Add]);
        f.str_data(s);
        f.get(lb);
        f.all([Ins::I32WrapI64, Ins::MemoryCopy]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(r);
        f
    }

    /// `str(n)` for an `Int`, which is Rust's `i64::to_string` to the digit.
    ///
    /// An integer's decimal *is* reproducible — the one that is not is a real's, whose shortest
    /// round-trip form is a whole algorithm — which is why this compiles and `str` of a `Float`
    /// does not.
    fn str_from_int(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (n, span) = (0, 1);
        let u = f.local(ValType::I64);
        let d = f.local(ValType::I64);
        let t = f.local(ValType::I64);
        let bytes = f.local(ValType::I64);
        let r = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let i = f.local(ValType::I64);
        let neg = f.local(ValType::I32);
        f.get(n);
        f.all([Ins::I64Const(0), Ins::I64LtS, Ins::LocalSet(neg)]);
        // `0 - i64::MIN` wraps to 2^63, which read as unsigned is exactly its magnitude — the one
        // input where negating in signed arithmetic has no answer.
        f.all([Ins::I64Const(0)]);
        f.get(n);
        f.ins(Ins::I64Sub);
        f.get(n);
        f.all([Ins::LocalGet(neg), Ins::Select, Ins::LocalSet(u)]);
        f.all([
            Ins::I64Const(1),
            Ins::LocalSet(d),
            Ins::LocalGet(u),
            Ins::LocalSet(t),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(t);
        f.all([
            Ins::I64Const(10),
            Ins::I64DivU,
            Ins::LocalTee(t),
            Ins::I64Eqz,
            Ins::BrIf(1),
        ]);
        f.all([
            Ins::LocalGet(d),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(d),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(d);
        f.get(neg);
        f.all([Ins::I64ExtendI32U, Ins::I64Add, Ins::LocalTee(bytes)]);
        // Every byte is a digit or a minus, so the character count is the byte count.
        f.get(bytes);
        f.get(span);
        let alloc = self.idx(Helper::StrAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.str_data(r);
        f.set(p);
        f.get(neg);
        f.ins(Ins::If(None));
        f.get(p);
        f.all([Ins::I32Const(45), Ins::I32Store8(0), Ins::End]);
        // Backwards from the last byte, which is the order division produces them in.
        f.all([
            Ins::LocalGet(bytes),
            Ins::LocalSet(i),
            Ins::LocalGet(u),
            Ins::LocalSet(t),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.all([Ins::I64Const(1), Ins::I64Sub, Ins::LocalSet(i)]);
        f.get(p);
        f.get(i);
        f.all([Ins::I32WrapI64, Ins::I32Add]);
        f.get(t);
        f.all([
            Ins::I64Const(10),
            Ins::I64RemU,
            Ins::I32WrapI64,
            Ins::I32Const(48),
            Ins::I32Add,
            Ins::I32Store8(0),
        ]);
        f.get(t);
        f.all([
            Ins::I64Const(10),
            Ins::I64DivU,
            Ins::LocalTee(t),
            Ins::I64Eqz,
            Ins::I32Eqz,
            Ins::BrIf(0),
            Ins::End,
        ]);
        f.get(r);
        f
    }
}

impl Rt<'_> {
    /// A list's data block: `[cap, used, …]`.
    fn list_block(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (cap, used, span) = (0, 1, 2);
        let off = f.local(ValType::I64);
        f.get(cap);
        f.all([
            Ins::I64Const(heap::WORD as i64),
            Ins::I64Mul,
            Ins::I64Const(heap::DATA_HEADER as i64),
            Ins::I64Add,
        ]);
        f.get(span);
        let alloc = self.idx(Helper::Alloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(off)]);
        f.checked();
        f.get(off);
        f.addr();
        f.get(cap);
        f.ins(Ins::I64Store(at(0)));
        f.get(off);
        f.addr();
        f.get(used);
        f.ins(Ins::I64Store(at(1)));
        f.get(off);
        f
    }

    /// A list's header: how many elements it has, and where they are.
    fn list_head(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (n, data, span) = (0, 1, 2);
        let off = f.local(ValType::I64);
        f.all([Ins::I64Const(heap::LIST_HEADER as i64)]);
        f.get(span);
        let alloc = self.idx(Helper::Alloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(off)]);
        f.checked();
        f.get(off);
        f.addr();
        f.get(n);
        f.ins(Ins::I64Store(at(0)));
        f.get(off);
        f.addr();
        f.get(data);
        f.ins(Ins::I64Store(at(1)));
        f.get(off);
        f
    }

    fn list_alloc(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (n, span) = (0, 1);
        let data = f.local(ValType::I64);
        f.get(n);
        f.get(n);
        f.get(span);
        let block = self.idx(Helper::ListBlock);
        f.all([Ins::Call(block), Ins::LocalSet(data)]);
        f.checked();
        f.get(n);
        f.get(data);
        f.get(span);
        let head = self.idx(Helper::ListHead);
        f.ins(Ins::Call(head));
        f
    }

    /// `list_append` — a new header over the same block when the block has room and this list is
    /// the one standing at its end, and a doubled copy otherwise.
    ///
    /// The test is `count == used`, and it is the whole of what makes this sound: every header over
    /// a block has a count of at most `used`, so the slot at `used` is one no reader can see.
    /// Writing it and answering a *new* header leaves every existing list exactly as it was — no
    /// ownership analysis, no reference count, and no way for two holders to disagree.
    fn list_append(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (xs, w, span) = (0, 1, 2);
        let n = f.local(ValType::I64);
        let data = f.local(ValType::I64);
        let block = f.local(ValType::I32);
        let len = f.local(ValType::I64);
        f.word(xs, 0);
        f.set(n);
        f.word(xs, 1);
        f.all([Ins::LocalTee(data), Ins::I32WrapI64, Ins::LocalSet(block)]);
        // `count == used` and `used < cap`.
        f.get(n);
        f.get(block);
        f.ins(Ins::I64Load(at(1)));
        f.ins(Ins::I64Eq);
        f.get(block);
        f.ins(Ins::I64Load(at(1)));
        f.get(block);
        f.all([Ins::I64Load(at(0)), Ins::I64LtU, Ins::I32And, Ins::If(None)]);
        f.get(block);
        f.get(n);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I32Const(heap::DATA_HEADER as i32),
            Ins::I32Add,
        ]);
        f.get(w);
        f.ins(Ins::I64Store(0));
        f.get(block);
        f.get(n);
        f.all([
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalTee(len),
            Ins::I64Store(at(1)),
            Ins::Else,
        ]);
        // Doubled, so the copies over a whole accumulator sum to a constant per element.
        let make = self.idx(Helper::ListBlock);
        f.all([Ins::I64Const(4)]);
        f.get(n);
        f.all([
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalTee(len),
            Ins::I64Const(2),
            Ins::I64Mul,
        ]);
        f.get(len);
        f.all([
            Ins::I64Const(2),
            Ins::I64Mul,
            Ins::I64Const(4),
            Ins::I64LtU,
            Ins::Select,
        ]);
        f.get(len);
        f.get(span);
        f.all([Ins::Call(make), Ins::LocalSet(data)]);
        f.checked();
        f.get(data);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::DATA_HEADER as i32),
            Ins::I32Add,
        ]);
        f.get(block);
        f.all([Ins::I32Const(heap::DATA_HEADER as i32), Ins::I32Add]);
        f.get(n);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::MemoryCopy,
        ]);
        f.get(data);
        f.ins(Ins::I32WrapI64);
        f.get(n);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.get(w);
        f.all([Ins::I64Store(heap::DATA_HEADER as u32), Ins::End]);
        f.get(len);
        f.get(data);
        f.get(span);
        let head = self.idx(Helper::ListHead);
        f.ins(Ins::Call(head));
        f
    }

    fn list_copy(&mut self) -> Fun {
        let mut f = Fun::new(
            &[ValType::I64, ValType::I64, ValType::I64, ValType::I32],
            ValType::I64,
        );
        let (xs, from, count, span) = (0, 1, 2, 3);
        let r = f.local(ValType::I64);
        f.get(count);
        f.get(span);
        let alloc = self.idx(Helper::ListAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.data_of(r);
        f.data_of(xs);
        f.get(from);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.get(count);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::MemoryCopy,
        ]);
        f.get(r);
        f
    }

    fn list_reverse(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (xs, span) = (0, 1);
        let n = f.local(ValType::I64);
        let r = f.local(ValType::I64);
        let src = f.local(ValType::I32);
        let dst = f.local(ValType::I32);
        let i = f.local(ValType::I64);
        f.word(xs, 0);
        f.set(n);
        f.get(n);
        f.get(span);
        let alloc = self.idx(Helper::ListAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.data_of(xs);
        f.set(src);
        f.data_of(r);
        f.set(dst);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(dst);
        f.get(n);
        f.get(i);
        f.all([
            Ins::I64Sub,
            Ins::I64Const(1),
            Ins::I64Sub,
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.get(src);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::I64Store(0),
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(r);
        f
    }

    /// A list of lists into one list.
    ///
    /// Not a growth: one pass for the size — every inner list's length is a header word — so the
    /// allocation happens once and after it, and then one `memory.copy` per inner list.
    fn list_concat(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (xss, span) = (0, 1);
        let n = f.local(ValType::I64);
        let outer = f.local(ValType::I32);
        let total = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let k = f.local(ValType::I64);
        let one = f.local(ValType::I64);
        let out = f.local(ValType::I64);
        let dst = f.local(ValType::I32);
        f.word(xss, 0);
        f.set(n);
        f.data_of(xss);
        f.set(outer);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(total),
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(total);
        f.get(outer);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::I32WrapI64,
            Ins::I64Load(at(0)),
            Ins::I64Add,
            Ins::LocalSet(total),
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(total);
        f.get(span);
        let alloc = self.idx(Helper::ListAlloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(out)]);
        f.checked();
        f.data_of(out);
        f.set(dst);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::I64Const(0),
            Ins::LocalSet(k),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(outer);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::LocalSet(one),
        ]);
        f.get(dst);
        f.get(k);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.data_of(one);
        f.word(one, 0);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::MemoryCopy,
        ]);
        f.get(k);
        f.word(one, 0);
        f.all([Ins::I64Add, Ins::LocalSet(k)]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(out);
        f
    }

    /// Two lists in `Vec<Value>`'s order: element by element, and a prefix is less than the whole.
    fn list_cmp(&mut self, table: u32) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (a, b) = (0, 1);
        let la = f.local(ValType::I64);
        let lb = f.local(ValType::I64);
        let pa = f.local(ValType::I32);
        let pb = f.local(ValType::I32);
        let n = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let c = f.local(ValType::I64);
        let cmp = self.idx(Helper::ElemCmp(table));
        f.word(a, 0);
        f.set(la);
        f.word(b, 0);
        f.set(lb);
        f.data_of(a);
        f.set(pa);
        f.data_of(b);
        f.set(pb);
        f.get(la);
        f.get(lb);
        f.get(la);
        f.get(lb);
        f.all([Ins::I64LtU, Ins::Select, Ins::LocalSet(n)]);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(pa);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
        ]);
        f.get(pb);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::Call(cmp),
            Ins::LocalTee(c),
            Ins::I64Eqz,
            Ins::I32Eqz,
            Ins::If(None),
            Ins::LocalGet(c),
            Ins::Return,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.three_way(la, lb, false);
        f
    }

    /// Where a word first occurs in a list, or `-1`.
    fn list_find(&mut self, table: u32) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (xs, w) = (0, 1);
        let n = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let i = f.local(ValType::I64);
        let cmp = self.idx(Helper::ElemCmp(table));
        f.word(xs, 0);
        f.set(n);
        f.data_of(xs);
        f.set(p);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(p);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
        ]);
        f.get(w);
        f.all([
            Ins::Call(cmp),
            Ins::I64Eqz,
            Ins::If(None),
            Ins::LocalGet(i),
            Ins::Return,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.ins(Ins::I64Const(-1));
        f
    }

    /// A stable merge sort over two parallel runs of words: the keys, and the elements they
    /// decorate.
    ///
    /// **Stability is the property that matters**, and it is one `<=`: on equal keys the element
    /// from the left run goes first. The input order is itself deterministic — a `Map`'s values
    /// come out in key order — so a stable sort is what makes the answer total without a second key.
    ///
    /// Recursive, which is the difference between a function with one loop in it and a function
    /// with three nested ones. The depth is `log n` on the engine's own stack.
    fn list_sort(&mut self, table: u32) -> Fun {
        let mut f = Fun::new(
            &[
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I64,
                ValType::I64,
            ],
            ValType::I64,
        );
        let (keys, vals, tk, tv, lo, hi) = (0, 1, 2, 3, 4, 5);
        let mid = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let j = f.local(ValType::I64);
        let k = f.local(ValType::I64);
        let me = self.idx(Helper::ListSort(table));
        let cmp = self.idx(Helper::ElemCmp(table));
        let word = |f: &mut Fun, run: u32, index: u32| {
            f.get(run);
            f.get(index);
            f.all([
                Ins::I32WrapI64,
                Ins::I32Const(heap::WORD as i32),
                Ins::I32Mul,
                Ins::I32Add,
            ]);
        };
        f.get(hi);
        f.get(lo);
        f.all([
            Ins::I64Sub,
            Ins::I64Const(1),
            Ins::I64LeU,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        f.get(lo);
        f.get(hi);
        f.get(lo);
        f.all([
            Ins::I64Sub,
            Ins::I64Const(2),
            Ins::I64DivU,
            Ins::I64Add,
            Ins::LocalSet(mid),
        ]);
        for (a, b) in [(lo, mid), (mid, hi)] {
            f.get(keys);
            f.get(vals);
            f.get(tk);
            f.get(tv);
            f.get(a);
            f.get(b);
            f.all([Ins::Call(me), Ins::Drop]);
        }
        // Merge into the temporaries, then copy back — so the two halves are read while they are
        // still in place.
        f.all([
            Ins::LocalGet(lo),
            Ins::LocalSet(i),
            Ins::LocalGet(mid),
            Ins::LocalSet(j),
            Ins::LocalGet(lo),
            Ins::LocalSet(k),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(k);
        f.get(hi);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        // Which side the next element comes from: the left when it is not exhausted and either the
        // right is, or its key is not greater.
        f.get(i);
        f.get(mid);
        f.ins(Ins::I64LtU);
        f.get(j);
        f.get(hi);
        f.ins(Ins::I64GeU);
        word(&mut f, keys, i);
        f.ins(Ins::I64Load(0));
        word(&mut f, keys, j);
        f.all([
            Ins::I64Load(0),
            Ins::Call(cmp),
            Ins::I64Const(0),
            Ins::I64LeS,
            Ins::I32Or,
            Ins::I32And,
            Ins::If(None),
        ]);
        word(&mut f, tk, k);
        word(&mut f, keys, i);
        f.all([Ins::I64Load(0), Ins::I64Store(0)]);
        word(&mut f, tv, k);
        word(&mut f, vals, i);
        f.all([Ins::I64Load(0), Ins::I64Store(0)]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Else,
        ]);
        word(&mut f, tk, k);
        word(&mut f, keys, j);
        f.all([Ins::I64Load(0), Ins::I64Store(0)]);
        word(&mut f, tv, k);
        word(&mut f, vals, j);
        f.all([Ins::I64Load(0), Ins::I64Store(0)]);
        f.all([
            Ins::LocalGet(j),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(j),
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(k),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(k),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        for (dst, src) in [(keys, tk), (vals, tv)] {
            word(&mut f, dst, lo);
            word(&mut f, src, lo);
            f.get(hi);
            f.get(lo);
            f.all([
                Ins::I64Sub,
                Ins::I32WrapI64,
                Ins::I32Const(heap::WORD as i32),
                Ins::I32Mul,
                Ins::MemoryCopy,
            ]);
        }
        f.ins(Ins::I64Const(0));
        f
    }
}

impl Rt<'_> {
    /// `str_split`, and `str_chars` with it — the evaluator answers characters for an empty
    /// separator, so the two primitives are one function with two ways of cutting.
    ///
    /// Two passes, and the first one exists so the second allocates nothing it has to grow: count
    /// the pieces, take the list, then fill it.
    fn str_split(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (s, sep, span) = (0, 1, 2);
        let len = f.local(ValType::I64);
        let seplen = f.local(ValType::I64);
        let bychar = f.local(ValType::I32);
        let count = f.local(ValType::I64);
        let at_ = f.local(ValType::I64);
        let hit = f.local(ValType::I64);
        let xs = f.local(ValType::I64);
        let dst = f.local(ValType::I32);
        let slot = f.local(ValType::I64);
        let lo = f.local(ValType::I64);
        let upto = f.local(ValType::I64);
        let piece = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let j = f.local(ValType::I64);
        let findat = self.idx(Helper::StrFindAt);
        let cut = self.idx(Helper::StrPiece);
        let alloc = self.idx(Helper::ListAlloc);

        f.word(s, 0);
        f.set(len);
        // `str_chars` passes the offset **0**, which is never a live object, so it costs no
        // literal — and a program that writes `str_split(s, "")` reaches the same path.
        f.get(sep);
        f.ins(Ins::I64Eqz);
        f.all([Ins::I64Const(0), Ins::LocalSet(seplen)]);
        f.get(sep);
        f.all([
            Ins::I64Eqz,
            Ins::If(Some(ValType::I32)),
            Ins::I32Const(1),
            Ins::Else,
        ]);
        f.word(sep, 0);
        f.all([
            Ins::LocalTee(seplen),
            Ins::I64Eqz,
            Ins::End,
            Ins::I32Or,
            Ins::LocalSet(bychar),
        ]);

        f.get(bychar);
        f.ins(Ins::If(Some(ValType::I64)));
        f.word(s, 1);
        f.ins(Ins::Else);
        // One more piece than there are occurrences, which is what `str::split` answers.
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(at_),
            Ins::I64Const(0),
            Ins::LocalSet(count),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(s);
        f.get(sep);
        f.get(at_);
        f.all([
            Ins::Call(findat),
            Ins::LocalTee(hit),
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::BrIf(1),
        ]);
        f.get(hit);
        f.get(seplen);
        f.all([Ins::I64Add, Ins::LocalSet(at_)]);
        f.all([
            Ins::LocalGet(count),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(count),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(count);
        f.all([
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::End,
            Ins::LocalSet(count),
        ]);

        f.get(count);
        f.get(span);
        f.all([Ins::Call(alloc), Ins::LocalSet(xs)]);
        f.checked();
        f.data_of(xs);
        f.set(dst);
        f.all([Ins::I64Const(0), Ins::LocalSet(slot)]);

        f.get(bychar);
        f.ins(Ins::If(None));
        // A character is its lead byte and every continuation after it. Nothing here decodes: a
        // piece is the byte range between two lead bytes.
        f.str_data(s);
        f.set(p);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(lo),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(lo);
        f.get(len);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(lo);
        f.all([
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(j),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(j);
        f.get(len);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(p);
        f.get(j);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Add,
            Ins::I32Load8U(0),
            Ins::I32Const(0xc0),
            Ins::I32And,
            Ins::I32Const(0x80),
            Ins::I32Ne,
            Ins::BrIf(1),
        ]);
        f.all([
            Ins::LocalGet(j),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(j),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(s);
        f.get(lo);
        f.get(j);
        f.get(span);
        f.all([Ins::Call(cut), Ins::LocalSet(piece)]);
        f.checked();
        f.get(dst);
        f.get(slot);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.get(piece);
        f.ins(Ins::I64Store(0));
        f.all([
            Ins::LocalGet(slot),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(slot),
        ]);
        f.all([
            Ins::LocalGet(j),
            Ins::LocalSet(lo),
            Ins::Br(0),
            Ins::End,
            Ins::End,
            Ins::Else,
        ]);

        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(lo),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(s);
        f.get(sep);
        f.get(lo);
        f.all([Ins::Call(findat), Ins::LocalSet(hit)]);
        f.get(len);
        f.get(hit);
        f.get(hit);
        f.all([
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::Select,
            Ins::LocalSet(upto),
        ]);
        f.get(s);
        f.get(lo);
        f.get(upto);
        f.get(span);
        f.all([Ins::Call(cut), Ins::LocalSet(piece)]);
        f.checked();
        f.get(dst);
        f.get(slot);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.get(piece);
        f.ins(Ins::I64Store(0));
        f.get(hit);
        f.all([Ins::I64Const(0), Ins::I64LtS, Ins::BrIf(1)]);
        f.get(hit);
        f.get(seplen);
        f.all([Ins::I64Add, Ins::LocalSet(lo)]);
        f.all([
            Ins::LocalGet(slot),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(slot),
            Ins::Br(0),
            Ins::End,
            Ins::End,
            Ins::End,
        ]);
        f.get(xs);
        f
    }

    fn str_join(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (xs, sep, span) = (0, 1, 2);
        let n = f.local(ValType::I64);
        let p = f.local(ValType::I32);
        let bytes = f.local(ValType::I64);
        let chars = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let one = f.local(ValType::I64);
        let r = f.local(ValType::I64);
        let dst = f.local(ValType::I32);
        let alloc = self.idx(Helper::StrAlloc);
        f.word(xs, 0);
        f.set(n);
        f.data_of(xs);
        f.set(p);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(bytes),
            Ins::I64Const(0),
            Ins::LocalSet(chars),
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(p);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::LocalSet(one),
        ]);
        f.get(bytes);
        f.word(one, 0);
        f.all([Ins::I64Add, Ins::LocalSet(bytes)]);
        f.get(chars);
        f.word(one, 1);
        f.all([Ins::I64Add, Ins::LocalSet(chars)]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        // One separator between each pair, which is `n - 1` of them — and none at all for a list
        // with nothing in it, where `n - 1` would wrap.
        f.all([Ins::I64Const(0)]);
        f.get(n);
        f.all([
            Ins::I64Const(1),
            Ins::I64Sub,
            Ins::LocalGet(n),
            Ins::I64Eqz,
            Ins::Select,
            Ins::LocalSet(i),
        ]);
        f.get(bytes);
        f.get(i);
        f.word(sep, 0);
        f.all([Ins::I64Mul, Ins::I64Add]);
        f.get(chars);
        f.get(i);
        f.word(sep, 1);
        f.all([Ins::I64Mul, Ins::I64Add]);
        f.get(span);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.str_data(r);
        f.set(dst);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(i);
        f.all([Ins::I64Eqz, Ins::I32Eqz, Ins::If(None)]);
        f.get(dst);
        f.str_data(sep);
        f.word(sep, 0);
        f.all([Ins::I32WrapI64, Ins::MemoryCopy]);
        f.get(dst);
        f.word(sep, 0);
        f.all([Ins::I32WrapI64, Ins::I32Add, Ins::LocalSet(dst), Ins::End]);
        f.get(p);
        f.get(i);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::LocalSet(one),
        ]);
        f.get(dst);
        f.str_data(one);
        f.word(one, 0);
        f.all([Ins::I32WrapI64, Ins::MemoryCopy]);
        f.get(dst);
        f.word(one, 0);
        f.all([Ins::I32WrapI64, Ins::I32Add, Ins::LocalSet(dst)]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(r);
        f
    }
}

/// A `Map` is a weight-balanced tree; [`beck_llvm::heap::MAP_NODE`] is the shape and the argument
/// for it. An empty map is the offset `0`, which is the one offset no live object has.
///
/// Everything here but the three that compare a key is one function for the whole module:
/// rebalancing moves *words* — sizes, keys, values and two children — and never looks at what a key
/// is.
impl Rt<'_> {
    /// How many entries a subtree holds.
    ///
    /// The word at offset `0` is the one [`heap::FIRST`] reserves and nothing ever writes, so it is
    /// zero — which makes the empty map's size a load rather than a branch.
    fn size(f: &mut Fun, n: u32) {
        f.word(n, 0);
    }

    fn map_node(&mut self) -> Fun {
        let mut f = Fun::new(
            &[
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I32,
            ],
            ValType::I64,
        );
        let (k, v, l, r, span) = (0, 1, 2, 3, 4);
        let off = f.local(ValType::I64);
        f.all([Ins::I64Const(heap::MAP_NODE as i64)]);
        f.get(span);
        let alloc = self.idx(Helper::Alloc);
        f.all([Ins::Call(alloc), Ins::LocalSet(off)]);
        f.checked();
        f.get(off);
        f.addr();
        Rt::size(&mut f, l);
        Rt::size(&mut f, r);
        f.all([
            Ins::I64Add,
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::I64Store(at(0)),
        ]);
        for (slot, from) in [
            (heap::NODE_KEY, k),
            (heap::NODE_VALUE, v),
            (heap::NODE_LEFT, l),
            (heap::NODE_RIGHT, r),
        ] {
            f.get(off);
            f.addr();
            f.get(from);
            f.ins(Ins::I64Store(at(slot)));
        }
        f.get(off);
        f
    }

    /// Adams's rebalance, with [`beck_core::pmap`]'s own `DELTA` and `RATIO`. Four cases and no
    /// loop: a subtree that grew by one is at most one rotation away from balanced.
    fn map_balance(&mut self) -> Fun {
        let mut f = Fun::new(
            &[
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I64,
                ValType::I32,
            ],
            ValType::I64,
        );
        let (k, v, l, r, span) = (0, 1, 2, 3, 4);
        let node = self.idx(Helper::MapNode);
        let ls = f.local(ValType::I64);
        let rs = f.local(ValType::I64);
        let inner = f.local(ValType::I64);
        let x = f.local(ValType::I64);
        let y = f.local(ValType::I64);
        Rt::size(&mut f, l);
        f.set(ls);
        Rt::size(&mut f, r);
        f.set(rs);
        let plain = |f: &mut Fun| {
            f.get(k);
            f.get(v);
            f.get(l);
            f.get(r);
            f.get(span);
            f.all([Ins::Call(node), Ins::Return]);
        };
        f.get(ls);
        f.get(rs);
        f.all([Ins::I64Add, Ins::I64Const(1), Ins::I64LeU, Ins::If(None)]);
        plain(&mut f);
        f.ins(Ins::End);

        // The right is too heavy: one rotation left, single or double.
        f.get(rs);
        f.get(ls);
        f.all([
            Ins::I64Const(heap::DELTA as i64),
            Ins::I64Mul,
            Ins::I64GtU,
            Ins::If(None),
        ]);
        f.word(r, heap::NODE_LEFT);
        f.set(x);
        f.word(r, heap::NODE_RIGHT);
        f.set(y);
        Rt::size(&mut f, x);
        Rt::size(&mut f, y);
        f.all([
            Ins::I64Const(heap::RATIO as i64),
            Ins::I64Mul,
            Ins::I64LtU,
            Ins::If(None),
        ]);
        // Single left.
        f.get(k);
        f.get(v);
        f.get(l);
        f.get(x);
        f.get(span);
        f.all([Ins::Call(node), Ins::LocalSet(inner)]);
        f.checked();
        f.word(r, heap::NODE_KEY);
        f.word(r, heap::NODE_VALUE);
        f.get(inner);
        f.get(y);
        f.get(span);
        f.all([Ins::Call(node), Ins::Return, Ins::Else]);
        // Double left: the right child's left child comes to the top.
        f.get(k);
        f.get(v);
        f.get(l);
        f.word(x, heap::NODE_LEFT);
        f.get(span);
        f.all([Ins::Call(node), Ins::LocalSet(inner)]);
        f.checked();
        f.word(r, heap::NODE_KEY);
        f.word(r, heap::NODE_VALUE);
        f.word(x, heap::NODE_RIGHT);
        f.get(y);
        f.get(span);
        f.all([Ins::Call(node), Ins::LocalSet(y)]);
        f.checked();
        f.word(x, heap::NODE_KEY);
        f.word(x, heap::NODE_VALUE);
        f.get(inner);
        f.get(y);
        f.get(span);
        f.all([Ins::Call(node), Ins::Return, Ins::End, Ins::End]);

        // The left is too heavy: the mirror image.
        f.get(ls);
        f.get(rs);
        f.all([
            Ins::I64Const(heap::DELTA as i64),
            Ins::I64Mul,
            Ins::I64GtU,
            Ins::If(None),
        ]);
        f.word(l, heap::NODE_LEFT);
        f.set(x);
        f.word(l, heap::NODE_RIGHT);
        f.set(y);
        Rt::size(&mut f, y);
        Rt::size(&mut f, x);
        f.all([
            Ins::I64Const(heap::RATIO as i64),
            Ins::I64Mul,
            Ins::I64LtU,
            Ins::If(None),
        ]);
        // Single right.
        f.get(k);
        f.get(v);
        f.get(y);
        f.get(r);
        f.get(span);
        f.all([Ins::Call(node), Ins::LocalSet(inner)]);
        f.checked();
        f.word(l, heap::NODE_KEY);
        f.word(l, heap::NODE_VALUE);
        f.get(x);
        f.get(inner);
        f.get(span);
        f.all([Ins::Call(node), Ins::Return, Ins::Else]);
        // Double right.
        f.word(l, heap::NODE_KEY);
        f.word(l, heap::NODE_VALUE);
        f.get(x);
        f.word(y, heap::NODE_LEFT);
        f.get(span);
        f.all([Ins::Call(node), Ins::LocalSet(inner)]);
        f.checked();
        f.get(k);
        f.get(v);
        f.word(y, heap::NODE_RIGHT);
        f.get(r);
        f.get(span);
        f.all([Ins::Call(node), Ins::LocalSet(x)]);
        f.checked();
        f.word(y, heap::NODE_KEY);
        f.word(y, heap::NODE_VALUE);
        f.get(inner);
        f.get(x);
        f.get(span);
        f.all([Ins::Call(node), Ins::Return, Ins::End, Ins::End]);
        plain(&mut f);
        f
    }

    /// The smallest node of a subtree, which is its leftmost.
    fn map_min(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64], ValType::I64);
        let m = 0;
        let n = f.local(ValType::I64);
        f.all([
            Ins::LocalGet(m),
            Ins::LocalSet(n),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.word(n, heap::NODE_LEFT);
        f.ins(Ins::I64Eqz);
        f.ins(Ins::BrIf(1));
        f.word(n, heap::NODE_LEFT);
        f.all([Ins::LocalSet(n), Ins::Br(0), Ins::End, Ins::End]);
        f.get(n);
        f
    }

    /// A subtree with its smallest node taken out, rebalanced on the way back.
    fn map_pop(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I32], ValType::I64);
        let (m, span) = (0, 1);
        let me = self.idx(Helper::MapPop);
        let balance = self.idx(Helper::MapBalance);
        let left = f.local(ValType::I64);
        f.word(m, heap::NODE_LEFT);
        f.all([Ins::LocalTee(left), Ins::I64Eqz, Ins::If(None)]);
        f.word(m, heap::NODE_RIGHT);
        f.all([Ins::Return, Ins::End]);
        f.word(m, heap::NODE_KEY);
        f.word(m, heap::NODE_VALUE);
        f.get(left);
        f.get(span);
        f.ins(Ins::Call(me));
        let rest = f.local(ValType::I64);
        f.set(rest);
        f.checked();
        f.get(rest);
        f.word(m, heap::NODE_RIGHT);
        f.get(span);
        f.ins(Ins::Call(balance));
        f
    }

    /// The `i`th entry in key order, by subtree size.
    fn map_nth(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (m, i) = (0, 1);
        let n = f.local(ValType::I64);
        let want = f.local(ValType::I64);
        let ls = f.local(ValType::I64);
        f.all([
            Ins::LocalGet(m),
            Ins::LocalSet(n),
            Ins::LocalGet(i),
            Ins::LocalSet(want),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(n);
        f.all([Ins::I64Eqz, Ins::BrIf(1)]);
        f.word(n, heap::NODE_LEFT);
        f.ins(Ins::I32WrapI64);
        f.ins(Ins::I64Load(at(0)));
        f.set(ls);
        f.get(want);
        f.get(ls);
        f.all([
            Ins::I64Eq,
            Ins::If(None),
            Ins::LocalGet(n),
            Ins::Return,
            Ins::End,
        ]);
        f.get(want);
        f.get(ls);
        f.ins(Ins::I64LtU);
        f.ins(Ins::If(None));
        f.word(n, heap::NODE_LEFT);
        f.all([Ins::LocalSet(n), Ins::Else]);
        f.get(want);
        f.get(ls);
        f.all([
            Ins::I64Sub,
            Ins::I64Const(1),
            Ins::I64Sub,
            Ins::LocalSet(want),
        ]);
        f.word(n, heap::NODE_RIGHT);
        f.all([Ins::LocalSet(n), Ins::End, Ins::Br(0), Ins::End, Ins::End]);
        f.ins(Ins::I64Const(0));
        f
    }

    /// The in-order walk that fills a run. One function for keys and values, told which word to
    /// take — the two differ by eight bytes and nothing else.
    fn map_into(&mut self) -> Fun {
        let mut f = Fun::new(
            &[ValType::I64, ValType::I32, ValType::I64, ValType::I64],
            ValType::I64,
        );
        let (n, dst, i, slot) = (0, 1, 2, 3);
        let me = self.idx(Helper::MapInto);
        let k = f.local(ValType::I64);
        f.get(n);
        f.all([
            Ins::I64Eqz,
            Ins::If(None),
            Ins::LocalGet(i),
            Ins::Return,
            Ins::End,
        ]);
        f.word(n, heap::NODE_LEFT);
        f.get(dst);
        f.get(i);
        f.get(slot);
        f.all([Ins::Call(me), Ins::LocalSet(k)]);
        f.get(dst);
        f.get(k);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
        ]);
        f.get(n);
        f.ins(Ins::I32WrapI64);
        f.get(slot);
        f.all([
            Ins::I32WrapI64,
            Ins::I32Const(heap::WORD as i32),
            Ins::I32Mul,
            Ins::I32Add,
            Ins::I64Load(0),
            Ins::I64Store(0),
        ]);
        f.word(n, heap::NODE_RIGHT);
        f.get(dst);
        f.get(k);
        f.all([Ins::I64Const(1), Ins::I64Add]);
        f.get(slot);
        f.ins(Ins::Call(me));
        f
    }

    /// `map_keys` and `map_values`: a fresh list of the map's size, filled by the walk above.
    fn map_run(&mut self) -> Fun {
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (m, slot, span) = (0, 1, 2);
        let alloc = self.idx(Helper::ListAlloc);
        let into = self.idx(Helper::MapInto);
        let r = f.local(ValType::I64);
        Rt::size(&mut f, m);
        f.get(span);
        f.all([Ins::Call(alloc), Ins::LocalSet(r)]);
        f.checked();
        f.get(m);
        f.data_of(r);
        f.all([Ins::I64Const(0)]);
        f.get(slot);
        f.all([Ins::Call(into), Ins::Drop]);
        f.get(r);
        f
    }

    /// The search: down the tree, comparing keys. Answers the *node*, or `0` — a lookup and a
    /// containment test are the same walk, and so is the value, which is a word off the node.
    fn map_find(&mut self, table: u32) -> Fun {
        let (key, _) = self.heap.entry(table);
        let cmp = self.idx(Helper::ElemCmp(key));
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (m, k) = (0, 1);
        let n = f.local(ValType::I64);
        let c = f.local(ValType::I64);
        f.all([
            Ins::LocalGet(m),
            Ins::LocalSet(n),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(n);
        f.all([Ins::I64Eqz, Ins::BrIf(1)]);
        f.get(k);
        f.word(n, heap::NODE_KEY);
        f.all([
            Ins::Call(cmp),
            Ins::LocalTee(c),
            Ins::I64Eqz,
            Ins::If(None),
            Ins::LocalGet(n),
            Ins::Return,
            Ins::End,
        ]);
        f.word(n, heap::NODE_LEFT);
        f.word(n, heap::NODE_RIGHT);
        f.get(c);
        f.all([
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::Select,
            Ins::LocalSet(n),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.ins(Ins::I64Const(0));
        f
    }

    /// `map_insert`: rebuild the path, share everything off it, rebalance on the way out.
    /// `O(log n)` fresh nodes, which is [`beck_core::pmap`]'s own cost.
    fn map_ins(&mut self, table: u32) -> Fun {
        let (key, _) = self.heap.entry(table);
        let cmp = self.idx(Helper::ElemCmp(key));
        let node = self.idx(Helper::MapNode);
        let balance = self.idx(Helper::MapBalance);
        let me = self.idx(Helper::MapIns(table));
        let mut f = Fun::new(
            &[ValType::I64, ValType::I64, ValType::I64, ValType::I32],
            ValType::I64,
        );
        let (m, k, v, span) = (0, 1, 2, 3);
        let c = f.local(ValType::I64);
        let sub = f.local(ValType::I64);
        f.get(m);
        f.ins(Ins::I64Eqz);
        f.ins(Ins::If(None));
        f.get(k);
        f.get(v);
        f.all([Ins::I64Const(0), Ins::I64Const(0)]);
        f.get(span);
        f.all([Ins::Call(node), Ins::Return, Ins::End]);
        f.get(k);
        f.word(m, heap::NODE_KEY);
        f.all([Ins::Call(cmp), Ins::LocalTee(c), Ins::I64Eqz, Ins::If(None)]);
        // The *new* key as well as the new value, which is what the evaluator's `Ordering::Equal`
        // arm does — two keys that compare equal need not be the same value.
        f.get(k);
        f.get(v);
        f.word(m, heap::NODE_LEFT);
        f.word(m, heap::NODE_RIGHT);
        f.get(span);
        f.all([Ins::Call(node), Ins::Return, Ins::End]);
        f.get(c);
        f.all([Ins::I64Const(0), Ins::I64LtS, Ins::If(Some(ValType::I64))]);
        f.word(m, heap::NODE_LEFT);
        f.ins(Ins::Else);
        f.word(m, heap::NODE_RIGHT);
        f.ins(Ins::End);
        f.get(k);
        f.get(v);
        f.get(span);
        f.all([Ins::Call(me), Ins::LocalSet(sub)]);
        f.checked();
        f.word(m, heap::NODE_KEY);
        f.word(m, heap::NODE_VALUE);
        // The rebuilt child goes on the side the walk went down, and the untouched one stays —
        // through locals, because a block that answered with *two* values would need a block type
        // this encoder does not write.
        let (lhs, rhs) = (f.local(ValType::I64), f.local(ValType::I64));
        f.get(c);
        f.all([
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::If(None),
            Ins::LocalGet(sub),
            Ins::LocalSet(lhs),
        ]);
        f.word(m, heap::NODE_RIGHT);
        f.all([Ins::LocalSet(rhs), Ins::Else]);
        f.word(m, heap::NODE_LEFT);
        f.all([
            Ins::LocalSet(lhs),
            Ins::LocalGet(sub),
            Ins::LocalSet(rhs),
            Ins::End,
        ]);
        f.get(lhs);
        f.get(rhs);
        f.get(span);
        f.ins(Ins::Call(balance));
        f
    }

    /// `map_remove`: the same path rebuild as an insert. A node with two children is replaced by
    /// the smallest of its right subtree — the textbook deletion, with the rebalance the weights
    /// need.
    fn map_del(&mut self, table: u32) -> Fun {
        let (key, _) = self.heap.entry(table);
        let cmp = self.idx(Helper::ElemCmp(key));
        let balance = self.idx(Helper::MapBalance);
        let min = self.idx(Helper::MapMin);
        let pop = self.idx(Helper::MapPop);
        let me = self.idx(Helper::MapDel(table));
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (m, k, span) = (0, 1, 2);
        let c = f.local(ValType::I64);
        let sub = f.local(ValType::I64);
        let least = f.local(ValType::I64);
        f.get(m);
        f.all([
            Ins::I64Eqz,
            Ins::If(None),
            Ins::I64Const(0),
            Ins::Return,
            Ins::End,
        ]);
        f.get(k);
        f.word(m, heap::NODE_KEY);
        f.all([Ins::Call(cmp), Ins::LocalTee(c), Ins::I64Eqz, Ins::If(None)]);
        // Here. One child, or none, lifts; two children join through the right's leftmost.
        f.word(m, heap::NODE_LEFT);
        f.all([Ins::I64Eqz, Ins::If(None)]);
        f.word(m, heap::NODE_RIGHT);
        f.all([Ins::Return, Ins::End]);
        f.word(m, heap::NODE_RIGHT);
        f.all([Ins::I64Eqz, Ins::If(None)]);
        f.word(m, heap::NODE_LEFT);
        f.all([Ins::Return, Ins::End]);
        f.word(m, heap::NODE_RIGHT);
        f.all([Ins::Call(min), Ins::LocalSet(least)]);
        f.word(m, heap::NODE_RIGHT);
        f.get(span);
        f.all([Ins::Call(pop), Ins::LocalSet(sub)]);
        f.checked();
        f.word(least, heap::NODE_KEY);
        f.word(least, heap::NODE_VALUE);
        f.word(m, heap::NODE_LEFT);
        f.get(sub);
        f.get(span);
        f.all([Ins::Call(balance), Ins::Return, Ins::End]);
        f.get(c);
        f.all([Ins::I64Const(0), Ins::I64LtS, Ins::If(Some(ValType::I64))]);
        f.word(m, heap::NODE_LEFT);
        f.ins(Ins::Else);
        f.word(m, heap::NODE_RIGHT);
        f.ins(Ins::End);
        f.get(k);
        f.get(span);
        f.all([Ins::Call(me), Ins::LocalSet(sub)]);
        f.checked();
        f.word(m, heap::NODE_KEY);
        f.word(m, heap::NODE_VALUE);
        let (lhs, rhs) = (f.local(ValType::I64), f.local(ValType::I64));
        f.get(c);
        f.all([
            Ins::I64Const(0),
            Ins::I64LtS,
            Ins::If(None),
            Ins::LocalGet(sub),
            Ins::LocalSet(lhs),
        ]);
        f.word(m, heap::NODE_RIGHT);
        f.all([Ins::LocalSet(rhs), Ins::Else]);
        f.word(m, heap::NODE_LEFT);
        f.all([
            Ins::LocalSet(lhs),
            Ins::LocalGet(sub),
            Ins::LocalSet(rhs),
            Ins::End,
        ]);
        f.get(lhs);
        f.get(rhs);
        f.get(span);
        f.ins(Ins::Call(balance));
        f
    }

    /// `map_merge`: every entry of the second map inserted into the first, in key order, so the
    /// later map wins — which is what the evaluator's own merge does.
    fn map_merge(&mut self, table: u32) -> Fun {
        let nth = self.idx(Helper::MapNth);
        let ins = self.idx(Helper::MapIns(table));
        let mut f = Fun::new(&[ValType::I64, ValType::I64, ValType::I32], ValType::I64);
        let (a, b, span) = (0, 1, 2);
        let n = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let acc = f.local(ValType::I64);
        let node = f.local(ValType::I64);
        Rt::size(&mut f, b);
        f.set(n);
        f.all([
            Ins::LocalGet(a),
            Ins::LocalSet(acc),
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(b);
        f.get(i);
        f.all([Ins::Call(nth), Ins::LocalSet(node)]);
        f.get(acc);
        f.word(node, heap::NODE_KEY);
        f.word(node, heap::NODE_VALUE);
        f.get(span);
        f.all([Ins::Call(ins), Ins::LocalSet(acc)]);
        f.checked();
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        f.get(acc);
        f
    }

    /// Two maps in key order: the keys, then the values, entry by entry, then the sizes. The order
    /// is `beck_core`'s own — what a `PMap` iterates is what this walks.
    fn map_cmp(&mut self, table: u32) -> Fun {
        let (key, value) = self.heap.entry(table);
        let kcmp = self.idx(Helper::ElemCmp(key));
        let vcmp = self.idx(Helper::ElemCmp(value));
        let nth = self.idx(Helper::MapNth);
        let mut f = Fun::new(&[ValType::I64, ValType::I64], ValType::I64);
        let (a, b) = (0, 1);
        let la = f.local(ValType::I64);
        let lb = f.local(ValType::I64);
        let n = f.local(ValType::I64);
        let i = f.local(ValType::I64);
        let na = f.local(ValType::I64);
        let nb = f.local(ValType::I64);
        let c = f.local(ValType::I64);
        Rt::size(&mut f, a);
        f.set(la);
        Rt::size(&mut f, b);
        f.set(lb);
        f.get(la);
        f.get(lb);
        f.get(la);
        f.get(lb);
        f.all([Ins::I64LtU, Ins::Select, Ins::LocalSet(n)]);
        f.all([
            Ins::I64Const(0),
            Ins::LocalSet(i),
            Ins::Block(None),
            Ins::Loop(None),
        ]);
        f.get(i);
        f.get(n);
        f.all([Ins::I64GeU, Ins::BrIf(1)]);
        f.get(a);
        f.get(i);
        f.all([Ins::Call(nth), Ins::LocalSet(na)]);
        f.get(b);
        f.get(i);
        f.all([Ins::Call(nth), Ins::LocalSet(nb)]);
        f.word(na, heap::NODE_KEY);
        f.word(nb, heap::NODE_KEY);
        f.all([
            Ins::Call(kcmp),
            Ins::LocalTee(c),
            Ins::I64Eqz,
            Ins::I32Eqz,
            Ins::If(None),
            Ins::LocalGet(c),
            Ins::Return,
            Ins::End,
        ]);
        f.word(na, heap::NODE_VALUE);
        f.word(nb, heap::NODE_VALUE);
        f.all([
            Ins::Call(vcmp),
            Ins::LocalTee(c),
            Ins::I64Eqz,
            Ins::I32Eqz,
            Ins::If(None),
            Ins::LocalGet(c),
            Ins::Return,
            Ins::End,
        ]);
        f.all([
            Ins::LocalGet(i),
            Ins::I64Const(1),
            Ins::I64Add,
            Ins::LocalSet(i),
            Ins::Br(0),
            Ins::End,
            Ins::End,
        ]);
        // Equal as far as both go, so the smaller map is the smaller value.
        f.three_way(la, lb, false);
        f
    }
}

impl Rt<'_> {
    /// Applying a closure of one family: the rank in its first word, through that family's table.
    ///
    /// This is where [`heap::CLOSURE_HEADER`]'s decision is spent, and where this target differs
    /// from the two native ones. There a closure's rank is a **switch** into a direct call per
    /// rank, because their arena crosses a pipe as bytes and may hold no code address. A table
    /// index is not a code address either, so the rank travels unchanged and `call_indirect`
    /// replaces the switch — with the same guarantee on the way out, since the hop is
    /// `return_call_indirect` and therefore a jump.
    ///
    /// A rank the family has no arm for is a **null element**, which
    /// [`Trap::NoSuchLambda`] reports rather than aborting the instance. The span index is past the
    /// end of the table on purpose: there is no source position for a wrong rank, and the host
    /// reads one it cannot find as `Span::NONE`.
    fn apply(&mut self, at: u32) -> Fun {
        let fam = self.heap.family(at).clone();
        let arms: Vec<u32> = fam
            .ranks
            .iter()
            .copied()
            .filter(|r| match &self.heap.lam(*r).def {
                Some(name) => self.compiled.contains_key(name),
                None => self.emitted.contains(r),
            })
            .collect();
        let size = arms.iter().copied().max().map_or(0, |m| m as usize + 1);
        let mut entries = vec![None; size];
        for r in arms {
            let index = self.idx(Helper::Lam(r));
            entries[r as usize] = Some(index);
        }
        let table = self.types.tables.len() as u32;
        self.types.tables.push(crate::binary::Table {
            name: fam.shown.clone(),
            entries,
        });

        let mut shape = vec![ValType::I64];
        shape.extend(fam.params.iter().map(|r| crate::emit::val(*r)));
        let result = crate::emit::val(fam.ret);
        let ty = self.types.ty(shape.clone(), vec![result]);

        let mut f = Fun::new(&shape, result);
        let rank = f.local(ValType::I32);
        f.word(0, 0);
        f.all([
            Ins::I32WrapI64,
            Ins::LocalTee(rank),
            Ins::I32Const(size as i32),
            Ins::I32GeU,
        ]);
        f.all([Ins::If(Some(ValType::I32)), Ins::I32Const(1), Ins::Else]);
        f.all([
            Ins::LocalGet(rank),
            Ins::TableGet(table),
            Ins::RefIsNull,
            Ins::End,
            Ins::If(None),
        ]);
        f.word(0, 0);
        f.ins(Ins::GlobalSet(TRAP_PAYLOAD));
        f.trap(Trap::NoSuchLambda, Span::At(u32::MAX));
        f.ins(Ins::End);
        for i in 0..shape.len() as u32 {
            f.get(i);
        }
        f.all([Ins::LocalGet(rank), Ins::ReturnCallIndirect { ty, table }]);
        f
    }

    /// The arm for a **definition** named as a value: drop the closure and jump to the definition.
    ///
    /// `None` for a real `lam`, whose body is a program's and is emitted by the code generator.
    /// A definition closes over nothing, so there is nothing to read off the closure — but a table
    /// has one signature and a compiled definition does not take one, which is why this exists
    /// where the native backends' switch needed no arm at all.
    fn thunk(&mut self, rank: u32) -> Option<Fun> {
        let lam = self.heap.lam(rank).clone();
        let name = lam.def?;
        let sig = self.compiled.get(&name)?.clone();
        let family = lam.family?;
        let fam = self.heap.family(family).clone();
        let mut shape = vec![ValType::I64];
        shape.extend(fam.params.iter().map(|r| crate::emit::val(*r)));
        let mut f = Fun::new(&shape, crate::emit::val(fam.ret));
        for i in 1..shape.len() as u32 {
            f.get(i);
        }
        f.ins(Ins::ReturnCall(sig.index));
        Some(f)
    }
}
