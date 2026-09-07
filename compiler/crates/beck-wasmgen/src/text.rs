//! The module, as text a person reads.
//!
//! [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.1 names this as a property
//! worth having rather than a nicety: "the artefact is readable — a codegen defect is a diff in a
//! text file". `beck native --backend llvm` leaves a `.ll`; this leaves a `.wat`-shaped listing of
//! the same [`Ins`] list the bytes were encoded from, so the two cannot disagree about what was
//! emitted.
//!
//! It is a *listing* rather than a WebAssembly text module: names are the Beck definitions' and the
//! runtime's, locals are numbered, and nothing here is meant to be fed back to an assembler.
//! Round-tripping would be a second format to keep true.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::binary::{Ins, ModuleBuilder};

pub fn render(builder: &ModuleBuilder) -> String {
    let mut out = String::from("(module\n");
    for i in &builder.imports {
        let ty = &builder.types[i.ty as usize];
        let _ = writeln!(
            out,
            "  (import \"{}\" \"{}\" (func (param {}) (result {})))",
            i.module,
            i.field,
            joined(&ty.params),
            joined(&ty.results)
        );
    }
    if let Some((min, max)) = builder.memory {
        let _ = writeln!(out, "  (memory (export \"memory\") {min} {max})");
    }
    for (offset, bytes) in &builder.data {
        let _ = writeln!(
            out,
            "  (data (i32.const {offset}) \"…{} bytes of literal pool…\")",
            bytes.len()
        );
    }
    for (name, ty, mutable, init) in &builder.globals {
        let shape = if *mutable {
            format!("(mut {})", ty.text())
        } else {
            ty.text().to_string()
        };
        let _ = writeln!(out, "  (global ${name} (export \"{name}\") {shape} {init})");
    }
    for (t, table) in builder.tables.iter().enumerate() {
        let _ = writeln!(
            out,
            "  (table ${} {} funcref)  ;; applying a {}",
            t,
            table.entries.len(),
            table.name
        );
        for (rank, entry) in table.entries.iter().enumerate() {
            if let Some(f) = entry {
                let _ = writeln!(
                    out,
                    "    (elem (table {t}) (i32.const {rank}) func {})",
                    builder
                        .names
                        .get((*f - builder.defined_at()) as usize)
                        .map_or_else(|| f.to_string(), |n| format!("${n}"))
                );
            }
        }
    }
    let exported: BTreeSet<u32> = builder.exports.iter().map(|(_, i)| *i).collect();
    let base = builder.defined_at();
    for (i, body) in builder.bodies.iter().enumerate() {
        let name = &builder.names[i];
        let ty = &builder.types[builder.funcs[i] as usize];
        let export = if exported.contains(&(base + i as u32)) {
            format!(" (export \"{name}\")")
        } else {
            String::new()
        };
        let _ = writeln!(
            out,
            "  (func ${name}{export} (param {}) (result {})",
            joined(&ty.params),
            joined(&ty.results)
        );
        if !body.locals.is_empty() {
            let _ = writeln!(out, "    (local {})", joined(&body.locals));
        }
        let mut depth = 2usize;
        for ins in &body.code {
            if matches!(ins, Ins::Else | Ins::End) {
                depth = depth.saturating_sub(1);
            }
            let _ = writeln!(out, "{}{}", "  ".repeat(depth), one(*ins, builder));
            if matches!(ins, Ins::Block(_) | Ins::Loop(_) | Ins::If(_) | Ins::Else) {
                depth += 1;
            }
        }
        out.push_str("  )\n");
    }
    out.push_str(")\n");
    out
}

fn joined(types: &[crate::binary::ValType]) -> String {
    types.iter().map(|t| t.text()).collect::<Vec<_>>().join(" ")
}

/// The name of a function index, so a listing reads as calls rather than as numbers.
fn called(index: u32, builder: &ModuleBuilder) -> String {
    let base = builder.defined_at();
    if index < base {
        let i = &builder.imports[index as usize];
        return format!("{index} ;; {}.{}", i.module, i.field);
    }
    match builder.names.get((index - base) as usize) {
        Some(name) => format!("{index} ;; ${name}"),
        None => index.to_string(),
    }
}

