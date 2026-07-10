//! `table` intrinsic module (`ELARA_CAP_TABLE`) — strict 0-based array ops.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::CallContext;
use crate::stdlib::util::{
    arg, argc, intern, ret, runtime_err, table_set_fn, tostring_raw, value_from_i64,
};
use crate::value::nan::{ObjType, Value};
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Builds the `table` exports table.
pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 32)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();
    table_set_fn(state, tp, "push", table_push)?;
    table_set_fn(state, tp, "pop", table_pop)?;
    table_set_fn(state, tp, "insert", table_insert)?;
    table_set_fn(state, tp, "remove", table_remove)?;
    table_set_fn(state, tp, "concat", table_concat)?;
    table_set_fn(state, tp, "sort", table_sort)?;
    table_set_fn(state, tp, "keys", table_keys)?;
    table_set_fn(state, tp, "values", table_values)?;
    table_set_fn(state, tp, "clone", table_clone)?;
    table_set_fn(state, tp, "set_metatable", table_set_metatable)?;
    table_set_fn(state, tp, "get_metatable", table_get_metatable)?;
    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        ObjType::Table,
    ))
}

unsafe fn as_table(state: &mut ElaraState, v: Value) -> Result<*mut ObjTable, ElaraStatus> {
    if !v.is_obj_type(ObjType::Table) {
        return Err(runtime_err(state, "table: table expected"));
    }
    Ok(v.as_obj().cast())
}

fn norm_idx(idx: i64, len: u32) -> i64 {
    if idx < 0 { idx + i64::from(len) } else { idx }
}

unsafe extern "C" fn table_push(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let val = arg(ctx, 1);
    match (*t).push(&mut s.alloc, val) {
        Ok(len) => match value_from_i64(s, i64::from(len)) {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        },
        Err(_) => ElaraStatus::ErrOom,
    }
}

unsafe extern "C" fn table_pop(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    ret(ctx, (*t).pop());
    ElaraStatus::Ok
}

unsafe extern "C" fn table_insert(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let len = (*t).array_len;
    let idx = match arg(ctx, 1).to_i64() {
        Some(i) => norm_idx(i, len),
        None => return runtime_err(s, "table.insert: index expected"),
    };
    if idx < 0 || idx > i64::from(len) {
        return runtime_err(s, "table.insert: index out of bounds");
    }
    let idx = idx as u32;
    let val = arg(ctx, 2);
    // Shift right: move len-1 .. idx down by one toward higher indices.
    let mut i = len;
    while i > idx {
        let prev = (*t).get_int(i64::from(i - 1));
        if (*t)
            .set(&mut s.alloc, Value::from_i48(i64::from(i)), prev)
            .is_err()
        {
            return ElaraStatus::ErrOom;
        }
        i -= 1;
    }
    if (*t)
        .set(&mut s.alloc, Value::from_i48(i64::from(idx)), val)
        .is_err()
    {
        return ElaraStatus::ErrOom;
    }
    ret(ctx, Value::NIL);
    ElaraStatus::Ok
}

unsafe extern "C" fn table_remove(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let len = (*t).array_len;
    if len == 0 {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }
    let idx = if argc(ctx) >= 2 {
        match arg(ctx, 1).to_i64() {
            Some(i) => norm_idx(i, len),
            None => return runtime_err(s, "table.remove: index expected"),
        }
    } else {
        i64::from(len) - 1
    };
    if idx < 0 || idx >= i64::from(len) {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }
    let idx = idx as u32;
    let removed = (*t).get_int(i64::from(idx));
    let mut i = idx;
    while i + 1 < len {
        let next = (*t).get_int(i64::from(i + 1));
        let _ = (*t).set(&mut s.alloc, Value::from_i48(i64::from(i)), next);
        i += 1;
    }
    let _ = (*t).set(
        &mut s.alloc,
        Value::from_i48(i64::from(len - 1)),
        Value::NIL,
    );
    ret(ctx, removed);
    ElaraStatus::Ok
}

