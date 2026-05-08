//! Root marking and Propagate/Atomic object traversal.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::mem::size_of;

use crate::compiler::proto::ObjProto;
use crate::gc::state::{GcPhase, GcState};
use crate::gc::{GC_BLACK, GC_FIXED, GC_GRAY};
use crate::value::nan::{GcHeader, ObjType, Value};
use crate::value::string::ObjString;
use crate::value::table::ObjTable;
use crate::vm::fiber::ObjFiber;
use crate::vm::state::ElaraState;
use crate::vm::upvalue::{ObjClosure, ObjUpvalue};

/// Shades a [`Value`] gray when it points at a current-white heap object.
pub unsafe fn shade_value(gc: &mut GcState, v: Value) {
    if !v.is_obj() {
        return;
    }
    shade_object(gc, v.as_obj());
}

/// Shades `obj` gray if it is still white.
pub unsafe fn shade_object(gc: &mut GcState, obj: *mut GcHeader) {
    if obj.is_null() {
        return;
    }
    let color = (*obj).gc_color;
    if color == GC_FIXED || color == GC_GRAY || color == GC_BLACK {
        return;
    }
    if gc.is_current_white(color) {
        gc.push_gray(obj);
    }
}

/// Marks VM roots and enters [`GcPhase::Propagate`].
pub unsafe fn mark_roots(state: &mut ElaraState) {
    let gc = &mut state.gc;
    gc.gray_stack.clear();
    gc.gray_head = core::ptr::null_mut();
    gc.gray_again = core::ptr::null_mut();
    gc.weak_tables = core::ptr::null_mut();

    shade_value(gc, state.mm_index);
    shade_value(gc, state.mm_newindex);
    shade_value(gc, state.mm_add);
    shade_value(gc, state.mm_call);
    shade_value(gc, state.mm_close);

    if !state.current_fiber.is_null() {
        shade_object(gc, state.current_fiber.cast::<GcHeader>());
    }

    gc.phase = GcPhase::Propagate;
}

/// Propagates up to `work_budget` gray objects. Returns bytes roughly traversed.
pub unsafe fn propagate_step(state: &mut ElaraState, work_budget: usize) -> usize {
    let mut work = 0usize;
    while work < work_budget {
        let obj = state.gc.pop_gray();
        if obj.is_null() {
            state.gc.phase = GcPhase::Atomic;
            break;
        }
        work += traverse_object(state, obj);
    }
    work
}

/// Atomic phase: drain gray_again, rescan current fiber, flip white, start string sweep.
pub unsafe fn atomic_phase(state: &mut ElaraState) {
    // Drain barrier-dirtied tables.
    while !state.gc.gray_again.is_null() {
        let obj = state.gc.gray_again;
        // Tables store gray_again link in `gclist`.
        let table = obj.cast::<ObjTable>();
        state.gc.gray_again = (*table).gclist;
        (*table).gclist = core::ptr::null_mut();
        if (*obj).gc_color != GC_FIXED {
            (*obj).gc_color = GC_GRAY;
            state.gc.gray_stack.push(obj);
        }
    }
    while !state.gc.gray_stack.is_empty() {
        let obj = state.gc.pop_gray();
        if !obj.is_null() {
            let _ = traverse_object(state, obj);
        }
    }

    if !state.current_fiber.is_null() {
        mark_fiber_stack(state, state.current_fiber);
    }

    // Flip whites: former current_white becomes garbage.
    state.gc.current_white ^= 1;
    state.gc.string_sweep_idx = 0;
    state.gc.sweep_cursor = &mut state.all_objects;
    state.gc.phase = GcPhase::SweepString;
}

unsafe fn traverse_object(state: &mut ElaraState, obj: *mut GcHeader) -> usize {
    (*obj).gc_color = GC_BLACK;
    match (*obj).obj_type {
        ObjType::String | ObjType::BoxedI64 | ObjType::Userdata | ObjType::NativeFn => {
            size_of::<ObjString>()
        }
        ObjType::Table => traverse_table(state, obj.cast()),
        ObjType::Closure => traverse_closure(state, obj.cast()),
        ObjType::Proto => traverse_proto(state, obj.cast()),
        ObjType::Upvalue => traverse_upvalue(state, obj.cast()),
        ObjType::Fiber => traverse_fiber(state, obj.cast()),
    }
}

