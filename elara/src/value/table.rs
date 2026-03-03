//! Hybrid 0-based dense array and Robin Hood open-addressed hash table (`ObjTable`).
//!
//! Every `ObjTable` header is `#[repr(C, align(8))]` and exactly 64 bytes (one CPU cache line
//! on 64-bit targets).

use core::mem::size_of;
use core::ptr::NonNull;

use crate::gc::GcAllocator;
use crate::value::nan::{GcHeader, ObjType, Value};
use crate::value::ops::float_to_exact_i64;

#[repr(C, align(8))]
#[derive(Debug, Clone, Copy)]
pub struct MapEntry {
    pub key: Value,   // 8 bytes: Value::NIL indicates empty bucket
    pub value: Value, // 8 bytes
    pub hash: u32,    // 4 bytes: Cached non-zero 32-bit hash
    pub psl: u16,     // 2 bytes: Probe Sequence Length (Robin Hood)
    pub flags: u16,   // 2 bytes
} // 24 bytes

impl MapEntry {
    pub const EMPTY: Self = Self {
        key: Value::NIL,
        value: Value::NIL,
        hash: 0,
        psl: 0,
        flags: 0,
    };
}

#[repr(C, align(8))]
pub struct ObjTable {
    pub header: GcHeader, // 16 bytes (`header.aux_hash` = Shape / Rehash Generation ID)
    pub array: *mut Value, // 8 bytes: 0-based dense array [0..array_capacity]
    pub entries: *mut MapEntry, // 8 bytes: Robin Hood open-addressed buckets
    pub metatable: *mut ObjTable, // 8 bytes: Optional metatable pointer (or null)
    pub array_len: u32,   // 4 bytes: Dense logical length from index 0 (#t)
    pub array_capacity: u32, // 4 bytes: Allocated slots in array
    pub hash_count: u32,  // 4 bytes: Active entries in hash partition
    pub hash_mask: u32,   // 4 bytes: (hash_capacity - 1), or 0 when empty
    pub gclist: *mut GcHeader, // 8 bytes: Intrusive GC gray stack link
} // 64 bytes on 64-bit targets

/// Converts a key `Value` into a non-negative `u32` array index if it represents an exact integer in `0..=u32::MAX - 1`.
#[inline]
#[must_use]
pub fn key_to_array_index(key: Value) -> Option<u32> {
    if let Some(i) = key.to_i64() {
        if i >= 0 && i < i64::from(u32::MAX) {
            return Some(i as u32);
        }
        return None;
    }
    if key.is_float()
        && let Some(i) = float_to_exact_i64(key.as_f64())
        && i >= 0
        && i < i64::from(u32::MAX)
    {
        return Some(i as u32);
    }
    None
}

impl ObjTable {
    /// Allocates and initializes a new `ObjTable` on the GC heap with optional preallocated
    /// `array_cap` and `hash_cap` capacities.
    pub fn new(
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        shape_seed: u32,
        array_cap: u32,
        hash_cap: u32,
    ) -> Option<NonNull<Self>> {
        let raw = alloc.alloc_bytes(size_of::<Self>())?;
        let ptr = raw.as_ptr().cast::<Self>();

        let mut header = GcHeader::new(ObjType::Table, gc_color);
        header.aux_hash = if shape_seed == 0 { 1 } else { shape_seed };

        // SAFETY: `ptr` is valid, 8-byte aligned, and sized for `ObjTable`.
        unsafe {
            core::ptr::write(
                ptr,
                Self {
                    header,
                    array: core::ptr::null_mut(),
                    entries: core::ptr::null_mut(),
                    metatable: core::ptr::null_mut(),
                    array_len: 0,
                    array_capacity: 0,
                    hash_count: 0,
                    hash_mask: 0,
                    gclist: core::ptr::null_mut(),
                },
            );
        }

        if array_cap > 0 || hash_cap > 0 {
            let norm_array = if array_cap > 0 {
                array_cap.next_power_of_two()
            } else {
                0
            };
            let norm_hash = if hash_cap > 0 {
                hash_cap.next_power_of_two().max(4)
            } else {
                0
            };
            // SAFETY: `ptr` was just initialized above.
            if unsafe { (*ptr).realloc_partitions(alloc, norm_array, norm_hash) }.is_none() {
                unsafe { alloc.free_bytes(ptr.cast::<u8>(), size_of::<Self>()) };
                return None;
            }
        }

        // Link onto global GC object list (`all_objects`).
        // SAFETY: `ptr` is valid and initialized.
        unsafe {
            (*ptr).header.next = *all_objects;
            *all_objects = ptr.cast::<GcHeader>();
        }

        NonNull::new(ptr)
    }

    /// Returns the number of allocated hash buckets (`0` when empty, or `hash_mask + 1`).
    #[inline]
    #[must_use]
    pub const fn hash_capacity(&self) -> u32 {
        if self.entries.is_null() {
            0
        } else {
            self.hash_mask + 1
        }
    }

    /// Bumps the table's shape/rehash generation counter (`header.aux_hash`), skipping `0`.
    #[inline]
    pub(super) fn bump_shape(&mut self) {
        let next = self.header.aux_hash.wrapping_add(1);
        self.header.aux_hash = if next == 0 { 1 } else { next };
    }

    /// Reads the value associated with `key`, returning `Value::NIL` if absent.
    #[inline]
    #[must_use]
    pub fn get(&self, key: Value) -> Value {
        if key.is_nil() {
            return Value::NIL;
        }
        if let Some(idx) = key_to_array_index(key)
            && idx < self.array_capacity
        {
            // SAFETY: `idx < self.array_capacity`.
            return unsafe { *self.array.add(idx as usize) };
        }
        self.get_from_hash(key)
    }

