//! 64-bit IEEE-754 Quiet NaN-tagging, `GcHeader`, and `ObjBoxedI64`.

#[cfg(target_pointer_width = "64")]
use core::ffi::c_void;
use core::fmt;
#[cfg(target_pointer_width = "64")]
use core::ptr::NonNull;

#[cfg(not(target_pointer_width = "64"))]
pub use super::tagged32::Value;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjType {
    String = 0,
    Table = 1,
    Closure = 2,
    Proto = 3,
    Upvalue = 4,
    Fiber = 5,
    Userdata = 6,
    NativeFn = 7,
    BoxedI64 = 8,
}

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct GcHeader {
    pub next: *mut GcHeader, // Intrusive global GC list (8 bytes)
    pub obj_type: ObjType,   // 1 byte
    pub gc_color: u8,        // WHITE0=0, WHITE1=1, GRAY=2, BLACK=3, FIXED=4 (1 byte)
    pub flags: u8,           // Type-specific flags (1 byte)
    pub generation: u8,      // G_NEW=0, G_OLD=1 (1 byte)
    pub aux_hash: u32,       // Precomputed 32-bit FNV-1a hash or Table Shape ID (4 bytes)
}

impl GcHeader {
    /// Creates a new initialized `GcHeader` for the given `obj_type`.
    #[inline]
    #[must_use]
    pub const fn new(obj_type: ObjType, gc_color: u8) -> Self {
        Self {
            next: core::ptr::null_mut(),
            obj_type,
            gc_color,
            flags: 0,
            generation: 0,
            aux_hash: 0,
        }
    }
}

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct ObjBoxedI64 {
    pub header: GcHeader, // 16 bytes (ObjType::BoxedI64)
    pub value: i64,       // Full 64-bit signed integer outside [-2^47, 2^47 - 1]
}

impl ObjBoxedI64 {
    /// Constructs a new `ObjBoxedI64` wrapping a full 64-bit signed integer.
    #[inline]
    #[must_use]
    pub const fn new(value: i64, gc_color: u8) -> Self {
        Self {
            header: GcHeader::new(ObjType::BoxedI64, gc_color),
            value,
        }
    }
}

pub const ELARA_SIGN_BIT: u64 = 1 << 63;
pub const ELARA_QNAN: u64 = 0x7FFC_0000_0000_0000;
pub const ELARA_PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
pub const ELARA_TAG_MASK: u64 = 0xFFFF_0000_0000_0000;

pub const ELARA_TAG_NIL: u64 = ELARA_QNAN;
pub const ELARA_TAG_BOOL: u64 = ELARA_QNAN | (1 << 48);
pub const ELARA_TAG_INT: u64 = ELARA_QNAN | (2 << 48);
pub const ELARA_TAG_LIGHTUD: u64 = ELARA_QNAN | (3 << 48);
pub const ELARA_TAG_OBJ: u64 = ELARA_SIGN_BIT | ELARA_QNAN;

pub const ELARA_INT48_MIN: i64 = -(1_i64 << 47); // -140_737_488_355_328
pub const ELARA_INT48_MAX: i64 = (1_i64 << 47) - 1; //  140_737_488_355_327

#[cfg(target_pointer_width = "64")]
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Value(pub u64);

#[cfg(target_pointer_width = "64")]
impl Value {
    pub const NIL: Self = Self(ELARA_TAG_NIL);
    pub const FALSE: Self = Self(ELARA_TAG_BOOL);
    pub const TRUE: Self = Self(ELARA_TAG_BOOL | 1);

    #[inline(always)]
    #[must_use]
    pub const fn from_bool(b: bool) -> Self {
        Self(ELARA_TAG_BOOL | (b as u64))
    }

