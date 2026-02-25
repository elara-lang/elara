//! Garbage collection, host memory allocator gateway, and write barriers.

pub mod alloc;

pub use alloc::{ElaraAllocFn, GcAllocator, default_alloc_fn};

pub const GC_WHITE0: u8 = 0;
pub const GC_WHITE1: u8 = 1;
pub const GC_GRAY: u8 = 2;
pub const GC_BLACK: u8 = 3;
pub const GC_FIXED: u8 = 4;

pub const GC_GEN_NEW: u8 = 0;
pub const GC_GEN_OLD: u8 = 1;

