#![allow(unsafe_op_in_unsafe_fn)]

use core::mem::size_of;
use core::ptr::NonNull;

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::gc::GcAllocator;
use elara::value::{ObjString, ObjType, Value};
use elara::vm::{
    CallFrame, ELARA_MAX_CALL_DEPTH, ELARA_MAX_STACK_SLOTS, ELARA_MIN_STACK_SLOTS, ElaraState,
    ElaraStatus, FiberStatus, ObjClosure, ObjFiber, capture_upvalue, close_upvalues_from,
    fiber_create, fiber_resume, fiber_status, fiber_status_name, fiber_try_resume,
};

fn emit(p: &mut ObjProto, alloc: &mut GcAllocator, inst: Instruction, line: u32) {
    p.emit_instruction(alloc, inst, line).expect("emit");
}

fn closure_val(c: *mut ObjClosure) -> Value {
    Value::from_obj(NonNull::new(c.cast()).expect("non-null"), ObjType::Closure)
}

fn str_eq(v: Value, expected: &str) -> bool {
    if !v.is_obj_type(ObjType::String) {
        return false;
    }
    unsafe { (*v.as_obj().cast::<ObjString>()).as_str() == expected }
}

unsafe fn free_proto_closure(
    state: &mut ElaraState,
    proto: NonNull<ObjProto>,
    c: NonNull<ObjClosure>,
) {
    (*c.as_ptr()).free_closure(&mut state.alloc);
    (*proto.as_ptr()).free_proto(&mut state.alloc);
    state
        .alloc
        .free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
}

unsafe fn free_fiber_obj(state: &mut ElaraState, fiber: *mut ObjFiber) {
    (*fiber).free_fiber_buffers(&mut state.alloc);
    state
        .alloc
        .free_bytes(fiber.cast::<u8>(), size_of::<ObjFiber>());
}

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

