//! Fiber create / resume / try_resume / yield / status — zero-C-stack switching.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::missing_safety_doc)]

use core::ptr::NonNull;

use crate::value::{ObjString, ObjType, Value};
use crate::vm::closure::ObjClosure;
use crate::vm::dispatch::execute;
use crate::vm::fiber::{FiberStatus, ObjFiber};
use crate::vm::frame::{CallFrame, FRAME_FLAG_C_BOUNDARY};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Outcome of `OP_YIELD` / [`fiber_yield`].
pub enum YieldOutcome {
    /// No caller — return [`ElaraStatus::Yield`] to the host.
    ToHost,
    /// Switched to `caller`; dispatch must reload the register window.
    SwitchTo(*mut ObjFiber),
}

/// Outcome when a fiber's bottom frame returns.
pub enum FinishOutcome {
    /// No caller — fiber is dead; return [`ElaraStatus::Ok`] to the host.
    ToHost,
    /// Results copied; switched to `caller`.
    SwitchTo(*mut ObjFiber),
}

/// `fiber.create(fn)` — allocates a suspended fiber with `closure` at `stack[0]`.
pub fn fiber_create(state: &mut ElaraState, closure: Value) -> Result<Value, ElaraStatus> {
    if !closure.is_obj_type(ObjType::Closure) {
        return Err(state.runtime_error("fiber.create expects a function"));
    }
    let c = closure.as_obj().cast::<ObjClosure>();
    let fiber = ObjFiber::create(&mut state.alloc, &mut state.all_objects, 0, c)?;
    Ok(Value::from_obj(fiber.cast(), ObjType::Fiber))
}

/// `fiber.status(f)` — interned `"suspended" | "running" | "normal" | "dead"`.
pub fn fiber_status(state: &mut ElaraState, fiber_v: Value) -> Result<Value, ElaraStatus> {
    if !fiber_v.is_obj_type(ObjType::Fiber) {
        return Err(state.runtime_error("fiber.status expects a fiber"));
    }
    let f = unsafe { &*fiber_v.as_obj().cast::<ObjFiber>() };
    let name = match f.status {
        FiberStatus::Suspended => "suspended",
        FiberStatus::Running => "running",
        FiberStatus::Normal => "normal",
        FiberStatus::Dead => "dead",
    };
    let s = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, name.as_bytes())
        .ok_or(ElaraStatus::ErrOom)?;
    Ok(Value::from_obj(s.cast(), ObjType::String))
}

/// Returns the UTF-8 status name for tests / host inspection.
#[must_use]
pub fn fiber_status_name(fiber: &ObjFiber) -> &'static str {
    match fiber.status {
        FiberStatus::Suspended => "suspended",
        FiberStatus::Running => "running",
        FiberStatus::Normal => "normal",
        FiberStatus::Dead => "dead",
    }
}

/// Result of `fiber.try_resume`: ok flag plus yielded/returned values or an error message.
pub struct TryResumeResult {
    pub ok: bool,
    pub values: [Value; 8],
    pub nvalues: usize,
    pub error_msg: Option<&'static str>,
}

/// `fiber.resume(target, args...)` — raises on a non-suspended target.
pub unsafe fn fiber_resume(
    state: &mut ElaraState,
    caller: *mut ObjFiber,
    target_v: Value,
    args: &[Value],
    dest_base: u32,
    expected_results: u16,
) -> ElaraStatus {
    match prepare_resume(
        state,
        caller,
        target_v,
        args,
        dest_base,
        expected_results,
        false,
    ) {
        Ok(target) => execute(state, target),
        Err(e) => e,
    }
}

