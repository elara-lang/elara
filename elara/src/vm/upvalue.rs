//! Open/closed upvalues and closure capture (`capture_upvalue`, `close_upvalues_from`).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::too_many_arguments)]

use core::mem::size_of;
use core::ptr::NonNull;

use crate::compiler::proto::{ObjProto, UpvalDesc};
use crate::gc::GcAllocator;
use crate::value::{GcHeader, ObjTable, ObjType, Value};
use crate::vm::fiber::ObjFiber;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// Shared mutable upvalue: points at a live stack slot while open, or `closed` once migrated.
#[repr(C, align(8))]
pub struct ObjUpvalue {
    pub header: GcHeader,
    pub location: *mut Value,
    pub closed: Value,
    pub next_open: *mut ObjUpvalue,
    pub stack_idx: u32,
    pub reserved: u32,
}

impl ObjUpvalue {
    /// Allocates an open upvalue for `stack_idx` with `location` pointing at that stack slot.
    pub fn new_open(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        location: *mut Value,
        stack_idx: u32,
    ) -> Option<NonNull<Self>> {
        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::Upvalue, gc_color),
                    location,
                    closed: Value::NIL,
                    next_open: core::ptr::null_mut(),
                    stack_idx,
                    reserved: 0,
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }
        NonNull::new(ptr)
    }

    /// Frees the upvalue header bytes.
    pub fn free_upvalue(&mut self, alloc: &mut GcAllocator) {
        unsafe {
            alloc.free_bytes((self as *mut Self).cast::<u8>(), size_of::<Self>());
        }
    }
}

/// Runtime closure: prototype plus captured upvalue pointer array.
#[repr(C, align(8))]
pub struct ObjClosure {
    pub header: GcHeader,
    pub proto: *mut ObjProto,
    pub upvalue_count: u8,
    pub reserved: [u8; 7],
    pub upvalues: *mut *mut ObjUpvalue,
}

impl ObjClosure {
    /// Allocates a closure for `proto`, with an upvalue pointer array sized to `proto.upvalue_count`
    /// (slots start as null until `fill_upvalues` / capture runs).
    ///
    /// # Safety
    ///
    /// `proto` must be a valid, aligned `ObjProto` for the duration of the call.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        proto: *mut ObjProto,
    ) -> Option<NonNull<Self>> {
        // SAFETY: caller provides a live proto (same contract as prior closure constructor).
        let upvalue_count = unsafe { (*proto).upvalue_count };
        let upvalues = if upvalue_count > 0 {
            let bytes = usize::from(upvalue_count) * size_of::<*mut ObjUpvalue>();
            let raw = alloc.alloc_bytes(bytes)?;
            let ptr = raw.as_ptr().cast::<*mut ObjUpvalue>();
            unsafe {
                for i in 0..usize::from(upvalue_count) {
                    *ptr.add(i) = core::ptr::null_mut();
                }
            }
            ptr
        } else {
            core::ptr::null_mut()
        };

        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::Closure, gc_color),
                    proto,
                    upvalue_count,
                    reserved: [0; 7],
                    upvalues,
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }
        NonNull::new(ptr)
    }

    /// Frees the upvalue pointer array and the closure header (does not free `proto` or upvalues).
    pub fn free_closure(&mut self, alloc: &mut GcAllocator) {
        if !self.upvalues.is_null() && self.upvalue_count > 0 {
            let bytes = usize::from(self.upvalue_count) * size_of::<*mut ObjUpvalue>();
            unsafe {
                alloc.free_bytes(self.upvalues.cast::<u8>(), bytes);
            }
            self.upvalues = core::ptr::null_mut();
            self.upvalue_count = 0;
        }
        unsafe {
            alloc.free_bytes((self as *mut Self).cast::<u8>(), size_of::<Self>());
        }
    }
}

/// Finds or creates the open upvalue for `target_idx`, keeping `fiber.open_upvalues` sorted
/// descending by `stack_idx`.
pub unsafe fn capture_upvalue(
    alloc: &mut GcAllocator,
    all_objects: &mut *mut GcHeader,
    gc_color: u8,
    fiber: &mut ObjFiber,
    target_idx: u32,
) -> Result<*mut ObjUpvalue, ElaraStatus> {
    let mut prev: *mut *mut ObjUpvalue = &mut fiber.open_upvalues;
    let mut curr = fiber.open_upvalues;
    while !curr.is_null() && (*curr).stack_idx > target_idx {
        prev = &mut (*curr).next_open;
        curr = (*curr).next_open;
    }
    if !curr.is_null() && (*curr).stack_idx == target_idx {
        return Ok(curr);
    }

    let location = fiber.stack.add(target_idx as usize);
    let upval = ObjUpvalue::new_open(alloc, all_objects, gc_color, location, target_idx)
        .ok_or(ElaraStatus::ErrOom)?
        .as_ptr();
    (*upval).next_open = curr;
    *prev = upval;
    Ok(upval)
}

