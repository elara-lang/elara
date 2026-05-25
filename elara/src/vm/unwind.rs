//! Frame-based error unwinding to `FRAME_FLAG_PCALL` (and gas bypass).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::value::{ObjType, Value};
use crate::vm::call::RegWindow;
use crate::vm::error::capture_stack_trace;
use crate::vm::fiber::{FiberStatus, ObjFiber};
use crate::vm::frame::{CallFrame, FRAME_FLAG_PCALL};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::upvalue::close_upvalues_from;

/// Result of attempting to catch a runtime error inside a fiber.
#[derive(Debug)]
pub enum CatchOutcome {
    /// Error caught by `pcall`; resume at the caller's register window.
    Resume(RegWindow),
    /// No catcher (or non-catchable status); propagate to host / fiber caller.
    Propagate(ElaraStatus),
}

/// Raises `ErrRuntime` with `message`, capturing a stack trace and unwinding to `pcall` if any.
pub unsafe fn raise_runtime(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
    message: Value,
) -> CatchOutcome {
    state.error.set_error(ElaraStatus::ErrRuntime, message);
    state.last_error = Some("runtime error");
    unwind_fiber(state, fiber, ElaraStatus::ErrRuntime, message)
}

/// `panic(msg)` — coerce `msg` to a string value when possible, then raise.
pub unsafe fn intrinsic_panic(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
    msg: Value,
) -> CatchOutcome {
    let message = coerce_error_message(state, msg);
    raise_runtime(state, fiber, message)
}

/// `assert(cond, msg?)` — returns `Ok(cond)` when truthy; otherwise raises.
pub unsafe fn intrinsic_assert(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
    cond: Value,
    msg: Option<Value>,
) -> Result<Value, CatchOutcome> {
    if cond.is_truthy() {
        return Ok(cond);
    }
    let message = match msg {
        Some(m) if !m.is_nil() => coerce_error_message(state, m),
        _ => state.error.assert_failed_value(),
    };
    Err(raise_runtime(state, fiber, message))
}

/// Unwinds `fiber` after `err`. `ErrGas` never enters a script `pcall`.
pub unsafe fn unwind_fiber(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
    err: ElaraStatus,
    error_val: Value,
) -> CatchOutcome {
    if fiber.is_null() {
        return CatchOutcome::Propagate(err);
    }
    capture_stack_trace(&mut state.error, fiber);
    state.error.set_error(err, error_val);

    let f = &mut *fiber;

    // Gas exhaustion bypasses pcall entirely.
    if err == ElaraStatus::ErrGas {
        let _ = close_upvalues_from(state, f, 0);
        f.frame_count = 0;
        f.status = FiberStatus::Dead;
        return CatchOutcome::Propagate(err);
    }

    if !matches!(err, ElaraStatus::ErrRuntime | ElaraStatus::ErrOom) {
        let _ = close_upvalues_from(state, f, 0);
        f.frame_count = 0;
        f.status = FiberStatus::Dead;
        return CatchOutcome::Propagate(err);
    }

    let mut i = f.frame_count;
    while i > 0 {
        i -= 1;
        let frame = &mut *f.frames.add(i as usize);
        let _ = close_upvalues_from(state, f, frame.base_idx);

        if (frame.flags & FRAME_FLAG_PCALL) != 0 {
            let dest = frame.func_idx;
            f.frame_count = i; // drop pcall frame and everything above
            *f.stack.add(dest as usize) = Value::FALSE;
            *f.stack.add(dest as usize + 1) = error_val;
            f.open_top = dest + 2;

            if f.frame_count == 0 {
                // Caught at the bottom-most pcall; host sees Ok with false/msg on stack.
                return CatchOutcome::Propagate(ElaraStatus::Ok);
            }
            let caller = &*f.frames.add(f.frame_count as usize - 1);
            return CatchOutcome::Resume(RegWindow {
                base: f.stack.add(caller.base_idx as usize),
                k: (*(*caller.closure).proto).constants.cast_const(),
                pc: caller.pc,
            });
        }
    }

    let _ = close_upvalues_from(state, f, 0);
    f.frame_count = 0;
    f.status = FiberStatus::Dead;
    CatchOutcome::Propagate(err)
}

/// Pushes a protected call frame (`FRAME_FLAG_PCALL`) for `closure` at `func_idx`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn push_pcall_frame(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    caller: &mut CallFrame,
    pc: *const crate::compiler::opcode::Instruction,
    closure: *mut crate::vm::upvalue::ObjClosure,
    func_idx: u32,
    num_args: u32,
    num_results: u16,
) -> Result<RegWindow, ElaraStatus> {
    let proto = &*(*closure).proto;
    let new_base = func_idx + 1;
    for i in num_args..u32::from(proto.num_params) {
        *fiber.stack.add((new_base + i) as usize) = Value::NIL;
    }
    let need = new_base + u32::from(proto.max_stack_size);
    fiber.open_top = fiber.open_top.max(need);
    fiber.ensure_stack(&mut state.alloc, need)?;
    caller.pc = pc;
    let new_frame = CallFrame {
        closure,
        pc: proto.code.cast_const(),
        base_idx: new_base,
        func_idx,
        num_results,
        vararg_count: num_args.saturating_sub(u32::from(proto.num_params)) as u16,
        flags: FRAME_FLAG_PCALL,
        reserved: [0; 3],
    };
    fiber.push_frame(&mut state.alloc, new_frame)?;
    let frame = &mut *fiber.frames.add(fiber.frame_count as usize - 1);
    Ok(RegWindow {
        base: fiber.stack.add(frame.base_idx as usize),
        k: (*(*frame.closure).proto).constants.cast_const(),
        pc: frame.pc,
    })
}

unsafe fn coerce_error_message(state: &mut ElaraState, msg: Value) -> Value {
    if msg.is_obj_type(ObjType::String) {
        return msg;
    }
    if let Some(i) = msg.to_i64() {
        let mut buf = [0u8; 32];
        let n = itoa_simple(i, &mut buf);
        if let Some(s) =
            state
                .strings
                .new_string(&mut state.alloc, &mut state.all_objects, 0, &buf[..n])
        {
            return Value::from_obj(s.cast(), ObjType::String);
        }
    }
    state.error.assert_failed_value()
}

fn itoa_simple(mut n: i64, buf: &mut [u8; 32]) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    let neg = n < 0;
    if neg {
        n = -n;
    }
    let mut i = 0;
    while n > 0 && i < 31 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    if neg && i < 31 {
        buf[i] = b'-';
        i += 1;
    }
    buf[..i].reverse();
    i
}