    /// Fast-path 0-based integer key read (`t[idx]`), normalizing negative indices (`array_len + idx`)
    /// when `-array_len <= idx < 0`.
    #[inline]
    #[must_use]
    pub fn get_int(&self, idx: i64) -> Value {
        let norm = if idx < 0 {
            i64::from(self.array_len) + idx
        } else {
            idx
        };
        if norm >= 0 && norm < i64::from(self.array_capacity) {
            // SAFETY: `0 <= norm < self.array_capacity`.
            return unsafe { *self.array.add(norm as usize) };
        }
        self.get_from_hash(Value::from_i48(idx))
    }

    /// Writes `val` at `key`, maintaining `array_len` bidirectionally and resizing partitions as needed.
    ///
    /// # Errors
    ///
    /// - Returns `Err("Invalid table key: nil or NaN")` if `key` is `nil` or `NaN`.
    /// - Returns `Err("Out of memory")` if partition growth exceeds the allocator limit.
    pub fn set(
        &mut self,
        alloc: &mut GcAllocator,
        key: Value,
        val: Value,
    ) -> Result<(), &'static str> {
        if key.is_nil() || (key.is_float() && key.as_f64().is_nan()) {
            return Err("Invalid table key: nil or NaN");
        }

        // Fast-path: key is a 0-based index within current `array_capacity`
        if let Some(idx) = key_to_array_index(key) {
            if idx < self.array_capacity {
                self.write_array_slot(idx, val);
                return Ok(());
            }
            // If writing a non-nil value at or near the dense frontier, trigger hybrid resize!
            if !val.is_nil() && idx <= self.array_capacity.saturating_mul(2) {
                self.resize_with_hint(alloc, Some((idx, val)))
                    .ok_or("Out of memory")?;
                return Ok(());
            }
        }

        self.set_in_hash(alloc, key, val)
    }

    /// Appends `val` at the 0-based end of the array (`t[array_len] = val`) and returns the new `array_len`.
    ///
    /// # Errors
    ///
    /// Returns `Err("Out of memory")` if array growth fails.
    pub fn push(&mut self, alloc: &mut GcAllocator, val: Value) -> Result<u32, &'static str> {
        let idx = self.array_len;
        self.set(alloc, Value::from_i48(i64::from(idx)), val)?;
        Ok(self.array_len)
    }

    /// Pops and returns the last element `t[array_len - 1]`, or `Value::NIL` if `array_len == 0`.
    pub fn pop(&mut self) -> Value {
        if self.array_len == 0 {
            return Value::NIL;
        }
        let idx = self.array_len - 1;
        // SAFETY: `idx < self.array_len <= self.array_capacity`.
        let val = unsafe { *self.array.add(idx as usize) };
        self.write_array_slot(idx, Value::NIL);
        val
    }

    pub(super) fn write_array_slot(&mut self, idx: u32, val: Value) {
        // SAFETY: Caller guarantees `idx < self.array_capacity`.
        unsafe {
            *self.array.add(idx as usize) = val;
            if !val.is_nil() {
                if idx == self.array_len {
                    self.array_len += 1;
                    while self.array_len < self.array_capacity
                        && !(*self.array.add(self.array_len as usize)).is_nil()
                    {
                        self.array_len += 1;
                    }
                }
            } else if idx + 1 == self.array_len {
                while self.array_len > 0
                    && (*self.array.add((self.array_len - 1) as usize)).is_nil()
                {
                    self.array_len -= 1;
                }
            }
        }
    }

    /// Zero-allocation table iteration cursor (`OP_FORIN`).
    ///
    /// Returns `Some((next_cursor, key, value))` or `None` when both array and hash partitions are exhausted.
    ///
    /// # Safety
    ///
    /// `self` must have valid `array` and `entries` pointers matching `array_capacity` and `hash_mask`.
    #[must_use]
    pub unsafe fn next_entry(&self, cursor: u32) -> Option<(u32, Value, Value)> {
        let mut idx = cursor;
        while idx < self.array_capacity {
            // SAFETY: `idx < self.array_capacity`.
            let v = unsafe { *self.array.add(idx as usize) };
            if !v.is_nil() {
                return Some((idx + 1, Value::from_i48(i64::from(idx)), v));
            }
            idx += 1;
        }

        if !self.entries.is_null() {
            let mut hash_idx = cursor.saturating_sub(self.array_capacity);
            while hash_idx <= self.hash_mask {
                // SAFETY: `hash_idx <= self.hash_mask`.
                let entry = unsafe { *self.entries.add(hash_idx as usize) };
                if !entry.key.is_nil() {
                    return Some((self.array_capacity + hash_idx + 1, entry.key, entry.value));
                }
                hash_idx += 1;
            }
        }

        None
    }

    /// Frees the table's internal `array` and `entries` buffers and the `ObjTable` struct itself.
    pub fn free_table(&mut self, alloc: &mut GcAllocator) {
        if !self.array.is_null() && self.array_capacity > 0 {
            let bytes = (self.array_capacity as usize) * size_of::<Value>();
            // SAFETY: `self.array` was allocated with `bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.array.cast::<u8>(), bytes) };
            self.array = core::ptr::null_mut();
            self.array_capacity = 0;
            self.array_len = 0;
        }
        if !self.entries.is_null() {
            let cap = self.hash_capacity() as usize;
            let bytes = cap * size_of::<MapEntry>();
            // SAFETY: `self.entries` was allocated with `bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.entries.cast::<u8>(), bytes) };
            self.entries = core::ptr::null_mut();
            self.hash_count = 0;
            self.hash_mask = 0;
        }
    }
}
