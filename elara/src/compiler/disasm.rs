//! Bytecode disassembler and instruction formatter for tests and debugging.

use core::fmt::Write;

use crate::compiler::opcode::{InstrFormat, Instruction, OpCode, decode_rk};
use crate::compiler::proto::ObjProto;

/// Formats a single instruction at `pc` (with optional 1-based source `line`) into a readable line.
///
/// Example: `0003  [   12]  ADD       R[2] R[0] K[5]`
#[must_use]
pub fn disassemble_instruction(inst: Instruction, pc: u32, line: u32) -> String {
    let mut out = String::with_capacity(64);
    let _ = write!(out, "{pc:04}  ");
    if line > 0 {
        let _ = write!(out, "[{line:5}]  ");
    } else {
        out.push_str("[    -]  ");
    }

    match inst.opcode() {
        Some(op) => {
            let _ = write!(out, "{:<10}", op.name());
            format_operands(&mut out, op, inst, pc);
        }
        None => {
            let _ = write!(
                out,
                "OP_{:02X}     ; unknown opcode raw={:#010x}",
                inst.opcode_raw(),
                inst.0
            );
        }
    }
    out
}

/// Disassembles every instruction in `proto`, annotating each with its RLE source line.
#[must_use]
pub fn disassemble_proto(proto: &ObjProto) -> String {
    let mut out = String::new();
    if let Some(name) = source_name(proto) {
        let _ = writeln!(out, "; proto {name}");
    } else {
        let _ = writeln!(out, "; proto <anonymous>");
    }
    let _ = writeln!(
        out,
        "; params={} vararg={} maxstack={} ups={} consts={} code={}",
        proto.num_params,
        proto.is_vararg,
        proto.max_stack_size,
        proto.upvalue_count,
        proto.const_count,
        proto.code_count
    );

    for pc in 0..proto.code_count {
        // SAFETY: `pc < proto.code_count` and `code` is allocated with that length.
        let inst = unsafe { *proto.code.add(pc as usize) };
        let line = proto.get_line(pc);
        out.push_str(&disassemble_instruction(inst, pc, line));
        out.push('\n');
    }
    out
}

fn source_name(proto: &ObjProto) -> Option<String> {
    if proto.source_name.is_null() {
        return None;
    }
    // SAFETY: `source_name` is an optional interned `ObjString` owned by the GC heap.
    let s = unsafe { &*proto.source_name };
    Some(String::from_utf8_lossy(s.as_bytes()).into_owned())
}

fn format_operands(out: &mut String, op: OpCode, inst: Instruction, pc: u32) {
    match op.format() {
        InstrFormat::Abx => {
            let _ = write!(out, " R[{}] K[{}]", inst.a(), inst.bx());
        }
        InstrFormat::Asbx => {
            let target = (pc as i32 + 1 + inst.sbx()) as u32;
            let _ = write!(
                out,
                " R[{}] sBx={:+}  ; -> {}",
                inst.a(),
                inst.sbx(),
                target
            );
        }
        InstrFormat::Sj => {
            let target = (pc as i32 + 1 + inst.sj()) as u32;
            let _ = write!(out, " sJ={:+}  ; -> {}", inst.sj(), target);
        }
        InstrFormat::Abc => format_abc(out, op, inst),
    }
}

fn format_abc(out: &mut String, op: OpCode, inst: Instruction) {
    let a = inst.a();
    let b = inst.b();
    let c = inst.c();

    match op {
        OpCode::Nop => {}
        OpCode::Move
        | OpCode::GetUpval
        | OpCode::SetUpval
        | OpCode::Unm
        | OpCode::BNot
        | OpCode::Not
        | OpCode::Len
        | OpCode::Coalesce => {
            let _ = write!(out, " R[{a}] R[{b}]");
        }
        OpCode::LoadNil => {
            let _ = write!(out, " R[{a}..=R[{}]", a.saturating_add(b));
        }
        OpCode::LoadBool => {
            let _ = write!(out, " R[{a}] {} skip={}", b != 0, c != 0);
        }
        OpCode::Close => {
            let _ = write!(out, " R[{a}…]");
        }
        OpCode::NewTable
        | OpCode::SetList
        | OpCode::Call
        | OpCode::ConcatN
        | OpCode::In
        | OpCode::Range
        | OpCode::GetTab
        | OpCode::GetI
        | OpCode::SetI
        | OpCode::GetField
        | OpCode::SetField
        | OpCode::SelfMethod
        | OpCode::Pow
        | OpCode::BAnd
        | OpCode::BOr
        | OpCode::BXor
        | OpCode::Shl
        | OpCode::Shr
        | OpCode::TestSet => {
            let _ = write!(out, " R[{a}] {b} {c}");
        }
        OpCode::Test => {
            let _ = write!(out, " R[{a}] cond={}", c != 0);
        }
        OpCode::TailCall | OpCode::Return | OpCode::Vararg | OpCode::Yield => {
            let _ = write!(out, " R[{a}] {b}");
        }
        _ if op.uses_rk_c() => {
            let _ = write!(out, " R[{a}] R[{b}] ");
            format_rk(out, c);
        }
        _ if op.uses_sc() => {
            let _ = write!(out, " R[{a}] R[{b}] sC={}", inst.sc());
        }
        _ => {
            let _ = write!(out, " R[{a}] {b} {c}");
        }
    }
}

fn format_rk(out: &mut String, c: u8) {
    match decode_rk(c) {
        Ok(reg) => {
            let _ = write!(out, "R[{reg}]");
        }
        Err(k) => {
            let _ = write!(out, "K[{k}]");
        }
    }
}
