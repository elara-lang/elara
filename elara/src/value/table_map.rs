//! Robin Hood open-addressed hash probing, backward-shift deletion, and log2 histogram resizing.

use core::mem::size_of;

use crate::gc::GcAllocator;
use crate::value::nan::Value;
use crate::value::table::{MapEntry, ObjTable, key_to_array_index};

impl ObjTable {
    pub(super) fn get_from_hash(&self, key: Value) -> Value {
        if self.entries.is_null() || self.hash_count == 0 {
            return Value::NIL;
        }
        let hash = key.val_hash();
        let mask = self.hash_mask;
        let mut idx = hash & mask;
        let mut psl = 0u16;

        loop {
            // SAFETY: `idx <= self.hash_mask`.
            let entry = unsafe { &*self.entries.add(idx as usize) };
            if entry.key.is_nil() || psl > entry.psl {
                // Early termination via Robin Hood PSL invariant!
                return Value::NIL;
            }
            if entry.hash == hash && entry.key.val_equal(key) {
                return entry.value;
            }
            idx = (idx + 1) & mask;
            psl += 1;
        }
    }

    pub(super) fn set_in_hash(
        &mut self,
        alloc: &mut GcAllocator,
        key: Value,
        val: Value,
    ) -> Result<(), &'static str> {
        let hash = key.val_hash();

        // Check if key already exists in hash partition
        if !self.entries.is_null() {
            let mask = self.hash_mask;
            let mut idx = hash & mask;
            let mut psl = 0u16;

            loop {
                // SAFETY: `idx <= self.hash_mask`.
                let entry = unsafe { &mut *self.entries.add(idx as usize) };
                if entry.key.is_nil() || psl > entry.psl {
                    break;
                }
                if entry.hash == hash && entry.key.val_equal(key) {
                    if val.is_nil() {
                        self.backward_shift_delete(idx);
                    } else {
                        entry.value = val;
                    }
                    return Ok(());
                }
                idx = (idx + 1) & mask;
                psl += 1;
            }
        }

        // Setting a non-existent key to `nil` is a no-op
        if val.is_nil() {
            return Ok(());
        }

        // Resize if hash load factor reaches 75% (`(hash_count + 1) * 4 > hash_capacity * 3`)
        let cap = self.hash_capacity();
        if cap == 0 || (self.hash_count + 1) * 4 > cap * 3 {
            self.resize_with_hint(alloc, None).ok_or("Out of memory")?;
            // After hybrid resize, an integer key might now fit inside the newly grown `array`!
            if let Some(arr_idx) = key_to_array_index(key)
                && arr_idx < self.array_capacity
            {
                self.write_array_slot(arr_idx, val);
                return Ok(());
            }
        }

