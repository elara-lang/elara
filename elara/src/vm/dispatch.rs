//! Register VM dispatch loop: dense `match` over opcode bytes, fused compare-and-branch.

use crate::compiler::opcode::{Instruction, OpCode};
use crate::value::Value;
use crate::vm::arith::{
    bin_add, bin_band, bin_bor, bin_bxor, bin_div, bin_mod, bin_mul, bin_shl, bin_shr, bin_sub,
    cmp_le, cmp_lt, rkc, un_bnot, un_neg,
};
use crate::vm::call::{ReturnOutcome, op_call, op_concat_n, op_return, op_tailcall, op_vararg};
use crate::vm::fiber::ObjFiber;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Executes `fiber` until return from the bottom frame, yield, or error.
///
/// # Safety
///
/// `fiber` must be a valid, aligned `ObjFiber` with a consistent frame stack for the duration
/// of the call.
pub unsafe fn execute(state: &mut ElaraState, fiber: *mut ObjFiber) -> ElaraStatus {
    // SAFETY: caller upholds the contract above.
    let fiber = unsafe { &mut *fiber };
    if fiber.frame_count == 0 {
        return ElaraStatus::Ok;
    }

    let mut fuel = state.gas_remaining;
    let gas_limited = state.gas_limit > 0;

    // SAFETY: frame_count > 0.
    let mut frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
    // SAFETY: closure/proto established by frame setup.
    let mut base = unsafe { fiber.stack.add(frame.base_idx as usize) };
    let mut k = unsafe { (*(*frame.closure).proto).constants.cast_const() };
    let mut pc = frame.pc;

    loop {
        // SAFETY: `pc` points inside verified proto code.
        let inst = unsafe { *pc };
        pc = unsafe { pc.add(1) };

        if gas_limited {
            fuel -= 1;
            if fuel <= 0 {
                state.gas_remaining = fuel;
                frame.pc = pc;
                return ElaraStatus::ErrGas;
            }
        }

        let op = match OpCode::from_u8(inst.opcode_raw()) {
            Some(op) => op,
            None => {
                state.gas_remaining = fuel;
                frame.pc = pc;
                return ElaraStatus::ErrRuntime;
            }
        };

        match op {
            OpCode::Nop => {}
            OpCode::Move => {
                let (a, b) = (inst.a() as usize, inst.b() as usize);
                // SAFETY: register indices within max_stack_size.
                unsafe { *base.add(a) = *base.add(b) };
            }
            OpCode::LoadI => {
                let a = inst.a() as usize;
                unsafe { *base.add(a) = Value::from_i48(i64::from(inst.sbx())) };
            }
            OpCode::LoadK => {
                let a = inst.a() as usize;
                unsafe { *base.add(a) = *k.add(inst.bx() as usize) };
            }
            OpCode::LoadNil => {
                let a = inst.a() as usize;
                let n = inst.b() as usize;
                for i in 0..=n {
                    unsafe { *base.add(a + i) = Value::NIL };
                }
            }
            OpCode::LoadBool => {
                let a = inst.a() as usize;
                unsafe { *base.add(a) = Value::from_bool(inst.b() != 0) };
                if inst.c() != 0 {
                    pc = unsafe { pc.add(1) };
                }
            }
            OpCode::Add => {
                if let Err(e) = bin_op(base, k, inst, bin_add) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::AddI => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                let vb = unsafe { *base.add(b) };
                let sc = i64::from(inst.sc());
                let res = match vb.to_i64() {
                    Some(i) => Value::from_i48(i.wrapping_add(sc)),
                    None => match as_num(vb) {
                        Some(f) => Value::from_f64(f + sc as f64),
                        None => return finish_err(state, frame, pc, fuel, ElaraStatus::ErrRuntime),
                    },
                };
                unsafe { *base.add(a) = res };
            }
            OpCode::Sub => {
                if let Err(e) = bin_op(base, k, inst, bin_sub) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::Mul => {
                if let Err(e) = bin_op(base, k, inst, bin_mul) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::Div => {
                if let Err(e) = bin_op(base, k, inst, bin_div) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::Mod => {
                if let Err(e) = bin_op(base, k, inst, bin_mod) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::BAnd => {
                if let Err(e) = bin_op(base, k, inst, bin_band) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::BOr => {
                if let Err(e) = bin_op(base, k, inst, bin_bor) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::BXor => {
                if let Err(e) = bin_op(base, k, inst, bin_bxor) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::Shl => {
                if let Err(e) = bin_op(base, k, inst, bin_shl) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::Shr => {
                if let Err(e) = bin_op(base, k, inst, bin_shr) {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            OpCode::Unm => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                match un_neg(unsafe { *base.add(b) }) {
                    Ok(v) => unsafe { *base.add(a) = v },
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                }
            }
            OpCode::BNot => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                match un_bnot(unsafe { *base.add(b) }) {
                    Ok(v) => unsafe { *base.add(a) = v },
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                }
            }
            OpCode::Not => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                let falsy = unsafe { (*base.add(b)).is_falsy() };
                unsafe { *base.add(a) = Value::from_bool(falsy) };
            }
            OpCode::Eq => {
                // SAFETY: fused compare consumes the trailing OP_JMP.
                pc = match unsafe { fused_cmp(base, k, pc, inst, |vb, vc| Ok(vb.val_equal(vc))) } {
                    Ok(p) => p,
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                };
            }
            OpCode::Lt => {
                pc = match unsafe { fused_cmp(base, k, pc, inst, cmp_lt) } {
                    Ok(p) => p,
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                };
            }
            OpCode::Le => {
                pc = match unsafe { fused_cmp(base, k, pc, inst, cmp_le) } {
                    Ok(p) => p,
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                };
            }
            OpCode::EqI => {
                pc = match unsafe {
                    fused_cmp_i(base, pc, inst, |vb, sc| {
                        Ok(vb.to_i64().is_some_and(|i| i == sc))
                    })
                } {
                    Ok(p) => p,
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                };
            }
            OpCode::LtI => {
                pc = match unsafe {
                    fused_cmp_i(base, pc, inst, |vb, sc| {
                        Ok(vb.to_i64().is_some_and(|i| i < sc))
                    })
                } {
                    Ok(p) => p,
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                };
            }
            OpCode::LeI => {
                pc = match unsafe {
                    fused_cmp_i(base, pc, inst, |vb, sc| {
                        Ok(vb.to_i64().is_some_and(|i| i <= sc))
                    })
                } {
                    Ok(p) => p,
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                };
            }
            OpCode::Test => {
                let a = inst.a() as usize;
                let cond = unsafe { (*base.add(a)).is_truthy() } == (inst.c() != 0);
                pc = take_or_skip_jmp(pc, cond);
            }
            OpCode::Jmp => {
                let sj = inst.sj();
                if sj <= 0 && gas_limited {
                    fuel -= 1;
                    if fuel <= 0 {
                        state.gas_remaining = fuel;
                        frame.pc = pc;
                        return ElaraStatus::ErrGas;
                    }
                }
                pc = unsafe { pc.offset(sj as isize) };
            }
            OpCode::Call => {
                frame.pc = pc;
                match unsafe { op_call(state, fiber, frame, base, pc, inst) } {
                    Ok(w) => {
                        frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
                        base = w.base;
                        k = w.k;
                        pc = w.pc;
                    }
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                }
            }
            OpCode::TailCall => {
                match unsafe { op_tailcall(state, fiber, frame, base, inst.a(), inst.b()) } {
                    Ok(w) => {
                        base = w.base;
                        k = w.k;
                        pc = w.pc;
                    }
                    Err(e) => return finish_err(state, frame, pc, fuel, e),
                }
            }
            OpCode::Return => {
                match unsafe { op_return(state, fiber, frame, base, inst.a(), inst.b(), fuel) } {
                    ReturnOutcome::Finished => return ElaraStatus::Ok,
                    ReturnOutcome::Resume(w) => {
                        frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
                        base = w.base;
                        k = w.k;
                        pc = w.pc;
                        fuel = state.gas_remaining;
                    }
                }
            }
            OpCode::Vararg => {
                unsafe { op_vararg(fiber, frame, base, inst.a(), inst.b()) };
            }
            OpCode::ConcatN => {
                if let Err(e) = unsafe { op_concat_n(state, base, inst.a(), inst.b(), inst.c()) } {
                    return finish_err(state, frame, pc, fuel, e);
                }
            }
            _ => {
                return finish_err(state, frame, pc, fuel, ElaraStatus::ErrRuntime);
            }
        }
    }
}

#[inline(always)]
fn bin_op(
    base: *mut Value,
    k: *const Value,
    inst: Instruction,
    f: fn(Value, Value) -> Result<Value, ElaraStatus>,
) -> Result<(), ElaraStatus> {
    let a = inst.a() as usize;
    let vb = unsafe { *base.add(inst.b() as usize) };
    let vc = unsafe { rkc(base, k, inst.c()) };
    let res = f(vb, vc)?;
    unsafe { *base.add(a) = res };
    Ok(())
}

#[inline(always)]
unsafe fn fused_cmp(
    base: *mut Value,
    k: *const Value,
    pc: *const Instruction,
    inst: Instruction,
    pred: impl FnOnce(Value, Value) -> Result<bool, ElaraStatus>,
) -> Result<*const Instruction, ElaraStatus> {
    let vb = unsafe { *base.add(inst.b() as usize) };
    let vc = unsafe { rkc(base, k, inst.c()) };
    let cond = pred(vb, vc)? == (inst.a() != 0);
    Ok(take_or_skip_jmp(pc, cond))
}

#[inline(always)]
unsafe fn fused_cmp_i(
    base: *mut Value,
    pc: *const Instruction,
    inst: Instruction,
    pred: impl FnOnce(Value, i64) -> Result<bool, ElaraStatus>,
) -> Result<*const Instruction, ElaraStatus> {
    let vb = unsafe { *base.add(inst.b() as usize) };
    let sc = i64::from(inst.sc());
    let cond = pred(vb, sc)? == (inst.a() != 0);
    Ok(take_or_skip_jmp(pc, cond))
}

#[inline(always)]
fn take_or_skip_jmp(pc: *const Instruction, take: bool) -> *const Instruction {
    // SAFETY: fused compare is followed by OP_JMP (compiler / verifier invariant).
    let jmp = unsafe { *pc };
    if take {
        unsafe { pc.offset(1 + jmp.sj() as isize) }
    } else {
        unsafe { pc.add(1) }
    }
}

#[inline]
fn as_num(v: Value) -> Option<f64> {
    if let Some(i) = v.to_i64() {
        Some(i as f64)
    } else if v.is_float() {
        Some(v.as_f64())
    } else {
        None
    }
}

#[inline]
fn finish_err(
    state: &mut ElaraState,
    frame: &mut crate::vm::frame::CallFrame,
    pc: *const Instruction,
    fuel: i64,
    err: ElaraStatus,
) -> ElaraStatus {
    state.gas_remaining = fuel;
    frame.pc = pc;
    err
}
