use core::ffi::c_void;
use core::mem::size_of;
use core::ptr::NonNull;

use elara::gc::GcAllocator;
use elara::value::{
    ELARA_INT48_MAX, ELARA_INT48_MIN, ELARA_SHORT_STR_MAX, GcHeader, InternPool, MapEntry,
    ObjBoxedI64, ObjString, ObjTable, ObjType, Value, hash_u64,
};

#[test]
fn nan_tagging_encodes_and_decodes_f64_i48_boxed_i64_bool_nil_and_aligned_objects() {
    #[cfg(target_pointer_width = "64")]
    assert_eq!(size_of::<Value>(), 8);
    assert_eq!(size_of::<GcHeader>(), 16);
    assert_eq!(size_of::<ObjBoxedI64>(), 24);

    // Nil and Booleans (truthiness invariant: only nil and false are falsy)
    assert!(Value::NIL.is_nil());
    assert!(Value::NIL.is_falsy());
    assert!(!Value::NIL.is_truthy());

    assert!(Value::FALSE.is_bool());
    assert!(!Value::FALSE.as_bool());
    assert!(Value::FALSE.is_falsy());

    assert!(Value::TRUE.is_bool());
    assert!(Value::TRUE.as_bool());
    assert!(Value::TRUE.is_truthy());

    // 0 and 0.0 MUST be truthy
    let zero_i48 = Value::from_i48(0);
    let zero_f64 = Value::from_f64(0.0);
    assert!(zero_i48.is_truthy());
    assert!(zero_f64.is_truthy());

    // 48-bit signed integers (two's complement sign extension at boundaries)
    for &i in &[0_i64, 1, -1, 42, -42, ELARA_INT48_MIN, ELARA_INT48_MAX] {
        assert!(Value::fits_i48(i));
        let v = Value::from_i48(i);
        assert!(v.is_i48());
        assert!(v.is_int());
        assert!(v.is_number());
        assert!(!v.is_float());
        assert_eq!(v.as_i48(), i);
        assert_eq!(v.to_i64(), Some(i));
    }

    assert!(!Value::fits_i48(ELARA_INT48_MAX + 1));
    assert!(!Value::fits_i48(ELARA_INT48_MIN - 1));

    // Boxed 64-bit signed integers (`ObjBoxedI64`)
    let mut boxed_max = ObjBoxedI64::new(i64::MAX, 0);
    let mut boxed_min = ObjBoxedI64::new(i64::MIN, 0);
    let v_max = Value::from_obj(
        NonNull::new(&raw mut boxed_max.header).expect("non-null"),
        ObjType::BoxedI64,
    );
    let v_min = Value::from_obj(
        NonNull::new(&raw mut boxed_min.header).expect("non-null"),
        ObjType::BoxedI64,
    );

    assert!(!v_max.is_i48());
    assert!(v_max.is_int());
    assert!(v_max.is_number());
    assert!(v_max.is_obj_type(ObjType::BoxedI64));
    assert_eq!(v_max.to_i64(), Some(i64::MAX));
    assert_eq!(v_min.to_i64(), Some(i64::MIN));

    // IEEE-754 floats (including -0.0, infinities, and canonicalized NaN)
    for &d in &[
        0.0_f64,
        -0.0,
        core::f64::consts::PI,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        let v = Value::from_f64(d);
        assert!(v.is_float());
        assert!(v.is_number());
        assert_eq!(v.as_f64().to_bits(), d.to_bits());
    }

    let nan_v = Value::from_f64(f64::NAN);
    assert!(nan_v.is_float());
    assert!(nan_v.as_f64().is_nan());

    // Light userdata
    let raw_ptr = 0x1234_5678_9ABC_usize as *mut c_void;
    let lud = Value::from_lightud(raw_ptr);
    assert!(lud.is_lightud());
    assert_eq!(lud.as_lightud(), raw_ptr);

    // Embedded 5-bit ObjType tag round-trip across all 9 ObjType variants
    let all_types = [
        ObjType::String,
        ObjType::Table,
        ObjType::Closure,
        ObjType::Proto,
        ObjType::Upvalue,
        ObjType::Fiber,
        ObjType::Userdata,
        ObjType::NativeFn,
        ObjType::BoxedI64,
    ];
    for t in all_types {
        let mut hdr = GcHeader::new(t, 0);
        let ptr = NonNull::new(&raw mut hdr).expect("non-null");
        let val = Value::from_obj(ptr, t);
        assert!(val.is_obj());
        assert_eq!(val.obj_type(), Some(t));
        assert!(val.is_obj_type(t));
        assert_eq!(val.as_obj() as *const GcHeader, &raw const hdr);
    }
}

