//! Minimal VM host state (gas + allocator + string pool) for the dispatch loop.

use crate::gc::GcAllocator;
use crate::value::{GcHeader, InternPool};

/// Runtime state shared across fibers for a single embedder context.
pub struct ElaraState {
    pub alloc: GcAllocator,
    pub all_objects: *mut GcHeader,
    pub strings: InternPool,
    pub gas_remaining: i64,
    pub gas_limit: i64,
}

impl ElaraState {
    /// Creates a state with unlimited gas (`gas_limit == 0` disables the fuel check).
    #[must_use]
    pub fn new() -> Self {
        Self {
            alloc: GcAllocator::new(),
            all_objects: core::ptr::null_mut(),
            strings: InternPool::new(0xC0FF_EE42),
            gas_remaining: i64::MAX / 4,
            gas_limit: 0,
        }
    }

    /// Creates a state with a hard instruction budget.
    #[must_use]
    pub fn with_gas(limit: i64) -> Self {
        Self {
            alloc: GcAllocator::new(),
            all_objects: core::ptr::null_mut(),
            strings: InternPool::new(0xC0FF_EE42),
            gas_remaining: limit,
            gas_limit: limit,
        }
    }
}

impl Default for ElaraState {
    fn default() -> Self {
        Self::new()
    }
}
