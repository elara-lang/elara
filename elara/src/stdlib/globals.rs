//! Core global intrinsics: `type`, `tostring`, `tonumber`, `print`, `assert`, `panic`, `pcall`.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::io::Write;

use crate::ffi::CallContext;
use crate::ffi::sandbox::{ELARA_CAP_STDIO, has_capability};
use crate::stdlib::util::{
    arg, argc, as_f64, intern, ret, ret_multi, runtime_err, set_global_fn, tostring_raw, type_name,
    value_from_i64,
};
use crate::value::nan::{ObjType, Value};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

pub unsafe fn register(state: &mut ElaraState) -> Result<(), ElaraStatus> {
    set_global_fn(state, "type", native_type)?;
    set_global_fn(state, "tostring", native_tostring)?;
    set_global_fn(state, "tonumber", native_tonumber)?;
    set_global_fn(state, "print", native_print)?;
    set_global_fn(state, "assert", native_assert)?;
    set_global_fn(state, "panic", native_panic)?;
    set_global_fn(state, "pcall", native_pcall)?;
    Ok(())
}

unsafe extern "C" fn native_type(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let name = type_name(arg(ctx, 0));
    match intern(s, name.as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe fn tostring_value(state: &mut ElaraState, v: Value) -> Result<Value, ElaraStatus> {
    if (v.is_obj_type(ObjType::Table) || v.is_obj_type(ObjType::Userdata))
        && let Ok(key) = intern(state, b"__tostring")
    {
        let mt = if v.is_obj_type(ObjType::Table) {
            (*v.as_obj().cast::<ObjTable>()).metatable
        } else {
            core::ptr::null_mut()
        };
        if !mt.is_null() {
            let mm = (*mt).get(key);
            if mm.is_obj_type(ObjType::NativeFn) || mm.is_obj_type(ObjType::Closure) {
                let mut out = [Value::NIL; 1];
                match crate::ffi::c_api::call_value(state, mm, &[v], &mut out) {
                    Ok(_) if out[0].is_obj_type(ObjType::String) => return Ok(out[0]),
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    tostring_raw(state, v)
}

unsafe extern "C" fn native_tostring(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    match tostring_value(s, arg(ctx, 0)) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn native_tonumber(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let v = arg(ctx, 0);
    let base = if argc(ctx) >= 2 {
        match arg(ctx, 1).to_i64() {
            Some(b) if (2..=36).contains(&b) => b as u32,
            _ => {
                ret(ctx, Value::NIL);
                return ElaraStatus::Ok;
            }
        }
    } else {
        10
    };

    if let Some(n) = as_f64(v) {
        if base == 10 {
            if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                match value_from_i64(s, n as i64) {
                    Ok(iv) => {
                        ret(ctx, iv);
                        return ElaraStatus::Ok;
                    }
                    Err(e) => return e,
                }
            }
            ret(ctx, Value::from_f64(n));
            return ElaraStatus::Ok;
        }
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }

    if !v.is_obj_type(ObjType::String) {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }
    let text = (*v.as_obj().cast::<ObjString>()).as_str().trim();
    if text.is_empty() {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }

    if base == 10 {
        if let Ok(i) = text.parse::<i64>() {
            return match value_from_i64(s, i) {
                Ok(iv) => {
                    ret(ctx, iv);
                    ElaraStatus::Ok
                }
                Err(e) => e,
            };
        }
        if let Ok(f) = text.parse::<f64>() {
            ret(ctx, Value::from_f64(f));
            return ElaraStatus::Ok;
        }
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }

    match i64::from_str_radix(text, base) {
        Ok(i) => match value_from_i64(s, i) {
            Ok(iv) => {
                ret(ctx, iv);
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

unsafe extern "C" fn native_print(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if !has_capability(s, ELARA_CAP_STDIO) {
        return runtime_err(s, "sandbox: stdio capability denied");
    }
    let n = argc(ctx);
    let mut stdout = std::io::stdout().lock();
    for i in 0..n {
        if i > 0 {
            let _ = stdout.write_all(b"\t");
        }
        match tostring_value(s, arg(ctx, i)) {
            Ok(sv) => {
                let bytes = (*sv.as_obj().cast::<ObjString>()).as_bytes();
                let _ = stdout.write_all(bytes);
            }
            Err(e) => return e,
        }
    }
    let _ = stdout.write_all(b"\n");
    let _ = stdout.flush();
    ret(ctx, Value::NIL);
    ElaraStatus::Ok
}

unsafe extern "C" fn native_assert(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let cond = arg(ctx, 0);
    if cond.is_truthy() {
        let n = argc(ctx) as usize;
        let mut buf = [Value::NIL; 16];
        let take = n.min(16);
        for (i, slot) in buf.iter_mut().enumerate().take(take) {
            *slot = arg(ctx, i as u16);
        }
        ret_multi(ctx, &buf[..take]);
        return ElaraStatus::Ok;
    }
    let msg = if argc(ctx) >= 2 && !arg(ctx, 1).is_nil() {
        match tostring_value(s, arg(ctx, 1)) {
            Ok(v) => v,
            Err(e) => return e,
        }
    } else {
        s.error.assert_failed_value()
    };
    s.error.last_message = msg;
    s.error.last_status = ElaraStatus::ErrRuntime;
    s.last_error = Some("Assertion failed");
    ElaraStatus::ErrRuntime
}

unsafe extern "C" fn native_panic(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let msg = match tostring_value(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    s.error.last_message = msg;
    s.error.last_status = ElaraStatus::ErrRuntime;
    s.last_error = Some("runtime error");
    ElaraStatus::ErrRuntime
}

unsafe extern "C" fn native_pcall(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    if argc(ctx) == 0 {
        ret_multi(ctx, &[Value::FALSE, Value::NIL]);
        return ElaraStatus::Ok;
    }
    let f = arg(ctx, 0);
    let n = argc(ctx) as usize;
    let mut args = [Value::NIL; 15];
    let argc_call = n.saturating_sub(1).min(15);
    for (i, slot) in args.iter_mut().enumerate().take(argc_call) {
        *slot = arg(ctx, (i + 1) as u16);
    }
    let mut results = [Value::NIL; 15];
    match crate::ffi::c_api::call_value(s, f, &args[..argc_call], &mut results) {
        Ok(nr) => {
            let mut out = [Value::NIL; 16];
            out[0] = Value::TRUE;
            let copy = (nr as usize).min(15);
            out[1..1 + copy].copy_from_slice(&results[..copy]);
            ret_multi(ctx, &out[..1 + copy]);
            ElaraStatus::Ok
        }
        Err(_) => {
            let msg = if s.error.last_message.is_obj_type(ObjType::String) {
                s.error.last_message
            } else {
                intern(s, b"runtime error").unwrap_or(Value::NIL)
            };
            ret_multi(ctx, &[Value::FALSE, msg]);
            ElaraStatus::Ok
        }
    }
}
