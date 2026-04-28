//! Cooperative fiber value stack, call frames, and lifecycle status.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::mem::size_of;
use core::ptr::NonNull;

use crate::gc::GcAllocator;
use crate::value::{GcHeader, ObjType, Value};
use crate::vm::closure::ObjClosure;
use crate::vm::frame::CallFrame;
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;
use crate::vm::upvalue::{ObjUpvalue, close_upvalues_from, rebase_open_upvalues};

/// Minimum register-stack slots for a newly created fiber.
pub const ELARA_MIN_STACK_SLOTS: u32 = 64;
/// Hard upper bound on a fiber's register-stack capacity.
pub const ELARA_MAX_STACK_SLOTS: u32 = 65536;
/// Hard upper bound on nested call frames per fiber.
pub const ELARA_MAX_CALL_DEPTH: u32 = 1024;

/// Cooperative fiber lifecycle state.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberStatus {
    Suspended = 0,
    Running = 1,
    Normal = 2,
    Dead = 3,
}

/// Execution fiber owning a growable value stack and call-frame array.
#[repr(C, align(8))]
pub struct ObjFiber {
    pub header: GcHeader,
    pub stack: *mut Value,
    pub stack_capacity: u32,
    /// High-water active register index (exclusive).
    pub stack_top: u32,
    pub open_top: u32,
    pub frames: *mut CallFrame,
    pub frame_capacity: u32,
    pub frame_count: u32,
    /// Open upvalues sorted descending by `stack_idx`.
    pub open_upvalues: *mut ObjUpvalue,
    /// Parent fiber that called `fiber.resume` (null for the root).
    pub caller: *mut ObjFiber,
    pub status: FiberStatus,
    /// Caller's destination register base when this fiber yields.
    pub yield_dest_base: u32,
    /// Caller's expected result count when this fiber yields.
    pub yield_expected_results: u16,
    /// Last unhandled runtime error value carried by this fiber.
    pub error_val: Value,
    /// Intrusive GC gray-stack link.
    pub gclist: *mut GcHeader,
    /// LIFO stack of register indices registered for `defer` / `__close`.
    pub close_slots: *mut u32,
    pub close_slot_count: u32,
    pub close_slot_capacity: u32,
}

