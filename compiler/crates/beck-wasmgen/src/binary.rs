//! The WebAssembly binary format, written by hand.
//!
//! # Why by hand
//!
//! The same reason [`docs/92`](../../../../../docs/92-supply-chain-and-release-report.md) gives for
//! `beck image` writing an OCI layout in one process: the format is a documented byte string, the
//! encoder is a page of code, and a dependency here would be one more thing the SBOM has to state
//! for a job a `Vec<u8>` does. There is no `wat2wasm` on the path either, which matters more than
//! it sounds — the artefact this crate produces has to be loadable by a browser that installed
//! nothing.
//!
//! # One instruction list, two renderings
//!
//! [`Ins`] is emitted once and encoded twice: to the bytes a runtime loads, and to the text a
//! person reads ([`crate::text`]). That is deliberate rather than convenient —
//! [`docs/92`](../../../../../docs/92-supply-chain-and-release-report.md) §92.2's rule is that a
//! gate reads a *rendering* of the artefact rather than the notes taken while building it, and a
//! listing written independently of the encoder would be a second account of what was emitted.
//!
//! # The heap is a memory, a data segment and a table
//!
//! [`adr/0032`](../../../../../docs/adr/0032-the-webassembly-heap-is-the-arena-in-linear-memory.md)
//! is the decision this encoder carries: one linear memory whose bytes *are*
//! [`beck_llvm::heap`]'s arena, a data segment holding the literal pool, and one `funcref` table
//! per closure family so that applying a closure is a `call_indirect` rather than a switch.

/// A WebAssembly value type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F64,
}

impl ValType {
    pub fn byte(self) -> u8 {
        match self {
            ValType::I32 => 0x7f,
            ValType::I64 => 0x7e,
            ValType::F64 => 0x7c,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            ValType::I32 => "i32",
            ValType::I64 => "i64",
            ValType::F64 => "f64",
        }
    }
}

/// The instructions this backend emits.
///
/// A closed list rather than a general encoder: what is here is what the subset needs, and an
/// opcode nobody emits is an opcode nobody has tested.
///
/// A memory instruction carries a **byte offset** and takes its alignment from its width, because
/// every object in the arena starts on a word ([`beck_llvm::heap::WORD`]) and a `Str`'s bytes are
/// read one at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ins {
    /// `block`/`loop`/`if` with an optional result type, and the `else`/`end` that close them.
    Block(Option<ValType>),
    Loop(Option<ValType>),
    If(Option<ValType>),
    Else,
    End,
    /// `br` and `br_if` to a label `n` levels out — the only way out of a `loop`, which has no
    /// implicit back edge.
    Br(u32),
    BrIf(u32),
    Return,
    Unreachable,
    Call(u32),
    /// The tail-call proposal's `return_call` — WebAssembly 2.0, and §93.4's guarantee.
    ReturnCall(u32),
    /// `call_indirect (type t) (table x)` — how a closure is applied.
    CallIndirect {
        ty: u32,
        table: u32,
    },
    ReturnCallIndirect {
        ty: u32,
        table: u32,
    },
    /// `table.get x`, whose answer is a `funcref` — paired with [`Ins::RefIsNull`] so that a rank
    /// no lambda of this family answers to is a [`beck_llvm::Trap`] and not an aborted instance.
    TableGet(u32),
    RefIsNull,
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    GlobalGet(u32),
    GlobalSet(u32),
    Drop,
    Select,
    I32Const(i32),
    I64Const(i64),
    F64Const(f64),
    I64Load(u32),
    I64Store(u32),
    F64Load(u32),
    F64Store(u32),
    I32Load8U(u32),
    I32Store8(u32),
    MemorySize,
    MemoryGrow,
    MemoryCopy,
    MemoryFill,
    I32Eqz,
    I32Eq,
    I32Ne,
    I32And,
    I32Or,
    I32Add,
    I32Sub,
    I32Mul,
    I32LtU,
    I32LeU,
    I32GtU,
    I32GeU,
    I32WrapI64,
    I64ExtendI32U,
    I64ExtendI32S,
    I64Eqz,
    I64Eq,
    I64Ne,
    I64LtS,
    I64LeS,
    I64GtS,
    I64GeS,
    I64LtU,
    I64LeU,
    I64GtU,
    I64GeU,
    I64Add,
    I64Sub,
    I64Mul,
    I64DivS,
    I64DivU,
    I64RemS,
    I64RemU,
    I64And,
    I64Or,
    I64Xor,
    I64Shl,
    I64ShrU,
    F64Add,
    F64Sub,
    F64Mul,
    F64Div,
    F64Abs,
    F64Neg,
    F64Sqrt,
    F64Eq,
    F64Ne,
    F64ConvertI64S,
    I64TruncSatF64S,
    I64ReinterpretF64,
    F64ReinterpretI64,
}