        self.insert_fresh_hash_entry(key, val, hash);
        Ok(())
    }

    fn insert_fresh_hash_entry(&mut self, key: Value, val: Value, hash: u32) {
        let mask = self.hash_mask;
        let mut idx = hash & mask;
        let mut incoming = MapEntry {
            key,
            value: val,
            hash,
            psl: 0,
            flags: 0,
        };
        let mut displaced_any = false;

        loop {
            // SAFETY: `idx <= self.hash_mask`.
            let slot = unsafe { &mut *self.entries.add(idx as usize) };
            if slot.key.is_nil() {
                *slot = incoming;
                self.hash_count += 1;
                if displaced_any {
                    self.bump_shape();
                }
                return;
            }
            if incoming.psl > slot.psl {
                core::mem::swap(slot, &mut incoming);
                displaced_any = true;
            }
            idx = (idx + 1) & mask;
            incoming.psl += 1;
        }
    }

    fn backward_shift_delete(&mut self, start_idx: u32) {
        let mask = self.hash_mask;
        let mut i = start_idx;
        loop {
            let next = (i + 1) & mask;
            // SAFETY: `i <= mask` and `next <= mask`.
            unsafe {
                let next_entry = *self.entries.add(next as usize);
                if next_entry.key.is_nil() || next_entry.psl == 0 {
                    *self.entries.add(i as usize) = MapEntry::EMPTY;
                    break;
                }
                let mut shifted = next_entry;
                shifted.psl -= 1;
                *self.entries.add(i as usize) = shifted;
            }
            i = next;
        }
        self.hash_count -= 1;
        self.bump_shape();
    }

    /// Computes a 31-bucket log2 histogram of all non-negative integer keys across `array`,
    /// `entries`, and an optional incoming `(hint_idx, hint_val)` pair, sizing both partitions
    /// so the dense array holds the largest $N = 2^i$ with $> 50\%$ occupancy.
    pub(super) fn resize_with_hint(
        &mut self,
        alloc: &mut GcAllocator,
        hint: Option<(u32, Value)>,
    ) -> Option<()> {
        let mut hist = [0u32; 31];
        let mut total_int_keys = 0u32;
        let mut non_int_keys = 0u32;

        // 1. Count existing array elements
        for i in 0..self.array_capacity {
            // SAFETY: `i < self.array_capacity`.
            let v = unsafe { *self.array.add(i as usize) };
            if !v.is_nil() {
                hist[log2_bucket(i)] += 1;
                total_int_keys += 1;
            }
        }

        // 2. Count existing hash entries
        if !self.entries.is_null() {
            for i in 0..=self.hash_mask {
                // SAFETY: `i <= self.hash_mask`.
                let e = unsafe { *self.entries.add(i as usize) };
                if !e.key.is_nil() {
                    if let Some(k) = key_to_array_index(e.key) {
                        hist[log2_bucket(k)] += 1;
                        total_int_keys += 1;
                    } else {
                        non_int_keys += 1;
                    }
                }
            }
        }

        // 3. Include incoming integer hint if present
        if let Some((k, v)) = hint
            && !v.is_nil()
        {
            hist[log2_bucket(k)] += 1;
            total_int_keys += 1;
        }

        // 4. Pick largest N = 2^b where cumulative integer keys in [0..N) > N / 2
        let mut best_array_cap = 0u32;
        let mut best_in_array = 0u32;
        let mut cum = 0u32;
        for (b, &count) in hist.iter().enumerate() {
            cum += count;
            let cap = 1u32 << b;
            if cum > cap / 2 {
                best_array_cap = cap;
                best_in_array = cum;
            }
        }

        let spill_to_hash =
            (total_int_keys - best_in_array) + non_int_keys + u32::from(hint.is_none());
        let needed_hash_cap = if spill_to_hash == 0 {
            0
        } else {
            ((spill_to_hash * 4) / 3 + 1).next_power_of_two().max(4)
        };

        self.realloc_partitions(alloc, best_array_cap, needed_hash_cap)?;
        if let Some((k, v)) = hint {
            if k < self.array_capacity {
                self.write_array_slot(k, v);
            } else {
                let h = Value::from_i48(i64::from(k)).val_hash();
                self.insert_fresh_hash_entry(Value::from_i48(i64::from(k)), v, h);
            }
        }
        Some(())
    }

    pub(super) fn realloc_partitions(
        &mut self,
        alloc: &mut GcAllocator,
        new_array_cap: u32,
        new_hash_cap: u32,
    ) -> Option<()> {
        let old_array = self.array;
        let old_array_cap = self.array_capacity;
        let old_entries = self.entries;
        let old_hash_cap = self.hash_capacity();

        // Allocate new array buffer if requested
        let new_array = if new_array_cap > 0 {
            let bytes = (new_array_cap as usize).checked_mul(size_of::<Value>())?;
            let raw = alloc.alloc_bytes(bytes)?;
            let ptr = raw.as_ptr().cast::<Value>();
            for i in 0..new_array_cap as usize {
                // SAFETY: `i < new_array_cap`.
                unsafe { *ptr.add(i) = Value::NIL };
            }
            ptr
        } else {
            core::ptr::null_mut()
        };

        // Allocate new hash entries buffer if requested
        let new_entries = if new_hash_cap > 0 {
            let bytes = (new_hash_cap as usize).checked_mul(size_of::<MapEntry>())?;
            let Some(raw) = alloc.alloc_bytes(bytes) else {
                if !new_array.is_null() {
                    let arr_bytes = (new_array_cap as usize) * size_of::<Value>();
                    // SAFETY: `new_array` was allocated above.
                    unsafe { alloc.free_bytes(new_array.cast::<u8>(), arr_bytes) };
                }
                return None;
            };
            let ptr = raw.as_ptr().cast::<MapEntry>();
            for i in 0..new_hash_cap as usize {
                // SAFETY: `i < new_hash_cap`.
                unsafe { *ptr.add(i) = MapEntry::EMPTY };
            }
            ptr
        } else {
            core::ptr::null_mut()
        };

        self.array = new_array;
        self.array_capacity = new_array_cap;
        self.array_len = 0;
        self.entries = new_entries;
        self.hash_count = 0;
        self.hash_mask = if new_hash_cap > 0 {
            new_hash_cap - 1
        } else {
            0
        };
        self.bump_shape();

        // Migrate old array entries
        if !old_array.is_null() && old_array_cap > 0 {
            for i in 0..old_array_cap {
                // SAFETY: `i < old_array_cap`.
                let v = unsafe { *old_array.add(i as usize) };
                if !v.is_nil() {
                    if i < new_array_cap {
                        // SAFETY: `i < new_array_cap`.
                        unsafe { *self.array.add(i as usize) = v };
                    } else {
                        let k = Value::from_i48(i64::from(i));
                        self.insert_fresh_hash_entry(k, v, k.val_hash());
                    }
                }
            }
            let old_bytes = (old_array_cap as usize) * size_of::<Value>();
            // SAFETY: `old_array` was allocated with `old_bytes`.
            unsafe { alloc.free_bytes(old_array.cast::<u8>(), old_bytes) };
        }

        // Migrate old hash entries (promoting integer keys `< new_array_cap` into `array`!)
        if !old_entries.is_null() && old_hash_cap > 0 {
            for i in 0..old_hash_cap {
                // SAFETY: `i < old_hash_cap`.
                let e = unsafe { *old_entries.add(i as usize) };
                if !e.key.is_nil() {
                    if let Some(arr_idx) = key_to_array_index(e.key)
                        && arr_idx < new_array_cap
                    {
                        // SAFETY: `arr_idx < new_array_cap`.
                        unsafe { *self.array.add(arr_idx as usize) = e.value };
                    } else {
                        self.insert_fresh_hash_entry(e.key, e.value, e.hash);
                    }
                }
            }
            let old_bytes = (old_hash_cap as usize) * size_of::<MapEntry>();
            // SAFETY: `old_entries` was allocated with `old_bytes`.
            unsafe { alloc.free_bytes(old_entries.cast::<u8>(), old_bytes) };
        }

        // Recompute contiguous `array_len` from index 0
        while self.array_len < self.array_capacity {
            // SAFETY: `self.array_len < self.array_capacity`.
            let v = unsafe { *self.array.add(self.array_len as usize) };
            if v.is_nil() {
                break;
            }
            self.array_len += 1;
        }

        Some(())
    }
}

/// Maps a 0-based index `idx` to its log2 histogram bucket `0..=30` where bucket `b` covers `[0..2^b)`.
#[inline]
const fn log2_bucket(idx: u32) -> usize {
    if idx == 0 {
        0
    } else {
        let b = (32 - idx.leading_zeros()) as usize;
        if b > 30 { 30 } else { b }
    }
}