#[test]
fn numeric_equality_and_splitmix64_hashing_unify_i48_boxed_i64_and_f64() {
    // 1. i48 == f64 and hash equality
    let i_42 = Value::from_i48(42);
    let f_42 = Value::from_f64(42.0);
    let f_42_5 = Value::from_f64(42.5);
    assert!(i_42.val_equal(f_42));
    assert!(f_42.val_equal(i_42));
    assert!(!i_42.val_equal(f_42_5));
    assert_eq!(i_42.val_hash(), f_42.val_hash());

    // 2. -0.0 == +0.0 == 0
    let pos_zero = Value::from_f64(0.0);
    let neg_zero = Value::from_f64(-0.0);
    let int_zero = Value::from_i48(0);
    assert!(pos_zero.val_equal(neg_zero));
    assert!(neg_zero.val_equal(int_zero));
    assert_eq!(pos_zero.val_hash(), neg_zero.val_hash());
    assert_eq!(pos_zero.val_hash(), int_zero.val_hash());

    // 3. NaN != NaN
    let nan = Value::from_f64(f64::NAN);
    assert!(!nan.val_equal(nan));

    // 4. Two distinct ObjBoxedI64 heap allocations with the same i64 value MUST be val_equal and hash-equal!
    let big_val = ELARA_INT48_MAX + 999_999;
    let mut b1 = ObjBoxedI64::new(big_val, 0);
    let mut b2 = ObjBoxedI64::new(big_val, 0);
    let mut b3 = ObjBoxedI64::new(big_val + 1, 0);

    let v1 = Value::from_obj(
        NonNull::new(&raw mut b1.header).expect("non-null"),
        ObjType::BoxedI64,
    );
    let v2 = Value::from_obj(
        NonNull::new(&raw mut b2.header).expect("non-null"),
        ObjType::BoxedI64,
    );
    let v3 = Value::from_obj(
        NonNull::new(&raw mut b3.header).expect("non-null"),
        ObjType::BoxedI64,
    );

    assert_ne!(v1.as_obj(), v2.as_obj());
    assert!(v1.val_equal(v2));
    assert!(!v1.val_equal(v3));
    assert_eq!(v1.val_hash(), v2.val_hash());
    assert_eq!(v1.val_hash(), hash_u64(big_val as u64));

    // 5. Boundary precision check: i64::MAX is 2^63 - 1, while (i64::MAX as f64) rounds to 2^63.
    // They MUST NOT compare equal.
    let mut b_max = ObjBoxedI64::new(i64::MAX, 0);
    let v_max = Value::from_obj(
        NonNull::new(&raw mut b_max.header).expect("non-null"),
        ObjType::BoxedI64,
    );
    let f_rounded = Value::from_f64(i64::MAX as f64);
    assert!(!v_max.val_equal(f_rounded));
}

