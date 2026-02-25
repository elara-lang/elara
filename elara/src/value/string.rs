//! Dual-tier immutable UTF-8 strings (`ObjString`) and open-addressed `InternPool`.
//!
//! - Short strings (`byte_len <= 64`) and compiler constant-pool strings (`K[i]`) are eagerly
//!   hashed with 32-bit FNV-1a and deduplicated in `InternPool` for O(1) pointer equality.
//! - Runtime long strings (`byte_len > 64`) are allocated on the GC heap without global interning
//!   and compute their FNV-1a hash lazily on first hash lookup or equal-length string comparison.

use core::mem::size_of;
use core::ptr::NonNull;

use crate::gc::GcAllocator;
use crate::value::nan::{GcHeader, ObjType, Value};

pub const ELARA_SHORT_STR_MAX: usize = 64;
pub const STR_FLAG_INTERNED: u8 = 1 << 0;
pub const STR_FLAG_HAS_HASH: u8 = 1 << 1;

pub const FNV1A_OFFSET_BASIS: u32 = 0x811C_9DC5;
pub const FNV1A_PRIME: u32 = 0x0100_0193;

/// Computes the 32-bit FNV-1a hash of `bytes` mixed with `seed`, guaranteeing a non-zero return.
#[inline]
#[must_use]
pub const fn fnv1a_hash(bytes: &[u8], seed: u32) -> u32 {
    let mut h = FNV1A_OFFSET_BASIS ^ seed;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u32;
        h = h.wrapping_mul(FNV1A_PRIME);
        i += 1;
    }
    if h == 0 { 1 } else { h }
}

#[repr(C, align(8))]
pub struct ObjString {
    pub header: GcHeader, // 16 bytes (`header.aux_hash` stores FNV-1a 32-bit hash)
    pub byte_len: u32,    // Byte length (excludes trailing NUL byte)
    pub reserved: u32,    // Explicit padding to 24 bytes
                          // Followed inline by `byte_len` UTF-8 bytes + 1 NUL ('\0') byte
}

impl ObjString {
    /// Returns the total heap allocation size in bytes for a string of `byte_len` bytes.
    #[inline]
    #[must_use]
    pub const fn alloc_size(byte_len: usize) -> usize {
        size_of::<Self>() + byte_len + 1
    }

    /// Returns a pointer to the inline NUL-terminated UTF-8 payload immediately following the header.
    #[inline]
    #[must_use]
    pub const fn payload_ptr(&self) -> *const u8 {
        // SAFETY: The payload is allocated inline immediately after `ObjString` (24 bytes).
        unsafe { (self as *const Self).add(1).cast::<u8>() }
    }

    /// Returns the string's raw byte slice (`&[u8]`), excluding the trailing NUL terminator.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: `ObjString` is always allocated with `byte_len` valid initialized bytes inline.
        unsafe { core::slice::from_raw_parts(self.payload_ptr(), self.byte_len as usize) }
    }

    /// Returns the string as a `&str` if valid UTF-8, or converts lossily / unchecked as appropriate.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    /// Returns `true` if this string is globally interned in `InternPool`.
    #[inline]
    #[must_use]
    pub const fn is_interned(&self) -> bool {
        (self.header.flags & STR_FLAG_INTERNED) != 0
    }

    /// Returns `true` if this string's FNV-1a hash is already computed and cached in `header.aux_hash`.
    #[inline]
    #[must_use]
    pub const fn has_hash(&self) -> bool {
        (self.header.flags & STR_FLAG_HAS_HASH) != 0
    }

    /// Returns the cached 32-bit FNV-1a hash, computing and caching it lazily if not yet present.
    #[inline]
    pub fn ensure_hash(&mut self) -> u32 {
        if self.has_hash() {
            return self.header.aux_hash;
        }
        let h = fnv1a_hash(self.as_bytes(), self.reserved);
        self.header.aux_hash = h;
        self.header.flags |= STR_FLAG_HAS_HASH;
        h
    }
}

/// Open-addressed Robin Hood string interning table and dual-tier string factory.
pub struct InternPool {
    buckets: *mut *mut ObjString,
    capacity: usize,
    count: usize,
    hash_seed: u32,
}

impl Default for InternPool {
    fn default() -> Self {
        Self::new(0)
    }
}