impl Ins {
    fn encode(self, out: &mut Vec<u8>) {
        match self {
            Ins::Block(t) => {
                out.push(0x02);
                block_type(t, out);
            }
            Ins::Loop(t) => {
                out.push(0x03);
                block_type(t, out);
            }
            Ins::If(t) => {
                out.push(0x04);
                block_type(t, out);
            }
            Ins::Else => out.push(0x05),
            Ins::End => out.push(0x0b),
            Ins::Br(d) => {
                out.push(0x0c);
                uleb(u64::from(d), out);
            }
            Ins::BrIf(d) => {
                out.push(0x0d);
                uleb(u64::from(d), out);
            }
            Ins::Return => out.push(0x0f),
            Ins::Unreachable => out.push(0x00),
            Ins::Call(i) => {
                out.push(0x10);
                uleb(u64::from(i), out);
            }
            Ins::CallIndirect { ty, table } => {
                out.push(0x11);
                uleb(u64::from(ty), out);
                uleb(u64::from(table), out);
            }
            Ins::ReturnCall(i) => {
                out.push(0x12);
                uleb(u64::from(i), out);
            }
            Ins::ReturnCallIndirect { ty, table } => {
                out.push(0x13);
                uleb(u64::from(ty), out);
                uleb(u64::from(table), out);
            }
            Ins::TableGet(t) => {
                out.push(0x25);
                uleb(u64::from(t), out);
            }
            Ins::RefIsNull => out.push(0xd1),
            Ins::LocalGet(i) => {
                out.push(0x20);
                uleb(u64::from(i), out);
            }
            Ins::LocalSet(i) => {
                out.push(0x21);
                uleb(u64::from(i), out);
            }
            Ins::LocalTee(i) => {
                out.push(0x22);
                uleb(u64::from(i), out);
            }
            Ins::GlobalGet(i) => {
                out.push(0x23);
                uleb(u64::from(i), out);
            }
            Ins::GlobalSet(i) => {
                out.push(0x24);
                uleb(u64::from(i), out);
            }
            Ins::Drop => out.push(0x1a),
            Ins::Select => out.push(0x1b),
            Ins::I32Const(v) => {
                out.push(0x41);
                sleb(i64::from(v), out);
            }
            Ins::I64Const(v) => {
                out.push(0x42);
                sleb(v, out);
            }
            Ins::F64Const(v) => {
                out.push(0x44);
                out.extend_from_slice(&v.to_bits().to_le_bytes());
            }
            Ins::I64Load(off) => mem(0x29, 3, off, out),
            Ins::I64Store(off) => mem(0x37, 3, off, out),
            Ins::F64Load(off) => mem(0x2b, 3, off, out),
            Ins::F64Store(off) => mem(0x39, 3, off, out),
            Ins::I32Load8U(off) => mem(0x2d, 0, off, out),
            Ins::I32Store8(off) => mem(0x3a, 0, off, out),
            // The one memory each: a memory index immediate, zero.
            Ins::MemorySize => {
                out.push(0x3f);
                out.push(0x00);
            }
            Ins::MemoryGrow => {
                out.push(0x40);
                out.push(0x00);
            }
            // Bulk memory, and the two memory indices `memory.copy` takes.
            Ins::MemoryCopy => {
                out.push(0xfc);
                uleb(10, out);
                out.push(0x00);
                out.push(0x00);
            }
            Ins::MemoryFill => {
                out.push(0xfc);
                uleb(11, out);
                out.push(0x00);
            }
            // `i64.trunc_sat_f64_s` is in the saturating-conversion prefix. It is not optional:
            // the evaluator's `f as i64` is Rust's *saturating* cast, and plain `i64.trunc_f64_s`
            // traps out of range (docs/93 §93.3). The index is `6` rather than `2` because the
            // four `i32` conversions come first in that table.
            Ins::I64TruncSatF64S => {
                out.push(0xfc);
                uleb(6, out);
            }
            other => out.push(other.opcode()),
        }
    }

