//! Tri-color incremental GC phase machine (`GcState` / `GcPhase`).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;

use crate::gc::alloc::ElaraAllocFn;
use crate::gc::{GC_WHITE0, GC_WHITE1};
use crate::value::GcHeader;
use crate::value::table::ObjTable;

/// Incremental mark-and-sweep phase.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcPhase {
    Pause = 0,
    Propagate = 1,
    Atomic = 2,
    SweepString = 3,
    SweepObj = 4,
}

/// Collector control block: colors, gray worklists, and sweep cursors.
pub struct GcState {
    pub alloc_fn: ElaraAllocFn,
    pub alloc_ud: *mut c_void,
    pub phase: GcPhase,
    /// Toggles between [`GC_WHITE0`] and [`GC_WHITE1`] at the Atomic phase.
    pub current_white: u8,
    pub all_objects: *mut GcHeader,
    pub sweep_cursor: *mut *mut GcHeader,
    pub string_sweep_idx: usize,
    /// Primary gray worklist (intrusive via object `gclist` where available; else stack).
    pub gray_head: *mut GcHeader,
    /// Tables dirtied by the backward write barrier during Propagate.
    pub gray_again: *mut GcHeader,
    pub weak_tables: *mut ObjTable,
    pub bytes_allocated: usize,
    pub gc_threshold: usize,
    pub mem_limit_bytes: usize,
    pub pause_multiplier: u32,
    pub step_multiplier: u32,
    pub pause_budget_us: u32,
    pub generational_mode: bool,
    /// Non-intrusive gray stack for object types without a `gclist` field.
    pub(crate) gray_stack: Vec<*mut GcHeader>,
}

impl Default for GcState {
    fn default() -> Self {
        Self::new()
    }
}

impl GcState {
    /// Creates a paused collector with default pause/step multipliers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            alloc_fn: crate::gc::alloc::default_alloc_fn,
            alloc_ud: core::ptr::null_mut(),
            phase: GcPhase::Pause,
            current_white: GC_WHITE0,
            all_objects: core::ptr::null_mut(),
            sweep_cursor: core::ptr::null_mut(),
            string_sweep_idx: 0,
            gray_head: core::ptr::null_mut(),
            gray_again: core::ptr::null_mut(),
            weak_tables: core::ptr::null_mut(),
            bytes_allocated: 0,
            gc_threshold: 256 * 1024,
            mem_limit_bytes: 0,
            pause_multiplier: 200,
            step_multiplier: 100,
            pause_budget_us: 500,
            generational_mode: false,
            gray_stack: Vec::new(),
        }
    }

    /// Returns the white color that is garbage after the Atomic flip.
    #[inline]
    #[must_use]
    pub fn other_white(&self) -> u8 {
        self.current_white ^ 1
    }

    /// Returns true when `color` is the live-cycle white (`current_white`).
    #[inline]
    #[must_use]
    pub fn is_white(&self, color: u8) -> bool {
        color == GC_WHITE0 || color == GC_WHITE1
    }

    /// Returns true when `color` is the current cycle's white.
    #[inline]
    #[must_use]
    pub fn is_current_white(&self, color: u8) -> bool {
        color == self.current_white
    }

    /// Pushes `obj` onto the gray worklist (idempotent if already gray/black/fixed).
    pub unsafe fn push_gray(&mut self, obj: *mut GcHeader) {
        if obj.is_null() {
            return;
        }
        let color = (*obj).gc_color;
        if color == crate::gc::GC_GRAY
            || color == crate::gc::GC_BLACK
            || color == crate::gc::GC_FIXED
        {
            return;
        }
        (*obj).gc_color = crate::gc::GC_GRAY;
        self.gray_stack.push(obj);
    }

    /// Pops one gray object, or `null` when the worklist is empty.
    pub fn pop_gray(&mut self) -> *mut GcHeader {
        self.gray_stack.pop().unwrap_or(core::ptr::null_mut())
    }
}
