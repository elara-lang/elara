//! `OP_CALL`, `OP_TAILCALL`, `OP_RETURN`, `OP_VARARG`, and `OP_CONCAT_N` helpers.

#![allow(unsafe_op_in_unsafe_fn)]

use crate::compiler::opcode::Instruction;
use crate::value::{ObjString, ObjType, Value};
use crate::vm::closure::ObjClosure;
use crate::vm::fiber::ObjFiber;
use crate::vm::frame::CallFrame;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Saved / restored register window after a call transition.
pub struct RegWindow {
    pub base: *mut Value,
    pub k: *const Value,
    pub pc: *const Instruction,
}

/// Closes open upvalues (and defer slots) at or above `level`.
#[inline]
pub fn close_upvalues(state: &mut ElaraState, fiber: &mut ObjFiber, level: u32) {
    let _ = unsafe { crate::vm::upvalue::close_upvalues_from(state, fiber, level) };
}

/// Prepares and pushes a new frame for `OP_CALL A, B, C`.
pub unsafe fn op_call(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    pc: *const Instruction,
    inst: Instruction,
) -> Result<RegWindow, ElaraStatus> {
    let a = inst.a();
    let b = inst.b();
    let c = inst.c();
    let num_args = if b == 0 {
        fiber
            .open_top
            .saturating_sub(frame.base_idx + u32::from(a) + 1)
    } else {
        u32::from(b) - 1
    };
    let num_results: u16 = if c == 0 { 0xFFFF } else { u16::from(c) - 1 };

    crate::vm::meta::call_or_metamethod(state, fiber, frame, base, pc, a, num_args, num_results)
}

/// In-place `OP_TAILCALL A, B` reusing the current frame ($O(1)$ stack frames).
pub unsafe fn op_tailcall(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &mut CallFrame,
    base: *mut Value,
    a: u8,
    b: u8,
) -> Result<RegWindow, ElaraStatus> {
    close_upvalues(state, fiber, frame.base_idx);

    let num_args = if b == 0 {
        fiber
            .open_top
            .saturating_sub(frame.base_idx + u32::from(a) + 1)
    } else {
        u32::from(b) - 1
    };

    let callable = unsafe { *base.add(a as usize) };
    let closure = closure_from_value(callable)?;
    let proto = unsafe { &*(*closure).proto };

    // Move callable into the callee slot and slide args down.
    unsafe {
        *fiber.stack.add(frame.func_idx as usize) = callable;
        if num_args > 0 {
            let src = base.add(a as usize + 1);
            let dst = fiber.stack.add(frame.func_idx as usize + 1);
            // Overlapping windows are possible; use copy (memmove semantics).
            core::ptr::copy(src, dst, num_args as usize);
        }
    }

    let new_base = frame.func_idx + 1;
    for i in num_args..u32::from(proto.num_params) {
        unsafe {
            *fiber.stack.add((new_base + i) as usize) = Value::NIL;
        }
    }

    let need = new_base + u32::from(proto.max_stack_size);
    fiber.open_top = fiber.open_top.max(need);
    fiber.ensure_stack(&mut state.alloc, need)?;

    frame.closure = closure;
    frame.base_idx = new_base;
    frame.vararg_count = num_args.saturating_sub(u32::from(proto.num_params)) as u16;
    frame.pc = proto.code.cast_const();

    Ok(RegWindow {
        base: unsafe { fiber.stack.add(frame.base_idx as usize) },
        k: unsafe { (*(*frame.closure).proto).constants.cast_const() },
        pc: frame.pc,
    })
}

/// Outcome of `OP_RETURN`.
pub enum ReturnOutcome {
    /// No frames remain; fiber execution is complete.
    Finished,
    /// Resume the caller with the restored register window.
    Resume(RegWindow),
}