    /// The single-byte opcode, for the instructions that take no immediate.
    fn opcode(self) -> u8 {
        match self {
            Ins::I32Eqz => 0x45,
            Ins::I32Eq => 0x46,
            Ins::I32Ne => 0x47,
            Ins::I32LtU => 0x49,
            Ins::I32GtU => 0x4b,
            Ins::I32LeU => 0x4d,
            Ins::I32GeU => 0x4f,
            Ins::I64Eqz => 0x50,
            Ins::I64Eq => 0x51,
            Ins::I64Ne => 0x52,
            Ins::I64LtS => 0x53,
            Ins::I64LtU => 0x54,
            Ins::I64GtS => 0x55,
            Ins::I64GtU => 0x56,
            Ins::I64LeS => 0x57,
            Ins::I64LeU => 0x58,
            Ins::I64GeS => 0x59,
            Ins::I64GeU => 0x5a,
            Ins::F64Eq => 0x61,
            Ins::F64Ne => 0x62,
            Ins::I32Add => 0x6a,
            Ins::I32Sub => 0x6b,
            Ins::I32Mul => 0x6c,
            Ins::I32And => 0x71,
            Ins::I32Or => 0x72,
            Ins::I64Add => 0x7c,
            Ins::I64Sub => 0x7d,
            Ins::I64Mul => 0x7e,
            Ins::I64DivS => 0x7f,
            Ins::I64DivU => 0x80,
            Ins::I64RemS => 0x81,
            Ins::I64RemU => 0x82,
            Ins::I64And => 0x83,
            Ins::I64Or => 0x84,
            Ins::I64Xor => 0x85,
            Ins::I64Shl => 0x86,
            Ins::I64ShrU => 0x88,
            Ins::F64Abs => 0x99,
            Ins::F64Neg => 0x9a,
            Ins::F64Sqrt => 0x9f,
            Ins::F64Add => 0xa0,
            Ins::F64Sub => 0xa1,
            Ins::F64Mul => 0xa2,
            Ins::F64Div => 0xa3,
            Ins::I32WrapI64 => 0xa7,
            Ins::I64ExtendI32S => 0xac,
            Ins::I64ExtendI32U => 0xad,
            Ins::F64ConvertI64S => 0xb9,
            Ins::I64ReinterpretF64 => 0xbd,
            Ins::F64ReinterpretI64 => 0xbf,
            other => unreachable!("{other:?} takes an immediate and is encoded by `encode`"),
        }
    }
}

fn mem(opcode: u8, align: u32, offset: u32, out: &mut Vec<u8>) {
    out.push(opcode);
    uleb(u64::from(align), out);
    uleb(u64::from(offset), out);
}

fn block_type(t: Option<ValType>, out: &mut Vec<u8>) {
    match t {
        None => out.push(0x40),
        Some(v) => out.push(v.byte()),
    }
}

/// Unsigned LEB128.
pub fn uleb(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            return;
        }
    }
}

