//! Static linear-time bytecode verifier (`verify_proto`).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use crate::compiler::opcode::{InstrFormat, Instruction, OpCode};
use crate::compiler::proto::{ELARA_MAX_REGISTERS, ObjProto};
use crate::value::nan::ObjType;

/// Verifies `proto` (and recursively every child) against the static bytecode rules.
pub unsafe fn verify_proto(
    proto: &ObjProto,
    parent: Option<&ObjProto>,
) -> Result<(), &'static str> {
    // 1. Stack window
    if proto.max_stack_size < 2 {
        return Err("verify: max_stack_size < 2");
    }
    if proto.num_params > proto.max_stack_size {
        return Err("verify: num_params > max_stack_size");
    }
    if proto.max_stack_size as usize > ELARA_MAX_REGISTERS {
        return Err("verify: max_stack_size > ELARA_MAX_REGISTERS");
    }
    if proto.code_count == 0 || proto.code.is_null() {
        return Err("verify: empty code");
    }

    // 5. Parent upvalue descriptor bounds
    if let Some(p) = parent {
        for i in 0..proto.upvalue_count as usize {
            let uv = *proto.upvalues.add(i);
            if uv.is_local != 0 {
                if uv.index >= p.max_stack_size {
                    return Err("verify: upvalue local index out of bounds");
                }
            } else if uv.index >= p.upvalue_count {
                return Err("verify: upvalue parent index out of bounds");
            }
        }
    }

    let max = proto.max_stack_size;
    let nconst = proto.const_count;
    let nup = proto.upvalue_count;
    let nproto = proto.proto_count;
    let ncode = proto.code_count as usize;

    for pc in 0..ncode {
        let inst = *proto.code.add(pc);
        let op = OpCode::from_u8(inst.opcode_raw()).ok_or("verify: invalid opcode")?;
        // SAFETY: `code` / `constants` are live for `proto`'s lifetime.
        check_instruction(proto, op, inst, pc, max, nconst, nup, nproto, ncode)?;
    }

    // 7. Guaranteed termination
    let last = *proto.code.add(ncode - 1);
    match last.opcode() {
        Some(OpCode::Return) | Some(OpCode::Jmp) => {}
        _ => return Err("verify: last instruction must be RETURN or JMP"),
    }

    // Recurse into children
    for i in 0..proto.proto_count as usize {
        let child = *proto.protos.add(i);
        if child.is_null() {
            return Err("verify: null child proto");
        }
        verify_proto(&*child, Some(proto))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
unsafe fn check_instruction(
    proto: &ObjProto,
    op: OpCode,
    inst: Instruction,
    pc: usize,
    max: u8,
    nconst: u16,
    nup: u8,
    nproto: u16,
    ncode: usize,
) -> Result<(), &'static str> {
    let a = inst.a();
    let b = inst.b();
    let c = inst.c();

    match op.format() {
        InstrFormat::Sj => {
            // JMP: no register A
        }
        InstrFormat::Abx | InstrFormat::Asbx | InstrFormat::Abc => {
            reg(a, max)?;
        }
    }

    match op {
        OpCode::Jmp => {
            jump_target(pc, inst.sj(), ncode)?;
        }
        OpCode::ForPrep | OpCode::ForLoop | OpCode::ForIn => {
            // A..A+3 control registers
            if (a as u16) + 3 >= u16::from(max) {
                return Err("verify: for-loop register window out of bounds");
            }
            jump_target(pc, inst.sbx(), ncode)?;
        }
        OpCode::LoadK | OpCode::Import => {
            let bx = inst.bx();
            if bx >= nconst {
                return Err("verify: LoadK/Import Bx out of bounds");
            }
            if op == OpCode::Import {
                const_is_string(proto, bx)?;
            }
        }
        OpCode::Closure => {
            if inst.bx() >= nproto {
                return Err("verify: Closure Bx out of bounds");
            }
        }
        OpCode::GetUpval | OpCode::SetUpval => {
            if b >= nup {
                return Err("verify: upvalue B out of bounds");
            }
        }
        OpCode::GetField => {
            reg(b, max)?;
            if u16::from(c) >= nconst {
                return Err("verify: GetField C const out of bounds");
            }
            const_is_string(proto, u16::from(c))?;
        }
        OpCode::SetField => {
            if u16::from(b) >= nconst {
                return Err("verify: SetField B const out of bounds");
            }
            const_is_string(proto, u16::from(b))?;
            reg(c, max)?;
        }
        OpCode::LoadNil => {
            if (a as u16) + u16::from(b) >= u16::from(max) {
                return Err("verify: LoadNil range out of bounds");
            }
        }
        OpCode::ConcatN => {
            if c == 0 {
                return Ok(());
            }
            let last = (b as u16) + u16::from(c) - 1;
            if last >= u16::from(max) {
                return Err("verify: ConcatN range out of bounds");
            }
        }
        OpCode::SelfMethod => {
            if (a as u16) + 1 >= u16::from(max) {
                return Err("verify: Self A+1 out of bounds");
            }
            reg(b, max)?;
            // C is method name RK or const index depending on ISA; treat as RK-like const string idx
            if u16::from(c) >= nconst {
                return Err("verify: Self C out of bounds");
            }
        }
        OpCode::Range => {
            reg(b, max)?;
            reg(c, max)?;
        }
        OpCode::SetList => {
            // A is table; B is count of values in A+1..
            if b > 0 && (a as u16) + u16::from(b) >= u16::from(max) {
                return Err("verify: SetList range out of bounds");
            }
        }
        OpCode::LoadBool => {
            if c != 0 && pc + 1 >= ncode {
                return Err("verify: LoadBool skip past end");
            }
        }
        OpCode::Eq
        | OpCode::Lt
        | OpCode::Le
        | OpCode::EqI
        | OpCode::LtI
        | OpCode::LeI
        | OpCode::Test
        | OpCode::TestSet
        | OpCode::Coalesce => {
            fused_jmp(proto, pc, ncode)?;
            if op.uses_rk_c() {
                reg(b, max)?;
                rk(c, max, nconst)?;
            } else if matches!(
                op,
                OpCode::EqI | OpCode::LtI | OpCode::LeI | OpCode::TestSet | OpCode::Coalesce
            ) {
                reg(b, max)?;
            }
        }
        _ => {
            if op.uses_rk_c() {
                reg(b, max)?;
                rk(c, max, nconst)?;
            } else {
                // Generic Abc: treat B and C as registers when format is Abc and not special.
                if op.format() == InstrFormat::Abc {
                    match op {
                        OpCode::Nop | OpCode::Return | OpCode::Close | OpCode::Yield => {
                            // B/C may be counts / flags
                            if matches!(op, OpCode::Return) {
                                // A is base; B is result count encoding — not a high register index
                            }
                        }
                        OpCode::Call | OpCode::TailCall | OpCode::Vararg | OpCode::NewTable => {}
                        OpCode::LoadI => {}
                        _ => {
                            // Conservative: if B looks like a register operand for Move/GetTab/...
                            if needs_reg_b(op) {
                                reg(b, max)?;
                            }
                            if needs_reg_c(op) {
                                reg(c, max)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn needs_reg_b(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::Move
            | OpCode::GetTab
            | OpCode::SetTab
            | OpCode::GetI
            | OpCode::SetI
            | OpCode::Unm
            | OpCode::BNot
            | OpCode::Not
            | OpCode::Len
            | OpCode::Pow
            | OpCode::BAnd
            | OpCode::BOr
            | OpCode::BXor
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::AddI
            | OpCode::In
    )
}

fn needs_reg_c(op: OpCode) -> bool {
    matches!(
        op,
        OpCode::GetTab
            | OpCode::SetTab
            | OpCode::GetI
            | OpCode::SetI
            | OpCode::Pow
            | OpCode::BAnd
            | OpCode::BOr
            | OpCode::BXor
            | OpCode::Shl
            | OpCode::Shr
            | OpCode::In
    )
}

fn reg(r: u8, max: u8) -> Result<(), &'static str> {
    if r >= max {
        Err("verify: register out of bounds")
    } else {
        Ok(())
    }
}

fn rk(c: u8, max: u8, nconst: u16) -> Result<(), &'static str> {
    if c & 0x80 != 0 {
        if u16::from(c & 0x7F) >= nconst {
            Err("verify: RK constant out of bounds")
        } else {
            Ok(())
        }
    } else {
        reg(c, max)
    }
}

fn jump_target(pc: usize, offset: i32, ncode: usize) -> Result<(), &'static str> {
    let target = (pc as i64) + 1 + i64::from(offset);
    if target < 0 || target >= ncode as i64 {
        Err("verify: jump target out of bounds")
    } else {
        Ok(())
    }
}

unsafe fn fused_jmp(proto: &ObjProto, pc: usize, ncode: usize) -> Result<(), &'static str> {
    if pc + 1 >= ncode {
        return Err("verify: fused compare missing JMP");
    }
    let next = *proto.code.add(pc + 1);
    if next.opcode() != Some(OpCode::Jmp) {
        return Err("verify: fused compare not followed by JMP");
    }
    Ok(())
}

unsafe fn const_is_string(proto: &ObjProto, idx: u16) -> Result<(), &'static str> {
    let v = *proto.constants.add(idx as usize);
    if v.is_obj_type(ObjType::String) {
        Ok(())
    } else {
        Err("verify: expected string constant")
    }
}
