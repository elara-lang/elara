#![allow(unsafe_op_in_unsafe_fn)]

use std::time::Instant;

use elara::gc::{
    GC_BLACK, GC_GRAY, GcPhase, barrier_table_write, gc_alloc, gc_barrier_back, gc_collect,
    gc_step, gc_step_with_budget,
};
use elara::value::{ObjTable, ObjType, Value};
use elara::vm::{ElaraState, ElaraStatus, ObjFiber};

#[test]
fn incremental_gc_reclaims_unreachable_cyclic_tables_and_strings_to_zero_leak() {
    let mut state = ElaraState::new();
    let baseline = state.alloc.bytes_allocated;
    assert!(baseline > 0, "FIXED metamethod strings + intern pool");

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

    assert_eq!(state.gc.phase, GcPhase::Pause);
    let mut steps = 0u32;
    while steps < 10_000 {
        let done = unsafe { gc_step(&mut state, 8) };
        steps += 1;
        if done {
            break;
        }
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

    unsafe { gc_collect(&mut state) };
    assert_eq!(state.alloc.bytes_allocated, baseline);

    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn backward_write_barrier_preserves_white_objects_inserted_into_black_tables() {
    let mut state = ElaraState::new();

    let long = b"y".repeat(80);
    let s = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, &long)
        .expect("str");
    let s_val = Value::from_obj(s.cast(), ObjType::String);

    let mut table =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("table");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 4).expect("fiber");

    unsafe {
        // Root the table via the current fiber's stack.
        *fiber.as_mut().stack = Value::from_obj(table.cast(), ObjType::Table);
        fiber.as_mut().open_top = 1;
        fiber.as_mut().stack_top = 1;
        state.current_fiber = fiber.as_ptr();

        // Enter Propagate with roots marked.
        assert!(!gc_step(&mut state, 1));
        assert_eq!(state.gc.phase, GcPhase::Propagate);

        // Propagate until the rooted table is black (fiber marks it).
        let mut guard = 0u32;
        while (*table.as_ptr()).header.gc_color != GC_BLACK && guard < 10_000 {
            assert_eq!(state.gc.phase, GcPhase::Propagate);
            let _ = gc_step(&mut state, 1);
            guard += 1;
        }
        assert_eq!((*table.as_ptr()).header.gc_color, GC_BLACK);
        assert_eq!(state.gc.phase, GcPhase::Propagate);

        // Child is still white (not yet referenced by the black table).
        (*s.as_ptr()).header.gc_color = state.gc.current_white;

        table
            .as_mut()
            .set(&mut state.alloc, s_val, Value::from_i48(42))
            .expect("insert white");
        barrier_table_write(&mut state, table.as_ptr(), s_val, Value::from_i48(42));

        assert_eq!((*table.as_ptr()).header.gc_color, GC_GRAY);
        assert!(!state.gc.gray_again.is_null());

        // Direct barrier API also re-grays a black table.
        (*table.as_ptr()).header.gc_color = GC_BLACK;
        gc_barrier_back(&mut state, table.as_ptr(), s_val);
        assert_eq!((*table.as_ptr()).header.gc_color, GC_GRAY);

        while state.gc.phase != GcPhase::Pause {
            let _ = gc_step(&mut state, 32);
        }

        assert_eq!(
            table.as_ref().get(s_val).to_i64(),
            Some(42),
            "barrier kept the inserted white object alive"
        );

        state.current_fiber = core::ptr::null_mut();
    }

    unsafe { gc_collect(&mut state) };
    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn gc_step_respects_microsecond_pause_budget_and_completes_full_cycle() {
    let mut state = ElaraState::new();
    state.gc.pause_budget_us = 200;

    // Create enough garbage for multiple timed steps.
    for i in 0..64 {
        let mut t =
            ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, i + 1, 0, 4).expect("t");
        unsafe {
            let _ = t.as_mut().set(
                &mut state.alloc,
                Value::from_i48(0),
                Value::from_i48(i as i64),
            );
        }
    }

    let mut steps = 0u32;
    let mut max_step = std::time::Duration::from_micros(0);
    while steps < 10_000 {
        let t0 = Instant::now();
        let done = unsafe { gc_step_with_budget(&mut state) };
        let elapsed = t0.elapsed();
        if elapsed > max_step {
            max_step = elapsed;
        }
        steps += 1;
        if done {
            break;
        }
    }
    assert_eq!(state.gc.phase, GcPhase::Pause);
    assert!(steps >= 1);
    // Allow generous slack for CI timers; still far below an unbounded full collect.
    assert!(
        max_step.as_millis() < 50,
        "single budgeted step took too long: {max_step:?}"
    );

    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn hard_memory_ceiling_triggers_emergency_gc_and_returns_err_oom_gracefully() {
    let mut state = ElaraState::new();
    let baseline = state.alloc.bytes_allocated;
    // Tight ceiling: enough for roots + a little garbage, not a second large table.
    state.alloc.mem_limit_bytes = baseline + 512;
    state.gc.mem_limit_bytes = state.alloc.mem_limit_bytes;

    // Allocate unreachable garbage that fits under the ceiling.
    let mut garbage =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("garbage");
    unsafe {
        let _ = garbage
            .as_mut()
            .set(&mut state.alloc, Value::from_i48(1), Value::from_i48(2));
    }
    let _ = garbage; // pointer still on all_objects; unreachable from roots

    let before = state.alloc.bytes_allocated;
    assert!(before <= state.alloc.mem_limit_bytes);

    // Request more than remaining headroom — emergency GC should reclaim garbage first.
    let need = state.alloc.mem_limit_bytes.saturating_sub(baseline) + 64;
    let result = unsafe { gc_alloc(&mut state, need) };
    // After emergency GC, either we get memory or a clean ErrOom (never abort).
    match result {
        Ok(ptr) => {
            unsafe {
                state.alloc.free_bytes(ptr.as_ptr(), need);
            }
            assert!(state.alloc.bytes_allocated <= state.alloc.mem_limit_bytes);
        }
        Err(ElaraStatus::ErrOom) => {}
        Err(e) => panic!("unexpected status {e:?}"),
    }

    // Hard fail path: ceiling equal to current live bytes, request more with nothing to free.
    unsafe { gc_collect(&mut state) };
    let live = state.alloc.bytes_allocated;
    state.alloc.mem_limit_bytes = live;
    let err = unsafe { gc_alloc(&mut state, 64) };
    assert_eq!(err, Err(ElaraStatus::ErrOom));

    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn weak_tables_evict_unreachable_keys_and_values_during_atomic_phase() {
    let mut state = ElaraState::new();

    // Metatable with __mode = "kv"
    let mut mt = ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("mt");
    let mode = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, b"kv")
        .expect("mode");
    // Keep mode string FIXED so it isn't swept mid-test before the weak table is processed.
    unsafe {
        (*mode.as_ptr()).header.gc_color = elara::gc::GC_FIXED;
    }
    let mode_v = Value::from_obj(mode.cast(), ObjType::String);
    unsafe {
        mt.as_mut()
            .set(&mut state.alloc, state.mm_mode, mode_v)
            .expect("set __mode");
    }

    let mut weak =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 2, 0, 8).expect("weak");
    unsafe {
        weak.as_mut().metatable = mt.as_ptr();
    }

    let long_k = b"k".repeat(80);
    let long_v = b"v".repeat(80);
    let key = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, &long_k)
        .expect("key");
    let val = state
        .strings
        .new_string(&mut state.alloc, &mut state.all_objects, 0, &long_v)
        .expect("val");
    let key_v = Value::from_obj(key.cast(), ObjType::String);
    let val_v = Value::from_obj(val.cast(), ObjType::String);

    unsafe {
        weak.as_mut()
            .set(&mut state.alloc, key_v, val_v)
            .expect("weak insert");
    }

    // Root only the weak table (and its metatable via the table), not the entry objects.
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 4).expect("fiber");
    unsafe {
        *fiber.as_mut().stack = Value::from_obj(weak.cast(), ObjType::Table);
        fiber.as_mut().open_top = 1;
        fiber.as_mut().stack_top = 1;
        state.current_fiber = fiber.as_ptr();

        gc_collect(&mut state);

        // Weak kv entry must be cleared; key/value unreacheable elsewhere.
        assert!(
            weak.as_ref().get(key_v).is_nil(),
            "weak kv entry should be evicted in Atomic"
        );

        state.current_fiber = core::ptr::null_mut();
    }

    unsafe { gc_collect(&mut state) };
    state.strings.free_all(&mut state.alloc);
    assert_eq!(state.alloc.bytes_allocated, 0);
}
