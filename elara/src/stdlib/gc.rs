//! `gc` intrinsic module — incremental collect / timed step.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::CallContext;
use crate::gc::{gc_collect, gc_step_with_budget};
use crate::stdlib::util::{arg, argc, ret, table_set_fn};
use crate::value::nan::{ObjType, Value};
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 8)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();
    table_set_fn(state, tp, "collect", native_collect)?;
    table_set_fn(state, tp, "step", native_step)?;
    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        ObjType::Table,
    ))
}

unsafe extern "C" fn native_collect(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    gc_collect(s);
    ret(ctx, Value::NIL);
    ElaraStatus::Ok
}

unsafe extern "C" fn native_step(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if argc(ctx) >= 1
        && let Some(us) = arg(ctx, 0).to_i64()
        && us > 0
    {
        s.gc.pause_budget_us = us.min(u32::MAX as i64) as u32;
    }
    let done = gc_step_with_budget(s);
    ret(ctx, Value::from_bool(done));
    ElaraStatus::Ok
}