/// Signed LEB128.
pub fn sleb(mut v: i64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        let sign_bit_set = byte & 0x40 != 0;
        if (v == 0 && !sign_bit_set) || (v == -1 && sign_bit_set) {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// One function's type: what it takes and what it gives back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuncType {
    pub params: Vec<ValType>,
    pub results: Vec<ValType>,
}

/// A function body: the locals it declares beyond its parameters, and its instructions.
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub locals: Vec<ValType>,
    pub code: Vec<Ins>,
}

/// One function the module imports — a host effect the loader supplies.
#[derive(Clone, Debug)]
pub struct Import {
    pub module: String,
    pub field: String,
    pub ty: u32,
}

/// One `funcref` table: a closure family's arms, indexed by the rank in a closure's first word.
#[derive(Clone, Debug)]
pub struct Table {
    /// The name a listing shows, which is the family's.
    pub name: String,
    /// `Some(index)` per rank, `None` where the family has no lambda of that rank — which is a
    /// null element and therefore [`beck_llvm::Trap::NoSuchLambda`] rather than an aborted
    /// instance.
    pub entries: Vec<Option<u32>>,
}

/// A module under construction.
#[derive(Default)]
pub struct ModuleBuilder {
    pub types: Vec<FuncType>,
    pub imports: Vec<Import>,
    /// One per defined function, indexing [`ModuleBuilder::types`].
    pub funcs: Vec<u32>,
    pub bodies: Vec<Body>,
    /// One per defined function, for the listing. Not a WebAssembly name section: what a person
    /// reads and what an engine loads are two artefacts, and only one of them is shipped.
    pub names: Vec<String>,
    /// `(name, type, mutable, initial)`, in index order.
    pub globals: Vec<(String, ValType, bool, i64)>,
    /// Exported functions, as `(name, function index)`.
    pub exports: Vec<(String, u32)>,
    /// `(minimum pages, maximum pages)`, when the module has a heap at all.
    pub memory: Option<(u32, u32)>,
    /// `(offset, bytes)` — the literal pool, written into the memory at instantiation.
    pub data: Vec<(u32, Vec<u8>)>,
    pub tables: Vec<Table>,
}

impl ModuleBuilder {
    pub fn new() -> ModuleBuilder {
        ModuleBuilder::default()
    }

    /// Intern a function type, so a module of similar signatures carries one entry each.
    pub fn ty(&mut self, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
        let want = FuncType { params, results };
        if let Some(i) = self.types.iter().position(|t| *t == want) {
            return i as u32;
        }
        self.types.push(want);
        (self.types.len() - 1) as u32
    }

    /// The index space's offset: an imported function comes before every defined one.
    pub fn defined_at(&self) -> u32 {
        self.imports.len() as u32
    }

    /// The bytes a runtime loads.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\0asm");
        out.extend_from_slice(&1u32.to_le_bytes());

        // 1 — types
        let mut s = Vec::new();
        uleb(self.types.len() as u64, &mut s);
        for t in &self.types {
            s.push(0x60);
            uleb(t.params.len() as u64, &mut s);
            for p in &t.params {
                s.push(p.byte());
            }
            uleb(t.results.len() as u64, &mut s);
            for r in &t.results {
                s.push(r.byte());
            }
        }
        section(1, &s, &mut out);

        // 2 — imports
        if !self.imports.is_empty() {
            let mut s = Vec::new();
            uleb(self.imports.len() as u64, &mut s);
            for i in &self.imports {
                name_bytes(&i.module, &mut s);
                name_bytes(&i.field, &mut s);
                s.push(0x00);
                uleb(u64::from(i.ty), &mut s);
            }
            section(2, &s, &mut out);
        }

        // 3 — functions
        let mut s = Vec::new();
        uleb(self.funcs.len() as u64, &mut s);
        for f in &self.funcs {
            uleb(u64::from(*f), &mut s);
        }
        section(3, &s, &mut out);

