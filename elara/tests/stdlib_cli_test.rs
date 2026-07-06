#![allow(unsafe_op_in_unsafe_fn)]

use elara::ffi::c_api::{elara_call, elara_get_global};
use elara::value::{ObjType, Value};
use elara::vm::{ElaraState, ElaraStatus};

unsafe fn call1(state: &mut ElaraState, fn_val: u64, arg0: Value) -> (ElaraStatus, Value) {
    let args = [arg0.0];
    let mut results = [0u64; 4];
    let mut n = 0u16;
    let st = elara_call(
        state,
        fn_val,
        args.as_ptr(),
        1,
        results.as_mut_ptr(),
        4,
        &mut n,
    );
    (st, Value(results[0]))
}

unsafe fn call2(state: &mut ElaraState, fn_val: u64, a: Value, b: Value) -> (ElaraStatus, Value) {
    let args = [a.0, b.0];
    let mut results = [0u64; 4];
    let mut n = 0u16;
    let st = elara_call(
        state,
        fn_val,
        args.as_ptr(),
        2,
        results.as_mut_ptr(),
        4,
        &mut n,
    );
    (st, Value(results[0]))
}

unsafe fn global_fn(state: &mut ElaraState, name: &str) -> u64 {
    let c = std::ffi::CString::new(name).unwrap();
    elara_get_global(state, c.as_ptr())
}

unsafe fn module_field(state: &mut ElaraState, module: &str, field: &str) -> Value {
    // Build via stdlib intrinsic_module directly (same as OP_IMPORT path).
    let table = elara::stdlib::intrinsic_module(state, module)
        .expect("mod")
        .expect("known");
    assert!(table.is_obj_type(ObjType::Table));
    let key = state
        .strings
        .new_string(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            field.as_bytes(),
        )
        .unwrap();
    (*table.as_obj().cast::<elara::value::ObjTable>())
        .get(Value::from_obj(key.cast(), ObjType::String))
}

#[test]
fn stdlib_math_string_table_and_bit_modules_obey_0_based_indexing_and_edge_cases() {
    // 014.1 covers math / bit / sys / core globals. string & table assertions land with 014.2.
    let mut state = ElaraState::new();
    unsafe {
        elara::stdlib::open_stdlib(&mut state).expect("stdlib");
        // --- type / tostring / tonumber ---
        let type_fn = global_fn(&mut state, "type");
        let (st, v) = call1(&mut state, type_fn, Value::from_i48(1));
        assert_eq!(st, ElaraStatus::Ok);
        assert!(v.is_obj_type(ObjType::String));
        assert_eq!(
            (*v.as_obj().cast::<elara::value::ObjString>()).as_bytes(),
            b"int"
        );

        let tostring = global_fn(&mut state, "tostring");
        let (st, v) = call1(&mut state, tostring, Value::from_bool(true));
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(
            (*v.as_obj().cast::<elara::value::ObjString>()).as_bytes(),
            b"true"
        );

        let tonumber = global_fn(&mut state, "tonumber");
        let s42 = state
            .strings
            .new_string(&mut state.alloc, &mut state.all_objects, 0, b"2A")
            .unwrap();
        let (st, v) = call2(
            &mut state,
            tonumber,
            Value::from_obj(s42.cast(), ObjType::String),
            Value::from_i48(16),
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(42));

        // --- math ---
        let abs = module_field(&mut state, "math", "abs");
        let (st, v) = call1(&mut state, abs.0, Value::from_i48(-7));
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(7));

        let floor = module_field(&mut state, "math", "floor");
        let (st, v) = call1(&mut state, floor.0, Value::from_f64(3.9));
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(3));

        let idiv = module_field(&mut state, "math", "idiv");
        let (st, v) = call2(&mut state, idiv.0, Value::from_i48(-7), Value::from_i48(3));
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(-3)); // floor div

        let (st, _) = call2(&mut state, idiv.0, Value::from_i48(1), Value::from_i48(0));
        assert_eq!(st, ElaraStatus::ErrRuntime);
        assert_eq!(state.last_error, Some("Division by zero"));

        let pi = module_field(&mut state, "math", "pi");
        assert!(pi.is_float());
        assert!((pi.as_f64() - core::f64::consts::PI).abs() < 1e-12);

        let clamp = module_field(&mut state, "math", "clamp");
        let args = [
            Value::from_f64(5.0).0,
            Value::from_f64(0.0).0,
            Value::from_f64(1.0).0,
        ];
        let mut results = [0u64; 2];
        let mut n = 0u16;
        let st = elara_call(
            &mut state,
            clamp.0,
            args.as_ptr(),
            3,
            results.as_mut_ptr(),
            2,
            &mut n,
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(Value(results[0]).as_f64(), 1.0);

        // --- bit ---
        let band = module_field(&mut state, "bit", "band");
        let (st, v) = call2(
            &mut state,
            band.0,
            Value::from_i48(0b1100),
            Value::from_i48(0b1010),
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(0b1000));

        let ushr = module_field(&mut state, "bit", "ushr");
        let (st, v) = call2(&mut state, ushr.0, Value::from_i48(-8), Value::from_i48(1));
        assert_eq!(st, ElaraStatus::Ok);
        // logical shift of -8 as u64 >> 1
        assert_eq!(v.to_i64(), Some(((-8i64) as u64 >> 1) as i64));

        let count = module_field(&mut state, "bit", "count_ones");
        let (st, v) = call1(&mut state, count.0, Value::from_i48(0b1011));
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(v.to_i64(), Some(3));

        // --- sys ---
        let clock = module_field(&mut state, "sys", "clock_ns");
        let mut results = [0u64; 1];
        let mut n = 0u16;
        let st = elara_call(
            &mut state,
            clock.0,
            core::ptr::null(),
            0,
            results.as_mut_ptr(),
            1,
            &mut n,
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert!(Value(results[0]).to_i64().unwrap_or(-1) >= 0);

        let ver = module_field(&mut state, "sys", "version");
        assert!(ver.is_obj_type(ObjType::String));
        assert_eq!(
            (*ver.as_obj().cast::<elara::value::ObjString>()).as_bytes(),
            b"Elara 1.0.0"
        );

        // --- assert / pcall ---
        let assert_fn = global_fn(&mut state, "assert");
        let (st, _) = call1(&mut state, assert_fn, Value::FALSE);
        assert_eq!(st, ElaraStatus::ErrRuntime);

        let pcall = global_fn(&mut state, "pcall");
        let mut results = [0u64; 4];
        let mut n = 0u16;
        let args = [assert_fn, Value::FALSE.0];
        let st = elara_call(
            &mut state,
            pcall,
            args.as_ptr(),
            2,
            results.as_mut_ptr(),
            4,
            &mut n,
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert!(!Value(results[0]).is_truthy());
        assert!(n >= 2);
    }
}
