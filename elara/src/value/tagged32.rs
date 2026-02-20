//! 32-bit `#[repr(C)]` tagged union representation for `Value` on non-64-bit targets.

use core::ffi::c_void;
use core::ptr::NonNull;

use super::nan::{ELARA_INT48_MAX, ELARA_INT48_MIN, GcHeader, ObjBoxedI64, ObjType};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Value {
    pub payload: u64,
    pub tag: u32,
    pub pad: u32,
}

impl Value {
    const TAG32_NIL: u32 = 0;
    const TAG32_BOOL: u32 = 1;
    const TAG32_I48: u32 = 2;
    const TAG32_FLOAT: u32 = 3;
    const TAG32_LIGHTUD: u32 = 4;
    const TAG32_OBJ_BASE: u32 = 16;

    pub const NIL: Self = Self {
        payload: 0,
        tag: Self::TAG32_NIL,
        pad: 0,
    };
    pub const FALSE: Self = Self {
        payload: 0,
        tag: Self::TAG32_BOOL,
        pad: 0,
    };
    pub const TRUE: Self = Self {
        payload: 1,
        tag: Self::TAG32_BOOL,
        pad: 0,
    };

    #[inline(always)]
    #[must_use]
    pub const fn from_bool(b: bool) -> Self {
        if b { Self::TRUE } else { Self::FALSE }
    }

    #[inline(always)]
    #[must_use]
    pub const fn as_bool(self) -> bool {
        self.payload != 0
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_float(self) -> bool {
        self.tag == Self::TAG32_FLOAT
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.tag == Self::TAG32_NIL
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_bool(self) -> bool {
        self.tag == Self::TAG32_BOOL
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_i48(self) -> bool {
        self.tag == Self::TAG32_I48
    }

    #[inline(always)]
    #[must_use]
    pub fn is_int(self) -> bool {
        self.is_i48() || self.is_obj_type(ObjType::BoxedI64)
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_lightud(self) -> bool {
        self.tag == Self::TAG32_LIGHTUD
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_obj(self) -> bool {
        self.tag >= Self::TAG32_OBJ_BASE
    }

    #[inline(always)]
    #[must_use]
    pub fn is_number(self) -> bool {
        self.is_int() || self.is_float()
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_falsy(self) -> bool {
        self.is_nil() || (self.is_bool() && self.payload == 0)
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_truthy(self) -> bool {
        !self.is_falsy()
    }

    #[inline(always)]
    #[must_use]
    pub const fn fits_i48(i: i64) -> bool {
        i >= ELARA_INT48_MIN && i <= ELARA_INT48_MAX
    }

    #[inline(always)]
    #[must_use]
    pub const fn from_i48(i: i64) -> Self {
        Self {
            payload: i as u64,
            tag: Self::TAG32_I48,
            pad: 0,
        }
    }

    #[inline(always)]
    #[must_use]
    pub const fn as_i48(self) -> i64 {
        self.payload as i64
    }

    #[inline(always)]
    #[must_use]
    pub fn to_i64(self) -> Option<i64> {
        if self.is_i48() {
            Some(self.as_i48())
        } else if self.is_obj_type(ObjType::BoxedI64) {
            // SAFETY: Tag verified as ObjType::BoxedI64.
            Some(unsafe { (*self.as_obj().cast::<ObjBoxedI64>()).value })
        } else {
            None
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn from_f64(d: f64) -> Self {
        Self {
            payload: d.to_bits(),
            tag: Self::TAG32_FLOAT,
            pad: 0,
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from_bits(self.payload)
    }

    #[inline(always)]
    #[must_use]
    pub fn from_lightud(ptr: *mut c_void) -> Self {
        Self {
            payload: ptr as usize as u64,
            tag: Self::TAG32_LIGHTUD,
            pad: 0,
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn as_lightud(self) -> *mut c_void {
        self.payload as usize as *mut c_void
    }

    #[inline(always)]
    #[must_use]
    pub fn from_obj(ptr: NonNull<GcHeader>, obj_type: ObjType) -> Self {
        Self {
            payload: ptr.as_ptr() as usize as u64,
            tag: Self::TAG32_OBJ_BASE + (obj_type as u32),
            pad: 0,
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn as_obj(self) -> *mut GcHeader {
        self.payload as usize as *mut GcHeader
    }

    #[inline(always)]
    #[must_use]
    pub fn obj_type(self) -> Option<ObjType> {
        if !self.is_obj() {
            return None;
        }
        let raw = (self.tag - Self::TAG32_OBJ_BASE) as u8;
        if raw > ObjType::BoxedI64 as u8 {
            return None;
        }
        // SAFETY: `raw` is verified in 0..=8 matching `ObjType`.
        Some(unsafe { core::mem::transmute::<u8, ObjType>(raw) })
    }

    #[inline(always)]
    #[must_use]
    pub fn is_obj_type(self, t: ObjType) -> bool {
        self.obj_type() == Some(t)
    }
}

