//! Shared helpers for intrinsic native registration and argument coercion.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::mem::size_of;
use core::ptr::NonNull;

use crate::ffi::CallContext;
use crate::ffi::native::{ElaraNativeFn, ObjNativeFn};
use crate::value::nan::{ObjBoxedI64, ObjType, Value};
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Interns `name` and stores `val` into `table`.
pub unsafe fn table_set(
    state: &mut ElaraState,
    table: *mut ObjTable,
    name: &str,
    val: Value,
) -> Result<(), ElaraStatus> {
    let key = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, name.as_bytes())
        .ok_or(ElaraStatus::ErrOom)?;
    let kv = Value::from_obj(key.cast(), ObjType::String);
    (*table)
        .set(&mut state.alloc, kv, val)
        .map_err(|_| ElaraStatus::ErrOom)?;
    Ok(())
}

/// Allocates a native fn and stores it under `name` on `table`.
pub unsafe fn table_set_fn(
    state: &mut ElaraState,
    table: *mut ObjTable,
    name: &str,
    f: ElaraNativeFn,
) -> Result<(), ElaraStatus> {
    let native = ObjNativeFn::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        f,
        core::ptr::null_mut(),
    )
    .ok_or(ElaraStatus::ErrOom)?;
    let v = Value::from_obj(native.cast(), ObjType::NativeFn);
    table_set(state, table, name, v)
}

/// Registers a global native function.
pub unsafe fn set_global_fn(
    state: &mut ElaraState,
    name: &str,
    f: ElaraNativeFn,
) -> Result<(), ElaraStatus> {
    ensure_globals(state)?;
    let native = ObjNativeFn::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        f,
        core::ptr::null_mut(),
    )
    .ok_or(ElaraStatus::ErrOom)?;
    let v = Value::from_obj(native.cast(), ObjType::NativeFn);
    table_set(state, state.globals, name, v)
}

pub unsafe fn ensure_globals(state: &mut ElaraState) -> Result<*mut ObjTable, ElaraStatus> {
    if !state.globals.is_null() {
        return Ok(state.globals);
    }
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 32)
        .ok_or(ElaraStatus::ErrOom)?;
    state.globals = t.as_ptr();
    Ok(state.globals)
}

/// Packs an `i64` into i48 or `ObjBoxedI64`.
pub unsafe fn value_from_i64(state: &mut ElaraState, i: i64) -> Result<Value, ElaraStatus> {
    if Value::fits_i48(i) {
        return Ok(Value::from_i48(i));
    }
    let raw = state
        .alloc
        .alloc_bytes(size_of::<ObjBoxedI64>())
        .ok_or(ElaraStatus::ErrOom)?;
    let ptr = raw.as_ptr().cast::<ObjBoxedI64>();
    core::ptr::write(ptr, ObjBoxedI64::new(i, 0));
    (*ptr).header.next = state.all_objects;
    state.all_objects = ptr.cast();
    Ok(Value::from_obj(
        NonNull::new(ptr.cast()).unwrap(),
        ObjType::BoxedI64,
    ))
}

pub unsafe fn arg(ctx: *mut CallContext, i: u16) -> Value {
    if ctx.is_null() {
        Value::NIL
    } else {
        (*ctx).arg(i)
    }
}

pub unsafe fn argc(ctx: *mut CallContext) -> u16 {
    if ctx.is_null() { 0 } else { (*ctx).argc() }
}

pub unsafe fn ret(ctx: *mut CallContext, v: Value) {
    if !ctx.is_null() {
        (*ctx).return_one(v);
    }
}

pub unsafe fn ret_multi(ctx: *mut CallContext, vals: &[Value]) {
    if !ctx.is_null() {
        (*ctx).return_multi(vals.as_ptr(), vals.len() as u16);
    }
}

pub fn as_f64(v: Value) -> Option<f64> {
    if let Some(i) = v.to_i64() {
        Some(i as f64)
    } else if v.is_float() {
        Some(v.as_f64())
    } else {
        None
    }
}

pub unsafe fn intern(state: &mut ElaraState, bytes: &[u8]) -> Result<Value, ElaraStatus> {
    let s = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, bytes)
        .ok_or(ElaraStatus::ErrOom)?;
    Ok(Value::from_obj(s.cast(), ObjType::String))
}

pub unsafe fn runtime_err(state: &mut ElaraState, msg: &'static str) -> ElaraStatus {
    state.last_error = Some(msg);
    state.error.last_status = ElaraStatus::ErrRuntime;
    if let Ok(v) = intern(state, msg.as_bytes()) {
        state.error.last_message = v;
    }
    ElaraStatus::ErrRuntime
}

pub fn type_name(v: Value) -> &'static str {
    if v.is_nil() {
        "nil"
    } else if v.is_bool() {
        "boolean"
    } else if v.is_int() {
        "int"
    } else if v.is_float() {
        "float"
    } else if v.is_obj_type(ObjType::String) {
        "string"
    } else if v.is_obj_type(ObjType::Table) {
        "table"
    } else if v.is_obj_type(ObjType::Closure) || v.is_obj_type(ObjType::NativeFn) {
        "function"
    } else if v.is_obj_type(ObjType::Fiber) {
        "fiber"
    } else if v.is_obj_type(ObjType::Userdata) {
        "userdata"
    } else {
        "nil"
    }
}

/// Formats `v` without metamethods (fallback / recursion base).
pub unsafe fn tostring_raw(state: &mut ElaraState, v: Value) -> Result<Value, ElaraStatus> {
    if v.is_obj_type(ObjType::String) {
        return Ok(v);
    }
    let s = if v.is_nil() {
        "nil".to_string()
    } else if v.is_bool() {
        if v.as_bool() {
            "true".into()
        } else {
            "false".into()
        }
    } else if let Some(i) = v.to_i64() {
        i.to_string()
    } else if v.is_float() {
        let f = v.as_f64();
        if f.is_nan() {
            "nan".into()
        } else if f.is_infinite() {
            if f.is_sign_negative() {
                "-inf".into()
            } else {
                "inf".into()
            }
        } else {
            format!("{f}")
        }
    } else {
        format!("<{}>", type_name(v))
    };
    intern(state, s.as_bytes())
}