/// Fills `closure.upvalues` from `child` descriptors relative to `enclosing` and `base_idx`.
pub unsafe fn fill_upvalues(
    alloc: &mut GcAllocator,
    all_objects: &mut *mut GcHeader,
    gc_color: u8,
    fiber: &mut ObjFiber,
    closure: *mut ObjClosure,
    enclosing: *mut ObjClosure,
    base_idx: u32,
) -> Result<(), ElaraStatus> {
    let proto = &*(*closure).proto;
    for i in 0..proto.upvalue_count {
        let desc = *proto.upvalues.add(i as usize);
        let uv = resolve_upvalue(
            alloc,
            all_objects,
            gc_color,
            fiber,
            enclosing,
            base_idx,
            desc,
        )?;
        *(*closure).upvalues.add(i as usize) = uv;
    }
    Ok(())
}

unsafe fn resolve_upvalue(
    alloc: &mut GcAllocator,
    all_objects: &mut *mut GcHeader,
    gc_color: u8,
    fiber: &mut ObjFiber,
    enclosing: *mut ObjClosure,
    base_idx: u32,
    desc: UpvalDesc,
) -> Result<*mut ObjUpvalue, ElaraStatus> {
    if desc.is_local != 0 {
        let target_idx = base_idx + u32::from(desc.index);
        capture_upvalue(alloc, all_objects, gc_color, fiber, target_idx)
    } else {
        if enclosing.is_null() {
            return Err(ElaraStatus::ErrRuntime);
        }
        let inherited = *(*enclosing).upvalues.add(desc.index as usize);
        if inherited.is_null() {
            return Err(ElaraStatus::ErrRuntime);
        }
        Ok(inherited)
    }
}

/// Rebases every open upvalue `location` after `fiber.stack` reallocation.
pub unsafe fn rebase_open_upvalues(fiber: &mut ObjFiber) {
    let mut curr = fiber.open_upvalues;
    while !curr.is_null() {
        let idx = (*curr).stack_idx;
        (*curr).location = fiber.stack.add(idx as usize);
        curr = (*curr).next_open;
    }
}

/// Closes open upvalues with `stack_idx >= min_stack_idx` (heap migration).
///
/// LIFO `defer` / `__close` hooks registered in `fiber.close_slots` are drained first.
pub unsafe fn close_upvalues_from(
    state: &mut ElaraState,
    fiber: &mut ObjFiber,
    min_stack_idx: u32,
) -> Result<(), ElaraStatus> {
    // Drain defer / __close slots in LIFO order for indices >= min_stack_idx.
    while fiber.close_slot_count > 0 {
        let last = *fiber.close_slots.add(fiber.close_slot_count as usize - 1);
        if last < min_stack_idx {
            break;
        }
        fiber.close_slot_count -= 1;
        let val = *fiber.stack.add(last as usize);
        run_close_hook(state, fiber, val)?;
    }

    while !fiber.open_upvalues.is_null() && (*fiber.open_upvalues).stack_idx >= min_stack_idx {
        let upval = fiber.open_upvalues;
        fiber.open_upvalues = (*upval).next_open;
        let val = *(*upval).location;
        (*upval).closed = val;
        (*upval).location = &mut (*upval).closed as *mut Value;
        (*upval).next_open = core::ptr::null_mut();
        (*upval).stack_idx = u32::MAX;
        state.gc_barrier_upvalue(upval, val);
    }
    Ok(())
}

/// Runs one defer / `__close` hook for `val` (closure call or metatable `__close`).
///
/// Full synchronous invocation of nested closures is completed with the opcode wiring task;
/// this records hook values for host tests via `state.last_close_hook` and validates types.
fn run_close_hook(
    state: &mut ElaraState,
    _fiber: &mut ObjFiber,
    val: Value,
) -> Result<(), ElaraStatus> {
    if val.is_nil() || val.is_bool() && !val.as_bool() {
        return Ok(());
    }
    if val.is_obj_type(ObjType::Closure) {
        state.push_close_hook(val);
        return Ok(());
    }
    if val.is_obj_type(ObjType::Table) {
        let mt = unsafe { (*val.as_obj().cast::<ObjTable>()).metatable };
        if mt.is_null() {
            return Err(ElaraStatus::ErrRuntime);
        }
        let mm = unsafe { (*mt).get(state.mm_close) };
        if mm.is_nil() {
            return Err(ElaraStatus::ErrRuntime);
        }
        state.push_close_hook(mm);
        return Ok(());
    }
    Err(ElaraStatus::ErrRuntime)
}
