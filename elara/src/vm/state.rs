//! Minimal VM host state (gas + allocator + string pool) for the dispatch loop.

use crate::gc::GcAllocator;
use crate::value::{GcHeader, InternPool, ObjType, Value};

/// Runtime state shared across fibers for a single embedder context.
pub struct ElaraState {
    pub alloc: GcAllocator,
    pub all_objects: *mut GcHeader,
    pub strings: InternPool,
    /// Interned `"__index"` key for metatable lookup.
    pub mm_index: Value,
    /// Interned `"__newindex"` key for metatable lookup.
    pub mm_newindex: Value,
    /// Interned `"__add"` key for metamethod fallback.
    pub mm_add: Value,
    /// Interned `"__call"` key for callable tables.
    pub mm_call: Value,
    pub gas_remaining: i64,
    pub gas_limit: i64,
}

impl ElaraState {
    /// Creates a state with unlimited gas (`gas_limit == 0` disables the fuel check).
    #[must_use]
    pub fn new() -> Self {
        Self::with_gas_inner(i64::MAX / 4, 0)
    }

    /// Creates a state with a hard instruction budget.
    #[must_use]
    pub fn with_gas(limit: i64) -> Self {
        Self::with_gas_inner(limit, limit)
    }

    fn with_gas_inner(gas_remaining: i64, gas_limit: i64) -> Self {
        let mut alloc = GcAllocator::new();
        let mut all_objects = core::ptr::null_mut();
        let mut strings = InternPool::new(0xC0FF_EE42);
        let mm_index = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__index");
        let mm_newindex = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__newindex");
        let mm_add = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__add");
        let mm_call = intern_mm(&mut strings, &mut alloc, &mut all_objects, b"__call");
        Self {
            alloc,
            all_objects,
            strings,
            mm_index,
            mm_newindex,
            mm_add,
            mm_call,
            gas_remaining,
            gas_limit,
        }
    }
}

impl Default for ElaraState {
    fn default() -> Self {
        Self::new()
    }
}

fn intern_mm(
    strings: &mut InternPool,
    alloc: &mut GcAllocator,
    all_objects: &mut *mut GcHeader,
    bytes: &[u8],
) -> Value {
    let s = strings
        .new_string(alloc, all_objects, 0, bytes)
        .expect("intern metamethod name");
    Value::from_obj(s.cast(), ObjType::String)
}
