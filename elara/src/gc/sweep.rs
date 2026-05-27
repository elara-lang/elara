//! Incremental SweepString / SweepObj and object finalization.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::mem::size_of;

use crate::compiler::proto::ObjProto;
use crate::ffi::{ObjNativeFn, ObjUserdata};
use crate::gc::state::{GcPhase, GcState};
use crate::gc::{GC_FIXED, GC_GEN_NEW, GC_GEN_OLD};
use crate::value::nan::{GcHeader, ObjBoxedI64, ObjType};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::fiber::ObjFiber;
use crate::vm::state::ElaraState;
use crate::vm::upvalue::{ObjClosure, ObjUpvalue};

/// Sweeps up to `budget` intern pool buckets. Advances to [`GcPhase::SweepObj`] when done.
pub unsafe fn sweep_strings_step(state: &mut ElaraState, budget: usize) -> usize {
    let mut work = 0usize;
    let cap = state.strings.capacity();
    let other = state.gc.other_white();
    while work < budget && state.gc.string_sweep_idx < cap {
        let idx = state.gc.string_sweep_idx;
        state.gc.string_sweep_idx += 1;
        work += 1;
        let s = state.strings.bucket(idx);
        if s.is_null() {
            continue;
        }
        let color = (*s).header.gc_color;
        if color == other {
            let total = ObjString::alloc_size((*s).byte_len as usize);
            state.strings.clear_bucket(idx);
            state.alloc.free_bytes(s.cast::<u8>(), total);
        } else if color != GC_FIXED {
            (*s).header.gc_color = state.gc.current_white;
            if state.gc.generational_mode && (*s).header.generation == GC_GEN_NEW {
                (*s).header.generation = GC_GEN_OLD;
            }
        }
    }
    if state.gc.string_sweep_idx >= cap {
        state.gc.phase = GcPhase::SweepObj;
        state.gc.sweep_cursor = &mut state.all_objects;
    }
    work
}

/// Sweeps up to `budget` objects from `all_objects`. Returns to [`GcPhase::Pause`] when done.
pub unsafe fn sweep_objects_step(state: &mut ElaraState, budget: usize) -> usize {
    let mut work = 0usize;
    let other = state.gc.other_white();
    while work < budget {
        let link = state.gc.sweep_cursor;
        if link.is_null() {
            finish_sweep(state);
            break;
        }
        let obj = *link;
        if obj.is_null() {
            finish_sweep(state);
            break;
        }
        work += 1;
        let color = (*obj).gc_color;
        if color == other {
            *link = (*obj).next;
            free_object(state, obj);
            // stay on same cursor slot (now points at next)
        } else {
            if color != GC_FIXED {
                (*obj).gc_color = state.gc.current_white;
                if state.gc.generational_mode && (*obj).generation == GC_GEN_NEW {
                    (*obj).generation = GC_GEN_OLD;
                }
            }
            state.gc.sweep_cursor = &mut (*obj).next;
        }
    }
    work
}

fn finish_sweep(state: &mut ElaraState) {
    let live = state.alloc.bytes_allocated;
    let mult = state.gc.pause_multiplier as usize;
    state.gc.gc_threshold = (live / 100).saturating_mul(mult).max(1024);
    state.gc.phase = GcPhase::Pause;
    state.gc.sweep_cursor = core::ptr::null_mut();
    state.gc.gray_stack.clear();
    state.gc.bytes_allocated = live;
}

unsafe fn free_object(state: &mut ElaraState, obj: *mut GcHeader) {
    match (*obj).obj_type {
        ObjType::Table => {
            let t = &mut *obj.cast::<ObjTable>();
            t.free_table(&mut state.alloc);
            state
                .alloc
                .free_bytes(obj.cast::<u8>(), size_of::<ObjTable>());
        }
        ObjType::String => {
            let s = &*obj.cast::<ObjString>();
            let total = ObjString::alloc_size(s.byte_len as usize);
            state.alloc.free_bytes(obj.cast::<u8>(), total);
        }
        ObjType::Closure => {
            (*obj.cast::<ObjClosure>()).free_closure(&mut state.alloc);
        }
        ObjType::Proto => {
            let p = &mut *obj.cast::<ObjProto>();
            p.free_proto(&mut state.alloc);
            state
                .alloc
                .free_bytes(obj.cast::<u8>(), size_of::<ObjProto>());
        }
        ObjType::Upvalue => {
            (*obj.cast::<ObjUpvalue>()).free_upvalue(&mut state.alloc);
        }
        ObjType::Fiber => {
            (*obj.cast::<ObjFiber>()).free_fiber(&mut state.alloc);
        }
        ObjType::BoxedI64 => {
            state
                .alloc
                .free_bytes(obj.cast::<u8>(), size_of::<ObjBoxedI64>());
        }
        ObjType::Userdata => {
            ObjUserdata::finalize_and_free(state, obj.cast());
        }
        ObjType::NativeFn => {
            ObjNativeFn::free(state, obj.cast());
        }
    }
}

/// Runs one incremental step with a unit work budget. Returns `true` when a full cycle finished
/// (phase returned to [`GcPhase::Pause`]).
pub unsafe fn gc_step(state: &mut ElaraState, work_budget: usize) -> bool {
    let budget = work_budget.max(1);
    match state.gc.phase {
        GcPhase::Pause => {
            crate::gc::mark::mark_roots(state);
            false
        }
        GcPhase::Propagate => {
            let _ = crate::gc::mark::propagate_step(state, budget);
            false
        }
        GcPhase::Atomic => {
            crate::gc::mark::atomic_phase(state);
            false
        }
        GcPhase::SweepString => {
            let _ = sweep_strings_step(state, budget);
            false
        }
        GcPhase::SweepObj => {
            let _ = sweep_objects_step(state, budget);
            state.gc.phase == GcPhase::Pause
        }
    }
}

/// Drives the collector until a full cycle returns to [`GcPhase::Pause`].
pub unsafe fn gc_collect(state: &mut ElaraState) {
    // Ensure we start a fresh cycle.
    if state.gc.phase == GcPhase::Pause {
        let _ = gc_step(state, 64);
    }
    while state.gc.phase != GcPhase::Pause {
        let _ = gc_step(state, 64);
    }
}

/// Syncs `GcState::all_objects` from the VM list (call after linking objects).
pub fn sync_all_objects(gc: &mut GcState, all_objects: *mut GcHeader) {
    gc.all_objects = all_objects;
}