/// Executes `OP_RETURN A, B`.
pub unsafe fn op_return(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    frame: &CallFrame,
    base: *mut Value,
    a: u8,
    b: u8,
    fuel: i64,
) -> ReturnOutcome {
    close_upvalues(state, fiber, frame.base_idx);

    let nret = if b == 0 {
        fiber.open_top.saturating_sub(frame.base_idx + u32::from(a))
    } else {
        u32::from(b) - 1
    };

    let dst = frame.func_idx;
    let want = frame.num_results;
    let copy_n = if want == 0xFFFF {
        nret
    } else {
        nret.min(u32::from(want))
    };

    for i in 0..copy_n {
        unsafe {
            *fiber.stack.add((dst + i) as usize) = *base.add(a as usize + i as usize);
        }
    }
    if want != 0xFFFF {
        for i in copy_n..u32::from(want) {
            unsafe {
                *fiber.stack.add((dst + i) as usize) = Value::NIL;
            }
        }
        fiber.open_top = dst + u32::from(want);
    } else {
        fiber.open_top = dst + nret;
    }

    fiber.frame_count = fiber.frame_count.saturating_sub(1);
    state.gas_remaining = fuel;

    if fiber.frame_count == 0 {
        return ReturnOutcome::Finished;
    }

    let caller = unsafe { &*fiber.frames.add(fiber.frame_count as usize - 1) };
    ReturnOutcome::Resume(RegWindow {
        base: unsafe { fiber.stack.add(caller.base_idx as usize) },
        k: unsafe { (*(*caller.closure).proto).constants.cast_const() },
        pc: caller.pc,
    })
}

/// Copies variadic arguments into `R[A..]`.
pub unsafe fn op_vararg(fiber: &mut ObjFiber, frame: &CallFrame, base: *mut Value, a: u8, b: u8) {
    let proto = unsafe { &*(*frame.closure).proto };
    let vararg_base = frame.func_idx + 1 + u32::from(proto.num_params);
    let available = u32::from(frame.vararg_count);

    if b == 0 {
        for i in 0..available {
            unsafe {
                *base.add(a as usize + i as usize) = *fiber.stack.add((vararg_base + i) as usize);
            }
        }
        fiber.open_top = frame.base_idx + u32::from(a) + available;
        return;
    }

    let want = u32::from(b) - 1;
    for i in 0..want {
        let v = if i < available {
            unsafe { *fiber.stack.add((vararg_base + i) as usize) }
        } else {
            Value::NIL
        };
        unsafe {
            *base.add(a as usize + i as usize) = v;
        }
    }
}

/// Concatenates `R[B] .. R[B+C-1]` into a new string in `R[A]`.
pub unsafe fn op_concat_n(
    state: &mut ElaraState,
    base: *mut Value,
    a: u8,
    b: u8,
    c: u8,
) -> Result<(), ElaraStatus> {
    if c == 0 {
        unsafe {
            *base.add(a as usize) = Value::NIL;
        }
        return Ok(());
    }

    let mut buf = Vec::new();
    for i in 0..c {
        let v = unsafe { *base.add(b as usize + i as usize) };
        if v.is_obj_type(ObjType::String) {
            let s = unsafe { &*v.as_obj().cast::<ObjString>() };
            buf.extend_from_slice(s.as_bytes());
        } else if let Some(i) = v.to_i64() {
            buf.extend_from_slice(i.to_string().as_bytes());
        } else if v.is_float() {
            buf.extend_from_slice(format!("{}", v.as_f64()).as_bytes());
        } else if v.is_bool() {
            buf.extend_from_slice(if v.as_bool() { b"true" } else { b"false" });
        } else if v.is_nil() {
            buf.extend_from_slice(b"nil");
        } else {
            return Err(ElaraStatus::ErrRuntime);
        }
    }

    let s = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, &buf)
        .ok_or(ElaraStatus::ErrOom)?;
    unsafe {
        *base.add(a as usize) = Value::from_obj(s.cast(), ObjType::String);
    }
    Ok(())
}

#[inline]
fn closure_from_value(v: Value) -> Result<*mut ObjClosure, ElaraStatus> {
    if !v.is_obj_type(ObjType::Closure) {
        return Err(ElaraStatus::ErrRuntime);
    }
    Ok(v.as_obj().cast::<ObjClosure>())
}
