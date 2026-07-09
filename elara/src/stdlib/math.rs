//! `math` intrinsic module (`ELARA_CAP_MATH`).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ffi::CallContext;
use crate::stdlib::util::{
    arg, argc, as_f64, ret, runtime_err, table_set, table_set_fn, value_from_i64,
};
use crate::value::nan::{ELARA_INT48_MAX, ELARA_INT48_MIN, Value};
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Builds the `math` exports table.
pub unsafe fn make_module(state: &mut ElaraState) -> Result<Value, ElaraStatus> {
    let t = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 64)
        .ok_or(ElaraStatus::ErrOom)?;
    let tp = t.as_ptr();

    table_set(state, tp, "pi", Value::from_f64(core::f64::consts::PI))?;
    table_set(state, tp, "huge", Value::from_f64(f64::INFINITY))?;
    table_set(state, tp, "max_i48", Value::from_i48(ELARA_INT48_MAX))?;
    table_set(state, tp, "min_i48", Value::from_i48(ELARA_INT48_MIN))?;
    let max_int = value_from_i64(state, i64::MAX)?;
    table_set(state, tp, "max_int", max_int)?;
    let min_int = value_from_i64(state, i64::MIN)?;
    table_set(state, tp, "min_int", min_int)?;

    table_set_fn(state, tp, "abs", math_abs)?;
    table_set_fn(state, tp, "floor", math_floor)?;
    table_set_fn(state, tp, "ceil", math_ceil)?;
    table_set_fn(state, tp, "round", math_round)?;
    table_set_fn(state, tp, "idiv", math_idiv)?;
    table_set_fn(state, tp, "sqrt", math_sqrt)?;
    table_set_fn(state, tp, "sin", math_sin)?;
    table_set_fn(state, tp, "cos", math_cos)?;
    table_set_fn(state, tp, "tan", math_tan)?;
    table_set_fn(state, tp, "asin", math_asin)?;
    table_set_fn(state, tp, "acos", math_acos)?;
    table_set_fn(state, tp, "atan2", math_atan2)?;
    table_set_fn(state, tp, "exp", math_exp)?;
    table_set_fn(state, tp, "log", math_log)?;
    table_set_fn(state, tp, "min", math_min)?;
    table_set_fn(state, tp, "max", math_max)?;
    table_set_fn(state, tp, "clamp", math_clamp)?;
    table_set_fn(state, tp, "random", math_random)?;
    table_set_fn(state, tp, "randomseed", math_randomseed)?;

    Ok(Value::from_obj(
        core::ptr::NonNull::new(tp.cast()).unwrap(),
        crate::value::ObjType::Table,
    ))
}

unsafe fn need_num(state: &mut ElaraState, v: Value) -> Result<f64, ElaraStatus> {
    as_f64(v).ok_or_else(|| runtime_err(state, "math: number expected"))
}

unsafe extern "C" fn math_abs(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    match need_num(s, arg(ctx, 0)) {
        Ok(x) => {
            if let Some(i) = arg(ctx, 0).to_i64() {
                match value_from_i64(s, i.saturating_abs()) {
                    Ok(v) => {
                        ret(ctx, v);
                        ElaraStatus::Ok
                    }
                    Err(e) => e,
                }
            } else {
                ret(ctx, Value::from_f64(x.abs()));
                ElaraStatus::Ok
            }
        }
        Err(e) => e,
    }
}

macro_rules! unary_f64 {
    ($name:ident, $op:expr) => {
        unsafe extern "C" fn $name(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
            let s = &mut *state;
            match need_num(s, arg(ctx, 0)) {
                Ok(x) => {
                    ret(ctx, Value::from_f64($op(x)));
                    ElaraStatus::Ok
                }
                Err(e) => e,
            }
        }
    };
}

unary_f64!(math_sqrt, f64::sqrt);
unary_f64!(math_sin, f64::sin);
unary_f64!(math_cos, f64::cos);
unary_f64!(math_tan, f64::tan);
unary_f64!(math_asin, f64::asin);
unary_f64!(math_acos, f64::acos);
unary_f64!(math_exp, f64::exp);

unsafe extern "C" fn math_floor(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    match need_num(s, arg(ctx, 0)) {
        Ok(x) => match value_from_i64(s, x.floor() as i64) {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        },
        Err(e) => e,
    }
}

unsafe extern "C" fn math_ceil(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    match need_num(s, arg(ctx, 0)) {
        Ok(x) => match value_from_i64(s, x.ceil() as i64) {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        },
        Err(e) => e,
    }
}

unsafe extern "C" fn math_round(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    match need_num(s, arg(ctx, 0)) {
        Ok(x) => match value_from_i64(s, x.round() as i64) {
            Ok(v) => {
                ret(ctx, v);
                ElaraStatus::Ok
            }
            Err(e) => e,
        },
        Err(e) => e,
    }
}