        // 4 — tables, one `funcref` per closure family
        if !self.tables.is_empty() {
            let mut s = Vec::new();
            uleb(self.tables.len() as u64, &mut s);
            for t in &self.tables {
                s.push(0x70); // funcref
                s.push(0x00); // minimum only
                uleb(t.entries.len() as u64, &mut s);
            }
            section(4, &s, &mut out);
        }

        // 5 — memory
        if let Some((min, max)) = self.memory {
            let mut s = Vec::new();
            uleb(1, &mut s);
            s.push(0x01); // a maximum follows
            uleb(u64::from(min), &mut s);
            uleb(u64::from(max), &mut s);
            section(5, &s, &mut out);
        }

        // 6 — globals
        let mut s = Vec::new();
        uleb(self.globals.len() as u64, &mut s);
        for (_, ty, mutable, init) in &self.globals {
            s.push(ty.byte());
            s.push(u8::from(*mutable));
            match ty {
                ValType::I32 => {
                    s.push(0x41);
                    sleb(*init, &mut s);
                }
                ValType::I64 => {
                    s.push(0x42);
                    sleb(*init, &mut s);
                }
                ValType::F64 => {
                    s.push(0x44);
                    s.extend_from_slice(&(*init as f64).to_bits().to_le_bytes());
                }
            }
            s.push(0x0b);
        }
        section(6, &s, &mut out);

        // 7 — exports. Every global is exported too: the host reads a trap out of one, clears them
        // before the next call, and moves the allocation pointer over the arguments it wrote. The
        // memory is exported because it *is* the arena.
        let mut s = Vec::new();
        let memories = usize::from(self.memory.is_some());
        uleb(
            (self.exports.len() + self.globals.len() + memories) as u64,
            &mut s,
        );
        for (name, index) in &self.exports {
            name_bytes(name, &mut s);
            s.push(0x00);
            uleb(u64::from(*index), &mut s);
        }
        if self.memory.is_some() {
            name_bytes("memory", &mut s);
            s.push(0x02);
            uleb(0, &mut s);
        }
        for (i, (name, _, _, _)) in self.globals.iter().enumerate() {
            name_bytes(name, &mut s);
            s.push(0x03);
            uleb(i as u64, &mut s);
        }
        section(7, &s, &mut out);

        // 9 — elements, one active segment per table
        if !self.tables.is_empty() {
            let mut s = Vec::new();
            let segments: Vec<(u32, u32, Vec<u32>)> = self
                .tables
                .iter()
                .enumerate()
                .flat_map(|(t, table)| runs(t as u32, &table.entries))
                .collect();
            uleb(segments.len() as u64, &mut s);
            for (table, offset, funcs) in &segments {
                // Kind 2: an explicit table index, an offset expression, an element kind, and the
                // function indices. Kind 0 would do for table zero alone; a family is a table.
                uleb(2, &mut s);
                uleb(u64::from(*table), &mut s);
                s.push(0x41);
                sleb(i64::from(*offset), &mut s);
                s.push(0x0b);
                s.push(0x00); // elemkind: funcref
                uleb(funcs.len() as u64, &mut s);
                for f in funcs {
                    uleb(u64::from(*f), &mut s);
                }
            }
            section(9, &s, &mut out);
        }

        // 10 — code
        let mut s = Vec::new();
        uleb(self.bodies.len() as u64, &mut s);
        for body in &self.bodies {
            let mut b = Vec::new();
            // Locals are run-length encoded by type; a run of one is still a run.
            let mut runs: Vec<(u32, ValType)> = Vec::new();
            for ty in &body.locals {
                match runs.last_mut() {
                    Some((n, t)) if t == ty => *n += 1,
                    _ => runs.push((1, *ty)),
                }
            }
            uleb(runs.len() as u64, &mut b);
            for (n, ty) in runs {
                uleb(u64::from(n), &mut b);
                b.push(ty.byte());
            }
            for ins in &body.code {
                ins.encode(&mut b);
            }
            b.push(0x0b);
            uleb(b.len() as u64, &mut s);
            s.extend_from_slice(&b);
        }
        section(10, &s, &mut out);

