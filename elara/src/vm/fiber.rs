//! Cooperative fiber value stack and call-frame array.

use core::mem::size_of;
use core::ptr::NonNull;

use crate::gc::GcAllocator;
use crate::value::{GcHeader, ObjType, Value};
use crate::vm::frame::CallFrame;
use crate::vm::status::ElaraStatus;

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
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }
        NonNull::new(ptr)
    }

    /// Grows `stack` so `needed` slots from the current `open_top` fit.
    ///
    /// Callers MUST refresh any cached `base: *mut Value` after this returns `Ok`, because
    /// reallocation invalidates raw stack pointers. Frame `base_idx` / `func_idx` stay valid.
    pub fn ensure_stack(
        &mut self,
        alloc: &mut GcAllocator,
        needed: u32,
    ) -> Result<(), ElaraStatus> {
        let required = self
            .open_top
            .checked_add(needed)
            .ok_or(ElaraStatus::ErrOom)?;
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

    /// Frees stack and frame arrays owned by this fiber (not the header itself).
    pub fn free_fiber_buffers(&mut self, alloc: &mut GcAllocator) {
        if !self.stack.is_null() && self.stack_capacity > 0 {
            let bytes = (self.stack_capacity as usize) * size_of::<Value>();
            // SAFETY: `stack` allocated with `bytes`.
            unsafe { alloc.free_bytes(self.stack.cast::<u8>(), bytes) };
            self.stack = core::ptr::null_mut();
            self.stack_capacity = 0;
        }
        if !self.frames.is_null() && self.frame_capacity > 0 {
            let bytes = (self.frame_capacity as usize) * size_of::<CallFrame>();
            // SAFETY: `frames` allocated with `bytes`.
            unsafe { alloc.free_bytes(self.frames.cast::<u8>(), bytes) };
            self.frames = core::ptr::null_mut();
            self.frame_capacity = 0;
            self.frame_count = 0;
        }
    }

    /// Frees buffers then the fiber header.
    pub fn free_fiber(&mut self, alloc: &mut GcAllocator) {
        self.free_fiber_buffers(alloc);
        // SAFETY: fiber was allocated with `size_of::<ObjFiber>()`.
        unsafe {
            alloc.free_bytes((self as *mut Self).cast::<u8>(), size_of::<Self>());
        }
    }
}
