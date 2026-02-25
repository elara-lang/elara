//! Value-semantic equality, cross-numeric comparison, and SplitMix64 hashing.

use super::nan::{ObjType, Value};
use super::string::ObjString;

/// SplitMix64 64-to-32-bit mixer returning a guaranteed non-zero 32-bit hash.
#[inline]
#[must_use]
pub const fn hash_u64(mut x: u64) -> u32 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x = x ^ (x >> 31);
    let h = (x ^ (x >> 32)) as u32;
    if h == 0 { 1 } else { h }
}

impl Value {
    /// Evaluates value-semantic equality between `self` and `other`.
    ///
    /// - Fast-path compares raw bits in 1 instruction (handling `NaN != NaN`).
    /// - Unifies `i48`, `ObjBoxedI64`, and `f64` values across representations.
    #[inline]
    #[must_use]
    pub fn val_equal(self, other: Self) -> bool {
        #[cfg(target_pointer_width = "64")]
        if self.0 == other.0 {
            return !self.is_float() || !self.as_f64().is_nan();
        }
        #[cfg(not(target_pointer_width = "64"))]
        if self.tag == other.tag && self.payload == other.payload {
            return !self.is_float() || !self.as_f64().is_nan();
        }

        // Cross-representation numeric equality (`i48`, `ObjBoxedI64`, `f64`)
        match (self.to_i64(), other.to_i64()) {
            (Some(ia), Some(ib)) => return ia == ib,
            (Some(ia), None) if other.is_float() => {
                return int_eq_float(ia, other.as_f64());
            }
            (None, Some(ib)) if self.is_float() => {
                return int_eq_float(ib, self.as_f64());
            }
            (None, None) if self.is_float() && other.is_float() => {
                return self.as_f64() == other.as_f64();
            }
            _ => {}
        }

        // Dual-tier string equality (`ObjType::String`)
        if self.is_obj_type(ObjType::String) && other.is_obj_type(ObjType::String) {
            let a_ptr = self.as_obj().cast::<ObjString>();
            let b_ptr = other.as_obj().cast::<ObjString>();
            // SAFETY: Both `self` and `other` are verified as `ObjType::String` with distinct pointers.
            unsafe {
                // Fast-path: if both strings are globally interned, distinct pointers are never equal!
                if (*a_ptr).is_interned() && (*b_ptr).is_interned() {
                    return false;
                }
                if (*a_ptr).byte_len != (*b_ptr).byte_len {
                    return false;
                }
                if (*a_ptr).ensure_hash() != (*b_ptr).ensure_hash() {
                    return false;
                }
                return (*a_ptr).as_bytes() == (*b_ptr).as_bytes();
            }
        }

        false
    }

    /// Computes a deterministic non-zero 32-bit hash for `self`.
    ///
    /// Unifies `i48`, `ObjBoxedI64`, and exact-integer `f64` values so `1`, `1.0`, and boxed
    /// 64-bit integers produce consistent hashes.
    #[inline]
    #[must_use]
    pub fn val_hash(self) -> u32 {
        if let Some(i) = self.to_i64() {
            return hash_u64(i as u64);
        }

        if self.is_float() {
            let d = self.as_f64();
            if d == 0.0 {
                // Normalize -0.0 and +0.0 to integer 0
                return hash_u64(0);
            }
            if let Some(i) = float_to_exact_i64(d) {
                return hash_u64(i as u64);
            }
            return hash_u64(d.to_bits());
        }

        if self.is_nil() {
            return 1;
        }

        if self.is_bool() {
            return if self.as_bool() { 3 } else { 2 };
        }

        if self.is_lightud() {
            return hash_u64(self.as_lightud() as usize as u64);
        }

        if let Some(obj_type) = self.obj_type() {
            let ptr = self.as_obj();
            if obj_type == ObjType::String {
                // SAFETY: `ptr` is verified as `ObjType::String`.
                return unsafe { (*ptr.cast::<ObjString>()).ensure_hash() };
            }
            return hash_u64(ptr as usize as u64);
        }

        1
    }
}

/// Checks whether a 64-bit signed integer `i` equals an IEEE-754 double `d` without precision loss.
#[inline]
#[must_use]
pub fn int_eq_float(i: i64, d: f64) -> bool {
    float_to_exact_i64(d) == Some(i)
}

/// Converts `d` to `Some(i64)` if and only if `d` is finite and represents an exact integer in `[i64::MIN, i64::MAX]`.
#[inline]
#[must_use]
pub fn float_to_exact_i64(d: f64) -> Option<i64> {
    // 9_223_372_036_854_775_808.0 is exactly 2^63 (`(i64::MAX as f64)` rounds up to 2^63).
    // Any float `>= 2^63` or `< -2^63` cannot fit in `i64`.
    const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
    if !d.is_finite() || !(-TWO_POW_63..TWO_POW_63).contains(&d) {
        return None;
    }
    let i = d as i64;
    if (i as f64) == d { Some(i) } else { None }
}
