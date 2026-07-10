//! `string` intrinsic module (`ELARA_CAP_STRING`) — strict 0-based byte indexing.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::CallContext;
use crate::stdlib::util::{arg, argc, intern, ret, runtime_err, table_set_fn, value_from_i64};
use crate::value::nan::{ObjType, Value};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Builds the `string` exports table.
pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 32)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();
    table_set_fn(state, tp, "len", string_len)?;
    table_set_fn(state, tp, "slice", string_slice)?;
    table_set_fn(state, tp, "byte", string_byte)?;
    table_set_fn(state, tp, "char", string_char)?;
    table_set_fn(state, tp, "find", string_find)?;
    table_set_fn(state, tp, "replace", string_replace)?;
    table_set_fn(state, tp, "split", string_split)?;
    table_set_fn(state, tp, "upper", string_upper)?;
    table_set_fn(state, tp, "lower", string_lower)?;
    table_set_fn(state, tp, "trim", string_trim)?;
    table_set_fn(state, tp, "starts_with", string_starts_with)?;
    table_set_fn(state, tp, "ends_with", string_ends_with)?;
    table_set_fn(state, tp, "rep", string_rep)?;
    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        ObjType::Table,
    ))
}

unsafe fn as_str<'a>(state: &mut ElaraState, v: Value) -> Result<&'a str, ElaraStatus> {
    if !v.is_obj_type(ObjType::String) {
        return Err(runtime_err(state, "string: string expected"));
    }
    // SAFETY: caller must not drop `v` / collect while the slice is used.
    Ok((*v.as_obj().cast::<ObjString>()).as_str())
}

fn norm_idx(idx: i64, len: usize) -> i64 {
    if idx < 0 { idx + len as i64 } else { idx }
}

unsafe extern "C" fn string_len(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    match value_from_i64(s, text.len() as i64) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_slice(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let len = text.len();
    let start = match arg(ctx, 1).to_i64() {
        Some(i) => norm_idx(i, len),
        None => return runtime_err(s, "string.slice: start expected"),
    };
    let end = if argc(ctx) >= 3 {
        match arg(ctx, 2).to_i64() {
            Some(i) => norm_idx(i, len),
            None => return runtime_err(s, "string.slice: end expected"),
        }
    } else {
        len as i64
    };
    let start = start.clamp(0, len as i64) as usize;
    let end = end.clamp(0, len as i64) as usize;
    let slice = if start <= end {
        &text.as_bytes()[start..end]
    } else {
        &[][..]
    };
    match intern(s, slice) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_byte(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let idx = if argc(ctx) >= 2 {
        match arg(ctx, 1).to_i64() {
            Some(i) => norm_idx(i, text.len()),
            None => {
                ret(ctx, Value::NIL);
                return ElaraStatus::Ok;
            }
        }
    } else {
        0
    };
    if idx < 0 || idx as usize >= text.len() {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }
    match value_from_i64(s, i64::from(text.as_bytes()[idx as usize])) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_char(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let n = argc(ctx);
    let mut buf = Vec::with_capacity(n as usize);
    for i in 0..n {
        let b = match arg(ctx, i).to_i64() {
            Some(v) if (0..=255).contains(&v) => v as u8,
            _ => return runtime_err(s, "string.char: byte 0..=255 expected"),
        };
        buf.push(b);
    }
    match intern(s, &buf) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_find(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let hay = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let needle = match as_str(s, arg(ctx, 1)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let start = if argc(ctx) >= 3 {
        match arg(ctx, 2).to_i64() {
            Some(i) => norm_idx(i, hay.len()).max(0) as usize,
            None => 0,
        }
    } else {
        0
    };
    if start > hay.len() {
        ret(ctx, Value::NIL);
        return ElaraStatus::Ok;
    }
    match hay[start..].find(needle) {
        Some(rel) => match value_from_i64(s, (start + rel) as i64) {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        },
        None => {
            ret(ctx, Value::NIL);
            ElaraStatus::Ok
        }
    }
}

unsafe extern "C" fn string_replace(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let hay = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let target = match as_str(s, arg(ctx, 1)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let repl = match as_str(s, arg(ctx, 2)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let out = if target.is_empty() {
        hay.to_string()
    } else {
        hay.replace(target, repl)
    };
    match intern(s, out.as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_split(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let hay = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let sep = match as_str(s, arg(ctx, 1)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let parts: Vec<&str> = if sep.is_empty() {
        hay.split("").filter(|p| !p.is_empty()).collect()
    } else {
        hay.split(sep).collect()
    };
    let t = ObjTable::new(
        &mut s.alloc,
        &mut s.all_objects,
        0,
        1,
        parts.len() as u32,
        0,
    )
    .ok_or(ElaraStatus::ErrOom);
    let t = match t {
        Ok(p) => p,
        Err(e) => return e,
    };
    for (i, part) in parts.iter().enumerate() {
        let sv = match intern(s, part.as_bytes()) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if (*t.as_ptr())
            .set(&mut s.alloc, Value::from_i48(i as i64), sv)
            .is_err()
        {
            return ElaraStatus::ErrOom;
        }
    }
    ret(
        ctx,
        Value::from_obj(
            core::ptr::NonNull::new(t.as_ptr().cast()).unwrap(),
            ObjType::Table,
        ),
    );
    ElaraStatus::Ok
}

unsafe extern "C" fn string_upper(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let out = text.to_ascii_uppercase();
    match intern(s, out.as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_lower(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let out = text.to_ascii_lowercase();
    match intern(s, out.as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_trim(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    match intern(s, text.trim().as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn string_starts_with(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let prefix = match as_str(s, arg(ctx, 1)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    ret(ctx, Value::from_bool(text.starts_with(prefix)));
    ElaraStatus::Ok
}

unsafe extern "C" fn string_ends_with(
    state: *mut ElaraState,
    ctx: *mut CallContext,
) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let suffix = match as_str(s, arg(ctx, 1)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    ret(ctx, Value::from_bool(text.ends_with(suffix)));
    ElaraStatus::Ok
}

unsafe extern "C" fn string_rep(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let text = match as_str(s, arg(ctx, 0)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let n = match arg(ctx, 1).to_i64() {
        Some(v) if v >= 0 => v as usize,
        _ => return runtime_err(s, "string.rep: non-negative count expected"),
    };
    let sep = if argc(ctx) >= 3 {
        match as_str(s, arg(ctx, 2)) {
            Ok(t) => t,
            Err(e) => return e,
        }
    } else {
        ""
    };
    if n == 0 {
        return match intern(s, b"") {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        };
    }
    let mut out = String::with_capacity(text.len() * n + sep.len() * n.saturating_sub(1));
    for i in 0..n {
        if i > 0 {
            out.push_str(sep);
        }
        out.push_str(text);
    }
    match intern(s, out.as_bytes()) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}
