#![allow(unsafe_op_in_unsafe_fn)]

use elara::gc::{GcPhase, gc_collect, gc_step};
use elara::value::{ObjTable, ObjType, Value};
use elara::vm::ElaraState;

#[test]
fn incremental_gc_reclaims_unreachable_cyclic_tables_and_strings_to_zero_leak() {
    let mut state = ElaraState::new();
    let baseline = state.alloc.bytes_allocated;
    assert!(baseline > 0, "FIXED metamethod strings + intern pool");

    // Build an unreachable cycle t1 <-> t2 that also retains a long (non-interned) string.
    let long = b"x".repeat(80);
    let s = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, &long)
        .expect("long string");
    let s_val = Value::from_obj(s.cast(), ObjType::String);

    let mut t1 = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("t1");
    let mut t2 = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 2, 0, 4).expect("t2");
    unsafe {
        let a = t1.as_mut();
        let b = t2.as_mut();
        a.set(
            &mut state.alloc,
            Value::from_i48(0),
            Value::from_obj(t2.cast(), ObjType::Table),
        )
        .expect("t1[0]=t2");
        a.set(&mut state.alloc, s_val, Value::from_i48(1))
            .expect("t1[str]=1");
        b.set(
            &mut state.alloc,
            Value::from_i48(0),
            Value::from_obj(t1.cast(), ObjType::Table),
        )
        .expect("t2[0]=t1");
    }

    assert!(state.alloc.bytes_allocated > baseline);
    assert!(!state.all_objects.is_null());

    // No roots point at the cycle — incremental steps must reclaim everything.
    assert_eq!(state.gc.phase, GcPhase::Pause);
    let mut steps = 0u32;
    while steps < 10_000 {
        let done = unsafe { gc_step(&mut state, 8) };
        steps += 1;
        if done {
            break;
        }
        // After the first step we leave Pause; keep stepping until Pause returns.
        if state.gc.phase == GcPhase::Pause && steps > 1 {
            break;
        }
    }
    assert_eq!(state.gc.phase, GcPhase::Pause);
    assert!(
        steps > 1,
        "collection should take multiple incremental steps"
    );

    assert!(
        state.all_objects.is_null(),
        "all non-string heap objects must be swept"
    );
    assert_eq!(
        state.alloc.bytes_allocated, baseline,
        "bytes must return to FIXED roots + intern pool only"
    );

    // A second full collect is a no-op on an already-clean heap.
    unsafe { gc_collect(&mut state) };
    assert_eq!(state.alloc.bytes_allocated, baseline);

    // Drop remaining FIXED / pool memory the same way other tests tear down.
    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}
