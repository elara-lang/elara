//! Cooperative fiber value stack and call-frame array.

use core::mem::size_of;
use core::ptr::NonNull;

use crate::gc::GcAllocator;
use crate::value::{GcHeader, ObjType, Value};
use crate::vm::frame::CallFrame;
use crate::vm::status::ElaraStatus;
use crate::vm::upvalue::{ObjUpvalue, rebase_open_upvalues};

/// Execution fiber owning a growable value stack and call-frame array.
#[repr(C, align(8))]
pub struct ObjFiber {
    pub header: GcHeader,
    pub stack: *mut Value,
    pub stack_capacity: u32,
    pub open_top: u32,
    pub frames: *mut CallFrame,
    pub frame_capacity: u32,
    pub frame_count: u32,
    /// Open upvalues sorted descending by `stack_idx` (null-terminated singly linked list).
    pub open_upvalues: *mut ObjUpvalue,
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
                    open_top: 0,
                    frames,
                    frame_capacity: frame_cap,
                    frame_count: 0,
                    open_upvalues: core::ptr::null_mut(),
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
    pub fn ensure_stack(
        &mut self,
        alloc: &mut GcAllocator,
        min_capacity: u32,
    ) -> Result<(), ElaraStatus> {
        let required = self.open_top.max(min_capacity);
        if required <= self.stack_capacity {
            return Ok(());
        }

        let mut new_cap = if self.stack_capacity == 0 {
            64
        } else {
            self.stack_capacity
        };
        while new_cap < required {
            new_cap = new_cap.checked_mul(2).ok_or(ElaraStatus::ErrOom)?;
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

    /// Pushes a call frame, growing the frame array when needed.
    pub fn push_frame(
        &mut self,
        alloc: &mut GcAllocator,
        frame: CallFrame,
    ) -> Result<(), ElaraStatus> {
        if self.frame_count == self.frame_capacity {
            let new_cap = if self.frame_capacity == 0 {
                8
            } else {
                self.frame_capacity
                    .checked_mul(2)
                    .ok_or(ElaraStatus::ErrOom)?
            };
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
    }

    /// Frees buffers then the fiber header.
    pub fn free_fiber(&mut self, alloc: &mut GcAllocator) {
        self.free_fiber_buffers(alloc);
        unsafe {
            alloc.free_bytes((self as *mut Self).cast::<u8>(), size_of::<Self>());
        }
    }
}