unsafe fn traverse_table(state: &mut ElaraState, table: *mut ObjTable) -> usize {
    let t = &*table;
    if !t.metatable.is_null() {
        shade_object(&mut state.gc, t.metatable.cast());
    }
    if !t.array.is_null() {
        for i in 0..t.array_capacity as usize {
            shade_value(&mut state.gc, *t.array.add(i));
        }
    }
    let hash_cap = t.hash_capacity() as usize;
    if !t.entries.is_null() {
        for i in 0..hash_cap {
            let e = *t.entries.add(i);
            if !e.key.is_nil() {
                shade_value(&mut state.gc, e.key);
                shade_value(&mut state.gc, e.value);
            }
        }
    }
    size_of::<ObjTable>()
        + (t.array_capacity as usize) * size_of::<Value>()
        + hash_cap * size_of::<crate::value::table::MapEntry>()
}

unsafe fn traverse_closure(state: &mut ElaraState, closure: *mut ObjClosure) -> usize {
    let c = &*closure;
    if !c.proto.is_null() {
        shade_object(&mut state.gc, c.proto.cast());
    }
    if !c.upvalues.is_null() {
        for i in 0..usize::from(c.upvalue_count) {
            let uv = *c.upvalues.add(i);
            if !uv.is_null() {
                shade_object(&mut state.gc, uv.cast());
            }
        }
    }
    size_of::<ObjClosure>() + usize::from(c.upvalue_count) * size_of::<*mut ObjUpvalue>()
}

unsafe fn traverse_proto(state: &mut ElaraState, proto: *mut ObjProto) -> usize {
    let p = &*proto;
    if !p.source_name.is_null() {
        shade_object(&mut state.gc, p.source_name.cast());
    }
    for i in 0..usize::from(p.const_count) {
        shade_value(&mut state.gc, *p.constants.add(i));
    }
    for i in 0..usize::from(p.proto_count) {
        let child = *p.protos.add(i);
        if !child.is_null() {
            shade_object(&mut state.gc, child.cast());
        }
    }
    size_of::<ObjProto>()
        + (p.code_capacity as usize) * size_of::<crate::compiler::opcode::Instruction>()
}

unsafe fn traverse_upvalue(state: &mut ElaraState, upval: *mut ObjUpvalue) -> usize {
    let u = &*upval;
    // Closed upvalues keep the value in `closed`; open ones are rooted via the fiber stack.
    if u.stack_idx == u32::MAX {
        shade_value(&mut state.gc, u.closed);
    } else if !u.location.is_null() {
        shade_value(&mut state.gc, *u.location);
    }
    size_of::<ObjUpvalue>()
}

unsafe fn traverse_fiber(state: &mut ElaraState, fiber: *mut ObjFiber) -> usize {
    mark_fiber_stack(state, fiber);
    let f = &*fiber;
    if !f.caller.is_null() {
        shade_object(&mut state.gc, f.caller.cast());
    }
    let mut uv = f.open_upvalues;
    while !uv.is_null() {
        shade_object(&mut state.gc, uv.cast());
        uv = (*uv).next_open;
    }
    for i in 0..f.frame_count as usize {
        let frame = *f.frames.add(i);
        if !frame.closure.is_null() {
            shade_object(&mut state.gc, frame.closure.cast());
        }
    }
    size_of::<ObjFiber>()
        + (f.stack_capacity as usize) * size_of::<Value>()
        + (f.frame_capacity as usize) * size_of::<crate::vm::frame::CallFrame>()
}

unsafe fn mark_fiber_stack(state: &mut ElaraState, fiber: *mut ObjFiber) {
    let f = &*fiber;
    let top = f.open_top.max(f.stack_top) as usize;
    let n = top.min(f.stack_capacity as usize);
    for i in 0..n {
        shade_value(&mut state.gc, *f.stack.add(i));
    }
    shade_value(&mut state.gc, f.error_val);
}