#[test]
fn dual_tier_strings_intern_short_strings_and_lazily_hash_long_strings() {
    assert_eq!(size_of::<ObjString>(), 24);

    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();
    let mut pool = InternPool::new(0x1234_5678);

    // 1. Short strings (`<= 64B`) are eagerly hashed and globally deduplicated in `InternPool`
    let s1 = pool
        .new_string(&mut alloc, &mut all_objects, 0, b"hello elara")
        .expect("short string 1");
    let s2 = pool
        .new_string(&mut alloc, &mut all_objects, 0, b"hello elara")
        .expect("short string 2");
    assert_eq!(s1.as_ptr(), s2.as_ptr());
    assert_eq!(pool.count(), 1);
    assert!(all_objects.is_null());

    unsafe {
        assert!((*s1.as_ptr()).is_interned());
        assert!((*s1.as_ptr()).has_hash());
        assert_ne!((*s1.as_ptr()).header.aux_hash, 0);
        assert_eq!((*s1.as_ptr()).as_str(), "hello elara");
    }

    // 2. 0-based string indexing (including negative index normalization)
    let ch_0 = pool
        .index_byte(&mut alloc, 0, unsafe { s1.as_ref() }, 0)
        .expect("index 0");
    let ch_last = pool
        .index_byte(&mut alloc, 0, unsafe { s1.as_ref() }, -1)
        .expect("index -1");
    let ch_oob = pool
        .index_byte(&mut alloc, 0, unsafe { s1.as_ref() }, 99)
        .expect("index oob");
    assert!(ch_oob.is_nil());
    unsafe {
        assert_eq!((*ch_0.as_obj().cast::<ObjString>()).as_str(), "h");
        assert_eq!((*ch_last.as_obj().cast::<ObjString>()).as_str(), "a");
    }

    // 3. Long strings (`> 64B`) are NOT interned and defer FNV-1a hashing until needed
    let long_bytes = vec![b'x'; ELARA_SHORT_STR_MAX + 32];
    let l1 = pool
        .new_string(&mut alloc, &mut all_objects, 0, &long_bytes)
        .expect("long string 1");
    let l2 = pool
        .new_string(&mut alloc, &mut all_objects, 0, &long_bytes)
        .expect("long string 2");

    // Distinct pointers on `all_objects`, initially unhashed (`has_hash() == false`, `aux_hash == 0`)
    assert_ne!(l1.as_ptr(), l2.as_ptr());
    unsafe {
        assert!(!(*l1.as_ptr()).is_interned());
        assert!(!(*l1.as_ptr()).has_hash());
        assert_eq!((*l1.as_ptr()).header.aux_hash, 0);
    }

    // `val_equal` and `val_hash` lazily compute and cache `aux_hash` and compare equal by content!
    let v_l1 = Value::from_obj(l1.cast::<GcHeader>(), ObjType::String);
    let v_l2 = Value::from_obj(l2.cast::<GcHeader>(), ObjType::String);
    assert!(v_l1.val_equal(v_l2));
    assert_eq!(v_l1.val_hash(), v_l2.val_hash());

    unsafe {
        assert!((*l1.as_ptr()).has_hash());
        assert!((*l2.as_ptr()).has_hash());
        assert_ne!((*l1.as_ptr()).header.aux_hash, 0);
    }

    // Clean up long strings on `all_objects` and interned pool; verify zero leaked bytes!
    unsafe {
        let mut cur = all_objects;
        while !cur.is_null() {
            let next = (*cur).next;
            let s_ptr = cur.cast::<ObjString>();
            let sz = ObjString::alloc_size((*s_ptr).byte_len as usize);
            alloc.free_bytes(s_ptr.cast::<u8>(), sz);
            cur = next;
        }
    }
    pool.free_all(&mut alloc);
    assert_eq!(alloc.bytes_allocated, 0);

    // 4. Verify `mem_limit_bytes` enforcement in `GcAllocator`
    let mut limited = GcAllocator::new();
    limited.mem_limit_bytes = 32;
    assert!(limited.alloc_bytes(64).is_none());
}

#[test]
fn hybrid_table_sizeof_is_64_bytes_and_maintains_array_len_on_push_and_tail_delete() {
    #[cfg(target_pointer_width = "64")]
    assert_eq!(size_of::<ObjTable>(), 64);
    assert_eq!(size_of::<MapEntry>(), 24);

    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();

    let mut tbl_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 0, 0).expect("table");
    let tbl = unsafe { tbl_ptr.as_mut() };

    // 1. Reject `nil` and `NaN` keys with exact message
    assert_eq!(
        tbl.set(&mut alloc, Value::NIL, Value::from_i48(1)),
        Err("Invalid table key: nil or NaN")
    );
    assert_eq!(
        tbl.set(&mut alloc, Value::from_f64(f64::NAN), Value::from_i48(1)),
        Err("Invalid table key: nil or NaN")
    );

    // 2. 0-based `push` and contiguous `array_len` maintenance
    for i in 0..8_i64 {
        let new_len = tbl
            .push(&mut alloc, Value::from_i48((i + 1) * 10))
            .expect("push");
        assert_eq!(new_len, (i + 1) as u32);
    }
    assert_eq!(tbl.array_len, 8);
    assert_eq!(tbl.get_int(0).to_i64(), Some(10));
    assert_eq!(tbl.get_int(7).to_i64(), Some(80));
    assert_eq!(tbl.get_int(-1).to_i64(), Some(80)); // Negative index normalization!

    // 3. Bridging a gap advances `array_len` across pre-populated slots:
    // Delete index 5 (inside `[0..7]`); `array_len` stays 8 until tail is deleted,
    // or if we delete 7, 6, 5 then `array_len` retreats to 5!
    tbl.set(&mut alloc, Value::from_i48(5), Value::NIL)
        .expect("clear 5");
    assert_eq!(tbl.array_len, 8);

    // Deleting tail index 7 and 6 retreats `array_len` across the hole at 5 all the way to 5!
    assert_eq!(tbl.pop().to_i64(), Some(80));
    assert_eq!(tbl.array_len, 7);
    assert_eq!(tbl.pop().to_i64(), Some(70));
    assert_eq!(tbl.array_len, 5);

    // Now write index 6 (beyond `array_len == 5`), then bridge index 5 -> `array_len` jumps from 5 to 7!
    tbl.set(&mut alloc, Value::from_i48(6), Value::from_i48(700))
        .expect("set 6");
    assert_eq!(tbl.array_len, 5);
    tbl.set(&mut alloc, Value::from_i48(5), Value::from_i48(600))
        .expect("set 5");
    assert_eq!(tbl.array_len, 7);

    tbl.free_table(&mut alloc);
    unsafe { alloc.free_bytes(tbl_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>()) };
    assert_eq!(alloc.bytes_allocated, 0);
}