unsafe extern "C" fn math_idiv(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let a = match arg(ctx, 0)
        .to_i64()
        .or_else(|| as_f64(arg(ctx, 0)).map(|f| f as i64))
    {
        Some(v) => v,
        None => return runtime_err(s, "math: number expected"),
    };
    let b = match arg(ctx, 1)
        .to_i64()
        .or_else(|| as_f64(arg(ctx, 1)).map(|f| f as i64))
    {
        Some(v) => v,
        None => return runtime_err(s, "math: number expected"),
    };
    if b == 0 {
        return runtime_err(s, "Division by zero");
    }
    // Floor division toward -inf.
    let q = a.div_euclid(b);
    match value_from_i64(s, q) {
        Ok(v) => {
            ret(ctx, v);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn math_atan2(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let y = match need_num(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let x = match need_num(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ret(ctx, Value::from_f64(y.atan2(x)));
    ElaraStatus::Ok
}

unsafe extern "C" fn math_log(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let x = match need_num(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = if argc(ctx) >= 2 {
        let base = match need_num(s, arg(ctx, 1)) {
            Ok(v) => v,
            Err(e) => return e,
        };
        x.log(base)
    } else {
        x.ln()
    };
    ret(ctx, Value::from_f64(r));
    ElaraStatus::Ok
}

unsafe extern "C" fn math_min(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let n = argc(ctx);
    if n == 0 {
        return runtime_err(s, "math.min: missing arguments");
    }
    let mut best = match need_num(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    for i in 1..n {
        match need_num(s, arg(ctx, i)) {
            Ok(v) => best = best.min(v),
            Err(e) => return e,
        }
    }
    ret(ctx, Value::from_f64(best));
    ElaraStatus::Ok
}

unsafe extern "C" fn math_max(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let n = argc(ctx);
    if n == 0 {
        return runtime_err(s, "math.max: missing arguments");
    }
    let mut best = match need_num(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    for i in 1..n {
        match need_num(s, arg(ctx, i)) {
            Ok(v) => best = best.max(v),
            Err(e) => return e,
        }
    }
    ret(ctx, Value::from_f64(best));
    ElaraStatus::Ok
}

unsafe extern "C" fn math_clamp(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let x = match need_num(s, arg(ctx, 0)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let lo = match need_num(s, arg(ctx, 1)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let hi = match need_num(s, arg(ctx, 2)) {
        Ok(v) => v,
        Err(e) => return e,
    };
    ret(ctx, Value::from_f64(x.clamp(lo, hi)));
    ElaraStatus::Ok
}

/// xoshiro256** next.
fn rng_next(s: &mut [u64; 4]) -> u64 {
    let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
    let t = s[1] << 17;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = s[3].rotate_left(45);
    result
}

unsafe extern "C" fn math_random(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let n = argc(ctx);
    let r = rng_next(&mut s.rng);
    if n == 0 {
        // [0, 1) float
        let f = (r >> 11) as f64 / ((1u64 << 53) as f64);
        ret(ctx, Value::from_f64(f));
        return ElaraStatus::Ok;
    }
    if n == 1 {
        let max = match arg(ctx, 0).to_i64() {
            Some(m) if m >= 1 => m,
            _ => return runtime_err(s, "math.random: invalid interval"),
        };
        let v = (r % max as u64) as i64 + 1; // 1..=max (Lua-like) — spec says min?/max?; use 1..max when one arg
        // Spec: random(min?, max?) — with one arg treat as max inclusive from 1? Or 0..max?
        // Prefer inclusive 0..=max-1 for 0-based language: return 0..max-1
        let v0 = (r % max as u64) as i64;
        let _ = v;
        return match value_from_i64(s, v0) {
            Ok(iv) => {
                ret(ctx, iv);
                ElaraStatus::Ok
            }
            Err(e) => e,
        };
    }
    let min = match arg(ctx, 0).to_i64() {
        Some(v) => v,
        None => return runtime_err(s, "math.random: invalid interval"),
    };
    let max = match arg(ctx, 1).to_i64() {
        Some(v) => v,
        None => return runtime_err(s, "math.random: invalid interval"),
    };
    if max < min {
        return runtime_err(s, "math.random: invalid interval");
    }
    let span = (max - min) as u64 + 1;
    let v = min + (r % span) as i64;
    match value_from_i64(s, v) {
        Ok(iv) => {
            ret(ctx, iv);
            ElaraStatus::Ok
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn math_randomseed(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus {
    let s = &mut *state;
    let seed = arg(ctx, 0).to_i64().unwrap_or(0) as u64;
    // SplitMix64 expand into 4 lanes.
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut next = || {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = z;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    };
    s.rng = [next(), next(), next(), next()];
    if s.rng.iter().all(|&x| x == 0) {
        s.rng[0] = 1;
    }
    ret(ctx, Value::NIL);
    ElaraStatus::Ok
}
