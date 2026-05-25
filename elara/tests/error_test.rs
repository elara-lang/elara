#![allow(unsafe_op_in_unsafe_fn)]

use core::mem::size_of;

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::gc::GC_FIXED;
use elara::value::{ObjString, ObjType, Value};
use elara::vm::{
    CallFrame, CatchOutcome, ELARA_MAX_TRACE_FRAMES, ElaraState, ElaraStatus, FiberStatus,
    ObjClosure, ObjFiber, capture_stack_trace, capture_upvalue, execute, intrinsic_assert,
    intrinsic_panic, push_pcall_frame, record_oom,
};

fn emit(p: &mut ObjProto, state: &mut ElaraState, inst: Instruction, line: u32) {
    p.emit_instruction(&mut state.alloc, inst, line)
        .expect("emit");
}

#[test]
fn stack_trace_captures_exact_line_numbers_without_heap_allocation_on_oom() {
    let mut state = ElaraState::new();

    // Pinned OOM string is FIXED and exact.
    unsafe {
        let oom = &*state.error.oom_string;
        assert_eq!(oom.header.gc_color, GC_FIXED);
        assert_eq!(oom.as_str(), "Out of memory");
        assert_eq!(state.error.oom_value().obj_type(), Some(ObjType::String));
    }

    let src = state
        .strings
        .new_string(
            &mut state.alloc,
            &mut state.all_objects,
            GC_FIXED,
            b"trace.el",
        )
        .expect("source name");
    unsafe {
        (*src.as_ptr()).header.gc_color = GC_FIXED;
    }

    // Outer proto: lines 10, 11 — calls into inner then returns.
    let mut outer =
        ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).expect("outer");
    // Inner proto: lines 20, 21
    let mut inner =
        ObjProto::new(&mut state.alloc, &mut state.all_objects, 0, src.as_ptr()).expect("inner");

    unsafe {
        let inn = inner.as_mut();
        inn.max_stack_size = 2;
        emit(
            inn,
            &mut state,
            Instruction::make_asbx(OpCode::LoadI, 0, 1),
            20,
        );
        emit(
            inn,
            &mut state,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            21,
        );

        let out = outer.as_mut();
        out.max_stack_size = 4;
        emit(
            out,
            &mut state,
            Instruction::make_asbx(OpCode::LoadI, 0, 0),
            10,
        );
        emit(
            out,
            &mut state,
            Instruction::make_abc(OpCode::Return, 0, 2, 0),
            11,
        );
    }

    let mut outer_c =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, outer.as_ptr()).expect("oc");
    let mut inner_c =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, inner.as_ptr()).expect("ic");

    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("fiber");

    unsafe {
        let f = fiber.as_mut();
        // Two frames: outer at bottom, inner on top (as if inner is the faulting frame).
        // Advance PCs past the first instruction so line lookup uses the faulting op.
        // PC points past the faulting instruction (dispatch already advanced).
        let mut outer_frame = CallFrame::new(outer_c.as_ptr(), 1, 0, 1);
        outer_frame.pc = (*outer.as_ptr()).code.add(2); // after Return @ line 11
        let mut inner_frame = CallFrame::new(inner_c.as_ptr(), 3, 2, 1);
        inner_frame.pc = (*inner.as_ptr()).code.add(2); // after Return @ line 21

        f.push_frame(&mut state.alloc, outer_frame).expect("outer");
        f.push_frame(&mut state.alloc, inner_frame).expect("inner");

        let before = state.alloc.bytes_allocated;
        capture_stack_trace(&mut state.error, fiber.as_ptr());
        let after = state.alloc.bytes_allocated;
        assert_eq!(
            before, after,
            "stack trace capture must not allocate on the heap"
        );

        assert_eq!(state.error.trace_count, 2);
        // Top frame first (inner).
        assert_eq!(state.error.trace_frames[0].line, 21);
        assert_eq!(state.error.trace_frames[1].line, 11);
        assert_eq!(state.error.trace_frames[0].is_native, 0);
        assert_eq!(
            (*state.error.trace_frames[0].source_name).as_str(),
            "trace.el"
        );

        // Fill beyond capacity: only ELARA_MAX_TRACE_FRAMES kept.
        f.frame_count = 0;
        for _ in 0..(ELARA_MAX_TRACE_FRAMES + 8) {
            let mut fr = CallFrame::new(inner_c.as_ptr(), 1, 0, 1);
            fr.pc = (*inner.as_ptr()).code.add(2);
            f.push_frame(&mut state.alloc, fr).expect("push");
        }
        let before2 = state.alloc.bytes_allocated;
        capture_stack_trace(&mut state.error, fiber.as_ptr());
        assert_eq!(state.alloc.bytes_allocated, before2);
        assert_eq!(state.error.trace_count, ELARA_MAX_TRACE_FRAMES);
        for i in 0..ELARA_MAX_TRACE_FRAMES {
            assert_eq!(state.error.trace_frames[i].line, 21);
        }

        // OOM recording uses the pinned string — still no alloc.
        let before3 = state.alloc.bytes_allocated;
        record_oom(&mut state.error);
        assert_eq!(state.alloc.bytes_allocated, before3);
        assert_eq!(state.error.last_status, ElaraStatus::ErrOom);
        let msg = state.error.last_message;
        assert!(msg.is_obj_type(ObjType::String));
        assert_eq!(
            (*msg.as_obj().cast::<ObjString>()).as_str(),
            "Out of memory"
        );

        // Cleanup
        outer_c.as_mut().free_closure(&mut state.alloc);
        inner_c.as_mut().free_closure(&mut state.alloc);
        outer.as_mut().free_proto(&mut state.alloc);
        inner.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(outer.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(inner.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        // Unlink FIXED strings from accounting via free_all (includes oom + metamethods + source).
        state.strings.free_all(&mut state.alloc);
    }
}

#[test]
fn pcall_on_success_prepends_true_to_multiple_return_values() {
    let mut state = ElaraState::new();

    let mut body = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("body");
    unsafe {
        let p = body.as_mut();
        p.max_stack_size = 4;
        emit(
            p,
            &mut state,
            Instruction::make_asbx(OpCode::LoadI, 0, 10),
            1,
        );
        emit(
            p,
            &mut state,
            Instruction::make_asbx(OpCode::LoadI, 1, 20),
            1,
        );
        emit(
            p,
            &mut state,
            Instruction::make_abc(OpCode::Return, 0, 3, 0),
            1,
        );
    }
    let mut closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, body.as_ptr()).expect("c");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("fiber");

    unsafe {
        let f = fiber.as_mut();
        *f.stack = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            ObjType::Closure,
        );
        f.open_top = 1;
        // Bottom frame is the protected call itself.
        let mut pcall = CallFrame::new(closure.as_ptr(), 1, 0, 0xFFFF);
        pcall.flags = elara::vm::FRAME_FLAG_PCALL;
        f.push_frame(&mut state.alloc, pcall).expect("pcall");

        assert_eq!(execute(&mut state, fiber.as_ptr()), ElaraStatus::Ok);
        assert!((*f.stack.add(0)).is_truthy());
        assert_eq!((*f.stack.add(1)).to_i64(), Some(10));
        assert_eq!((*f.stack.add(2)).to_i64(), Some(20));
        assert_eq!(f.open_top, 3);

        closure.as_mut().free_closure(&mut state.alloc);
        body.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(body.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
}

#[test]
fn pcall_catches_runtime_panic_closes_open_upvalues_and_returns_false_with_msg() {
    let mut state = ElaraState::new();

    let mut body = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("body");
    unsafe {
        let p = body.as_mut();
        p.max_stack_size = 4;
        // Invalid opcode → ErrRuntime inside pcall.
        emit(p, &mut state, Instruction(0x0000_00FF), 1);
    }
    let mut closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, body.as_ptr()).expect("c");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("fiber");

    unsafe {
        let f = fiber.as_mut();
        *f.stack = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            ObjType::Closure,
        );
        *f.stack.add(2) = Value::from_i48(99);
        f.open_top = 4;

        // Bottom frame is the protected call itself.
        let mut pcall = CallFrame::new(closure.as_ptr(), 1, 0, 0xFFFF);
        pcall.flags = elara::vm::FRAME_FLAG_PCALL;
        f.push_frame(&mut state.alloc, pcall).expect("pcall");

        let uv = capture_upvalue(&mut state.alloc, &mut state.all_objects, 0, f, 2).expect("uv");
        assert!(!f.open_upvalues.is_null());

        let st = execute(&mut state, fiber.as_ptr());
        assert_eq!(st, ElaraStatus::Ok);
        assert_eq!(f.frame_count, 0);
        assert!(!(*f.stack.add(0)).is_truthy());
        assert!((*f.stack.add(1)).is_obj_type(ObjType::String));
        assert_eq!(f.open_top, 2);
        assert!(f.open_upvalues.is_null(), "upvalues closed on unwind");
        assert_eq!((*uv).stack_idx, u32::MAX);

        // panic / assert with no remaining pcall → propagate ErrRuntime.
        let msg = state.error.assert_failed_value();
        match intrinsic_panic(&mut state, fiber.as_ptr(), msg) {
            CatchOutcome::Propagate(ElaraStatus::ErrRuntime) => {}
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(
            intrinsic_assert(&mut state, fiber.as_ptr(), Value::FALSE, None),
            Err(CatchOutcome::Propagate(ElaraStatus::ErrRuntime))
        ));
        assert!(intrinsic_assert(&mut state, fiber.as_ptr(), Value::TRUE, None).is_ok());

        (*uv).free_upvalue(&mut state.alloc);
        closure.as_mut().free_closure(&mut state.alloc);
        body.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(body.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
}

#[test]
fn gas_exhaustion_bypasses_script_pcall_and_terminates_at_host_boundary() {
    let mut state = ElaraState::with_gas(8);

    let mut body = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("body");
    unsafe {
        let p = body.as_mut();
        p.max_stack_size = 2;
        // Tight self-loop: JMP -1 forever.
        emit(p, &mut state, Instruction::make_sj(OpCode::Jmp, -1), 1);
    }
    let mut closure =
        ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, body.as_ptr()).expect("c");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 64, 8).expect("fiber");

    unsafe {
        let f = fiber.as_mut();
        *f.stack = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            ObjType::Closure,
        );
        f.open_top = 1;

        let mut host = CallFrame::new(closure.as_ptr(), 4, 3, 2);
        host.pc = (*body.as_ptr()).code;
        f.push_frame(&mut state.alloc, host).expect("host");
        let caller = &mut *f.frames.add(0);
        push_pcall_frame(
            &mut state,
            f,
            caller,
            (*body.as_ptr()).code,
            closure.as_ptr(),
            0,
            0,
            2,
        )
        .expect("pcall");

        let st = execute(&mut state, fiber.as_ptr());
        assert_eq!(st, ElaraStatus::ErrGas);
        assert_eq!(f.status, FiberStatus::Dead);
        assert_eq!(f.frame_count, 0, "gas must not resume into pcall caller");

        closure.as_mut().free_closure(&mut state.alloc);
        body.as_mut().free_proto(&mut state.alloc);
        state
            .alloc
            .free_bytes(body.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        f.free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state.strings.free_all(&mut state.alloc);
    }
}
