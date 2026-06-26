//! Register VM dispatch loop: dense `match` over opcode bytes, fused compare-and-branch.

#![allow(unsafe_op_in_unsafe_fn)]

use crate::compiler::opcode::{Instruction, OpCode};
use crate::value::Value;
use crate::vm::arith::{
    bin_add, bin_band, bin_bor, bin_bxor, bin_div, bin_mod, bin_mul, bin_shl, bin_shr, bin_sub,
    cmp_le, cmp_lt, rkc, un_bnot, un_neg,
};
use crate::vm::call::{
    RegWindow, ReturnOutcome, op_call, op_concat_n, op_return, op_tailcall, op_vararg,
};
use crate::vm::fiber::ObjFiber;
use crate::vm::meta::meta_add;
use crate::vm::sched::{
    FinishOutcome, YieldOutcome, do_yield, finish_fiber_return, handle_fiber_error,
};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::tab::{
    op_forin, op_forloop, op_forprep, op_getfield, op_geti, op_gettab, op_len, op_newtable,
    op_range, op_self, op_setfield, op_seti, op_setlist, op_settab,
};
use crate::vm::unwind::{CatchOutcome, unwind_fiber};
use crate::vm::upvalue::{op_close, op_closure, op_getupval, op_setupval};

/// Executes `fiber` until return from the bottom frame, yield, or error.
///
/// Fiber switches (resume / yield / child return) update `state.current_fiber` and continue
/// inside this loop without recursing on the host call stack.
///
/// # Safety
///
/// `fiber` must be a valid, aligned `ObjFiber` with a consistent frame stack for the duration
/// of the call.
pub unsafe fn execute(state: &mut ElaraState, fiber: *mut ObjFiber) -> ElaraStatus {
    let mut fiber_ptr = fiber;
    state.current_fiber = fiber_ptr;
    // SAFETY: caller upholds the contract above.
    if unsafe { (*fiber_ptr).frame_count } == 0 {
        return ElaraStatus::Ok;
    }

    let mut fuel = state.gas_remaining;
    let gas_limited = state.gas_limit > 0;

    // SAFETY: frame_count > 0.
    let mut fiber = unsafe { &mut *fiber_ptr };
    let mut frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
    // SAFETY: closure/proto established by frame setup.
    let mut base = unsafe { fiber.stack.add(frame.base_idx as usize) };
    let mut k = unsafe { (*(*frame.closure).proto).constants.cast_const() };
    let mut ic = unsafe { (*(*frame.closure).proto).ic_slots };
    let mut pc = frame.pc;

    loop {
        // SAFETY: `pc` points inside verified proto code.
        let inst = unsafe { *pc };
        pc = unsafe { pc.add(1) };

        // Single predicted check: zero overhead when hooks and profiler are off.
        if state.debug.hook_mask != 0 || state.debug.profiling {
            if state.debug.hook_mask != 0 {
                unsafe { crate::vm::debug::on_instruction(state, frame, pc) };
            }
            if state.debug.profiling {
                unsafe { crate::vm::debug::record_instruction(state, frame, inst) };
            }
        }

        if gas_limited {
            fuel -= 1;
            if fuel <= 0 {
                let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, ElaraStatus::ErrGas);
                if state.resume_after_catch {
                    state.resume_after_catch = false;
                    reload_after_catch(fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc);
                    continue;
                }
                return st;
            }
        }

        let op = match OpCode::from_u8(inst.opcode_raw()) {
            Some(op) => op,
            None => {
                state.gas_remaining = fuel;
                frame.pc = pc;
                {
                    let st = finish_err_fiber(
                        state,
                        fiber_ptr,
                        frame,
                        pc,
                        fuel,
                        ElaraStatus::ErrRuntime,
                    );
                    if state.resume_after_catch {
                        state.resume_after_catch = false;
                        reload_after_catch(
                            fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                        );
                        continue;
                    }
                    return st;
                }
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
            OpCode::GetUpval => {
                unsafe { op_getupval(frame, base, inst.a(), inst.b()) };
            }
            OpCode::SetUpval => {
                unsafe { op_setupval(state, frame, base, inst.a(), inst.b()) };
            }
            OpCode::Close => {
                if let Err(e) = unsafe { op_close(state, fiber, frame, inst.a()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Add => {
                let a = inst.a();
                let vb = unsafe { *base.add(inst.b() as usize) };
                let vc = unsafe { rkc(base, k, inst.c()) };
                match bin_add(vb, vc) {
                    Ok(v) => unsafe { *base.add(a as usize) = v },
                    Err(_) => match unsafe { meta_add(state, fiber, frame, base, pc, a, vb, vc) } {
                        Ok(w) => {
                            apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w)
                        }
                        Err(e) => {
                            let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                            if state.resume_after_catch {
                                state.resume_after_catch = false;
                                reload_after_catch(
                                    fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                                );
                                continue;
                            }
                            return st;
                        }
                    },
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
                        None => {
                            let st = finish_err_fiber(
                                state,
                                fiber_ptr,
                                frame,
                                pc,
                                fuel,
                                ElaraStatus::ErrRuntime,
                            );
                            if state.resume_after_catch {
                                state.resume_after_catch = false;
                                reload_after_catch(
                                    fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                                );
                                continue;
                            }
                            return st;
                        }
                    },
                };
                unsafe { *base.add(a) = res };
            }
            OpCode::Sub => {
                if let Err(e) = bin_op(base, k, inst, bin_sub) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Mul => {
                if let Err(e) = bin_op(base, k, inst, bin_mul) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Div => {
                if let Err(e) = bin_op(base, k, inst, bin_div) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Mod => {
                if let Err(e) = bin_op(base, k, inst, bin_mod) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::BAnd => {
                if let Err(e) = bin_op(base, k, inst, bin_band) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::BOr => {
                if let Err(e) = bin_op(base, k, inst, bin_bor) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::BXor => {
                if let Err(e) = bin_op(base, k, inst, bin_bxor) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Shl => {
                if let Err(e) = bin_op(base, k, inst, bin_shl) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Shr => {
                if let Err(e) = bin_op(base, k, inst, bin_shr) {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Unm => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                match un_neg(unsafe { *base.add(b) }) {
                    Ok(v) => unsafe { *base.add(a) = v },
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::BNot => {
                let a = inst.a() as usize;
                let b = inst.b() as usize;
                match un_bnot(unsafe { *base.add(b) }) {
                    Ok(v) => unsafe { *base.add(a) = v },
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
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
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                };
            }
            OpCode::Lt => {
                pc = match unsafe { fused_cmp(base, k, pc, inst, cmp_lt) } {
                    Ok(p) => p,
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                };
            }
            OpCode::Le => {
                pc = match unsafe { fused_cmp(base, k, pc, inst, cmp_le) } {
                    Ok(p) => p,
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                };
            }
            OpCode::EqI => {
                pc = match unsafe {
                    fused_cmp_i(base, pc, inst, |vb, sc| {
                        Ok(vb.to_i64().is_some_and(|i| i == sc))
                    })
                } {
                    Ok(p) => p,
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                };
            }
            OpCode::LtI => {
                pc = match unsafe {
                    fused_cmp_i(base, pc, inst, |vb, sc| {
                        Ok(vb.to_i64().is_some_and(|i| i < sc))
                    })
                } {
                    Ok(p) => p,
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                };
            }
            OpCode::LeI => {
                pc = match unsafe {
                    fused_cmp_i(base, pc, inst, |vb, sc| {
                        Ok(vb.to_i64().is_some_and(|i| i <= sc))
                    })
                } {
                    Ok(p) => p,
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
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
                        let st = finish_err_fiber(
                            state,
                            fiber_ptr,
                            frame,
                            pc,
                            fuel,
                            ElaraStatus::ErrGas,
                        );
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
                pc = unsafe { pc.offset(sj as isize) };
            }
            OpCode::NewTable => {
                if let Err(e) = unsafe { op_newtable(state, base, inst.a(), inst.b(), inst.c()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::GetTab => {
                let table = unsafe { *base.add(inst.b() as usize) };
                let key = unsafe { *base.add(inst.c() as usize) };
                match unsafe { op_gettab(state, fiber, frame, base, pc, inst.a(), table, key) } {
                    Ok(Some(w)) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w)
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::SetTab => {
                let table = unsafe { *base.add(inst.a() as usize) };
                let key = unsafe { *base.add(inst.b() as usize) };
                let val = unsafe { rkc(base, k, inst.c()) };
                match unsafe { op_settab(state, fiber, frame, base, pc, inst.a(), table, key, val) }
                {
                    Ok(Some(w)) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w)
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::GetI => {
                if let Err(e) = unsafe { op_geti(base, inst.a(), inst.b(), inst.c()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::SetI => {
                if let Err(e) = unsafe { op_seti(state, base, inst.a(), inst.b(), inst.c()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::GetField => {
                match unsafe { op_getfield(state, fiber, frame, base, k, ic, pc, inst) } {
                    Ok(Some(w)) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w)
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::SetField => {
                match unsafe { op_setfield(state, fiber, frame, base, k, ic, pc, inst) } {
                    Ok(Some(w)) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w)
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::SetList => {
                if let Err(e) = unsafe { op_setlist(state, base, inst.a(), inst.b(), inst.c()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::SelfMethod => {
                match unsafe { op_self(state, fiber, frame, base, k, ic, pc, inst) } {
                    Ok(Some(w)) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w)
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Len => {
                if let Err(e) = unsafe { op_len(base, inst.a(), inst.b()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::ForPrep => {
                pc = unsafe { op_forprep(base, pc, inst) };
            }
            OpCode::ForLoop => {
                match unsafe { op_forloop(base, pc, inst, gas_limited, &mut fuel) } {
                    Ok(p) => pc = p,
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::ForIn => match unsafe { op_forin(base, pc, inst) } {
                Ok(p) => pc = p,
                Err(e) => {
                    let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                    if state.resume_after_catch {
                        state.resume_after_catch = false;
                        reload_after_catch(
                            fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                        );
                        continue;
                    }
                    return st;
                }
            },
            OpCode::Range => {
                if let Err(e) = unsafe { op_range(state, base, inst.a(), inst.b(), inst.c()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Closure => {
                if let Err(e) =
                    unsafe { op_closure(state, fiber, frame, base, inst.a(), u32::from(inst.bx())) }
                {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Call => {
                frame.pc = pc;
                match unsafe { op_call(state, fiber, frame, base, pc, inst) } {
                    Ok(w) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w);
                        if state.debug.profiling {
                            unsafe { crate::vm::debug::record_call(state, frame) };
                        }
                        if state.debug.hook_mask != 0 {
                            unsafe { crate::vm::debug::on_call(state, frame) };
                        }
                    }
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::TailCall => {
                match unsafe { op_tailcall(state, fiber, frame, base, inst.a(), inst.b()) } {
                    Ok(w) => {
                        base = w.base;
                        k = w.k;
                        ic = unsafe { (*(*frame.closure).proto).ic_slots };
                        pc = w.pc;
                        if state.debug.profiling {
                            unsafe { crate::vm::debug::record_call(state, frame) };
                        }
                        if state.debug.hook_mask != 0 {
                            unsafe { crate::vm::debug::on_call(state, frame) };
                        }
                    }
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Return => {
                if state.debug.hook_mask != 0 {
                    unsafe { crate::vm::debug::on_return(state, frame) };
                }
                match unsafe { op_return(state, fiber, frame, base, inst.a(), inst.b(), fuel) } {
                    ReturnOutcome::Finished => {
                        match unsafe { finish_fiber_return(state, fiber_ptr) } {
                            Ok(FinishOutcome::ToHost) => return ElaraStatus::Ok,
                            Ok(FinishOutcome::SwitchTo(next)) => {
                                fuel = state.gas_remaining;
                                if let Err(e) = reload_fiber(
                                    &mut fiber_ptr,
                                    &mut fiber,
                                    &mut frame,
                                    &mut base,
                                    &mut k,
                                    &mut ic,
                                    &mut pc,
                                    next,
                                ) {
                                    return e;
                                }
                                continue;
                            }
                            Err(e) => return e,
                        }
                    }
                    ReturnOutcome::Resume(w) => {
                        apply_window(&mut frame, fiber, &mut base, &mut k, &mut ic, &mut pc, w);
                        fuel = state.gas_remaining;
                    }
                }
            }
            OpCode::Vararg => {
                unsafe { op_vararg(fiber, frame, base, inst.a(), inst.b()) };
            }
            OpCode::Yield => {
                frame.pc = pc;
                match unsafe { do_yield(state, fiber_ptr, frame, inst.a(), inst.b()) } {
                    Ok(YieldOutcome::ToHost) => {
                        state.gas_remaining = fuel;
                        return ElaraStatus::Yield;
                    }
                    Ok(YieldOutcome::SwitchTo(next)) => {
                        state.gas_remaining = fuel;
                        if let Err(e) = reload_fiber(
                            &mut fiber_ptr,
                            &mut fiber,
                            &mut frame,
                            &mut base,
                            &mut k,
                            &mut ic,
                            &mut pc,
                            next,
                        ) {
                            return e;
                        }
                        continue;
                    }
                    Err(e) => {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::ConcatN => {
                if let Err(e) = unsafe { op_concat_n(state, base, inst.a(), inst.b(), inst.c()) } {
                    {
                        let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                        if state.resume_after_catch {
                            state.resume_after_catch = false;
                            reload_after_catch(
                                fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                            );
                            continue;
                        }
                        return st;
                    }
                }
            }
            OpCode::Import => {
                state.gas_remaining = fuel;
                frame.pc = pc;
                if let Err(e) =
                    unsafe { crate::vm::module::op_import(state, base, k, inst.a(), inst.bx()) }
                {
                    let st = finish_err_fiber(state, fiber_ptr, frame, pc, fuel, e);
                    if state.resume_after_catch {
                        state.resume_after_catch = false;
                        reload_after_catch(
                            fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc,
                        );
                        continue;
                    }
                    return st;
                }
                fuel = state.gas_remaining;
                // Nested module exec may have swapped `current_fiber`; restore window.
                fiber = unsafe { &mut *fiber_ptr };
                frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
                base = unsafe { fiber.stack.add(frame.base_idx as usize) };
                k = unsafe { (*(*frame.closure).proto).constants.cast_const() };
                ic = unsafe { (*(*frame.closure).proto).ic_slots };
            }
            _ => {
                let st =
                    finish_err_fiber(state, fiber_ptr, frame, pc, fuel, ElaraStatus::ErrRuntime);
                if state.resume_after_catch {
                    state.resume_after_catch = false;
                    reload_after_catch(fiber_ptr, &mut frame, &mut base, &mut k, &mut ic, &mut pc);
                    continue;
                }
                return st;
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
fn apply_window(
    frame: &mut &mut crate::vm::frame::CallFrame,
    fiber: &mut ObjFiber,
    base: &mut *mut Value,
    k: &mut *const Value,
    ic: &mut *mut u16,
    pc: &mut *const Instruction,
    w: RegWindow,
) {
    *frame = unsafe { &mut *fiber.frames.add(fiber.frame_count as usize - 1) };
    *base = w.base;
    *k = w.k;
    *ic = unsafe { (*(*frame.closure).proto).ic_slots };
    *pc = w.pc;
}

#[inline]
#[allow(clippy::too_many_arguments)]
unsafe fn reload_fiber<'a>(
    fiber_ptr: &mut *mut ObjFiber,
    fiber: &mut &'a mut ObjFiber,
    frame: &mut &'a mut crate::vm::frame::CallFrame,
    base: &mut *mut Value,
    k: &mut *const Value,
    ic: &mut *mut u16,
    pc: &mut *const Instruction,
    next: *mut ObjFiber,
) -> Result<(), ElaraStatus> {
    *fiber_ptr = next;
    *fiber = &mut *next;
    if fiber.frame_count == 0 {
        return Err(ElaraStatus::ErrRuntime);
    }
    *frame = &mut *fiber.frames.add(fiber.frame_count as usize - 1);
    *base = fiber.stack.add(frame.base_idx as usize);
    *k = (*(*frame.closure).proto).constants.cast_const();
    *ic = (*(*frame.closure).proto).ic_slots;
    *pc = frame.pc;
    Ok(())
}

#[inline]
unsafe fn reload_after_catch(
    fiber_ptr: *mut ObjFiber,
    frame: &mut &mut crate::vm::frame::CallFrame,
    base: &mut *mut Value,
    k: &mut *const Value,
    ic: &mut *mut u16,
    pc: &mut *const Instruction,
) {
    let fiber = &mut *fiber_ptr;
    debug_assert!(fiber.frame_count > 0);
    *frame = &mut *fiber.frames.add(fiber.frame_count as usize - 1);
    *base = fiber.stack.add(frame.base_idx as usize);
    *k = (*(*frame.closure).proto).constants.cast_const();
    *ic = (*(*frame.closure).proto).ic_slots;
    *pc = frame.pc;
}

#[inline]
unsafe fn finish_err_fiber(
    state: &mut ElaraState,
    fiber_ptr: *mut ObjFiber,
    frame: &mut crate::vm::frame::CallFrame,
    pc: *const Instruction,
    fuel: i64,
    err: ElaraStatus,
) -> ElaraStatus {
    state.gas_remaining = fuel;
    frame.pc = pc;
    if err == ElaraStatus::ErrGas {
        state.last_error = Some(crate::ffi::sandbox::GAS_EXHAUSTED_MSG);
    } else if matches!(err, ElaraStatus::ErrRuntime) && state.last_error.is_none() {
        state.last_error = Some("runtime error");
    }

    let msg = if !state.error.last_message.is_nil() {
        state.error.last_message
    } else if err == ElaraStatus::ErrOom {
        state.error.oom_value()
    } else {
        state.error.assert_failed_value()
    };

    match unwind_fiber(state, fiber_ptr, err, msg) {
        CatchOutcome::Resume(_w) => {
            state.resume_after_catch = true;
            ElaraStatus::Ok
        }
        CatchOutcome::Propagate(ElaraStatus::Ok) => ElaraStatus::Ok,
        CatchOutcome::Propagate(e) => {
            if matches!(
                e,
                ElaraStatus::ErrRuntime | ElaraStatus::ErrGas | ElaraStatus::ErrOom
            ) && !(*fiber_ptr).caller.is_null()
            {
                return handle_fiber_error(state, fiber_ptr, e);
            }
            e
        }
    }
}