/// `fiber.try_resume(target, args...)` — catches child errors as `(false, msg)`.
pub unsafe fn fiber_try_resume(
    state: &mut ElaraState,
    caller: *mut ObjFiber,
    target_v: Value,
    args: &[Value],
    dest_base: u32,
    expected_results: u16,
) -> TryResumeResult {
    state.try_resume_mode = true;
    let prepared = prepare_resume(
        state,
        caller,
        target_v,
        args,
        dest_base,
        expected_results,
        true,
    );
    let (st, target_ptr) = match prepared {
        Ok(target) => (execute(state, target), Some(target)),
        Err(e) => (e, None),
    };
    state.try_resume_mode = false;

    let mut out = TryResumeResult {
        ok: false,
        values: [Value::NIL; 8],
        nvalues: 0,
        error_msg: None,
    };
    match st {
        ElaraStatus::Ok | ElaraStatus::Yield => {
            out.ok = true;
            if !caller.is_null() {
                let c = &*caller;
                let n = if expected_results == 0xFFFF {
                    c.open_top.saturating_sub(dest_base) as usize
                } else {
                    expected_results as usize
                };
                out.nvalues = n.min(8);
                for i in 0..out.nvalues {
                    out.values[i] = *c.stack.add(dest_base as usize + i);
                }
            } else if let Some(target) = target_ptr {
                let t = &*target;
                let slot = if st == ElaraStatus::Yield {
                    t.resume_slot
                } else {
                    0
                };
                let n = if expected_results == 0xFFFF {
                    if st == ElaraStatus::Yield {
                        t.open_top.saturating_sub(slot) as usize
                    } else {
                        t.open_top as usize
                    }
                } else {
                    expected_results as usize
                };
                out.nvalues = n.min(8);
                for i in 0..out.nvalues {
                    out.values[i] = *t.stack.add(slot as usize + i);
                }
            }
        }
        _ => {
            out.ok = false;
            out.error_msg = state.last_error.or(Some("runtime error"));
        }
    }
    out
}

/// `fiber.yield(...)` — suspends `state.current_fiber` (host / intrinsic entry).
pub unsafe fn fiber_yield(
    state: &mut ElaraState,
    a: u8,
    b: u8,
) -> Result<YieldOutcome, ElaraStatus> {
    let fiber = state.current_fiber;
    if fiber.is_null() || (*fiber).frame_count == 0 {
        return Err(state.runtime_error("fiber.yield with no running fiber"));
    }
    let frame = &*(*fiber).frames.add((*fiber).frame_count as usize - 1);
    do_yield(state, fiber, frame, a, b)
}

/// Prepares `target` for execution and switches `state.current_fiber`.
pub unsafe fn prepare_resume(
    state: &mut ElaraState,
    caller: *mut ObjFiber,
    target_v: Value,
    args: &[Value],
    dest_base: u32,
    expected_results: u16,
    try_mode: bool,
) -> Result<*mut ObjFiber, ElaraStatus> {
    if !target_v.is_obj_type(ObjType::Fiber) {
        return Err(state.runtime_error("fiber.resume expects a fiber"));
    }
    let target = target_v.as_obj().cast::<ObjFiber>();
    let t = &mut *target;

    if t.status != FiberStatus::Suspended {
        if try_mode {
            state.last_error = Some("Cannot resume non-suspended fiber");
            return Err(ElaraStatus::ErrRuntime);
        }
        return Err(state.runtime_error("Cannot resume dead or running fiber"));
    }

    if !caller.is_null() {
        (*caller).status = FiberStatus::Normal;
        (*caller).yield_dest_base = dest_base;
        (*caller).yield_expected_results = expected_results;
    }
    t.caller = caller;

    if t.frame_count == 0 {
        for (i, arg) in args.iter().enumerate() {
            *t.stack.add(1 + i) = *arg;
        }
        t.open_top = 1 + args.len() as u32;
        t.stack_top = t.open_top;
        let closure = (*t.stack).as_obj().cast::<ObjClosure>();
        let proto = &*(*closure).proto;
        let need = 1 + u32::from(proto.max_stack_size);
        match t.ensure_stack(&mut state.alloc, need) {
            Err(ElaraStatus::ErrRuntime) => {
                return Err(state.runtime_error("Fiber stack overflow"));
            }
            Err(e) => return Err(e),
            Ok(()) => {}
        }
        match t.push_frame(&mut state.alloc, CallFrame::new(closure, 1, 0, 0xFFFF)) {
            Err(ElaraStatus::ErrRuntime) => {
                return Err(state.runtime_error("Fiber stack overflow"));
            }
            Err(e) => return Err(e),
            Ok(()) => {}
        }
    } else {
        let slot = t.resume_slot;
        let need = slot + args.len() as u32;
        match t.ensure_stack(&mut state.alloc, need.max(t.open_top)) {
            Err(ElaraStatus::ErrRuntime) => {
                return Err(state.runtime_error("Fiber stack overflow"));
            }
            Err(e) => return Err(e),
            Ok(()) => {}
        }
        for (i, arg) in args.iter().enumerate() {
            *t.stack.add(slot as usize + i) = *arg;
        }
        t.open_top = t.open_top.max(slot + args.len() as u32);
    }

    t.status = FiberStatus::Running;
    state.current_fiber = target;
    Ok(target)
}