fn one(ins: Ins, builder: &ModuleBuilder) -> String {
    match ins {
        Ins::Block(t) => format!("block{}", result(t)),
        Ins::Loop(t) => format!("loop{}", result(t)),
        Ins::If(t) => format!("if{}", result(t)),
        Ins::Else => "else".into(),
        Ins::End => "end".into(),
        Ins::Br(d) => format!("br {d}"),
        Ins::BrIf(d) => format!("br_if {d}"),
        Ins::Return => "return".into(),
        Ins::Unreachable => "unreachable".into(),
        Ins::Call(i) => format!("call {}", called(i, builder)),
        Ins::ReturnCall(i) => format!("return_call {}", called(i, builder)),
        Ins::CallIndirect { ty, table } => format!("call_indirect (type {ty}) (table {table})"),
        Ins::ReturnCallIndirect { ty, table } => {
            format!("return_call_indirect (type {ty}) (table {table})")
        }
        Ins::TableGet(t) => format!("table.get {t}"),
        Ins::LocalGet(i) => format!("local.get {i}"),
        Ins::LocalSet(i) => format!("local.set {i}"),
        Ins::LocalTee(i) => format!("local.tee {i}"),
        Ins::GlobalGet(i) => format!("global.get {i}"),
        Ins::GlobalSet(i) => format!("global.set {i}"),
        Ins::I32Const(v) => format!("i32.const {v}"),
        Ins::I64Const(v) => format!("i64.const {v}"),
        Ins::F64Const(v) => format!("f64.const {v:?}"),
        Ins::I64Load(o) => offset("i64.load", o),
        Ins::I64Store(o) => offset("i64.store", o),
        Ins::F64Load(o) => offset("f64.load", o),
        Ins::F64Store(o) => offset("f64.store", o),
        Ins::I32Load8U(o) => offset("i32.load8_u", o),
        Ins::I32Store8(o) => offset("i32.store8", o),
        other => plain(other).into(),
    }
}

fn offset(name: &str, at: u32) -> String {
    if at == 0 {
        name.to_string()
    } else {
        format!("{name} offset={at}")
    }
}

fn result(t: Option<crate::binary::ValType>) -> String {
    match t {
        None => String::new(),
        Some(v) => format!(" (result {})", v.text()),
    }
}

fn plain(ins: Ins) -> &'static str {
    match ins {
        Ins::Drop => "drop",
        Ins::Select => "select",
        Ins::RefIsNull => "ref.is_null",
        Ins::MemorySize => "memory.size",
        Ins::MemoryGrow => "memory.grow",
        Ins::MemoryCopy => "memory.copy",
        Ins::MemoryFill => "memory.fill",
        Ins::I32Eqz => "i32.eqz",
        Ins::I32Eq => "i32.eq",
        Ins::I32Ne => "i32.ne",
        Ins::I32And => "i32.and",
        Ins::I32Or => "i32.or",
        Ins::I32Add => "i32.add",
        Ins::I32Sub => "i32.sub",
        Ins::I32Mul => "i32.mul",
        Ins::I32LtU => "i32.lt_u",
        Ins::I32LeU => "i32.le_u",
        Ins::I32GtU => "i32.gt_u",
        Ins::I32GeU => "i32.ge_u",
        Ins::I32WrapI64 => "i32.wrap_i64",
        Ins::I64ExtendI32U => "i64.extend_i32_u",
        Ins::I64ExtendI32S => "i64.extend_i32_s",
        Ins::I64Eqz => "i64.eqz",
        Ins::I64Eq => "i64.eq",
        Ins::I64Ne => "i64.ne",
        Ins::I64LtS => "i64.lt_s",
        Ins::I64LeS => "i64.le_s",
        Ins::I64GtS => "i64.gt_s",
        Ins::I64GeS => "i64.ge_s",
        Ins::I64LtU => "i64.lt_u",
        Ins::I64LeU => "i64.le_u",
        Ins::I64GtU => "i64.gt_u",
        Ins::I64GeU => "i64.ge_u",
        Ins::I64Add => "i64.add",
        Ins::I64Sub => "i64.sub",
        Ins::I64Mul => "i64.mul",
        Ins::I64DivS => "i64.div_s",
        Ins::I64DivU => "i64.div_u",
        Ins::I64RemS => "i64.rem_s",
        Ins::I64RemU => "i64.rem_u",
        Ins::I64And => "i64.and",
        Ins::I64Or => "i64.or",
        Ins::I64Xor => "i64.xor",
        Ins::I64Shl => "i64.shl",
        Ins::I64ShrU => "i64.shr_u",
        Ins::F64Add => "f64.add",
        Ins::F64Sub => "f64.sub",
        Ins::F64Mul => "f64.mul",
        Ins::F64Div => "f64.div",
        Ins::F64Abs => "f64.abs",
        Ins::F64Neg => "f64.neg",
        Ins::F64Sqrt => "f64.sqrt",
        Ins::F64Eq => "f64.eq",
        Ins::F64Ne => "f64.ne",
        Ins::F64ConvertI64S => "f64.convert_i64_s",
        Ins::I64TruncSatF64S => "i64.trunc_sat_f64_s",
        Ins::I64ReinterpretF64 => "i64.reinterpret_f64",
        Ins::F64ReinterpretI64 => "f64.reinterpret_i64",
        other => unreachable!("{other:?} takes an immediate and is rendered by `one`"),
    }
}
