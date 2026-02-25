//! Host allocator gateway (`ElaraAllocFn` / `GcAllocator`) enforcing 8-byte alignment and heap ceilings.

use core::ffi::c_void;
use core::ptr::NonNull;
use std::alloc::{Layout, alloc, dealloc, realloc};

/// Host-injectable memory allocator signature matching C99 `elara_Alloc`.
///
/// - When `new_size == 0`, frees `ptr` (with previous size `old_size`) and returns `null`.
/// - When `ptr.is_null()`, allocates `new_size` bytes aligned to at least 8 bytes.
/// - Otherwise, reallocates `ptr` from `old_size` to `new_size` bytes.
pub type ElaraAllocFn = unsafe extern "C" fn(
    ud: *mut c_void,
    ptr: *mut c_void,
    old_size: usize,
    new_size: usize,
) -> *mut c_void;

/// Default host allocator backed by Rust's `std::alloc` with mandatory 8-byte alignment.
///
/// # Safety
///
/// Caller must guarantee that `ptr` was previously allocated by `default_alloc_fn` with
/// `old_size` (or is null when `old_size == 0`).
pub unsafe extern "C" fn default_alloc_fn(
    _ud: *mut c_void,
    ptr: *mut c_void,
    old_size: usize,
    new_size: usize,
) -> *mut c_void {
    const ALIGN: usize = 8;

    if new_size == 0 {
        if !ptr.is_null()
            && old_size > 0
            && let Ok(layout) = Layout::from_size_align(old_size, ALIGN)
        {
            // SAFETY: `ptr` was allocated with `layout` by this allocator.
            unsafe { dealloc(ptr.cast::<u8>(), layout) };
        }
        return core::ptr::null_mut();
    }

    if ptr.is_null() || old_size == 0 {
        let Ok(layout) = Layout::from_size_align(new_size, ALIGN) else {
            return core::ptr::null_mut();
        };
        // SAFETY: `new_size > 0` and `layout` has valid 8-byte alignment.
        return unsafe { alloc(layout).cast::<c_void>() };
    }

    let Ok(old_layout) = Layout::from_size_align(old_size, ALIGN) else {
        return core::ptr::null_mut();
    };
    // SAFETY: `ptr` was allocated with `old_layout` and `new_size > 0`.
    unsafe { realloc(ptr.cast::<u8>(), old_layout, new_size).cast::<c_void>() }
}

///Tracked memory allocator gateway used by all GC heap objects, strings, tables, and buffers.
#[derive(Debug)]
pub struct GcAllocator {
    pub alloc_fn: ElaraAllocFn,
    pub alloc_ud: *mut c_void,
    pub bytes_allocated: usize,
    pub mem_limit_bytes: usize,
}

impl Default for GcAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl GcAllocator {
    /// Creates a new `GcAllocator` using `default_alloc_fn` and no memory ceiling (`0`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            alloc_fn: default_alloc_fn,
            alloc_ud: core::ptr::null_mut(),
            bytes_allocated: 0,
            mem_limit_bytes: 0,
        }
    }

    /// Creates a `GcAllocator` with a custom host allocator callback and optional memory ceiling.
    #[must_use]
    pub fn with_custom(
        alloc_fn: ElaraAllocFn,
        alloc_ud: *mut c_void,
        mem_limit_bytes: usize,
    ) -> Self {
        Self {
            alloc_fn,
            alloc_ud,
            bytes_allocated: 0,
            mem_limit_bytes,
        }
    }

    /// Allocates a block of `size` bytes aligned to at least 8 bytes.
    /// Returns `None` if `mem_limit_bytes` would be exceeded or the host allocator returns null.
    pub fn alloc_bytes(&mut self, size: usize) -> Option<NonNull<u8>> {
        if size == 0 {
            return NonNull::new(core::ptr::dangling_mut::<u8>());
        }
        let next_bytes = self.bytes_allocated.checked_add(size)?;
        if self.mem_limit_bytes > 0 && next_bytes > self.mem_limit_bytes {
            return None;
        }

        // SAFETY: Requesting fresh allocation (`ptr = null`, `old_size = 0`, `new_size = size`).
        let raw = unsafe { (self.alloc_fn)(self.alloc_ud, core::ptr::null_mut(), 0, size) };
        let ptr = NonNull::new(raw.cast::<u8>())?;
        debug_assert!(
            (ptr.as_ptr() as usize) & 0x7 == 0,
            "Host allocator must return 8-byte aligned pointers"
        );
        self.bytes_allocated = next_bytes;
        Some(ptr)
    }

    /// Reallocates an existing block from `old_size` to `new_size` bytes.
    ///
    /// # Safety
    ///
    /// `ptr` must have been allocated by this `GcAllocator` with `old_size` bytes (or be null if `old_size == 0`).
    pub unsafe fn realloc_bytes(
        &mut self,
        ptr: *mut u8,
        old_size: usize,
        new_size: usize,
    ) -> Option<NonNull<u8>> {
        if new_size == 0 {
            // SAFETY: Caller guarantees `ptr` was allocated with `old_size`.
            unsafe { self.free_bytes(ptr, old_size) };
            return NonNull::new(core::ptr::dangling_mut::<u8>());
        }
        if ptr.is_null() || old_size == 0 {
            return self.alloc_bytes(new_size);
        }

        if new_size > old_size {
            let delta = new_size - old_size;
            let next_bytes = self.bytes_allocated.checked_add(delta)?;
            if self.mem_limit_bytes > 0 && next_bytes > self.mem_limit_bytes {
                return None;
            }
        }

        // SAFETY: Reallocating block owned by this allocator from `old_size` to `new_size`.
        let raw =
            unsafe { (self.alloc_fn)(self.alloc_ud, ptr.cast::<c_void>(), old_size, new_size) };
        let new_ptr = NonNull::new(raw.cast::<u8>())?;
        debug_assert!(
            (new_ptr.as_ptr() as usize) & 0x7 == 0,
            "Host allocator must return 8-byte aligned pointers"
        );
        if new_size >= old_size {
            self.bytes_allocated += new_size - old_size;
        } else {
            self.bytes_allocated = self.bytes_allocated.saturating_sub(old_size - new_size);
        }
        Some(new_ptr)
    }

    /// Frees a block of `old_size` bytes previously allocated by this `GcAllocator`.
    ///
    /// # Safety
    ///
    /// `ptr` must have been allocated by this `GcAllocator` with `old_size` bytes.
    pub unsafe fn free_bytes(&mut self, ptr: *mut u8, old_size: usize) {
        if ptr.is_null() || old_size == 0 {
            return;
        }
        // SAFETY: Calling host allocator with `new_size = 0` to free `ptr`.
        unsafe {
            (self.alloc_fn)(self.alloc_ud, ptr.cast::<c_void>(), old_size, 0);
        }
        self.bytes_allocated = self.bytes_allocated.saturating_sub(old_size);
    }
}
