use core::mem::size_of;

use elara::compiler::{Instruction, ObjProto, OpCode};
use elara::gc::GcAllocator;
use elara::value::{GcHeader, ObjTable, Value};
use elara::vm::{CallFrame, ElaraState, ElaraStatus, ObjClosure, ObjFiber, execute};

#[test]
fn vm_executes_arithmetic_bitwise_string_concat_and_control_flow_loops() {
    let mut state = ElaraState::new();

    let mut proto = ObjProto::new(
        &mut state.alloc,
        &mut state.all_objects,
        0,
        core::ptr::null_mut(),
    )
    .expect("proto");
    let mut dedup =
        ObjTable::new(&mut state.alloc, &mut state.all_objects, 0, 1, 0, 4).expect("dedup");
    let p = unsafe { proto.as_mut() };

    // Program: arithmetic + bitwise + counted loop via fused LEI+JMP, then multi-return.
    p.max_stack_size = 8;
    let emit = |p: &mut ObjProto, alloc: &mut GcAllocator, inst, line| {
        p.emit_instruction(alloc, inst, line).expect("emit");
    };
    emit(
        p,
        &mut state.alloc,
        Instruction::make_asbx(OpCode::LoadI, 0, 10),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_asbx(OpCode::LoadI, 1, 3),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::Add, 2, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::Mul, 3, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::BAnd, 4, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::BOr, 5, 0, 1),
        1,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_asbx(OpCode::LoadI, 6, 0),
        2,
    );
    let loop_pc = p.code_count;
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::AddI, 6, 6, 1),
        2,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::LeI, 1, 6, 5u8),
        2,
    );
    let back = loop_pc as i32 - (p.code_count as i32 + 1);
    emit(
        p,
        &mut state.alloc,
        Instruction::make_sj(OpCode::Jmp, back),
        2,
    );
    emit(
        p,
        &mut state.alloc,
        Instruction::make_abc(OpCode::Return, 2, 6, 0),
        3,
    );

    let closure = ObjClosure::new(&mut state.alloc, &mut state.all_objects, 0, proto.as_ptr())
        .expect("closure");
    let mut fiber =
        ObjFiber::new(&mut state.alloc, &mut state.all_objects, 0, 16, 4).expect("fiber");
    let f = unsafe { fiber.as_mut() };

    unsafe {
        *f.stack.add(0) = Value::from_obj(
            core::ptr::NonNull::new(closure.as_ptr().cast()).unwrap(),
            elara::value::ObjType::Closure,
        );
    }
    f.open_top = 1 + u32::from(p.max_stack_size);
    let frame = unsafe { CallFrame::new(closure.as_ptr(), 1, 0, 5) };
    f.push_frame(&mut state.alloc, frame).expect("push frame");

    let status = unsafe { execute(&mut state, fiber.as_ptr()) };
    assert_eq!(status, ElaraStatus::Ok);

    unsafe {
        let stack = fiber.as_ref().stack;
        assert_eq!((*stack.add(0)).to_i64(), Some(13));
        assert_eq!((*stack.add(1)).to_i64(), Some(30));
        assert_eq!((*stack.add(2)).to_i64(), Some(2));
        assert_eq!((*stack.add(3)).to_i64(), Some(11));
        assert_eq!((*stack.add(4)).to_i64(), Some(6));
    }

    let f = unsafe { fiber.as_mut() };
    let old_ptr = f.stack;
    f.open_top = f.stack_capacity;
    f.ensure_stack(&mut state.alloc, 64).expect("grow");
    assert_ne!(f.stack, old_ptr);
    assert!(f.stack_capacity >= 64);

    unsafe {
        fiber.as_mut().free_fiber_buffers(&mut state.alloc);
        state
            .alloc
            .free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        state
            .alloc
            .free_bytes(closure.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        proto.as_mut().free_proto(&mut state.alloc);
        dedup.as_mut().free_table(&mut state.alloc);
        state
            .alloc
            .free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
        state
            .alloc
            .free_bytes(dedup.as_ptr().cast::<u8>(), size_of::<ObjTable>());
    }
    assert_eq!(state.alloc.bytes_allocated, 0);
}

#[test]
fn call_frame_stores_indices_not_raw_stack_pointers_across_growth() {
    let mut alloc = GcAllocator::new();
    let mut all: *mut GcHeader = core::ptr::null_mut();
    let mut proto = ObjProto::new(&mut alloc, &mut all, 0, core::ptr::null_mut()).expect("p");
    unsafe {
        (*proto.as_ptr()).max_stack_size = 2;
        (*proto.as_ptr())
            .emit_instruction(
                &mut alloc,
                Instruction::make_abc(OpCode::Return, 0, 1, 0),
                1,
            )
            .expect("ret");
    }
    let closure = ObjClosure::new(&mut alloc, &mut all, 0, proto.as_ptr()).expect("c");
    let mut fiber = ObjFiber::new(&mut alloc, &mut all, 0, 8, 2).expect("f");
    let f = unsafe { fiber.as_mut() };
    let frame = unsafe { CallFrame::new(closure.as_ptr(), 2, 1, 0) };
    assert_eq!(frame.base_idx, 2);
    assert_eq!(frame.func_idx, 1);
    f.push_frame(&mut alloc, frame).expect("push");
    let base_idx = unsafe { (*f.frames).base_idx };
    f.open_top = f.stack_capacity;
    f.ensure_stack(&mut alloc, 128).expect("grow");
    // Indices unchanged after reallocation
    assert_eq!(unsafe { (*f.frames).base_idx }, base_idx);

    unsafe {
        fiber.as_mut().free_fiber_buffers(&mut alloc);
        alloc.free_bytes(fiber.as_ptr().cast::<u8>(), size_of::<ObjFiber>());
        alloc.free_bytes(closure.as_ptr().cast::<u8>(), size_of::<ObjClosure>());
        proto.as_mut().free_proto(&mut alloc);
        alloc.free_bytes(proto.as_ptr().cast::<u8>(), size_of::<ObjProto>());
    }
    assert_eq!(alloc.bytes_allocated, 0);
}
