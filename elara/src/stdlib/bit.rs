//! `bit` intrinsic module (`ELARA_CAP_BIT`) — 48/64-bit bitwise ops.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::CallContext;
use crate::stdlib::util::{arg, ret, runtime_err, table_set_fn, value_from_i64};
use crate::value::nan::Value;
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 32)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();
    table_set_fn(state, tp, "band", bit_band)?;
    table_set_fn(state, tp, "bor", bit_bor)?;
    table_set_fn(state, tp, "bxor", bit_bxor)?;
    table_set_fn(state, tp, "bnot", bit_bnot)?;
    table_set_fn(state, tp, "shl", bit_shl)?;
    table_set_fn(state, tp, "shr", bit_shr)?;
    table_set_fn(state, tp, "ushr", bit_ushr)?;
    table_set_fn(state, tp, "rol", bit_rol)?;
    table_set_fn(state, tp, "ror", bit_ror)?;
    table_set_fn(state, tp, "count_ones", bit_count_ones)?;
    table_set_fn(state, tp, "leading_zeros", bit_leading_zeros)?;
    table_set_fn(state, tp, "trailing_zeros", bit_trailing_zeros)?;
    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        crate::value::ObjType::Table,
    ))
}

unsafe fn as_i64(state: &mut ElaraState, v: Value) -> Result<i64, ElaraStatus> {
    v.to_i64()
        .ok_or_else(|| runtime_err(state, "bit: integer expected"))
}

unsafe fn ret_i(state: &mut ElaraState, ctx: *mut CallContext, v: i64) -> ElaraStatus {
    match value_from_i64(state, v) {
        Ok(val) => {
            ret(ctx, val);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn bit_band(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ret_i(s, ctx, a & b)
}

unsafe extern "C" fn bit_bor(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ret_i(s, ctx, a | b)
}

unsafe extern "C" fn bit_bxor(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let b = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ret_i(s, ctx, a ^ b)
}

unsafe extern "C" fn bit_bnot(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ret_i(s, ctx, !a)
}

unsafe extern "C" fn bit_shl(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = (n as u32) & 63;
    ret_i(s, ctx, a.wrapping_shl(n))
}

unsafe extern "C" fn bit_shr(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = (n as u32) & 63;
    ret_i(s, ctx, a.wrapping_shr(n))
}

unsafe extern "C" fn bit_ushr(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v as u64,
        Err(e) => return e,
    };
    let n = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = (n as u32) & 63;
    ret_i(s, ctx, (a.wrapping_shr(n)) as i64)
}

unsafe extern "C" fn bit_rol(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v as u64,
        Err(e) => return e,
    };
    let n = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = (n as u32) & 63;
    ret_i(s, ctx, a.rotate_left(n) as i64)
}

unsafe extern "C" fn bit_ror(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v as u64,
        Err(e) => return e,
    };
    let n = match as_i64(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = (n as u32) & 63;
    ret_i(s, ctx, a.rotate_right(n) as i64)
}

unsafe extern "C" fn bit_count_ones(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v as u64,
        Err(e) => return e,
    };
    ret_i(s, ctx, i64::from(a.count_ones()))
}

unsafe extern "C" fn bit_leading_zeros(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v as u64,
        Err(e) => return e,
    };
    ret_i(s, ctx, i64::from(a.leading_zeros()))
}

unsafe extern "C" fn bit_trailing_zeros(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    let a = match as_i64(s, arg(ctx, 0)) {
        Ok(v) => v as u64,
        Err(e) => return e,
    };
    ret_i(s, ctx, i64::from(a.trailing_zeros()))
}