/// `OP_YIELD A, B` implementation.
pub unsafe fn do_yield(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
    frame: &CallFrame,
    a: u8,
    b: u8,
) -> Result<YieldOutcome, ElaraStatus> {
    let f = &mut *fiber;
    for i in 0..f.frame_count {
        if ((*f.frames.add(i as usize)).flags & FRAME_FLAG_C_BOUNDARY) != 0 {
            return Err(state.runtime_error("Cannot yield across C-FFI call boundary"));
        }
    }

    let abs_a = frame.base_idx + u32::from(a);
    let n = if b == 0 {
        f.open_top.saturating_sub(abs_a)
    } else {
        u32::from(b) - 1
    };
    f.resume_slot = abs_a;

    let caller = f.caller;
    if caller.is_null() {
        f.status = FiberStatus::Suspended;
        f.open_top = abs_a + n;
        return Ok(YieldOutcome::ToHost);
    }

    let c = &mut *caller;
    let dest = c.yield_dest_base;
    let expect = c.yield_expected_results;
    let want = if expect == 0xFFFF {
        n
    } else {
        u32::from(expect)
    };
    match c.ensure_stack(&mut state.alloc, dest + want) {
        Err(ElaraStatus::ErrRuntime) => {
            return Err(state.runtime_error("Fiber stack overflow"));
        }
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    let copy_n = n.min(want);
    for i in 0..copy_n {
        *c.stack.add(dest as usize + i as usize) = *f.stack.add(abs_a as usize + i as usize);
    }
    for i in copy_n..want {
        *c.stack.add(dest as usize + i as usize) = Value::NIL;
    }
    c.open_top = c.open_top.max(dest + want);

    f.status = FiberStatus::Suspended;
    f.caller = core::ptr::null_mut();
    c.status = FiberStatus::Running;
    state.current_fiber = caller;
    Ok(YieldOutcome::SwitchTo(caller))
}

/// Completes a fiber after its bottom `OP_RETURN` (marks dead, resumes caller if any).
pub unsafe fn finish_fiber_return(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
) -> Result<FinishOutcome, ElaraStatus> {
    let f = &mut *fiber;
    f.status = FiberStatus::Dead;
    let caller = f.caller;
    f.caller = core::ptr::null_mut();
    if caller.is_null() {
        return Ok(FinishOutcome::ToHost);
    }

    let c = &mut *caller;
    let dest = c.yield_dest_base;
    let expect = c.yield_expected_results;
    let n = f.open_top;
    let want = if expect == 0xFFFF {
        n
    } else {
        u32::from(expect)
    };
    match c.ensure_stack(&mut state.alloc, dest + want) {
        Err(ElaraStatus::ErrRuntime) => {
            return Err(state.runtime_error("Fiber stack overflow"));
        }
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    let copy_n = n.min(want);
    for i in 0..copy_n {
        *c.stack.add(dest as usize + i as usize) = *f.stack.add(i as usize);
    }
    for i in copy_n..want {
        *c.stack.add(dest as usize + i as usize) = Value::NIL;
    }
    c.open_top = c.open_top.max(dest + want);
    c.status = FiberStatus::Running;
    state.current_fiber = caller;
    Ok(FinishOutcome::SwitchTo(caller))
}

/// Uncaught error at the bottom of `fiber`: terminate, optionally restore `caller`.
pub unsafe fn handle_fiber_error(
    state: &mut ElaraState,
    fiber: *mut ObjFiber,
    err: ElaraStatus,
) -> ElaraStatus {
    let f = &mut *fiber;
    let caller = f.caller;
    let _ = f.terminate(state);
    f.caller = core::ptr::null_mut();
    if !caller.is_null() {
        (*caller).status = FiberStatus::Running;
        state.current_fiber = caller;
    }
    if state.last_error.is_none() {
        state.last_error = Some("runtime error");
    }
    let _ = err;
    err
}

/// Reads an interned / heap string value as `&str` (test helper).
#[must_use]
pub unsafe fn value_as_str<'a>(v: Value) -> Option<&'a str> {
    if !v.is_obj_type(ObjType::String) {
        return None;
    }
    let s = &*v.as_obj().cast::<ObjString>();
    Some(s.as_str())
}

/// Builds a [`Value`] tag for an existing fiber pointer.
#[must_use]
pub fn fiber_value(fiber: *mut ObjFiber) -> Value {
    Value::from_obj(
        NonNull::new(fiber.cast()).expect("non-null fiber"),
        ObjType::Fiber,
    )
}
