use core::mem::size_of;

use elara::compiler::ObjProto;
use elara::value::{ObjType, Value};
use elara::vm::{
    CallFrame, ELARA_MAX_CALL_DEPTH, ELARA_MAX_STACK_SLOTS, ELARA_MIN_STACK_SLOTS, ElaraState,
    ElaraStatus, FiberStatus, ObjClosure, ObjFiber, capture_upvalue, close_upvalues_from,
};

#[test]
fn fiber_create_allocates_suspended_fiber_with_min_stack_and_entry_closure() {
    let mut state = ElaraState::new();
    let mut proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    unsafe {
        proto.as_mut().max_stack_size = 2;
    }
    let mut closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr()).expect("c");

    let mut fiber = ObjFiber::create(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        closure.as_ptr(),
    )
    .expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        assert_eq!(f.status, FiberStatus::Suspended);
        assert_eq!(f.stack_capacity, ELARA_MIN_STACK_SLOTS);
        assert_eq!(f.stack_top, 1);
        assert_eq!(f.open_top, 1);
        assert!(f.caller.is_null());
        assert!((*f.stack).is_obj_type(ObjType::Closure));
        assert_eq!((*f.stack).as_obj().cast::<ObjClosure>(), closure.as_ptr());

        closure.as_mut().free_closure(&mut state.alloc);
        proto.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn fiber_stack_grows_dynamically_and_rejects_overflow_past_max_slots() {
    let mut state = ElaraState::new();
    let mut fiber = ObjFiber::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        ELARA_MIN_STACK_SLOTS,
        4,
    )
    .expect("fiber");
    unsafe {
        let f = fiber.as_mut();
        f.open_top = f.stack_capacity;
        let mid = f.stack_capacity + 128;
        f.ensure_stack(&mut state.alloc, mid).expect("grow");
        assert!(f.stack_capacity >= mid);
        assert!(f.stack_capacity <= ELARA_MAX_STACK_SLOTS);

        f.open_top = ELARA_MAX_STACK_SLOTS;
        let err = f
            .ensure_stack(&mut state.alloc, ELARA_MAX_STACK_SLOTS + 1)
            .expect_err("overflow");
        assert_eq!(err, ElaraStatus::ErrRuntime);

        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn fiber_frame_push_rejects_past_max_call_depth() {
    let mut state = ElaraState::new();
    let mut proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    let mut closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr()).expect("c");
    let mut fiber = ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("f");

    unsafe {
        let f = fiber.as_mut();
        for _ in 0..ELARA_MAX_CALL_DEPTH {
            f.push_frame(&mut state.alloc, CallFrame::new(closure.as_ptr(), 1, 0, 1))
                .expect("push");
        }
        assert_eq!(f.frame_count, ELARA_MAX_CALL_DEPTH);
        let err = f
            .push_frame(&mut state.alloc, CallFrame::new(closure.as_ptr(), 1, 0, 1))
            .expect_err("depth");
        assert_eq!(err, ElaraStatus::ErrRuntime);

        closure.as_mut().free_closure(&mut state.alloc);
        proto.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn fiber_termination_closes_open_upvalues_and_marks_dead() {
    let mut state = ElaraState::new();
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 32, 4).expect("fiber");

    unsafe {
        let f = fiber.as_mut();
        *f.stack.add(2) = Value::from_i48(11);
        f.open_top = 8;
        let uv = capture_upvalue(&mut state.alloc, &mut state.all_objects, 0, f, 2).expect("uv");
        assert!(!f.open_upvalues.is_null());
        assert_eq!((*uv).stack_idx, 2);

        f.terminate(&mut state).expect("terminate");
        assert_eq!(f.status, FiberStatus::Dead);
        assert_eq!(f.frame_count, 0);
        assert!(f.open_upvalues.is_null());
        assert_eq!((*uv).stack_idx, u32::MAX);
        assert_eq!((*(*uv).location).to_i64(), Some(11));

        // Idempotent second close path for already-closed upvalues list.
        close_upvalues_from(&mut state, f, 0).expect("noop");

        (*uv).free_upvalue(&mut state.alloc);
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}
