//! `sys` intrinsic module (clock/telemetry always; env/args/exit need `ELARA_CAP_SYS_ENV`).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::CallContext;
use crate::ffi::sandbox::{ELARA_CAP_SYS_ENV, has_capability};
use crate::stdlib::util::{
    arg, argc, intern, ret, runtime_err, table_set, table_set_fn, value_from_i64,
};
use crate::value::nan::{ObjType, Value};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 16)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();

    table_set_fn(state, tp, "clock_ns", sys_clock_ns)?;
    table_set_fn(state, tp, "mem_used", sys_mem_used)?;
    table_set_fn(state, tp, "gas_remaining", sys_gas_remaining)?;
    let ver = intern(state, crate::version().as_bytes())?;
    table_set(state, tp, "version", ver)?;

    if has_capability(state, ELARA_CAP_SYS_ENV) {
        table_set_fn(state, tp, "env", sys_env)?;
        table_set_fn(state, tp, "exit", sys_exit)?;
        let args_t = build_args_table(state)?;
        table_set(state, tp, "args", args_t)?;
    }

    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        ObjType::Table,
    ))
}

unsafe fn build_args_table(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let args: Vec<String> = state.cli_args.clone();
    let t = ObjTable::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        1,
        args.len() as u32,
        0,
    )
    .ok_or(ElaraStatus::ErrOom)?;
    for (i, a) in args.iter().enumerate() {
        let sv = intern(state, a.as_bytes())?;
        (*t.as_ptr())
            .set(&mut state.alloc, Value::from_i48(i as i64), sv)
            .map_err(|_| ElaraStatus::ErrOom)?;
    }
    Ok(Value::from_obj(
        core::ptr::NonNull::new(t.as_ptr().cast()).unwrap(),
        ObjType::Table,
    ))
}

unsafe extern "C" fn sys_clock_ns(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let ns = s.start_instant.elapsed().as_nanos().min(i64::MAX as u128) as i64;
    match value_from_i64(s, ns) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn sys_mem_used(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    match value_from_i64(s, s.alloc.bytes_allocated as i64) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn sys_gas_remaining(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    match value_from_i64(s, s.gas_remaining) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn sys_env(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if !has_capability(s, ELARA_CAP_SYS_ENV) {
        return runtime_err(s, "sandbox: sys.env capability denied");
    }
    let name_v = arg(ctx, 0);
    if !name_v.is_obj_type(ObjType::String) {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }
    let name = (*name_v.as_obj().cast::<ObjString>()).as_str();
    match std::env::var(name) {
        Ok(val) => match intern(s, val.as_bytes()) {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        },
        Err(_) => {
            ret(ctx, Value::NIL);
            ElaraStatus::Ok
        }
    }
}

unsafe extern "C" fn sys_exit(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if !has_capability(s, ELARA_CAP_SYS_ENV) {
        return runtime_err(s, "sandbox: sys.exit capability denied");
    }
    let code = if argc(ctx) >= 1 {
        arg(ctx, 0).to_i64().unwrap_or(0) as i32
    } else {
        0
    };
    s.exit_code = Some(code);
    s.last_error = Some("sys.exit");
    s.error.last_status = ElaraStatus::ErrRuntime;
    ElaraStatus::ErrRuntime
}
