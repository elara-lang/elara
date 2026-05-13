//! Write barriers, time-bounded GC steps, and emergency OOM collection.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::ptr::NonNull;
use core::time::Duration;
use std::time::Instant;

use crate::gc::mark::{shade_object, shade_value};
use crate::gc::state::GcPhase;
use crate::gc::sweep::{gc_collect, gc_step};
use crate::gc::{GC_BLACK, GC_FIXED, GC_GRAY, GC_WHITE0, GC_WHITE1};
use crate::value::nan::{GcHeader, ObjType, Value};
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::upvalue::ObjUpvalue;

/// Backward barrier: a black table that receives a white key/value is re-grayed onto `gray_again`.
#[inline(always)]
pub unsafe fn gc_barrier_back(state: &mut ElaraState, table: *mut ObjTable, val: Value) {
    if state.gc.phase != GcPhase::Propagate || table.is_null() {
        return;
    }
    if (*table).header.gc_color != GC_BLACK || !val.is_obj() {
        return;
    }
    let child = val.as_obj();
    if (*child).gc_color > GC_WHITE1 {
        return;
    }
    (*table).header.gc_color = GC_GRAY;
    (*table).gclist = state.gc.gray_again;
    state.gc.gray_again = core::ptr::addr_of_mut!((*table).header);
}

/// Forward barrier: shade a white child referenced by a black parent.
#[inline(always)]
pub unsafe fn gc_barrier_forward(state: &mut ElaraState, parent: *mut GcHeader, child: Value) {
    if parent.is_null() || !child.is_obj() {
        return;
    }
    if (*parent).gc_color != GC_BLACK {
        return;
    }
    let obj = child.as_obj();
    let color = (*obj).gc_color;
    if color == GC_FIXED || color == GC_GRAY || color == GC_BLACK {
        return;
    }
    match state.gc.phase {
        GcPhase::Propagate | GcPhase::Atomic => {
            if state.gc.is_current_white(color) {
                shade_object(&mut state.gc, obj);
            }
        }
        GcPhase::SweepString | GcPhase::SweepObj => {
            if color == GC_WHITE0 || color == GC_WHITE1 {
                (*obj).gc_color = state.gc.current_white;
            }
        }
        GcPhase::Pause => {}
    }
}

/// Forward barrier when setting a table's metatable.
#[inline(always)]
pub unsafe fn gc_barrier_metatable(
    state: &mut ElaraState,
    table: *mut ObjTable,
    mt: *mut ObjTable,
) {
    if mt.is_null() {
        return;
    }
    gc_barrier_forward(
        state,
        table.cast(),
        Value::from_obj(
            NonNull::new(mt.cast()).expect("non-null mt"),
            ObjType::Table,
        ),
    );
}

/// Runs GC work until `pause_budget_us` elapses or the cycle returns to Pause.
///
/// Returns `true` when a full cycle has completed (phase is [`GcPhase::Pause`]).
pub unsafe fn gc_step_with_budget(state: &mut ElaraState) -> bool {
    let budget = Duration::from_micros(u64::from(state.gc.pause_budget_us.max(1)));
    let start = Instant::now();

    if state.gc.phase == GcPhase::Pause {
        let done = gc_step(state, 16);
        if done {
            return true;
        }
    }

    while start.elapsed() < budget {
        if state.gc.phase == GcPhase::Pause {
            return true;
        }
        if gc_step(state, 16) {
            return true;
        }
    }
    state.gc.phase == GcPhase::Pause
}

/// Allocates `size` bytes, running an emergency full GC once if the memory ceiling would block.
pub unsafe fn gc_alloc(state: &mut ElaraState, size: usize) -> Result<NonNull<u8>, ElaraStatus> {
    if let Some(p) = state.alloc.alloc_bytes(size) {
        state.gc.bytes_allocated = state.alloc.bytes_allocated;
        return Ok(p);
    }
    if state.alloc.mem_limit_bytes > 0 {
        gc_collect(state);
        if let Some(p) = state.alloc.alloc_bytes(size) {
            state.gc.bytes_allocated = state.alloc.bytes_allocated;
            return Ok(p);
        }
    }
    Err(ElaraStatus::ErrOom)
}

/// Applies the backward barrier for both key and value after a table write.
#[inline(always)]
pub unsafe fn barrier_table_write(
    state: &mut ElaraState,
    table: *mut ObjTable,
    key: Value,
    val: Value,
) {
    gc_barrier_back(state, table, key);
    gc_barrier_back(state, table, val);
}

/// Forward barrier for closed upvalue writes.
#[inline(always)]
pub unsafe fn barrier_upvalue(state: &mut ElaraState, upval: *mut ObjUpvalue, val: Value) {
    if upval.is_null() {
        return;
    }
    gc_barrier_forward(state, upval.cast(), val);
}

/// Convenience: shade a value during tests / host hooks.
pub unsafe fn shade(state: &mut ElaraState, v: Value) {
    shade_value(&mut state.gc, v);
}