        // 11 — data, which is the literal pool
        if !self.data.is_empty() {
            let mut s = Vec::new();
            uleb(self.data.len() as u64, &mut s);
            for (offset, bytes) in &self.data {
                uleb(0, &mut s); // active, memory zero
                s.push(0x41);
                sleb(i64::from(*offset), &mut s);
                s.push(0x0b);
                uleb(bytes.len() as u64, &mut s);
                s.extend_from_slice(bytes);
            }
            section(11, &s, &mut out);
        }
        out
    }
}

/// The contiguous runs of a table's entries, so a family's gaps stay null.
fn runs(table: u32, entries: &[Option<u32>]) -> Vec<(u32, u32, Vec<u32>)> {
    let mut out: Vec<(u32, u32, Vec<u32>)> = Vec::new();
    let mut at = 0usize;
    while at < entries.len() {
        if entries[at].is_none() {
            at += 1;
            continue;
        }
        let start = at;
        let mut funcs = Vec::new();
        while at < entries.len() {
            match entries[at] {
                Some(f) => funcs.push(f),
                None => break,
            }
            at += 1;
        }
        out.push((table, start as u32, funcs));
    }
    out
}

fn name_bytes(name: &str, out: &mut Vec<u8>) {
    uleb(name.len() as u64, out);
    out.extend_from_slice(name.as_bytes());
}

fn section(id: u8, payload: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    uleb(payload.len() as u64, out);
    out.extend_from_slice(payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb128_round_trips_the_boundaries() {
        // The encoder is a page of code and every byte of the module goes through it, so the
        // boundaries are checked against a decoder written here rather than against expectations.
        for v in [0u64, 1, 63, 64, 127, 128, 16_383, 16_384, u64::MAX] {
            let mut out = Vec::new();
            uleb(v, &mut out);
            assert_eq!(read_uleb(&out), v, "unsigned {v}");
        }
        for v in [0i64, 1, -1, 63, -64, 64, -65, i64::MIN, i64::MAX] {
            let mut out = Vec::new();
            sleb(v, &mut out);
            assert_eq!(read_sleb(&out), v, "signed {v}");
        }
    }

    fn read_uleb(bytes: &[u8]) -> u64 {
        let (mut v, mut shift) = (0u64, 0);
        for b in bytes {
            v |= u64::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        v
    }

    fn read_sleb(bytes: &[u8]) -> i64 {
        let (mut v, mut shift) = (0i64, 0u32);
        let mut last = 0u8;
        for b in bytes {
            v |= i64::from(b & 0x7f) << shift;
            shift += 7;
            last = *b;
            if b & 0x80 == 0 {
                break;
            }
        }
        if shift < 64 && last & 0x40 != 0 {
            v |= -1i64 << shift;
        }
        v
    }

    #[test]
    fn an_empty_module_has_the_magic_and_the_version() {
        let bytes = ModuleBuilder::new().encode();
        assert_eq!(&bytes[..4], b"\0asm");
        assert_eq!(&bytes[4..8], &1u32.to_le_bytes());
    }

    /// A family's gaps are gaps, and each run is its own segment.
    ///
    /// The property matters rather than the encoding: a null element is what turns a rank no
    /// lambda answers to into [`beck_llvm::Trap::NoSuchLambda`], and a segment that filled the
    /// gaps with *something* would call whatever it filled them with.
    #[test]
    fn a_tables_runs_skip_the_ranks_the_family_has_no_lambda_for() {
        let entries = vec![None, Some(4), Some(5), None, None, Some(9)];
        assert_eq!(
            runs(1, &entries),
            vec![(1, 1, vec![4, 5]), (1, 5, vec![9])],
            "two runs, at the ranks they start at"
        );
        assert!(runs(0, &[None, None]).is_empty());
    }
}
