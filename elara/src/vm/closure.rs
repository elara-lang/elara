//! Closure object wrapping a function prototype (upvalue wiring lands in a later change).

use core::mem::size_of;
use core::ptr::NonNull;

use crate::compiler::proto::ObjProto;
use crate::gc::GcAllocator;
use crate::value::{GcHeader, ObjType};

/// Runtime closure: `proto` plus captured upvalue slots (empty until upvalue machinery lands).
#[repr(C, align(8))]
pub struct ObjClosure {
    pub header: GcHeader,
    pub proto: *mut ObjProto,
    pub upvals: *mut *mut core::ffi::c_void, // placeholder until ObjUpvalue exists
    pub upvalue_count: u8,
    pub reserved: [u8; 7],
}

impl ObjClosure {
    /// Allocates a closure referencing `proto` with zero upvalues.
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        proto: *mut ObjProto,
    ) -> Option<NonNull<Self>> {
        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();
        // SAFETY: `ptr` is valid, aligned storage for `ObjClosure`.
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::Closure, gc_color),
                    proto,
                    upvals: core::ptr::null_mut(),
                    upvalue_count: 0,
                    reserved: [0; 7],
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }
        NonNull::new(ptr)
    }

    /// Frees the closure header bytes (does not free `proto`).
    pub fn free_closure(&mut self, alloc: &mut GcAllocator) {
        // SAFETY: `self` was allocated with `size_of::<ObjClosure>()`.
        unsafe {
            alloc.free_bytes((self as *mut Self).cast::<u8>(), size_of::<Self>());
        }
    }
}