#[test]
fn fiber_create_resume_and_yield_pass_multiple_values_bidirectionally() {
    // Body: YIELD R[0..1] (echo first-resume args); RETURN R[0..1] (echo second-resume args).
    let mut state = ElaraState::new();
    let mut proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    unsafe {
        let p = proto.as_mut();
        p.max_stack_size = 4;
        p.num_params = 2;
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Yield, 0, 3, 0),
            1,
        );
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 3, 0),
            1,
        );
    }
    let closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr()).expect("c");
    let fiber_v = fiber_create(&mut state, closure_val(closure.as_ptr())).expect("create");
    assert!(str_eq(
        fiber_status(&mut state, fiber_v).expect("status"),
        "suspended"
    ));

    unsafe {
        let fiber = fiber_v.as_obj().cast::<ObjFiber>();
        let st = fiber_resume(
            &mut state,
            core::ptr::null_mut(),
            fiber_v,
            &[Value::from_i48(1), Value::from_i48(2)],
            0,
            2,
        );
        assert_eq!(st, ElaraStatus::Yield);
        assert_eq!(fiber_status_name(&*fiber), "suspended");
        assert_eq!((*(*fiber).stack.add(1)).to_i64(), Some(1));
        assert_eq!((*(*fiber).stack.add(2)).to_i64(), Some(2));

        let st = fiber_resume(
            &mut state,
            core::ptr::null_mut(),
            fiber_v,
            &[Value::from_i48(10), Value::from_i48(20)],
            0,
            2,
        );
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(fiber_status_name(&*fiber), "dead");
        assert_eq!((*(*fiber).stack.add(0)).to_i64(), Some(10));
        assert_eq!((*(*fiber).stack.add(1)).to_i64(), Some(20));

        free_fiber_obj(&mut state, fiber);
        free_proto_closure(&mut state, proto, closure);
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn nested_symmetric_chain_of_three_fibers_switches_states_accurately() {
    // Each fiber: LoadI R[0], N; YIELD R[0]; RETURN R[0].
    // Nested manually: A(Normal)←B(Normal)←C(Running), then execute C.
    let mut state = ElaraState::new();

    let make = |state: &mut ElaraState, n: i32| {
        let mut proto = ObjProto::new(
            &mut state.alloc,
            &mut state.all_objects,
            0,
            core::ptr::null_mut(),
        )
        .expect("proto");
        unsafe {
            let p = proto.as_mut();
            p.max_stack_size = 2;
            emit(
                p,
                &mut state.alloc,
                Instruction::make_asbx(OpCode::LoadI, 0, n),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Yield, 0, 2, 0),
                1,
            );
            emit(
                p,
                &mut state.alloc,
                Instruction::make_abc(OpCode::Return, 0, 2, 0),
                1,
            );
        }
        let c = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr())
            .expect("c");
        let fv = fiber_create(state, closure_val(c.as_ptr())).expect("fiber");
        (proto, c, fv)
    };

    let (pa, ca, fa_v) = make(&mut state, 1);
    let (pb, cb, fb_v) = make(&mut state, 2);
    let (pc, cc, fc_v) = make(&mut state, 3);

    unsafe {
        let fa = fa_v.as_obj().cast::<ObjFiber>();
        let fb = fb_v.as_obj().cast::<ObjFiber>();
        let fc = fc_v.as_obj().cast::<ObjFiber>();

        // Prime first-resume frames for all three (PC at LoadI).
        for f in [fa, fb, fc] {
            let closure = (*(*f).stack).as_obj().cast::<ObjClosure>();
            let need = 1 + u32::from((*(*closure).proto).max_stack_size);
            (*f).ensure_stack(&mut state.alloc, need).expect("stack");
            (*f).push_frame(&mut state.alloc, CallFrame::new(closure, 1, 0, 0xFFFF))
                .expect("frame");
        }

        (*fa).status = FiberStatus::Normal;
        (*fa).yield_dest_base = 0;
        (*fa).yield_expected_results = 1;
        (*fa).caller = core::ptr::null_mut();

        (*fb).status = FiberStatus::Normal;
        (*fb).yield_dest_base = 0;
        (*fb).yield_expected_results = 1;
        (*fb).caller = fa;

        (*fc).status = FiberStatus::Running;
        (*fc).caller = fb;

        assert_eq!(fiber_status_name(&*fa), "normal");
        assert_eq!(fiber_status_name(&*fb), "normal");
        assert_eq!(fiber_status_name(&*fc), "running");

        state.current_fiber = fc;
        let st = elara::vm::execute(&mut state, fc);
        assert_eq!(st, ElaraStatus::Yield);

        assert_eq!(fiber_status_name(&*fa), "suspended");
        assert_eq!(fiber_status_name(&*fb), "suspended");
        assert_eq!(fiber_status_name(&*fc), "suspended");
        // Yielded values propagated: C→B→A; host sees A's yield slot.
        assert_eq!(
            (*(*fa).stack.add((*fa).resume_slot as usize)).to_i64(),
            Some(1)
        );

        free_fiber_obj(&mut state, fa);
        free_fiber_obj(&mut state, fb);
        free_fiber_obj(&mut state, fc);
        free_proto_closure(&mut state, pa, ca);
        free_proto_closure(&mut state, pb, cb);
        free_proto_closure(&mut state, pc, cc);
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn resuming_dead_or_running_fiber_raises_expected_runtime_error() {
    let mut state = ElaraState::new();
    let mut proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    unsafe {
        let p = proto.as_mut();
        p.max_stack_size = 2;
        emit(
            p,
            &mut state.alloc,
            Instruction::make_abc(OpCode::Return, 0, 1, 0),
            1,
        );
    }
    let closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr()).expect("c");
    let fiber_v = fiber_create(&mut state, closure_val(closure.as_ptr())).expect("create");

    unsafe {
        let fiber = fiber_v.as_obj().cast::<ObjFiber>();

        // Resume to completion → dead.
        assert_eq!(
            fiber_resume(&mut state, core::ptr::null_mut(), fiber_v, &[], 0, 0),
            ElaraStatus::Ok
        );
        assert_eq!(fiber_status_name(&*fiber), "dead");

        let err = fiber_resume(&mut state, core::ptr::null_mut(), fiber_v, &[], 0, 0);
        assert_eq!(err, ElaraStatus::ErrRuntime);
        assert_eq!(
            state.last_error,
            Some("Cannot resume dead or running fiber")
        );

        // try_resume reports the non-suspended message.
        let tr = fiber_try_resume(&mut state, core::ptr::null_mut(), fiber_v, &[], 0, 0);
        assert!(!tr.ok);
        assert_eq!(tr.error_msg, Some("Cannot resume non-suspended fiber"));

        // Running fiber also rejected.
        (*fiber).status = FiberStatus::Running;
        let err = fiber_resume(&mut state, core::ptr::null_mut(), fiber_v, &[], 0, 0);
        assert_eq!(err, ElaraStatus::ErrRuntime);
        assert_eq!(
            state.last_error,
            Some("Cannot resume dead or running fiber")
        );

        free_fiber_obj(&mut state, fiber);
        free_proto_closure(&mut state, proto, closure);
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn uncaught_error_in_child_fiber_marks_it_dead_and_unwinds_or_returns_via_try_resume() {
    let mut state = ElaraState::new();

    // Child: invalid opcode → ErrRuntime.
    let mut child_proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("child proto");
    unsafe {
        let p = child_proto.as_mut();
        p.max_stack_size = 2;
        // 0xFF is not a valid opcode.
        emit(p, &mut state.alloc, Instruction(0x0000_00FF), 1);
    }
    let child_c = ObjClosure::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        child_proto.as_ptr(),
    )
    .expect("child c");
    let child_v = fiber_create(&mut state, closure_val(child_c.as_ptr())).expect("child");

    // Parent fiber used as caller so error unwinds through handle_fiber_error.
    let parent = ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 4).expect("p");

    unsafe {
        let child = child_v.as_obj().cast::<ObjFiber>();
        let parent_ptr = parent.as_ptr();
        (*parent_ptr).status = FiberStatus::Running;

        let tr = fiber_try_resume(&mut state, parent_ptr, child_v, &[], 0, 0);
        assert!(!tr.ok);
        assert!(tr.error_msg.is_some());
        assert_eq!(fiber_status_name(&*child), "dead");
        assert_eq!((*parent_ptr).status, FiberStatus::Running);

        // Direct resume also marks dead and raises.
        (*child).status = FiberStatus::Suspended;
        (*child).frame_count = 0;
        (*child).caller = core::ptr::null_mut();
        // Re-create suspended child with same closure for a hard resume error path.
        let child2_v = fiber_create(&mut state, closure_val(child_c.as_ptr())).expect("child2");
        let child2 = child2_v.as_obj().cast::<ObjFiber>();
        let err = fiber_resume(&mut state, parent_ptr, child2_v, &[], 0, 0);
        assert_eq!(err, ElaraStatus::ErrRuntime);
        assert_eq!(fiber_status_name(&*child2), "dead");

        free_fiber_obj(&mut state, child);
        free_fiber_obj(&mut state, child2);
        free_fiber_obj(&mut state, parent_ptr);
        free_proto_closure(&mut state, child_proto, child_c);
        state.strings.free_all(&mut state.alloc);
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}
