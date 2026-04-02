//! Integer / float arithmetic fast-paths for the dispatch loop.

use crate::value::{ELARA_INT48_MAX, ELARA_INT48_MIN, Value};
use crate::vm::status::ElaraStatus;

/// Resolves `RK(C)`: register when `c < 128`, else constant `K[c & 0x7F]`.
#[inline(always)]
pub unsafe fn rkc(base: *mut Value, k: *const Value, c: u8) -> Value {
    if c & 0x80 != 0 {
        // SAFETY: verifier / compiler guarantee `c & 0x7F` is in-range for `k`.
        unsafe { *k.add((c & 0x7F) as usize) }
    } else {
        unsafe { *base.add(c as usize) }
    }
}

#[inline]
pub fn bin_add(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a.wrapping_add(b))),
        _ => match (as_f64(vb), as_f64(vc)) {
            (Some(a), Some(b)) => Ok(Value::from_f64(a + b)),
            _ => Err(ElaraStatus::ErrRuntime),
        },
    }
}

#[inline]
pub fn bin_sub(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a.wrapping_sub(b))),
        _ => match (as_f64(vb), as_f64(vc)) {
            (Some(a), Some(b)) => Ok(Value::from_f64(a - b)),
            _ => Err(ElaraStatus::ErrRuntime),
        },
    }
}

#[inline]
pub fn bin_mul(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a.wrapping_mul(b))),
        _ => match (as_f64(vb), as_f64(vc)) {
            (Some(a), Some(b)) => Ok(Value::from_f64(a * b)),
            _ => Err(ElaraStatus::ErrRuntime),
        },
    }
}

#[inline]
pub fn bin_div(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    let (Some(a), Some(b)) = (as_f64(vb), as_f64(vc)) else {
        return Err(ElaraStatus::ErrRuntime);
    };
    if b == 0.0 {
        return Err(ElaraStatus::ErrRuntime);
    }
    Ok(Value::from_f64(a / b))
}

#[inline]
pub fn bin_mod(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(_), Some(0)) => Err(ElaraStatus::ErrRuntime),
        (Some(a), Some(b)) => Ok(int_result(a.wrapping_rem(b))),
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

#[inline]
pub fn bin_band(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a & b)),
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

#[inline]
pub fn bin_bor(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a | b)),
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

#[inline]
pub fn bin_bxor(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a ^ b)),
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

#[inline]
pub fn bin_shl(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a.wrapping_shl((b as u32) & 63))),
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

#[inline]
pub fn bin_shr(vb: Value, vc: Value) -> Result<Value, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(int_result(a.wrapping_shr((b as u32) & 63))),
        _ => Err(ElaraStatus::ErrRuntime),
    }
}

#[inline]
pub fn un_neg(v: Value) -> Result<Value, ElaraStatus> {
    if let Some(i) = v.to_i64() {
        Ok(int_result(i.wrapping_neg()))
    } else if v.is_float() {
        Ok(Value::from_f64(-v.as_f64()))
    } else {
        Err(ElaraStatus::ErrRuntime)
    }
}

#[inline]
pub fn un_bnot(v: Value) -> Result<Value, ElaraStatus> {
    v.to_i64()
        .map(|i| int_result(!i))
        .ok_or(ElaraStatus::ErrRuntime)
}

#[inline]
pub fn cmp_lt(vb: Value, vc: Value) -> Result<bool, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(a < b),
        _ => match (as_f64(vb), as_f64(vc)) {
            (Some(a), Some(b)) => Ok(a < b),
            _ => Err(ElaraStatus::ErrRuntime),
        },
    }
}

#[inline]
pub fn cmp_le(vb: Value, vc: Value) -> Result<bool, ElaraStatus> {
    match (vb.to_i64(), vc.to_i64()) {
        (Some(a), Some(b)) => Ok(a <= b),
        _ => match (as_f64(vb), as_f64(vc)) {
            (Some(a), Some(b)) => Ok(a <= b),
            _ => Err(ElaraStatus::ErrRuntime),
        },
    }
}

#[inline]
fn int_result(i: i64) -> Value {
    if (ELARA_INT48_MIN..=ELARA_INT48_MAX).contains(&i) {
        Value::from_i48(i)
    } else {
        // Full boxed path deferred; wrap via i48 when possible, else f64 lossy fallback avoided.
        // For values outside i48, still use from_i48 only when fits; otherwise keep as float.
        Value::from_f64(i as f64)
    }
}

#[inline]
fn as_f64(v: Value) -> Option<f64> {
    if let Some(i) = v.to_i64() {
        Some(i as f64)
    } else if v.is_float() {
        Some(v.as_f64())
    } else {
        None
    }
}
