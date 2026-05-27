//! Managed host userdata (`ObjUserdata`) with typed descriptor, finalizer, and GC trace hook.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::{c_char, c_void};
use core::mem::size_of;
use core::ptr::NonNull;

use crate::gc::GcAllocator;
use crate::value::nan::{GcHeader, ObjType};
use crate::value::table::ObjTable;
use crate::vm::state::ElaraState;

/// Host finalizer invoked when the GC sweeps an unreachable userdata.
pub type ElaraFinalizer =
    unsafe extern "C" fn(state: *mut ElaraState, data: *mut c_void, size: usize);

/// Optional GC trace hook: shade additional host-held GC roots from the payload.
pub type ElaraTraceFn = unsafe extern "C" fn(state: *mut ElaraState, data: *mut c_void);

/// Lifecycle descriptor for a userdata type (host-owned, usually `'static`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ElaraUserdataDesc {
    pub type_name: *const c_char,
    pub type_tag: u32,
    pub finalize: Option<ElaraFinalizer>,
    pub trace: Option<ElaraTraceFn>,
}

// Host descriptors are immutable after registration; pointer fields are opaque C ABI.
unsafe impl Sync for ElaraUserdataDesc {}
unsafe impl Send for ElaraUserdataDesc {}

/// GC-managed userdata: fixed header followed by an inline `size`-byte payload.
///
/// Layout (64-bit): `GcHeader` (16) + `metatable` (8) + `desc` (8) + `size` (8) = 40 bytes,
/// then the payload at 8-byte-aligned offset 40.
#[repr(C, align(8))]
pub struct ObjUserdata {
    pub header: GcHeader,
    pub metatable: *mut ObjTable,
    pub desc: *const ElaraUserdataDesc,
    pub size: usize,
}

const _: () = assert!(size_of::<ObjUserdata>() == 40);

impl ObjUserdata {
    /// Total heap allocation size for a userdata with `payload_size` bytes.
    #[inline]
    #[must_use]
    pub const fn alloc_size(payload_size: usize) -> usize {
        size_of::<Self>() + payload_size
    }

    /// Pointer to the inline payload immediately after the header.
    #[inline]
    #[must_use]
    pub fn payload_ptr(&self) -> *mut u8 {
        // SAFETY: payload is allocated inline immediately after `ObjUserdata` (40 bytes).
        unsafe { (self as *const Self).add(1).cast::<u8>().cast_mut() }
    }

    /// Allocates a new userdata of `payload_size` bytes with the given descriptor.
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        payload_size: usize,
        desc: *const ElaraUserdataDesc,
    ) -> Option<NonNull<Self>> {
        let total = Self::alloc_size(payload_size);
        let raw = alloc.alloc_bytes(total)?;
        let ptr = raw.as_ptr().cast::<Self>();
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header: GcHeader::new(ObjType::Userdata, gc_color),
                    metatable: core::ptr::null_mut(),
                    desc,
                    size: payload_size,
                },
            );
            if payload_size > 0 {
                core::ptr::write_bytes((*ptr).payload_ptr(), 0, payload_size);
            }
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
            Some(NonNull::new_unchecked(ptr))
        }
    }

    /// Returns the payload pointer when `val` is userdata with matching `expected_type_tag`.
    pub unsafe fn check(val: crate::value::Value, expected_type_tag: u32) -> Option<*mut c_void> {
        if !val.is_obj_type(ObjType::Userdata) {
            return None;
        }
        let ud = &*(val.as_obj().cast::<Self>());
        if ud.desc.is_null() || (*ud.desc).type_tag != expected_type_tag {
            return None;
        }
        Some(ud.payload_ptr().cast())
    }

    /// Runs the descriptor finalizer (if any), then frees the allocation.
    pub unsafe fn finalize_and_free(state: &mut ElaraState, ud: *mut Self) {
        if ud.is_null() {
            return;
        }
        let size = (*ud).size;
        let desc = (*ud).desc;
        if !desc.is_null()
            && let Some(fin) = (*desc).finalize
        {
            fin(state as *mut ElaraState, (*ud).payload_ptr().cast(), size);
        }
        let total = Self::alloc_size(size);
        state.alloc.free_bytes(ud.cast::<u8>(), total);
    }
}
