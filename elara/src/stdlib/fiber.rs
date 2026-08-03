//! `fiber` intrinsic module: create / resume / yield / status.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::context::CallContext;
use crate::stdlib::util::{arg, argc, ret, ret_multi, runtime_err, table_set_fn};
use crate::value::{ObjType, Value};
use crate::vm::sched::{fiber_create, fiber_resume, fiber_status};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = crate::value::ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();
    table_set_fn(state, tp, "create", native_create)?;
    table_set_fn(state, tp, "resume", native_resume)?;
    table_set_fn(state, tp, "status", native_status)?;
    table_set_fn(state, tp, "yield", native_yield_stub)?;
    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        ObjType::Table,
    ))
}

unsafe extern "C" fn native_create(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if argc(ctx) < 1 {
        return runtime_err(s, "fiber.create expects a function");
    }
    match fiber_create(s, arg(ctx, 0)) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn native_status(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if argc(ctx) < 1 {
        return runtime_err(s, "fiber.status expects a fiber");
    }
    match fiber_status(s, arg(ctx, 0)) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn native_yield_stub(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let _ = ctx;
    runtime_err(
        &mut *state,
        "use the 'yield' statement (OP_YIELD) instead of fiber.yield()",
    )
}

/// Host-style resume from script: `caller = null` so yields return here with values.
unsafe extern "C" fn native_resume(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if argc(ctx) < 1 {
        return runtime_err(s, "fiber.resume expects a fiber");
    }
    let target = arg(ctx, 0);
    let n = argc(ctx);
    let mut args = [Value::NIL; 8];
    let nargs = (n.saturating_sub(1) as usize).min(8);
    for (i, slot) in args.iter_mut().enumerate().take(nargs) {
        *slot = arg(ctx, (i as u16) + 1);
    }
    let saved = s.current_fiber;
    let st = fiber_resume(s, core::ptr::null_mut(), target, &args[..nargs], 0, 0xFFFF);
    s.current_fiber = saved;

    if !target.is_obj_type(ObjType::Fiber) {
        return st;
    }
    let fiber = target.as_obj().cast::<crate::vm::fiber::ObjFiber>();
    match st {
        ElaraStatus::Yield | ElaraStatus::Ok => {
            let f = &*fiber;
            let slot = if st == ElaraStatus::Yield {
                f.resume_slot
            } else {
                0
            };
            let nret = if st == ElaraStatus::Yield {
                f.open_top.saturating_sub(slot)
            } else {
                f.open_top
            };
            let take = (nret as usize).min(8);
            let mut out = [Value::NIL; 8];
            for (i, slot_out) in out.iter_mut().enumerate().take(take) {
                *slot_out = *f.stack.add(slot as usize + i);
            }
            ret_multi(ctx, &out[..take]);
            ElaraStatus::Ok
        }
        other => other,
    }
}