impl InternPool {
    /// Creates an empty `InternPool` with the given `hash_seed`.
    #[must_use]
    pub const fn new(hash_seed: u32) -> Self {
        Self {
            buckets: core::ptr::null_mut(),
            capacity: 0,
            count: 0,
            hash_seed,
        }
    }

    /// Returns the number of currently interned strings in the pool.
    #[inline]
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Allocates or interns `bytes` according to the Dual-Tier string rules:
    /// - If `bytes.len() <= ELARA_SHORT_STR_MAX` (64), interns in `self` (`STR_FLAG_INTERNED | STR_FLAG_HAS_HASH`).
    /// - If `bytes.len() > ELARA_SHORT_STR_MAX`, allocates a non-interned Long String on `all_objects`
    ///   with `flags = 0` and lazy hash computation.
    pub fn new_string(
        &mut self,
        alloc: &mut GcAllocator,
        all_objects: &mut *mut GcHeader,
        gc_color: u8,
        bytes: &[u8],
    ) -> Option<NonNull<ObjString>> {
        if bytes.len() <= ELARA_SHORT_STR_MAX {
            self.intern_always(alloc, gc_color, bytes)
        } else {
            let ptr = allocate_raw_string(alloc, gc_color, 0, 0, self.hash_seed, bytes)?;
            // Link runtime long string onto the global GC object list (`all_objects`).
            // SAFETY: `ptr` is a newly allocated valid `ObjString`.
            unsafe {
                (*ptr.as_ptr()).header.next = *all_objects;
                *all_objects = ptr.as_ptr().cast::<GcHeader>();
            }
            Some(ptr)
        }
    }

    /// Interns `bytes` unconditionally regardless of length (used for short strings and all
    /// compiler constant-pool strings `K[i]` so inline caches can always compare pointers in O(1)).
    pub fn intern_always(
        &mut self,
        alloc: &mut GcAllocator,
        gc_color: u8,
        bytes: &[u8],
    ) -> Option<NonNull<ObjString>> {
        let hash = fnv1a_hash(bytes, self.hash_seed);

        if self.capacity > 0
            && let Some(existing) = self.find_existing(bytes, hash)
        {
            return Some(existing);
        }

        // Resize at 75% load factor (`count + 1 > (capacity * 3) / 4`)
        if self.capacity == 0 || (self.count + 1) * 4 > self.capacity * 3 {
            let next_cap = if self.capacity == 0 {
                64
            } else {
                self.capacity * 2
            };
            self.resize(alloc, next_cap)?;
        }

        let flags = STR_FLAG_INTERNED | STR_FLAG_HAS_HASH;
        let str_ptr = allocate_raw_string(alloc, gc_color, flags, hash, self.hash_seed, bytes)?;
        self.insert_unique(str_ptr.as_ptr(), hash);
        self.count += 1;
        Some(str_ptr)
    }

    /// Indexes a string by 0-based byte offset `idx` (negative indices normalize as `len + idx`),
    /// returning an interned 1-byte `ObjString` `Value` or `Value::NIL` if out of bounds.
    pub fn index_byte(
        &mut self,
        alloc: &mut GcAllocator,
        gc_color: u8,
        s: &ObjString,
        idx: i64,
    ) -> Option<Value> {
        let len = i64::from(s.byte_len);
        let norm = if idx < 0 { len + idx } else { idx };
        if norm < 0 || norm >= len {
            return Some(Value::NIL);
        }
        let b = s.as_bytes()[norm as usize];
        let ch_str = self.intern_always(alloc, gc_color, &[b])?;
        Some(Value::from_obj(ch_str.cast::<GcHeader>(), ObjType::String))
    }

    /// Releases all interned strings and the bucket array back to `alloc`.
    pub fn free_all(&mut self, alloc: &mut GcAllocator) {
        if !self.buckets.is_null() && self.capacity > 0 {
            for i in 0..self.capacity {
                // SAFETY: `i < self.capacity` within valid `buckets` allocation.
                let s = unsafe { *self.buckets.add(i) };
                if !s.is_null() {
                    // SAFETY: `s` was allocated by `allocate_raw_string` on `alloc`.
                    let total = unsafe { ObjString::alloc_size((*s).byte_len as usize) };
                    unsafe { alloc.free_bytes(s.cast::<u8>(), total) };
                }
            }
            let buf_bytes = self.capacity * size_of::<*mut ObjString>();
            // SAFETY: `buckets` was allocated with `buf_bytes` by `alloc`.
            unsafe { alloc.free_bytes(self.buckets.cast::<u8>(), buf_bytes) };
            self.buckets = core::ptr::null_mut();
            self.capacity = 0;
            self.count = 0;
        }
    }