unsafe extern "C" fn table_concat(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let len = (*t).array_len;
    let sep = if argc(ctx) >= 2 && arg(ctx, 1).is_obj_type(ObjType::String) {
        (*arg(ctx, 1).as_obj().cast::<crate::value::ObjString>()).as_str()
    } else {
        ""
    };
    let start = if argc(ctx) >= 3 {
        match arg(ctx, 2).to_i64() {
            Some(i) => norm_idx(i, len).max(0) as u32,
            None => 0,
        }
    } else {
        0
    };
    let end = if argc(ctx) >= 4 {
        match arg(ctx, 3).to_i64() {
            Some(i) => norm_idx(i, len).clamp(0, i64::from(len)) as u32,
            None => len,
        }
    } else {
        len
    };
    let mut out = String::new();
    let mut first = true;
    let mut i = start;
    while i < end {
        if !first {
            out.push_str(sep);
        }
        first = false;
        let v = (*t).get_int(i64::from(i));
        match tostring_raw(s, v) {
            Ok(sv) => out.push_str((*sv.as_obj().cast::<crate::value::ObjString>()).as_str()),
            Err(e) => return e,
        }
        i += 1;
    }
    match intern(s, out.as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

fn default_less(a: Value, b: Value) -> Result<bool, ElaraStatus> {
    if let (Some(ia), Some(ib)) = (a.to_i64(), b.to_i64()) {
        return Ok(ia < ib);
    }
    match (
        a.to_i64()
            .map(|i| i as f64)
            .or_else(|| a.is_float().then(|| a.as_f64())),
        b.to_i64()
            .map(|i| i as f64)
            .or_else(|| b.is_float().then(|| b.as_f64())),
    ) {
        (Some(fa), Some(fb)) => Ok(fa < fb),
        _ if a.is_obj_type(ObjType::String) && b.is_obj_type(ObjType::String) => {
            let sa = unsafe { (*a.as_obj().cast::<crate::value::ObjString>()).as_bytes() };
            let sb = unsafe { (*b.as_obj().cast::<crate::value::ObjString>()).as_bytes() };
            Ok(sa < sb)
        }
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

unsafe fn cmp_call(
    state: &mut ElaraState,
    cmp: Value,
    a: Value,
    b: Value,
) -> Result<bool, ElaraStatus> {
    if cmp.is_nil() {
        return default_less(a, b);
    }
    let mut out = [Value::NIL; 1];
    crate::ffi::c_api::call_value(state, cmp, &[a, b], &mut out)?;
    Ok(out[0].is_truthy())
}

unsafe extern "C" fn table_sort(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cmp = if argc(ctx) >= 2 {
        arg(ctx, 1)
    } else {
        Value::NIL
    };
    let n = (*t).array_len as usize;
    let mut vals = Vec::with_capacity(n);
    for i in 0..n {
        vals.push((*t).get_int(i as i64));
    }
    // Stable insertion sort (no external crates).
    for i in 1..vals.len() {
        let mut j = i;
        while j > 0 {
            let less = match cmp_call(s, cmp, vals[j], vals[j - 1]) {
                Ok(b) => b,
                Err(e) => return e,
            };
            if less {
                vals.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
    }
    for (i, v) in vals.into_iter().enumerate() {
        if (*t)
            .set(&mut s.alloc, Value::from_i48(i as i64), v)
            .is_err()
        {
            return ElaraStatus::ErrOom;
        }
    }
    ret(ctx, Value::NIL);
    ElaraStatus::Ok
}

unsafe extern "C" fn table_keys(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let out =
        ObjTable::new(&mut s.alloc, &mut s.all_objects, 0, 1, 0, 16).ok_or(ElaraStatus::ErrOom);
    let out = match out {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut i = 0u32;
    let mut cursor = 0u32;
    while let Some((next, key, _val)) = (*t).next_entry(cursor) {
        if (*out.as_ptr())
            .set(&mut s.alloc, Value::from_i48(i64::from(i)), key)
            .is_err()
        {
            return ElaraStatus::ErrOom;
        }
        i += 1;
        cursor = next;
    }
    ret(
        ctx,
        Value::from_obj(
            core::ptr::NonNull::new(out.as_ptr().cast()).unwrap(),
            ObjType::Table,
        ),
    );
    ElaraStatus::Ok
}

unsafe extern "C" fn table_values(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let out = match ObjTable::new(&mut s.alloc, &mut s.all_objects, 0, 1, 0, 16) {
        Some(p) => p,
        None => return ElaraStatus::ErrOom,
    };
    let mut i = 0u32;
    let mut cursor = 0u32;
    while let Some((next, _key, val)) = (*t).next_entry(cursor) {
        if (*out.as_ptr())
            .set(&mut s.alloc, Value::from_i48(i64::from(i)), val)
            .is_err()
        {
            return ElaraStatus::ErrOom;
        }
        i += 1;
        cursor = next;
    }
    ret(
        ctx,
        Value::from_obj(
            core::ptr::NonNull::new(out.as_ptr().cast()).unwrap(),
            ObjType::Table,
        ),
    );
    ElaraStatus::Ok
}

unsafe extern "C" fn table_clone(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let out = match ObjTable::new(&mut s.alloc, &mut s.all_objects, 0, 1, (*t).array_len, 16) {
        Some(p) => p,
        None => return ElaraStatus::ErrOom,
    };
    let mut cursor = 0u32;
    while let Some((next, key, val)) = (*t).next_entry(cursor) {
        if (*out.as_ptr()).set(&mut s.alloc, key, val).is_err() {
            return ElaraStatus::ErrOom;
        }
        cursor = next;
    }
    (*out.as_ptr()).metatable = (*t).metatable;
    ret(
        ctx,
        Value::from_obj(
            core::ptr::NonNull::new(out.as_ptr().cast()).unwrap(),
            ObjType::Table,
        ),
    );
    ElaraStatus::Ok
}

unsafe extern "C" fn table_set_metatable(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mt = arg(ctx, 1);
    if mt.is_nil() {
        (*t).metatable = core::ptr::null_mut();
    } else if mt.is_obj_type(ObjType::Table) {
        (*t).metatable = mt.as_obj().cast();
    } else {
        return runtime_err(s, "table.set_metatable: table or nil expected");
    }
    ret(ctx, arg(ctx, 0));
    ElaraStatus::Ok
}

unsafe extern "C" fn table_get_metatable(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    let t = match as_table(s, arg(ctx, 0)) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if (*t).metatable.is_null() {
        ret(ctx, Value::NIL);
    } else {
        ret(
            ctx,
            Value::from_obj(
                core::ptr::NonNull::new((*t).metatable.cast()).unwrap(),
                ObjType::Table,
            ),
        );
    }
    ElaraStatus::Ok
}
