//! The module, as text a person reads.
//!
//! [`docs/93`](../../../../../docs/93-the-native-backends-report.md) §93.1 names this as a property
//! worth having rather than a nicety: "the artefact is readable — a codegen defect is a diff in a
//! text file". `beck native --backend llvm` leaves a `.ll`; this leaves a `.wat`-shaped listing of
//! the same [`Ins`] list the bytes were encoded from, so the two cannot disagree about what was
//! emitted.
//!
//! It is a *listing* rather than a WebAssembly text module: names are the Beck definitions',
//! locals are numbered, and nothing here is meant to be fed back to an assembler. Round-tripping
//! would be a second format to keep true.

use std::fmt::Write as _;

use beck_llvm::Signature;

use crate::binary::{Ins, ModuleBuilder};

pub fn render(builder: &ModuleBuilder, functions: &[Signature]) -> String {
    let mut out = String::from("(module\n");
    for (name, ty, mutable, init) in &builder.globals {
        let _ = writeln!(
            out,
            "  (global ${name} {}{}) (export \"{name}\")",
            if *mutable { "(mut " } else { "" },
            if *mutable {
                format!("{})", ty.text())
            } else {
                ty.text().to_string()
            }
        );
        let _ = init;
    }
    for (i, body) in builder.bodies.iter().enumerate() {
        let sig = &functions[i];
        let ty = &builder.types[builder.funcs[i] as usize];
        let params: Vec<String> = ty.params.iter().map(|p| p.text().to_string()).collect();
        let results: Vec<String> = ty.results.iter().map(|r| r.text().to_string()).collect();
        let _ = writeln!(
            out,
            "  (func ${} (export \"{}\") (param {}) (result {})",
            sig.name,
            sig.name,
            params.join(" "),
            results.join(" ")
        );
        if !body.locals.is_empty() {
            let locals: Vec<String> = body.locals.iter().map(|l| l.text().to_string()).collect();
            let _ = writeln!(out, "    (local {})", locals.join(" "));
        }
        let mut depth = 2usize;
        for ins in &body.code {
            if matches!(ins, Ins::Else | Ins::End) {
                depth = depth.saturating_sub(1);
            }
            let _ = writeln!(out, "{}{}", "  ".repeat(depth), one(*ins));
            if matches!(ins, Ins::Block(_) | Ins::If(_) | Ins::Else) {
                depth += 1;
            }
        }
        out.push_str("  )\n");
    }
    out.push_str(")\n");
    out
}

fn one(ins: Ins) -> String {
    match ins {
        Ins::Block(t) => format!("block{}", result(t)),
        Ins::If(t) => format!("if{}", result(t)),
        Ins::Else => "else".into(),
        Ins::End => "end".into(),
        Ins::Return => "return".into(),
        Ins::Call(i) => format!("call {i}"),
        Ins::ReturnCall(i) => format!("return_call {i}"),
        Ins::LocalGet(i) => format!("local.get {i}"),
        Ins::LocalSet(i) => format!("local.set {i}"),
        Ins::LocalTee(i) => format!("local.tee {i}"),
        Ins::GlobalGet(i) => format!("global.get {i}"),
        Ins::GlobalSet(i) => format!("global.set {i}"),
        Ins::Drop => "drop".into(),
        Ins::I32Const(v) => format!("i32.const {v}"),
        Ins::I64Const(v) => format!("i64.const {v}"),
        Ins::F64Const(v) => format!("f64.const {v:?}"),
        other => plain(other).into(),
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
        Ins::I32Eqz => "i32.eqz",
        Ins::I32Eq => "i32.eq",
        Ins::I32Ne => "i32.ne",
        Ins::I32And => "i32.and",
        Ins::I32Or => "i32.or",
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
        Ins::I64RemS => "i64.rem_s",
        Ins::I64And => "i64.and",
        Ins::I64Xor => "i64.xor",
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