    fn find_existing(&self, bytes: &[u8], hash: u32) -> Option<NonNull<ObjString>> {
        let mask = self.capacity - 1;
        let mut idx = (hash as usize) & mask;
        loop {
            // SAFETY: `idx <= mask < self.capacity`.
            let entry = unsafe { *self.buckets.add(idx) };
            if entry.is_null() {
                return None;
            }
            // SAFETY: Non-null entry in `InternPool` is a valid `ObjString`.
            unsafe {
                if (*entry).header.aux_hash == hash
                    && (*entry).byte_len as usize == bytes.len()
                    && (*entry).as_bytes() == bytes
                {
                    return NonNull::new(entry);
                }
            }
            idx = (idx + 1) & mask;
        }
    }

    fn insert_unique(&mut self, mut str_ptr: *mut ObjString, hash: u32) {
        let mask = self.capacity - 1;
        let mut idx = (hash as usize) & mask;
        let mut psl = 0usize;

        loop {
            // SAFETY: `idx < self.capacity`.
            let slot = unsafe { self.buckets.add(idx) };
            let cur = unsafe { *slot };
            if cur.is_null() {
                unsafe { *slot = str_ptr };
                return;
            }
            // Robin Hood displacement: compute existing entry's PSL from its home bucket
            let cur_hash = unsafe { (*cur).header.aux_hash };
            let cur_home = (cur_hash as usize) & mask;
            let cur_psl = idx.wrapping_sub(cur_home) & mask;
            if psl > cur_psl {
                unsafe { *slot = str_ptr };
                str_ptr = cur;
                psl = cur_psl;
            }
            idx = (idx + 1) & mask;
            psl += 1;
        }
    }

    fn resize(&mut self, alloc: &mut GcAllocator, new_cap: usize) -> Option<()> {
        debug_assert!(new_cap.is_power_of_two());
        let new_bytes = new_cap.checked_mul(size_of::<*mut ObjString>())?;
        let raw = alloc.alloc_bytes(new_bytes)?;
        let new_buckets = raw.as_ptr().cast::<*mut ObjString>();
        // SAFETY: Zero-initialize pointer array (`null_mut()` is 0).
        unsafe { core::ptr::write_bytes(new_buckets, 0, new_cap) };

        let old_buckets = self.buckets;
        let old_cap = self.capacity;

        self.buckets = new_buckets;
        self.capacity = new_cap;

        if !old_buckets.is_null() && old_cap > 0 {
            for i in 0..old_cap {
                // SAFETY: `i < old_cap`.
                let s = unsafe { *old_buckets.add(i) };
                if !s.is_null() {
                    let h = unsafe { (*s).header.aux_hash };
                    self.insert_unique(s, h);
                }
            }
            let old_bytes = old_cap * size_of::<*mut ObjString>();
            // SAFETY: `old_buckets` was allocated with `old_bytes`.
            unsafe { alloc.free_bytes(old_buckets.cast::<u8>(), old_bytes) };
        }
        Some(())
    }
}

fn allocate_raw_string(
    alloc: &mut GcAllocator,
    gc_color: u8,
    flags: u8,
    aux_hash: u32,
    hash_seed: u32,
    bytes: &[u8],
) -> Option<NonNull<ObjString>> {
    let byte_len = u32::try_from(bytes.len()).ok()?;
    let total_size = ObjString::alloc_size(bytes.len());
    let raw = alloc.alloc_bytes(total_size)?;
    let str_ptr = raw.as_ptr().cast::<ObjString>();

    // SAFETY: `raw` has `total_size` bytes aligned to 8 bytes.
    unsafe {
        let mut header = GcHeader::new(ObjType::String, gc_color);
        header.flags = flags;
        header.aux_hash = aux_hash;

        core::ptr::write(
            str_ptr,
            ObjString {
                header,
                byte_len,
                reserved: hash_seed,
            },
        );
        let payload = str_ptr.add(1).cast::<u8>();
        if !bytes.is_empty() {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), payload, bytes.len());
        }
        *payload.add(bytes.len()) = 0; // Trailing NUL byte for C-FFI zero-copy compatibility
    }

    NonNull::new(str_ptr)
}
