#![allow(unsafe_op_in_unsafe_fn)]

use core::mem::size_of;

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::gc::GC_FIXED;
use elara::value::{ObjString, ObjType};
use elara::vm::{
    CallFrame, ELARA_MAX_TRACE_FRAMES, ElaraState, ElaraStatus, ObjClosure, ObjFiber,
    capture_stack_trace, record_oom,
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
    // Long strings / objects on all_objects may remain; free_all only clears intern pool.
    // Sweep remaining via dropping state is fine for this unit test — assert pool is empty path.
}