    #[inline(always)]
    #[must_use]
    pub const fn as_bool(self) -> bool {
        (self.0 & 1) != 0
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_float(self) -> bool {
        (self.0 & ELARA_QNAN) != ELARA_QNAN
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0 == Self::NIL.0
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_bool(self) -> bool {
        (self.0 & !1) == Self::FALSE.0
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_i48(self) -> bool {
        (self.0 & ELARA_TAG_MASK) == ELARA_TAG_INT
    }

    #[inline(always)]
    #[must_use]
    pub fn is_int(self) -> bool {
        self.is_i48() || self.is_obj_type(ObjType::BoxedI64)
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_lightud(self) -> bool {
        (self.0 & ELARA_TAG_MASK) == ELARA_TAG_LIGHTUD
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_obj(self) -> bool {
        (self.0 & (ELARA_SIGN_BIT | ELARA_QNAN)) == ELARA_TAG_OBJ
    }

    #[inline(always)]
    #[must_use]
    pub fn is_number(self) -> bool {
        self.is_int() || self.is_float()
    }

    #[inline(always)]
    #[must_use]
    pub const fn is_falsy(self) -> bool {
        self.is_nil() || self.0 == Self::FALSE.0
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
        Self(ELARA_TAG_INT | ((i as u64) & ELARA_PAYLOAD_MASK))
    }

    #[inline(always)]
    #[must_use]
    pub const fn as_i48(self) -> i64 {
        ((self.0 << 16) as i64) >> 16
    }

    #[inline(always)]
    #[must_use]
    pub fn to_i64(self) -> Option<i64> {
        if self.is_i48() {
            Some(self.as_i48())
        } else if self.is_obj_type(ObjType::BoxedI64) {
            // SAFETY: Tag verified as ObjType::BoxedI64, and pointer is 8-byte aligned.
            Some(unsafe { (*self.as_obj().cast::<ObjBoxedI64>()).value })
        } else {
            None
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn from_f64(d: f64) -> Self {
        let bits = d.to_bits();
        if (bits & ELARA_QNAN) == ELARA_QNAN {
            Self(0x7FF8_0000_0000_0000) // Canonical IEEE-754 Quiet NaN
        } else {
            Self(bits)
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn as_f64(self) -> f64 {
        f64::from_bits(self.0)
    }

    #[inline(always)]
    #[must_use]
    pub fn from_lightud(ptr: *mut c_void) -> Self {
        let addr = ptr as usize as u64;
        Self(ELARA_TAG_LIGHTUD | (addr & ELARA_PAYLOAD_MASK))
    }

    #[inline(always)]
    #[must_use]
    pub fn as_lightud(self) -> *mut c_void {
        (self.0 & ELARA_PAYLOAD_MASK) as usize as *mut c_void
    }

    #[inline(always)]
    #[must_use]
    pub fn from_obj(ptr: NonNull<GcHeader>, obj_type: ObjType) -> Self {
        let addr = ptr.as_ptr() as usize as u64;
        debug_assert!(addr & 0x7 == 0, "GC object pointer must be 8-byte aligned");
        let t = obj_type as u64;
        let high_tag = (t & 0x03) << 48;
        let low_tag = (t >> 2) & 0x07;
        Self(ELARA_TAG_OBJ | high_tag | (addr & 0x0000_FFFF_FFFF_FFF8) | low_tag)
    }

    #[inline(always)]
    #[must_use]
    pub fn as_obj(self) -> *mut GcHeader {
        (self.0 & 0x0000_FFFF_FFFF_FFF8) as usize as *mut GcHeader
    }

    #[inline(always)]
    #[must_use]
    pub fn obj_type(self) -> Option<ObjType> {
        if !self.is_obj() {
            return None;
        }
        let low = (self.0 & 0x07) as u8;
        let high = ((self.0 >> 48) & 0x03) as u8;
        let raw = (low << 2) | high;
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

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_nil() {
            write!(f, "Value(nil)")
        } else if self.is_bool() {
            write!(f, "Value({})", self.as_bool())
        } else if let Some(i) = self.to_i64() {
            write!(f, "Value({i})")
        } else if self.is_float() {
            write!(f, "Value({:?})", self.as_f64())
        } else if self.is_lightud() {
            write!(f, "Value(lightud:{:p})", self.as_lightud())
        } else if let Some(t) = self.obj_type() {
            write!(f, "Value({t:?}:{:p})", self.as_obj())
        } else {
            write!(f, "Value(unknown)")
        }
    }
}
