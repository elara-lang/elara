//! Garbage collection, host memory allocator gateway, and write barriers.

pub mod alloc;
pub mod barrier;
pub mod mark;
pub mod state;
pub mod sweep;

pub use alloc::{ElaraAllocFn, GcAllocator, default_alloc_fn};
pub use barrier::{
    barrier_table_write, gc_alloc, gc_barrier_back, gc_barrier_forward, gc_barrier_metatable,
    gc_step_with_budget, shade,
};
pub use state::{GcPhase, GcState};
pub use sweep::{gc_collect, gc_step};

pub const GC_WHITE0: u8 = 0;
pub const GC_WHITE1: u8 = 1;
pub const GC_GRAY: u8 = 2;
pub const GC_BLACK: u8 = 3;
pub const GC_FIXED: u8 = 4;

pub const GC_GEN_NEW: u8 = 0;
pub const GC_GEN_OLD: u8 = 1;
