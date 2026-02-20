use core::ffi::c_void;
use core::mem::size_of;
use core::ptr::NonNull;

use elara::value::{
    ELARA_INT48_MAX, ELARA_INT48_MIN, GcHeader, ObjBoxedI64, ObjType, Value, hash_u64,
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