#[test]
fn robin_hood_hashing_maintains_psl_invariant_backward_shift_deletes_and_iterates_zero_alloc() {
    let mut alloc = GcAllocator::new();
    let mut all_objects: *mut GcHeader = core::ptr::null_mut();
    let mut pool = InternPool::new(0xCAFE_BABE);

    let mut tbl_ptr = ObjTable::new(&mut alloc, &mut all_objects, 0, 1, 2, 4).expect("table");
    let tbl = unsafe { tbl_ptr.as_mut() };

    let initial_shape = tbl.header.aux_hash;

    // 1. Insert 20 string keys and 4 dense array elements
    for i in 0..4_i64 {
        tbl.set(&mut alloc, Value::from_i48(i), Value::from_i48(i * 100))
            .expect("array set");
    }

    let mut keys = Vec::new();
    for i in 0..20_i64 {
        let name = format!("field_{i}");
        let s = pool
            .intern_always(&mut alloc, 0, name.as_bytes())
            .expect("intern");
        let k_val = Value::from_obj(s.cast::<GcHeader>(), ObjType::String);
        keys.push(k_val);
        tbl.set(&mut alloc, k_val, Value::from_i48(i + 1000))
            .expect("hash set");
    }

    assert_eq!(tbl.array_len, 4);
    assert_eq!(tbl.hash_count, 20);
    assert_ne!(tbl.header.aux_hash, initial_shape); // Shape version bumped on rehash!

    // 2. Verify every Robin Hood bucket's PSL matches its exact distance from `(hash & hash_mask)`
    let mask = tbl.hash_mask;
    for slot_idx in 0..=mask {
        let e = unsafe { *tbl.entries.add(slot_idx as usize) };
        if !e.key.is_nil() {
            let home = e.hash & mask;
            let expected_psl = slot_idx.wrapping_sub(home) & mask;
            assert_eq!(u32::from(e.psl), expected_psl);
        }
    }

    // 3. Delete half the hash keys via backward-shift deletion (assigning `Value::NIL`)
    let shape_before_del = tbl.header.aux_hash;
    for &k in keys.iter().step_by(2) {
        tbl.set(&mut alloc, k, Value::NIL).expect("delete key");
    }
    assert_eq!(tbl.hash_count, 10);
    assert_ne!(tbl.header.aux_hash, shape_before_del);

    // Verify deleted keys return `Value::NIL`, remaining keys are still found, and PSL invariant holds!
    for (i, &k) in keys.iter().enumerate() {
        let got = tbl.get(k);
        if i % 2 == 0 {
            assert!(got.is_nil());
        } else {
            assert_eq!(got.to_i64(), Some(i as i64 + 1000));
        }
    }

    for slot_idx in 0..=tbl.hash_mask {
        let e = unsafe { *tbl.entries.add(slot_idx as usize) };
        if !e.key.is_nil() {
            let home = e.hash & tbl.hash_mask;
            let expected_psl = slot_idx.wrapping_sub(home) & tbl.hash_mask;
            assert_eq!(u32::from(e.psl), expected_psl);
        }
    }

    // 4. Verify zero-allocation cursor iteration (`next_entry`) visits all 4 array + 10 hash entries
    let mut cursor = 0u32;
    let mut visited_count = 0usize;
    let bytes_before_iter = alloc.bytes_allocated;
    while let Some((next_c, _k, _v)) = unsafe { tbl.next_entry(cursor) } {
        visited_count += 1;
        cursor = next_c;
    }
    assert_eq!(visited_count, 14);
    assert_eq!(alloc.bytes_allocated, bytes_before_iter); // Zero heap allocations during iteration!

    tbl.free_table(&mut alloc);
    unsafe { alloc.free_bytes(tbl_ptr.as_ptr().cast::<u8>(), size_of::<ObjTable>()) };
    pool.free_all(&mut alloc);
    assert_eq!(alloc.bytes_allocated, 0);
}
