//! Host-callable native function objects (`ObjNativeFn`).

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;
use core::mem::size_of;
use core::ptr::NonNull;

use crate::ffi::context::CallContext;
use crate::gc::GcAllocator;
use crate::value::nan::{GcHeader, ObjType, Value};
use crate::vm::state::ElaraState;
use crate::vm::status::ElaraStatus;

/// C-compatible native function entry point.
pub type ElaraNativeFn =
    unsafe extern "C" fn(state: *mut ElaraState, ctx: *mut CallContext) -> ElaraStatus;

/// GC-managed wrapper around a host `ElaraNativeFn` plus opaque userdata.
#[repr(C, align(8))]
pub struct ObjNativeFn {
    pub header: GcHeader,
    pub func: ElaraNativeFn,
    pub userdata: *mut c_void,
}

impl ObjNativeFn {
    /// Allocates a new native function object on the GC heap.
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        func: ElaraNativeFn,
        userdata: *mut c_void,
    ) -> Option<NonNull<Self>> {
        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::NativeFn, gc_color),
                    func,
                    userdata,
                },
            );
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
            Some(NonNull::new_unchecked(ptr))
        }
    }

    /// Invokes this native with 0-based `args`, writing up to `results.len()` returns.
    ///
    /// Returns the number of results written on success.
    pub unsafe fn invoke(
        &self,
        state: &mut ElaraState,
        args: &[Value],
        results: &mut [Value],
    ) -> Result<u16, ElaraStatus> {
        let argc = u16::try_from(args.len()).unwrap_or(u16::MAX);
        let max_results = u16::try_from(results.len()).unwrap_or(u16::MAX);
        let mut ctx = CallContext {
            argc,
            max_results,
            nresults: 0,
            reserved: 0,
            args: args.as_ptr(),
            results: results.as_mut_ptr(),
            userdata: self.userdata,
        };
        let st = (self.func)(state as *mut ElaraState, &mut ctx);
        if st != ElaraStatus::Ok {
            return Err(st);
        }
        Ok(ctx.nresults.min(max_results))
    }

    /// Frees the native-fn heap allocation (no host finalizer).
    pub unsafe fn free(state: &mut ElaraState, native: *mut Self) {
        if native.is_null() {
            return;
        }
        state
            .alloc
            .free_bytes(native.cast::<u8>(), size_of::<Self>());
    }
}
