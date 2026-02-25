//! Unified value representation, NaN-tagging, and GC heap object structures.

pub mod nan;
pub mod ops;
pub mod string;
#[cfg(not(target_pointer_width = "64"))]
pub mod tagged32;

pub use nan::{
    ELARA_INT48_MAX, ELARA_INT48_MIN, ELARA_PAYLOAD_MASK, ELARA_QNAN, ELARA_SIGN_BIT,
    ELARA_TAG_BOOL, ELARA_TAG_INT, ELARA_TAG_LIGHTUD, ELARA_TAG_MASK, ELARA_TAG_NIL, ELARA_TAG_OBJ,
    GcHeader, ObjBoxedI64, ObjType, Value,
};
pub use ops::{float_to_exact_i64, hash_u64, int_eq_float};
pub use string::{
    ELARA_SHORT_STR_MAX, FNV1A_OFFSET_BASIS, FNV1A_PRIME, InternPool, ObjString, STR_FLAG_HAS_HASH,
    STR_FLAG_INTERNED, fnv1a_hash,
};