impl ObjFiber {
    /// Allocates an empty fiber with initial stack/frame capacities.
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        stack_cap: u32,
        frame_cap: u32,
    ) -> Option<NonNull<Self>> {
        let stack_cap = stack_cap.clamp(0, ELARA_MAX_STACK_SLOTS);
        let frame_cap = frame_cap.min(ELARA_MAX_CALL_DEPTH);

        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();

        let stack_bytes = (stack_cap as usize).checked_mul(size_of::<Value>())?;
        let stack = if stack_cap > 0 {
            alloc.alloc_bytes(stack_bytes)?.as_ptr().cast::<Value>()
        } else {
            core::ptr::null_mut()
        };

        let frame_bytes = (frame_cap as usize).checked_mul(size_of::<CallFrame>())?;
        let frames = if frame_cap > 0 {
            alloc.alloc_bytes(frame_bytes)?.as_ptr().cast::<CallFrame>()
        } else {
            core::ptr::null_mut()
        };

        // SAFETY: `ptr` is valid storage for `ObjFiber`.
        unsafe {
            if !stack.is_null() {
                for i in 0..stack_cap as usize {
                    *stack.add(i) = Value::NIL;
                }
            }
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::Fiber, gc_color),
                    stack,
                    stack_capacity: stack_cap,
                    stack_top: 0,
                    open_top: 0,
                    frames,
                    frame_capacity: frame_cap,
                    frame_count: 0,
                    open_upvalues: core::ptr::null_mut(),
                    caller: core::ptr::null_mut(),
                    status: FiberStatus::Suspended,
                    yield_dest_base: 0,
                    yield_expected_results: 0,
                    error_val: Value::NIL,
                    gclist: core::ptr::null_mut(),
                    close_slots: core::ptr::null_mut(),
                    close_slot_count: 0,
                    close_slot_capacity: 0,
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }
        NonNull::new(ptr)
    }

    /// Creates a suspended fiber whose `stack[0]` holds `closure` (for `fiber.create`).
    ///
    /// # Errors
    ///
    /// Returns `ErrRuntime` when `closure` is null.
    pub fn create(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        closure: *mut ObjClosure,
    ) -> Result<NonNull<Self>, ElaraStatus> {
        if closure.is_null() {
            return Err(ElaraStatus::ErrRuntime);
        }
        let fiber = Self::new(alloc, all_objects, gc_color, ELARA_MIN_STACK_SLOTS, 8)
            .ok_or(ElaraStatus::ErrOom)?;
        unsafe {
            let f = fiber.as_ptr();
            *((*f).stack) = Value::from_obj(
                NonNull::new(closure.cast()).expect("non-null"),
                ObjType::Closure,
            );
            (*f).stack_top = 1;
            (*f).open_top = 1;
            (*f).status = FiberStatus::Suspended;
        }
        Ok(fiber)
    }

    /// Pushes a register index onto the LIFO `defer` / `__close` stack.
    pub fn push_close_slot(
        &mut self,
        alloc: &mut GcAllocator,
        slot: u32,
    ) -> Result<(), ElaraStatus> {
        if self.close_slot_count == self.close_slot_capacity {
            let new_cap = if self.close_slot_capacity == 0 {
                8
            } else {
                self.close_slot_capacity
                    .checked_mul(2)
                    .ok_or(ElaraStatus::ErrOom)?
            };
            let old_bytes = (self.close_slot_capacity as usize) * size_of::<u32>();
            let new_bytes = (new_cap as usize)
                .checked_mul(size_of::<u32>())
                .ok_or(ElaraStatus::ErrOom)?;
            let raw =
                unsafe { alloc.realloc_bytes(self.close_slots.cast::<u8>(), old_bytes, new_bytes) }
                    .ok_or(ElaraStatus::ErrOom)?;
            self.close_slots = raw.as_ptr().cast::<u32>();
            self.close_slot_capacity = new_cap;
        }
        unsafe {
            *self.close_slots.add(self.close_slot_count as usize) = slot;
        }
        self.close_slot_count += 1;
        Ok(())
    }

    /// Grows `stack` so its capacity is at least `min_capacity`.
    ///
    /// Callers MUST refresh any cached `base: *mut Value` after this returns `Ok`, because
    /// reallocation invalidates raw stack pointers. Frame `base_idx` / `func_idx` stay valid.
    /// Open upvalue `location` pointers are rebased automatically.
    ///
    /// # Errors
    ///
    /// Returns `ErrRuntime` (fiber stack overflow) when growth would exceed
    /// [`ELARA_MAX_STACK_SLOTS`].
    pub fn ensure_stack(
        &mut self,
        alloc: &mut GcAllocator,
        min_capacity: u32,
    ) -> Result<(), ElaraStatus> {
        let required = self.open_top.max(min_capacity);
        if required <= self.stack_capacity {
            return Ok(());
        }
        if required > ELARA_MAX_STACK_SLOTS {
            return Err(ElaraStatus::ErrRuntime); // "Fiber stack overflow"
        }

        let mut new_cap = if self.stack_capacity == 0 {
            ELARA_MIN_STACK_SLOTS
        } else {
            self.stack_capacity
        };
        while new_cap < required {
            let Some(doubled) = new_cap.checked_mul(2) else {
                return Err(ElaraStatus::ErrRuntime); // "Fiber stack overflow"
            };
            if doubled > ELARA_MAX_STACK_SLOTS {
                if ELARA_MAX_STACK_SLOTS < required {
                    return Err(ElaraStatus::ErrRuntime); // "Fiber stack overflow"
                }
                new_cap = ELARA_MAX_STACK_SLOTS;
                break;
            }
            new_cap = doubled;
        }

        let old_bytes = (self.stack_capacity as usize) * size_of::<Value>();
        let new_bytes = (new_cap as usize)
            .checked_mul(size_of::<Value>())
            .ok_or(ElaraStatus::ErrOom)?;
        // SAFETY: `stack` owned by `alloc` with `old_bytes` size.
        let raw = unsafe { alloc.realloc_bytes(self.stack.cast::<u8>(), old_bytes, new_bytes) }
            .ok_or(ElaraStatus::ErrOom)?;
        self.stack = raw.as_ptr().cast::<Value>();

        // SAFETY: newly grown region `[stack_capacity, new_cap)` is uninitialized.
        unsafe {
            for i in self.stack_capacity as usize..new_cap as usize {
                *self.stack.add(i) = Value::NIL;
            }
        }
        self.stack_capacity = new_cap;
        unsafe {
            rebase_open_upvalues(self);
        }
        Ok(())
    }

    /// Pushes a call frame, growing the frame array when needed (up to [`ELARA_MAX_CALL_DEPTH`]).
    pub fn push_frame(
        &mut self,
        alloc: &mut GcAllocator,
        frame: CallFrame,
    ) -> Result<(), ElaraStatus> {
        if self.frame_count >= ELARA_MAX_CALL_DEPTH {
            return Err(ElaraStatus::ErrRuntime);
        }
        if self.frame_count == self.frame_capacity {
            let new_cap = if self.frame_capacity == 0 {
                8
            } else {
                let doubled = self
                    .frame_capacity
                    .checked_mul(2)
                    .ok_or(ElaraStatus::ErrRuntime)?;
                doubled.min(ELARA_MAX_CALL_DEPTH)
            };
            if new_cap <= self.frame_capacity {
                return Err(ElaraStatus::ErrRuntime);
            }
            let old_bytes = (self.frame_capacity as usize) * size_of::<CallFrame>();
            let new_bytes = (new_cap as usize)
                .checked_mul(size_of::<CallFrame>())
                .ok_or(ElaraStatus::ErrOom)?;
            // SAFETY: `frames` owned by `alloc`.
            let raw =
                unsafe { alloc.realloc_bytes(self.frames.cast::<u8>(), old_bytes, new_bytes) }
                    .ok_or(ElaraStatus::ErrOom)?;
            self.frames = raw.as_ptr().cast::<CallFrame>();
            self.frame_capacity = new_cap;
        }
        // SAFETY: `frame_count < frame_capacity` after growth.
        unsafe {
            *self.frames.add(self.frame_count as usize) = frame;
        }
        self.frame_count += 1;
        Ok(())
    }

    /// Closes all open upvalues / defer slots and marks the fiber [`FiberStatus::Dead`].
    pub fn terminate(&mut self, state: &mut ElaraState) -> Result<(), ElaraStatus> {
        unsafe {
            close_upvalues_from(state, self, 0)?;
        }
        self.frame_count = 0;
        self.status = FiberStatus::Dead;
        Ok(())
    }

    /// Frees stack, frame, and close-slot arrays owned by this fiber (not the header itself).
    pub fn free_fiber_buffers(&mut self, alloc: &mut GcAllocator) {
        if !self.stack.is_null() && self.stack_capacity > 0 {
            let bytes = (self.stack_capacity as usize) * size_of::<Value>();
            unsafe { alloc.free_bytes(self.stack.cast::<u8>(), bytes) };
            self.stack = core::ptr::null_mut();
            self.stack_capacity = 0;
        }
        if !self.frames.is_null() && self.frame_capacity > 0 {
            let bytes = (self.frame_capacity as usize) * size_of::<CallFrame>();
            unsafe { alloc.free_bytes(self.frames.cast::<u8>(), bytes) };
            self.frames = core::ptr::null_mut();
            self.frame_capacity = 0;
            self.frame_count = 0;
        }
        if !self.close_slots.is_null() && self.close_slot_capacity > 0 {
            let bytes = (self.close_slot_capacity as usize) * size_of::<u32>();
            unsafe { alloc.free_bytes(self.close_slots.cast::<u8>(), bytes) };
            self.close_slots = core::ptr::null_mut();
            self.close_slot_capacity = 0;
            self.close_slot_count = 0;
        }
        self.open_upvalues = core::ptr::null_mut();
        self.caller = core::ptr::null_mut();
        self.gclist = core::ptr::null_mut();
    }

    /// Frees buffers then the fiber header.
    pub fn free_fiber(&mut self, alloc: &mut GcAllocator) {
        self.free_fiber_buffers(alloc);
        unsafe {
            alloc.free_bytes((self as *mut Self).cast::<u8>(), size_of::<Self>());
        }
    }
}
